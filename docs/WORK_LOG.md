# Development work log

Append-only from 2026-10-08 UTC. Timestamps use UTC. Source inspection, executed checks and real-device validation are separate evidence classes.

## 2026-10-08 18:35 UTC — Establish current documentation baseline

- Baseline commit verified with `git rev-parse HEAD`: `9c159953163763a354634b3a9f95f84de174641b`; initial `git status --short` was empty.
- Replaced stale DEV_STATE with a current handoff, concrete roadmap and this work log. Preserved the prior Windows handoff verbatim in HISTORICAL_DEV_STATE with an explicit historical/unverified label.
- Source review confirmed `playlist::create_audio_crossfade`, UI entry points in app.rs, shared equal-power fade DSP, Master Capture writer and the rendered-master callback tap. Corrected README/parity gaps that incorrectly called all crossfade/realtime export unimplemented.
- Verification: file/source inspection completed. Build worker reported baseline test command exit 127 because `cargo` is absent. `command -v cargo` and `command -v rustc` returned no paths during documentation review. No Rust tests executed; fmt/Clippy/build/UI/audio tests not run.
- Blocker: local Rust toolchain unavailable. Continue source fixes and documentation while arranging a supported build environment.
- Active work: storage non-finite-number safety and Playlist Slip/Crossfade review. These remain in progress, with regression tests not yet executed.
- No push, public release or new release artifact produced.

## 2026-10-08 18:36 UTC — Playlist hardening implemented; tooling unavailable

- User approved the official toolchain and build dependencies after the earlier cancellation. The build worker retried the same bootstrap and reported installer download in progress. No Rust execution result yet.
- `src/playlist.rs` now rejects impossible group-resize bounds, uses wider Slip-grid arithmetic, keeps looping Slip in a half-open interval, saturates extreme audio-offset subtraction and rejects overflowing Clip endpoints during fade/crossfade edits. Four regression tests added; not executed.
- Audio/export review selected a possible Master Capture shutdown/drain race and offline-render input validation for implementation; these fixes are not complete at this checkpoint.
- README/parity now describe the already-present Crossfade and realtime Master Capture without claiming offline VST bounce. Build guidance no longer uses an obsolete fixed passing-test count.

## 2026-10-08 18:37 UTC — Toolchain unavailable; CI prepared locally

- Local toolchain setup did not complete. This supersedes the 18:36 pending setup status. No Rust tests, fmt, Clippy or builds executed.
- `.github/workflows/ci.yml` added locally with Windows MSVC stable checks: formatting, locked all-feature/all-target tests, Clippy with denied warnings, all-bin build and no-default-features check. Read-only contents permissions and nonpersisted checkout credentials are configured.
- Build worker reports YAML parse success and `git diff --check` success. Workflow has not been pushed, published or run, and is separate from the pinned gnullvm release recipe.
- Documentation `git diff --check` passed at the preceding checkpoint; later implementation checks will be logged separately.

## 2026-10-08 18:38 UTC — Storage and audio/export fixes implemented, runtime checks blocked

- `src/model.rs`: `validate_persisted_numbers` rejects NaN/infinity in persisted public numeric fields before touching the filesystem; failure cleanup closes the staging handle first. Four added tests cover 60 invalid field/value combinations preserving original bytes/loadability with no temporary leaks, no directory creation on validation failure, ignored legacy session-only data, and Unix invalid-Unicode serialization failure cleanup.
- Storage worker command: `cargo test --no-default-features model::tests:: -- --test-threads=1` could not start, exit 127 (`cargo` not found). This is a tooling block, not a passing or failing Rust test result; Windows-specific behavior remains unrun. `git diff --check -- src/model.rs` passed.
- `src/master_capture.rs`: observe shutdown before draining the queue so a final producer push followed by COMMIT cannot be replaced with silence. Added deterministic interleaving/PCM/metadata regression; not executed.
- `src/export.rs`: reject rates outside 8000..=192000 Hz rather than silently clamp, and reject non-finite samples/gain/products before staging output. Three new regressions cover rejected-rate file preservation, exact boundary-rate headers and invalid-audio file preservation; not executed. No dependency, project-format or application API changes.
- Audio/export worker reports scoped `git diff --check` passed. Pending focused commands once tooling is restored: `cargo test --locked --no-default-features master_capture::tests` and `cargo test --locked --no-default-features export::tests`; full tests and Windows integration still required.

