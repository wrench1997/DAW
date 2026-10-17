# Cloud integration validation — 2026-10-17

Base: `314779d87d629f2c8e31c361afd12af30cc5c8a2`, branch `ci/windows-reliability-20261009`.

## Exact source and scope

The complete fresh-process suites ran against source-manifest SHA256 `fbfc0d75589fa302b1686e581dd5fd44b681d78f99e8668e50055101aa65a47c`. Rechecking after both suites found no source drift. The final repaired source manifest is `f8f7995516d80e69bd906bb073df7b9435c6990e4b23d0eecede040b8001e208`.

Only four test files changed between these snapshots:

- `src/app/clip_render_current_source_dispatch_tests.rs`
- `src/app/clip_render_restoration_dispatch_order_tests.rs`
- `src/audio_capture_settlement_tests.rs`
- `src/audio_loop_restart_tests.rs`

Production source is byte-identical between the complete-suite snapshot and the final repaired candidate. Mutable QA documents are excluded from source manifests. Exact source-file hashes, tracked patches, new source files, binary hashes and original logs are retained in the evidence archive.

## Complete runs and targeted corrections

Every listed test was scheduled once, by exact name, in fresh processes containing at most 20 tests. No added ignores or hidden skips. Result totals are reconciled against enumeration; nested child-test output uses the outer final result rather than double-counting the child.

- No-default App: **1,993 passed, 1 failed, 1 ignored**, covering **1,995 unique tests** in 176 batches. The loop-restart admission fixture failed once. After the test-only repair, both affected modules passed: **7 capture + 19 loop tests**. Unchanged full-suite evidence is reused; the complete suite was not rerun after that repair. Combined checked coverage is 1,994 passing tests and one pre-existing ignored test.
- All-feature App: **2,587 passed, 4 failed, 9 ignored**, covering **2,600 unique tests** in 240 batches. Failures were three prepared-play dispatch fixture cases and one capture-settlement fixture case. After the test-only repairs, **27 topology/dispatch + 7 capture + 19 loop + 1 stream evidence tests passed**. Unchanged full-suite evidence is reused; this is not a claim of a fresh all-green complete run. Combined checked coverage is 2,591 passing tests and nine pre-existing ignored tests.
- The original core loop and all-feature capture failures each passed five isolated runs before repair. The deterministic dispatch failures required diagnosis of missing ordinary prepared-play polling, callback publication while Play is pending, and consuming the matching App acknowledgment before the Stop scenario. Exact assertions and deadlines were retained. See `20261017-linux-tests.md` for details.
- Failed fixture directories intentionally remain. Reruns use a unique `TMPDIR` to avoid process-ID reuse collisions. A directory-collision rerun failure is preserved separately.

Coverage includes actual production UI input, piano editing, navigation, automation, Undo/Redo, playback, project persistence, export/cancellation, timeline and plugin-worker mock/protocol tests. Ignored genuine-plugin/device qualifications remain unrun.

## Other verified gates

- Both feature-mode App test binaries compile.
- All-feature main/helper binary build passes.
- Helper unit tests: **59 passed**. Protocol tests: **13 passed**.
- Helper smoke: three JSON replies, invalid-command recovery, successful Shutdown with stdin open, process reaped; no plugin loaded.
- Python harness/contracts: **359 passed, 9 platform-conditional skips** (368 total).
- Final formatting, strict all-feature/all-target Clippy, strict no-default/all-target Clippy, and final binary/helper smoke gates are recorded in `repaired-final-*.log` in the archive.

The dependency `vst3-host` still emits the existing Rust 1.99 deprecation warning for `fetch_update`; strict root Clippy passes with ordinary dependency warning capping. No warning suppressions were added for the repaired root tests.

## Environment and limits

Linux cloud, official checksum-verified Rust 1.99.0, Debian 13 development packages extracted into a workspace-local sysroot; two build jobs and dev/test debug info disabled. No global installation or changes to the historical `/workspace/shared/DAW` checkout. The archive contains commands, package versions, source/binary hashes, all gate logs and a reproduction guide.

Automation screenshots are actual WGPU offscreen renders using the pre-installed Chromium SwiftShader ICD. They are not native Windows screenshots or audio-device/plugin qualification. Selection timings are isolated operation measurements, not frame rate or perceived GUI latency. No commit, push, Windows runtime acceptance or real-plugin acceptance is claimed by this cloud report.
