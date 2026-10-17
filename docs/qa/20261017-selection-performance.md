# Piano Roll selection retention performance — 2026-10-17

## Scope and behavior

`PianoRollState::retain_existing` previously scanned the full note slice separately for every selected ID. A fully selected score performed quadratic ID comparisons on the per-frame path.

The replacement:

- Returns immediately for an empty selection, without building a lookup.
- Clears selection immediately for an empty note slice.
- Keeps the allocation-free scan for up to 32 selected IDs, bounding its comparison cost to 32 passes over the score.
- Builds a temporary `HashSet<u64>` of existing note IDs for larger selections, then retains IDs by membership.

Expected large-selection work is linear in note count plus selection hash-table capacity, instead of note count multiplied by selected count (plus table traversal). Temporary lookup memory is O(note count). HashSet membership has expected constant-time cost; this is not a worst-case hashing guarantee. The small-selection branch avoids allocating an entire score lookup for normal single/few-note editing. The threshold is a pragmatic tradeoff, not a universal crossover: note order, hash seed, allocator, and hardware affect timings.

Membership semantics are unchanged, including duplicate input note IDs, zero, and `u64::MAX`. No normalization, selection expansion, note flags, musical data, persistence, undo, or project edits occur here. The note slice remains immutable.

## Functional checks

Four dedicated tests in `src/piano_roll.rs` cover:

1. All 10,000 notes selected, exact identity preservation, and unchanged note fields/selection flags.
2. A partial selection mixed with stale IDs, followed by an entirely stale selection.
3. Empty selection and empty score, including selections of 1, 32, 33, and 1,000 IDs.
4. Exact legacy/extreme/duplicate-ID semantics on both sides of the 32/33 threshold, checked against the original scan algorithm.

The actual updated method and these four tests were extracted into a standalone Rust test harness with a field-compatible note struct and minimal state wrapper: **4 passed, 0 failed**. This is an isolated algorithm check. The shared build owner also ran the `retain_existing` filter in the actual no-default-features application test binary: **4 passed, 0 failed** (log: `../daw-tooling-20261017/logs/retain-existing.log`). Wider application/audio test results are tracked separately by the build owner. No elapsed-time assertion was added to unit tests.

## Controlled microbenchmark

Environment: Linux x86_64 cloud workspace; `rustc 1.99.0 (b940084d7 2026-09-28)`, `rustc --edition 2024 -O`. This is a shared machine, without CPU pinning or an otherwise idle-machine guarantee. Treat absolute measurements as indicative.

The standalone harness compares the original method against the actual replacement method extracted from `src/piano_roll.rs`, using the same field layout as `PianoNote`. Notes have ascending IDs 1 through N. Partial selections use IDs at the end of the note slice, so the scan has to traverse most notes. These are intentionally scan-heavy partial selections, not a claim about typical selection positions. Full selections exercise every note ID.

Starting selections are cloned outside the timed interval. Timing includes lookup allocation/construction and destruction in the new method, but excludes input setup and starting-state cloning. Each case first verifies old/new selection equality. Inputs and retained outputs pass through `std::hint::black_box`. After warm-up, seven batches alternate old/new execution order. Reported values are the median microseconds per call: 30 calls per batch for selections >=1,000, otherwise 300. Empty-call values approach timer/loop resolution and are not meaningful speed claims.

An initial threshold of four caused a roughly 5× regression at five selected tail IDs due to lookup construction. Raising the allocation-free threshold to 32 removed that regression in this sample. The final implementation and tests use 32.

| Notes | Selected | Original µs/call | Replacement µs/call | Original / replacement |
| ---: | ---: | ---: | ---: | ---: |
| 1000 | 0 | 0.001 | 0.001 | 1.00 |
| 1000 | 1 | 0.418 | 0.421 | 0.99 |
| 1000 | 4 | 1.683 | 1.779 | 0.95 |
| 1000 | 5 | 2.136 | 2.134 | 1.00 |
| 1000 | 32 | 13.622 | 13.827 | 0.99 |
| 1000 | 33 | 14.154 | 10.990 | 1.29 |
| 1000 | 100 | 42.737 | 13.128 | 3.26 |
| 1000 | 1000 | 197.398 | 20.788 | 9.50 |
| 10000 | 0 | 0.001 | 0.001 | 1.00 |
| 10000 | 1 | 4.325 | 4.499 | 0.96 |
| 10000 | 4 | 18.411 | 18.276 | 1.01 |
| 10000 | 5 | 23.164 | 22.920 | 1.01 |
| 10000 | 32 | 148.624 | 153.740 | 0.97 |
| 10000 | 33 | 151.745 | 117.869 | 1.29 |
| 10000 | 1000 | 4274.494 | 128.087 | 33.37 |
| 10000 | 10000 | 22790.189 | 222.851 | 102.27 |

For all-selected scores, the isolated operation improved from about 197 µs to 21 µs at 1,000 notes and from 22.79 ms to 0.223 ms at 10,000 notes in this run. **These are not whole-GUI frame timings, FPS measurements, playback measurements, or end-to-end interaction latency claims.** Rendering and other per-frame scans remain outside this benchmark.

### Reproduction in this workspace

The scratch harness and CSV are intentionally outside the repository:

- `/workspace/shared/piano-selection-bench/bench.rs`
- `/workspace/shared/piano-selection-bench/results.csv`
- `/workspace/shared/piano-selection-bench/tests.rs`

```sh
source /workspace/scratch/c0dd1d60f798/daw-tooling-20261017/env.sh
rustc --edition 2024 -O /workspace/shared/piano-selection-bench/bench.rs \
  -o /workspace/shared/piano-selection-bench/bench
/workspace/shared/piano-selection-bench/bench
rustc --edition 2024 --test /workspace/shared/piano-selection-bench/tests.rs \
  -o /workspace/shared/piano-selection-bench/tests
/workspace/shared/piano-selection-bench/tests
# Run through the shared build owner to avoid overlapping Cargo builds:
cargo test --no-default-features retain_existing
```

The scratch harness copies the method body at extraction time; regenerate it if the production method changes. No dependencies, benchmark target, commits, or pushes were added for this experiment.