## 2026-10-08 18:39 UTC — Playlist gesture history now includes dependent state

- `PlaylistGestureSnapshot` in `src/playlist.rs` and its `src/app.rs` integration capture Clips, Automation lanes and Audio Clip Mixer routing together. This repairs incomplete undo snapshots for automation point edits/deleted lanes and audio split/delete routing. Existing per-frame Clip/lane clones are reused; no PCM media copy is introduced.
- Added snapshot and integration undo/redo regressions; not compiled or executed because Rust remains unavailable.
- Unresolved: splitting inside an existing Audio Clip fade still duplicates normalized fade lengths onto both halves. Exact envelope preservation requires origin/extent metadata plus coordinated schema, migration, render and UI changes; this is not fixed by the snapshot repair.
- Documentation checks: `git diff --check` passed; Python local Markdown-link existence scan across README, DEV_STATE and docs reported PASS. These checks do not certify Rust behavior.

## 2026-10-08 18:42 UTC — Final local handoff and static verification

- Playlist transaction lifecycle follow-through: automation-point drags explicitly start/end transactions; discrete split/delete/mute/automation-point insert/delete commit separately rather than waiting on generic timed history.
- Independent read-only static review completed; the lifecycle finding above was integrated. Static review cannot establish compiler, test, hardware or UI correctness.
- Verified source delta against baseline using `git show HEAD:<path>` and literal `#[test]` counts: model +4, export +3, master_capture +1, playlist +6, app +1 = **15 new test attributes** (760 → 775 total). These are not executed/passing test counts.
- Storage precision: preflight covers public persisted f32 fields. Private AutomationLane f64 fields retain existing constructor/edit/deserialization checks; nonserialized legacy piano_notes and waveform_peaks are excluded. Handle-before-cleanup ordering is defensive portability hardening, not a reproduced Windows defect. No range/schema migration changes were introduced by this storage fix.
- Final validation performed: `git diff --check` passed; local Markdown-link targets exist. YAML parse success was reported by the build worker. Rust compilation, tests, rustfmt, Clippy, CI execution and release build remain **blocked before execution / not run**.
- Blocker remains: local Rust tooling remains unavailable; no compiler or test results established.
- Required follow-up: establish a supported build environment, execute focused regressions and complete locked gates; then test real Audio Clip Slip/Crossfade, automation drag/delete undo/redo, audio split/delete routing restoration, Master Capture stop/finalization, save/reopen and Windows/helper/device behavior.
- Known unresolved split-fade envelope preservation and the commercial-scope gaps remain in DEV_STATE/DEVELOPMENT_ROADMAP. Realtime Master Capture does not solve offline VST bounce. No push, hosted workflow run, public publication or release artifact has occurred.


## 2026-10-09 01:27 UTC — Prepare independent Windows CI branch

- Remote `main` remains at baseline `9c159953163763a354634b3a9f95f84de174641b`. Prepared independent branch `ci/windows-reliability-20261009`; no merge or release is part of this validation.
- Added a branch-specific push trigger alongside the existing main/PR/manual triggers. Standard GitHub-hosted Windows MSVC checks retain read-only repository permissions, nonpersistent checkout credentials and no artifact publication.
- Local `git diff --check` passed before publication. Compiler, test, formatting and Clippy results remain pending until the actual workflow runs.
- Removed environment-specific operational details from the public documentation while preserving the technical validation history.
