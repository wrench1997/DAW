# Piano selected-note drag scalability — 2026-10-17

## Scope

Measured against exact base `39dfb2091f5f612eb65e958b5ab90a1fefcabebc` in an isolated worktree. Three operation-local lookups replace repeated full-note scans: group expansion on press, active-channel selection retention on press, and selected-origin target lookup during movement. Expected lookup work is O(N + S), using hash tables; this is not a worst-case hashing guarantee. The zero/one-selection paths retain the small allocation-free lookup where applicable. No persistent cache, history/fingerprint redesign, project-format change, or dependency was added.

Group expansion preserves the first matching note's channel/group when IDs are duplicated. Movement preserves the original first matching destination and last duplicate origin's final values; the move callback only assigns timing/pitch. Note vector order, group membership, ghost-channel filtering, clone allocation, viewport/focus interruption, snapping and history boundaries remain unchanged. Malformed cross-channel duplicate IDs retain the base behavior; this is not a new identity-normalization policy.

## Functional verification

- New real egui input test exercises 131 notes (128 offscreen), one/all selection, stale IDs, ghost exclusion, release/focus interruption, exact serialized musical fields and one-step Undo.
- Oracle tests compare move lookup against the prior linear implementation for duplicate/missing IDs; selection expansion against prior group expansion for duplicate/stale IDs, grouping on/off, channel separation, absent groups and explicit group 0.
- Focused no-default-features run: 30 piano mouse tests plus both lookup oracle tests passed.
- Full no-default-features application suite: 2,002 passed, 0 failed, 1 ignored.
- All-feature application checks: 31 `piano_mouse`, 4 `lookup`, and 32 `group` filtered tests passed (filters overlap; counts are not additive).
- Strict `cargo clippy --offline --locked --all-features --all-targets -- -D warnings` and `cargo build --offline --locked --all-features --bins` passed. Existing capped dependency warning in vendored `vst3-host` remains; no new application warning.
- `cargo fmt --all -- --check` and `git diff --check` passed. No new full all-feature-suite run is claimed.

## Measurement

Same corrected production `UiHarness`/egui input harness and toolchain as the baseline, with eight visible notes at both 1,000 and 10,000 total notes. Fresh baseline and optimized binaries ran sequentially in alternating order per case, after functional tests stopped. Each case has 30 measured idle frames, 20 press/release samples and 80 movement samples after warmup. No preselection means the idle frames have no selection and pressing begins a one-note drag. All-selected movement assertions verify that every note moved and exactly one Undo entry was added. p50 is upper-middle; p95 is nearest rank.

Linux cloud CPU wall time includes App UI processing, egui shape generation, accessibility collection/assertion and output drop. It excludes native event delivery, GPU render/present and audio devices/plugins. App/test crate uses opt-level 3 with cached unoptimized dependencies and debug assertions; this mixed profile is **not shipping-release performance, Windows FPS or an FL Studio comparison**. Shared virtualized host and run noise limit small-change claims.

Milliseconds, p50 / p95:

| Notes | Preselection | Phase | Baseline | Optimized |
| ---: | --- | --- | ---: | ---: |
| 1000 | none | idle | 2.723 / 3.741 | 2.643 / 3.970 |
| 1000 | none | press | 4.115 / 4.842 | 4.156 / 5.008 |
| 1000 | none | drag | 2.857 / 4.064 | 2.935 / 3.640 |
| 1000 | none | release | 5.538 / 7.792 | 5.461 / 6.523 |
| 1000 | all | idle | 2.554 / 3.823 | 2.860 / 4.284 |
| 1000 | all | press | 4.767 / 6.844 | 4.172 / 4.480 |
| 1000 | all | drag | 3.040 / 3.965 | 2.853 / 3.332 |
| 1000 | all | release | 5.717 / 6.784 | 5.456 / 5.958 |
| 10000 | none | idle | 3.271 / 13.998 | 3.664 / 14.767 |
| 10000 | none | press | 15.392 / 17.056 | 15.541 / 16.473 |
| 10000 | none | drag | 3.915 / 5.223 | 3.743 / 4.958 |
| 10000 | none | release | 27.971 / 29.961 | 27.453 / 29.249 |
| 10000 | all | idle | 4.160 / 14.824 | 3.665 / 14.974 |
| 10000 | all | press | 68.620 / 71.886 | 17.336 / 18.351 |
| 10000 | all | drag | 26.807 / 29.049 | 4.804 / 5.788 |
| 10000 | all | release | 50.409 / 53.809 | 28.709 / 31.249 |

At 10,000 all-selected notes, movement p50 decreased from 26.807 to 4.804 ms (about 5.6× faster); press from 68.620 to 17.336 ms and release from 50.409 to 28.709 ms. One-note gesture measurements show no material regression in this sample. Idle timing varies, including occasional periodic history-observer fingerprint costs; the optimization does not change that path. Remaining release/press costs are material, and whole-project fingerprinting remains deferred. No native/device smoothness qualification follows from these CPU numbers.

## Reproduction and evidence

Workspace evidence is deliberately outside the repository in `daw-tooling-20261017/validation/editing-latency-after/`: comparison report, `summary.json` with raw samples, `run_comparison.sh`, baseline-repeat/optimized logs, full-core logs, focused logs, and frozen `production.patch`. Original corrected harness and method are in the adjacent `editing-latency/` directory. Source toolchain environment is `daw-tooling-20261017/env.sh` (rustc 1.99.0). Benchmark uses `CARGO_BUILD_JOBS=1`, offline no-default-features `citrus-studio` test binary, and test-only app package opt-level 3. Benchmark source/profile additions were removed and the production diff verified identical to the frozen patch before handoff. They must not be shipped.

Optimized benchmark binary SHA-256: `632c34e34eb043550d8fb1d6c6666f46fe65b770ba9efb5957362717e66b8007`.
