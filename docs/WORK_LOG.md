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

## 2026-10-09 01:34 UTC — Source branch published; workflow still pending

- Source/docs commit `0146efd6cdc4832a3eaa9f6e99a252913c592144` was pushed to `ci/windows-reliability-20261009` and verified with the remote ref. `main` remains `9c159953163763a354634b3a9f95f84de174641b`.
- The small workflow-file publication did not complete. Subsequent read-only checks found the workflow path absent and zero Actions runs for the branch. No compiler, test, fmt or Clippy execution has occurred.
- The complete workflow remains available locally for the next authorized publication attempt. No release, deployment or merge was performed.

## 2026-10-09 01:45 UTC — First Windows run; formatting findings fixed

- Workflow published successfully as `90226a5e01e1f30299fb716fb1dd129006a9fd8d`. [Run 37871126045](https://github.com/wrench1997/DAW/actions/runs/37871126045), Windows Server 2025/MSVC, installed Rust/Cargo 1.99.0. Cargo.lock SHA256: `38d9767b7b618608e2e4ecc91e662b2048a64ea332acaefe338feb0b2189c338`.
- `cargo +stable-x86_64-pc-windows-msvc fmt --all -- --check` failed with exit 1, reporting six layout-only hunks across app.rs, master_capture.rs and playlist.rs. Applied those exact hunks. Local `git diff --check` passed.
- Locked tests, Clippy, all-bin build and no-default-features check were skipped after fmt failed. This run establishes no passing Rust tests. The next branch push will run the same complete gates.

## 2026-10-09 01:47 UTC — Local formatter available; Linux dependency check blocked

- Local Rust/Cargo 1.99.0, rustfmt 1.10.0 and Clippy 0.1.99 are now available. `cargo fmt --all -- --check` passed at `bcaf5e68dd8b865b9122ecb25423d010d7f60002` with exit 0.
- `cargo check --locked --no-default-features` on Linux exited 101 during `alsa-sys 0.4.0` build: pkg-config could not find `alsa.pc` (`libasound2-dev` unavailable). This is a pre-project native dependency failure, not a project compiler/test failure.
- [Windows run 37871292792](https://github.com/wrench1997/DAW/actions/runs/37871292792) passed formatting and entered the locked all-feature/all-target test command. Test counts and downstream gates remain pending.

## 2026-10-09 01:52 UTC — 775 Windows tests passed; Clippy compatibility fixes

- [Run 37871292792](https://github.com/wrench1997/DAW/actions/runs/37871292792) at `bcaf5e68dd8b865b9122ecb25423d010d7f60002`: fmt **passed**; locked all-feature/all-target tests **passed**, 775 passed / 0 failed / 0 ignored, plus helper target with 0 tests. All 15 new reliability regressions are included.
- Clippy with `-D warnings` **failed** on nine unique source findings: one deprecated atomic `fetch_update` (audio_device), seven constant-size `chunks_exact` calls (model 1, plugin_runtime 1, wav 4, export tests 1), and one complex test mutator type (model). All-bin build and no-default-features check were skipped.
- Replaced the atomic operation with the equivalent compare-exchange retry loop, retaining Release success / Relaxed failure ordering and compatibility with the documented Rust 1.97.1 release toolchain. Switched fixed-size slices to `as_chunks` and introduced a local test mutator type alias. No lint suppression, MSRV increase, release-toolchain change or workflow gate weakening.
- Local formatter and `git diff --check` passed after these changes. Fresh Windows tests, Clippy and remaining gates will run on the new commit. Real GUI/audio/VST/release acceptance is still outstanding.

## 2026-10-09 02:01 UTC — All Windows development gates passed

- Verified source commit: `122596ae37e23be4420db1766aff88019d231b3c`. [Run 37871876943](https://github.com/wrench1997/DAW/actions/runs/37871876943), job `113631600450`, completed successfully on Windows Server 2025 / x86_64-pc-windows-msvc with Rust/Cargo 1.99.0.
- Exact successful commands (each uses `+stable-x86_64-pc-windows-msvc`):
  - `cargo fmt --all -- --check`
  - `cargo test --locked --all-features --all-targets`: **775 passed, 0 failed, 0 ignored**; VST3 helper target: 0 tests.
  - `cargo clippy --locked --all-features --all-targets -- -D warnings`
  - `cargo build --locked --all-features --bins`: application and VST3 helper debug binaries built.
  - `cargo check --locked --no-default-features --all-targets`
- Cargo.lock remains unchanged, SHA256 `38d9767b7b618608e2e4ecc91e662b2048a64ea332acaefe338feb0b2189c338`. No project compiler/lint warnings in this passing run. GitHub emitted a nonblocking action-runtime deprecation notice for checkout v4.
- Independent static review found no blocker in the atomic update or slice changes: retry ordering and byte-decoding/remainder behavior are preserved. `as_chunks` predates the documented Rust 1.97.1 release toolchain; the explicit CAS loop preserves existing atomic semantics without changing the release recipe.
- This final validation-summary follow-up changes documentation only; its source, Cargo inputs and workflow are identical to the tested commit. Subsequent branch runs are visible in [GitHub Actions](https://github.com/wrench1997/DAW/actions); an additional documentation-only run does not replace the source-validation evidence above.
- Windows GUI/device/VST testing, helper protocol smoke, pinned gnullvm Release build, packaging and clean-system launch were **not run** in this CI. No merge into main, release, deployment or binary artifact publication was performed. Linux compilation remains blocked at the ALSA native dependency before project compilation. The known split-fade envelope gap remains open.

## 2026-10-09 02:14 UTC — Add bounded plugin-free helper protocol smoke

- Added `scripts/smoke_vst3_helper.py`: launch the built helper with redirected streams and Windows CREATE_NO_WINDOW; check three JSON replies (no-plugin error, invalid-command error, recovery), then require Shutdown exit 0 while stdin remains open. No application or helper protocol behavior changed, and no third-party plugin is loaded.
- The harness uses a five-second exchange/exit deadline, bounded output queues/diagnostic retention, concurrent stderr draining, and forced kill/reap cleanup on failure. CI adds independent outer step deadlines. Python is standard-library-only developer/CI tooling, not an application runtime or release-package dependency.
- Added 14 fake-process harness regressions covering success, malformed/unexpected responses, parser recovery, early EOF, no reply, ignored Shutdown, nonzero exit with diagnostics, stderr / stdout flooding, oversized stdout, extra stdout, timeout validation and child/pipe cleanup. Local command `python3 -B -m unittest discover -s scripts -p 'test_smoke_vst3_helper.py' -v`: **14 passed**, exit 0. These are harness tests, not Rust tests or evidence that a Windows helper has run.
- Prepared CI steps after the actual all-bin build, using the hosted Windows runner's preinstalled Python and recording its version. Existing Rust gates, features, Cargo.lock and pinned gnullvm release recipe are unchanged. Actual Windows helper smoke and repeated full CI remain **pending** until the branch workflow executes.
- Updated the release procedure to reuse the same harness for the separately built Release helper. Windows MSVC debug smoke does not attest gnullvm Release linking, packaging, GUI, real devices or plugin hosting.
