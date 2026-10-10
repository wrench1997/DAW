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

## 2026-10-09 02:25 UTC — Actual Windows helper protocol smoke passed

- Published harness/docs commit `2748a28e85c7a6bc6b91c5d4a5f7a0e4e65a0389` and workflow commit `acdcf236d49cd3e5cd4d09506856c935b79eb12a` on `ci/windows-reliability-20261009`. This milestone changes only QA tooling, CI and documentation; application/helper source, Cargo.lock, features and release toolchain are unchanged.
- [Run 37873807018](https://github.com/wrench1997/DAW/actions/runs/37873807018), [job 113637899980](https://github.com/wrench1997/DAW/actions/runs/37873807018/job/113637899980), completed successfully at 02:24:05 UTC on Windows Server 2025 / x86_64-pc-windows-msvc. Rust/Cargo 1.99.0, Python 3.12.10. All prior development gates passed again: fmt, locked all-feature/all-target Rust tests (**775 passed, 0 failed, 0 ignored**; helper Rust target 0 tests), Clippy `-D warnings`, application/helper debug build and no-default-features all-target check.
- `python -B -m unittest discover -s scripts -p "test_smoke_vst3_helper.py" -v`: **14 passed** in 5.560 seconds on Windows. The same 14 regressions also passed locally before publication. Independent read-only harness review found no blocker and verified bounded-output failure cleanup; oversized stdout and full-queue shutdown cases are included as regressions with elapsed-time assertions.
- `python -B scripts/smoke_vst3_helper.py target/debug/vst3-host-helper.exe --timeout 5`: **passed** at 02:23:50 UTC. Log: `VST3 helper smoke PASS: 3 JSON replies, invalid-command recovery, Shutdown exit 0 with stdin open; helper reaped; no plugin loaded`.
- Actual process launch and protocol exchange now have Windows execution evidence. This closes only the development helper startup/protocol gap. No real plugin, audio device, DAW GUI, fixed gnullvm Release binary, clean-system launch, signature or package was tested. No merge, release or binary artifact publication occurred.
- This result-summary follow-up changes documentation only; executable source, QA scripts, Cargo inputs and workflow are identical to the verified commit. Its branch CI will run the same gates. The checkout-v4 Node runtime deprecation notice remains nonblocking and unchanged.

## 2026-10-09 02:56 UTC — Continue with song-workflow acceptance

- Development continues beyond the passing Windows CI baseline. Reframed README/DEV_STATE/roadmap around an observable create/edit/save/recover-media/export/reopen workflow, with separate code-test, GUI/device, release-candidate and commercial-maturity evidence. Preserved all historical validation entries.
- Source inspection confirms existing autosave/restore, lifecycle guards, atomic saves, plugin-free offline export and realtime Master Capture. Corrected the stale README statement that the baseline storage/export hardening had never been compiled/tested; its regressions are covered by the already recorded Windows suite. No new execution evidence is implied.
- Two implementation slices are active in separate worktrees and not yet integrated into documentation checkpoint `598cd48`: preserving Audio Clip split envelopes/source phase, and persistent project-media diagnostics with validated undoable relinking. The integrated baseline still has both verified gaps. The media slice does not add autosave for the first time and does not claim comprehensive crash recovery or portable project packaging.
- Added explicit acceptance for repeated splits, save/reopen and undo; missing-media replacement/cancel/stale-result behavior; supported export and final-frame integrity; and clean Windows Release workflow checks. These are planned acceptance cases, not passing test results.
- Documentation-only checkpoint; no source changes, new Rust test results, push, merge or release performed by this documentation update.

## 2026-10-09 03:01 UTC — Clarify preview packaging and export acceptance

- Added the separately developed opt-in Windows MSVC/static-CRT preview-package lane to current status. It remains in progress with no Windows package execution/artifact evidence. The pinned gnullvm Release contract is unchanged.
- Source inspection at `598cd48`: `app.rs::export_wav` and `export.rs::ensure_no_active_plugins` refuse active plug-ins, and mixer graph compilation refuses active sidechains; the offline static mixer/event path does not render other automation families. `TempoMap::from_project` reads only enabled, nonempty Tempo lanes. Unsupported non-Tempo automation is not currently rejected by the export guards. Recorded explicit handling of this gap as M3 acceptance.
- Export inspection also confirms no running-job progress/cancel control and automatic whole-song gain reduction above a 0.95 peak. Documentation no longer implies cancellation exists or assumes level-identical output. These are source findings, not newly executed audio tests.
- Documentation validation: `git diff --check` passed; local Markdown-link target scan across the four maintained files passed. No application source changes, Rust tests, commit, push, merge or release were performed by this documentation checkpoint.

## 2026-10-09 03:09 UTC — Integrate reviewed project-media recovery

- Integrated reviewed source commit `f6eb7e5c177b080a66cfb9e696e4951b78ebfb45` as `4174649` on the independent development branch, based directly on `598cd48`. Preserved the four canonical documentation updates describing continuing commercial-quality/song-workflow development. No split/export/preview worktree changes are included.
- Added the persistent File → Project media / relink workflow, background WAV validation, explicit review/Apply/Cancel, stable-ID relinking with metadata guards, stale-result rejection and one-step undo/redo with runtime media invalidation. See `PROJECT_MEDIA.md` for scope, interruption behavior and manual acceptance. Existing autosave and project schema are unchanged.
- Feature validation before integration: 15 exact production-module regressions and subset Clippy with denied warnings passed in the supplemental module harness. These are not a full app build. Two new App integration tests await Windows CI. Source/UI lifecycle review found no blocking issue; real GUI/audio use is not yet validated.
- Integrated checkout `cargo fmt --all -- --check` and `git diff --check` passed. Full Windows application tests, Clippy, app/helper build, 14 harness regressions, actual helper smoke and internal-only feature check are pending the branch push. Previous 775-test baseline evidence remains historical, not a claim for this new source.
- Audio split preservation and preview packaging continue independently; no main merge, release, deployment or binary artifact publication is included in this integration.

## 2026-10-09 03:16 UTC — Media milestone passes; integrate export and preview sources

- [Media run 37877968003](https://github.com/wrench1997/DAW/actions/runs/37877968003) at `36d257749bddd31367395ff0205035556a6fbee5` completed successfully: **792 Rust tests passed, 0 failed, 0 ignored** (helper Rust target 0 tests), fmt, Clippy with denied warnings, app/helper debug build, 14 Python helper-harness regressions, real helper protocol smoke and no-default-features check all passed. The 17 new media/App tests are included; GUI/physical media scenarios remain unrun.
- Integrated reviewed export source `bcf0c7d6ae700dae3dc21de992d06d3ceabc3531` as `d04746c`; app/main auto-merge retains both media and export modules/state. Adds export progress/Cancel, atomic cancel-versus-commit destination protection, duplicate/stale-session controls and explicit active unsupported automation errors. Before integration, 44 exact-source exporter/job/egui tests (13 new), subset Clippy and fmt passed. Full integrated Windows tests remain pending.
- Integrated reviewed preview source `008217a80c63eabc0b31b3cc5313619cbd7a35cc` as `f4bd5ef`. Adds pinned MSVC/static-CRT optimized build orchestration, strict PE/runtime and document/archive validation, sanitized provenance/checksums and extracted-helper smoke. Local packaging + helper regressions passed (49 tests before the export-doc whitelist follow-up). Actual Windows package checks remain pending.
- The preview workflow is published separately through the workflow-capable connection; source pushes do not add workflow files. Its scoped branch trigger defaults to no artifact upload, and does not merge main, tag a release, deploy, change secrets or replace the official gnullvm release recipe.
- Canonical documents now distinguish the green media checkpoint from the new export/preview candidate. Split-fidelity source is still not integrated. Local integrated fmt and diff checks passed before publication.

- Integration packaging follow-through: explicitly allowlisted `docs/OFFLINE_EXPORT_WORKFLOW.md` beside the media guide, with a regression verifying both guides and their README links are packaged together. Full local Python discovery passed **50 tests** (36 packager + 14 helper harness); real-source packaged Markdown-link validation passed.

## 2026-10-09 03:24 UTC — First preview run exposes Windows fixture newline issue

- Workflow `43e7e418cf0dbb895593f3dc74d6d85ef1d6b2c2` started [preview run 37878961519](https://github.com/wrench1997/DAW/actions/runs/37878961519). Its Python stage ran 50 tests and failed with 14 fixture errors before any static-CRT application validation or package creation. The ordinary quality run is separate.
- Every error was the synthetic Cargo.lock integrity guard: Windows text-mode fixture writing converted LF to CRLF, but the fixture expected a hash of the pre-write LF string. Changed only the test fixture to hash actual persisted bytes and added explicit CRLF coverage. The production lock-integrity guard is unchanged.
- After correction, local Python packaging/helper discovery passed **51 tests**. Fresh Windows execution is still required; no preview package or upload is claimed from the failed run.

## 2026-10-09 03:38 UTC — Export passes; integrate v11 split-fidelity prerelease

- [Quality run 37878962285](https://github.com/wrench1997/DAW/actions/runs/37878962285) at `43e7e418cf0dbb895593f3dc74d6d85ef1d6b2c2` passed all Windows gates with **805 Rust tests / 0 failed / 0 ignored**, 14 Python helper-harness tests, actual helper smoke, fmt, Clippy, app/helper build and no-default check. Preview run remains a separate failed fixture run, not successful package evidence.
- Integrated split source `8287536` as `ba6a27f` and callback assertion/evidence follow-up `f2f9f47` as `21b716b`. Feature owner resolved the two export conflicts, retaining cancellation/progress/preflight and exact source/fade/end behavior, and adapted three test helper calls. Other files auto-merged; no media Clip literal required new fields.
- Project format is now v11 with legacy v10 input compatibility; application version explicitly changes to **0.5.0-alpha.1**. Root Cargo.toml/Cargo.lock version changes only; dependency versions and pinned release-toolchain recipe are unchanged. The feature deliberately enables serde_json float_roundtrip for exact persisted clock/domain parsing. New saves need the newer application; preserve existing project backups.
- Before integration, split validation included 200 exact-source tests, strict subset Clippy, independent numerical review, three real callback tests and four App/history tests. The merged candidate now passes real Linux `cargo check --offline --locked --no-default-features --all-targets`; full merged tests are running. This is not Windows/VST/GUI/device acceptance.
- Added the split guide to the explicit optional package-document whitelist and a prerelease-version archive regression. Local Python packaging/helper discovery passed **52 tests**; the production lock-integrity guard remains strict. Fresh Windows quality and static-CRT preview validation will run after the integration checkpoint is published.

- Merged full Linux no-default test execution **aborted with stack overflow** in `audio::tests::atomic_activation_releases_mixer_pan_after_preflight_and_before_identity_publish`; no complete passing count was established. Focused comparison reproduces failure on split-only/merged binaries at 2 MiB stack while the pre-split binary passes; 4/8 MiB diagnostic reruns pass. This is a source-growth regression under investigation, not a passing gate or a reason to suppress the Windows test.
- The reviewed WAV export-options slice is being integrated next: settings/review, PCM16/PCM24/float32 and explicit peak-attenuation/preserve-level choice. Its focused guide is explicitly included in the preview whitelist alongside media/split/export guides; package link checks stay strict. Full source validation follows conflict resolution.

- WAV options `44876da` integrated as `660d310`; owner resolved modal/export-test conflicts and reviewed exact unions. Both media and WAV modal priority/Escape/Enter/pointer guards remain, cancellation/preflight and v11 prepare/mix code are preserved. Existing default PCM24 golden bytes are retained; new PCM16/float32/rate/level controls still need combined Windows validation.
- Stack root cause isolated: event descriptor growth enlarged the 4096-event TimelinePacket, and boxed construction was still materializing large payloads on the stack. An in-place boxed constructor with low-stack regression is being prepared; no stack-limit increase or Windows-test removal is used as a fix.

## 2026-10-09 03:52 UTC — Complete native prerelease suite passes on default stack

- Verified merged source commit `bb5817118b16fde04689d21131fdf1510a177713` (0.5.0-alpha.1). `cargo check --offline --locked --no-default-features --all-targets` passed. `cargo test --offline --locked --no-default-features --bin citrus-studio` completed with **860 passed, 0 failed, 0 ignored**, using the default test stack and real linked installed ALSA runtime. The previously failing atomic activation and new 64KiB/zero-capacity packet construction tests both pass.
- Integrated narrow heap fix `3ccaf104` as `845fbc2`: allocate TimelinePacket directly on the control-thread heap, initialize required scalar fields in place, leave its existing MaybeUninit event storage untouched. Independent static review and 201-source subset tests passed. Refill/audio callback allocation behavior is unchanged; no stack/profile increase or test suppression was used.
- Initial complete rerun reached859 passes with one historical Windows-path fixture failure on Linux. Only its non-Windows test path is now POSIX-native; Windows retains the original backslash fixture and production normalization is unchanged. The final complete860-test run passed.
- Corrected two split App-test default-field Clippy findings. Local diagnostic no-default Clippy exits 0 with only inherited Linux MIDI unused/dead-code warnings; this is not a claim of strict Windows Clippy completion. Full fmt and diff checks pass.
- Packaging/helper Python discovery passes52 tests; explicit feature-document allowlist includes media recovery, cancellation/export contract, split fidelity and WAV options, and all14 packaged source-document links validate. The Windows fixture CRLF fix is included; production checksum enforcement remains unchanged.
- The following canonical documentation checkpoint changes no executable inputs. A single branch push will run both unweakened Windows quality and pinned static-CRT preview workflows. Main, official release tooling, publishing defaults and credentials remain unchanged.

## 2026-10-09 03:57 UTC — Windows packaging tests pass; repair VS shell initialization

- [Preview run 37881375728](https://github.com/wrench1997/DAW/actions/runs/37881375728) at `a760313d599f458976ad6f9fd8e31acd688d2906` passed all **52 Python packaging/helper tests**, including the corrected CRLF and prerelease fixtures.
- The next build step failed before Rust validation/build because nested cmd.exe quoting treated the installed Visual Studio path as an invalid command. No package was created or uploaded.
- Replaced shell-string parsing with the installed `Launch-VsDevShell.ps1`, explicitly selecting amd64 host/target and preserving the repository working directory, as recommended by [Microsoft's build-automation documentation](https://learn.microsoft.com/en-us/visualstudio/ide/reference/command-prompt-powershell?view=visualstudio). Existing toolchain/linker pinning, source-cleanliness checks and package guards remain intact. Fresh Windows execution is required to validate this wrapper repair.

## 2026-10-09 03:59 UTC — v11 prerelease passes complete Windows quality gates

- [Quality run 37881375638](https://github.com/wrench1997/DAW/actions/runs/37881375638) at `a760313d599f458976ad6f9fd8e31acd688d2906` is **successful**: Windows all-feature/all-target Rust **862 passed, 0 failed, 0 ignored**, helper Rust target0 tests; fmt, strict Clippy, app/helper debug build,14 Python helper tests, real helper smoke and no-default all-target check all passed.
- Counts are target/feature-specific: the real Linux no-default full suite passed860. Both cover the coherent0.5.0-alpha.1/v11 source, including packet heap initialization, media, split, cancellation and WAV options. Linux is the primary development/test environment going forward; Windows remains compatibility/Windows-feature/package validation.
- Preview run37881375728 independently passed52 Python tests but failed VS shell initialization before compiling/packaging. Its narrow installed-PowerShell-launcher fix is ready; no package/artifact success is inferred from quality CI.
- This follow-up changes packaging orchestration and documentation only; application/helper source and Cargo inputs remain identical to the fully verified candidate. No additional feature merge is mixed into the wrapper repair.


## 2026-10-09 04:18 UTC — Real mixer measurement and strict Linux integration

- Integrated reviewed `bc70b32` and `9a7b9ddf` as `21989d0` and `ad565d7`. Mixer strips now display real post-fader stereo sample peaks rather than synthetic sine motion; Master measures before output protection. dBFS, peak hold, resettable clip/fault, graph/epoch/stable-ID mapping, bounded full-queue peak retention, stale/no-device cleanup and idle live-MIDI repaint are connected without changing Project/history or DSP output. See `MIXER_METERING.md` for precise tap/identity/acceptance scope.
- Current source **`4045cb24da846057461dd6e464b1c90058a0835f`** passes Linux `cargo test --offline --locked --no-default-features --all-targets`: **878 passed, 0 failed, 0 ignored**, default test stack. `cargo fmt --all -- --check`, `cargo clippy --offline --locked --no-default-features --all-targets -- -D warnings` and `cargo build --offline --locked --no-default-features --bins` pass. Debug/test symbols and incremental compilation were disabled for resource use; no stack increase or skipped test was used. Linked to the installed ALSA runtime through verified local metadata. This configuration excludes VST2/VST3.
- Initial strict Linux Clippy exposed inherited platform-only MIDI dead code/imports. Corrected conditional compilation for Windows/test-only implementation helpers and moved BTreeMap into the Windows module; all portable tests remain compiled and Windows backend behavior is unchanged. No `allow` override or relaxed lint gate was added. A complete rerun after that correction passed.
- The prior [Windows quality run 37881848416](https://github.com/wrench1997/DAW/actions/runs/37881848416) at `2c9d81174e676272c70666ac56519084d7d55d20` succeeded through every step. It predates meter integration; its 862-test evidence does not automatically validate the new candidate.
- Prior [preview run 37881848475](https://github.com/wrench1997/DAW/actions/runs/37881848475) passed 52 Python regressions, all Rust gates and the actual optimized static-CRT build. It then failed PE auditing with `Non-Windows runtime import combase.dll`. Microsoft explicitly lists ComBase.dll as the Windows Runtime/COM system DLL in [WindowsPreallocateStringBuffer requirements](https://learn.microsoft.com/en-us/windows/win32/api/winstring/nf-winstring-windowspreallocatestringbuffer). Added only that exact OS import and normal/delay-import regression; unknown DLL, dynamic CRT, archive and provenance guards remain strict. No DLL redistribution, verified ZIP or extracted-helper success is claimed from the failed run.
- Added the metering guide to the explicit package-document allowlist and feature-guide/link regression. Full local Python discovery passes **53 tests** (39 packaging + 14 helper). Fresh Windows quality/preview runs remain required for this candidate. Artifact upload stays disabled; no main merge, Release, tag, deployment or binary publication was performed.
- Corrected stale roadmap language: aggregate M1 split and M3 export/cancellation source gates passed at earlier checkpoints; remaining broader acceptance is real GUI/media/device song workflows. Linux binary build/launch has not yet produced a reliable visible UI in the cloud renderer, so no GUI pass is recorded.

## 2026-10-09 04:33 UTC — Local WAV Browser replaces placeholder Sounds

- Implemented in a separate worktree based on `0b9cfa1`, branch `feat/local-sample-browser-20261009`; not yet integrated or pushed at this checkpoint. SOUNDS now uses explicit local folder selection, bounded nonrecursive directory/WAV listing, current-list filtering, Up/Refresh/Cancel, exact selection and the shared File-menu WAV import. Removed fake sample names, synthetic Browser waveform and nonfunctional audition prompt; audition remains explicitly unavailable.
- One directory worker, one coalesced latest request and a one-result mailbox cap retained entries at 512 and inspected entries at 4,096. Stale/canceled success and failure results cannot replace the current folder. Discovered symbolic links are skipped. No default disk scan, new preference persistence, remote upload, sample file write or new callback work.
- Import now prepares a Project candidate and commits one explicit undo transaction, preserving the existing asset/clip/native-frame/Mixer routing and background decoder. Worker spawn/decoder errors are visible; duplicate imports are blocked while pending. The WAV reader now bounds actual reads, including a separate one-byte over-limit probe, rather than relying only on a pre-read file-size check.
- Independent source review found two issues before acceptance: non-UTF-8 paths could make the existing JSON Project unsavable, and an asynchronous completion could be overwritten by a transform/gesture snapshot. Both were fixed and re-reviewed: shared decode/preparation refuses non-UTF-8 file or ancestor paths before mutation, checks that history really committed before runtime registration, and defers result consumption until snapshot-owning edits/save/recording/lifecycle/dialog barriers end. The existing project generation is rechecked afterward.
- Final Linux checks on this source: `cargo fmt --all -- --check`; `cargo test --offline --locked --no-default-features --all-targets` **901 passed / 0 failed / 0 ignored**; `cargo clippy --offline --locked --no-default-features --all-targets -- -D warnings`; and `cargo build --offline --locked --no-default-features --bins` all passed. Uses the shared native target and real installed ALSA runtime metadata; no warning suppression, enlarged test stack, added dependency or full temporary target. `git diff --check` passed.
- The 23 new regressions comprise 13 scanner/state tests, nine shared import/preparation/history/barrier tests and one bounded-reader regression. They include canceled/stale/latest navigation, exact path identity and symbolic-link cycles, candidate limits, malformed/deleted imports, UTF-8 persistence restrictions, Cancel/gesture history ordering and stale-session deferral.
- Added [Local sample browser](LOCAL_SAMPLE_BROWSER.md) workflow/limits/manual acceptance documentation and narrow README/DEV_STATE/parity pointers. Native Linux picker/GUI layout, audible output, physical devices and Windows all-feature validation of this slice remain unverified; no release/package or commercial-readiness claim.


## 2026-10-09 04:37 UTC — Meter Windows gates pass; integrate real local sample browsing

- [Quality run 37883317515](https://github.com/wrench1997/DAW/actions/runs/37883317515) at `0b9cfa1a5d7e8c3384bc3ac435bcedc834c005af` succeeded with **880 Windows all-feature/all-target Rust tests, zero failures/ignored** (helper Rust target 0), fmt, strict Clippy, app/helper build, 14 Python helper tests, actual 3-reply helper smoke and no-default all-target check. This closes the metering source checkpoint's Windows gates, not real-device/GUI acceptance.
- [Preview run 37883317601](https://github.com/wrench1997/DAW/actions/runs/37883317601) at the same SHA passed 53 Python regressions, all 880 Rust tests, strict gates and optimized static-CRT app/helper builds. ComBase.dll was accepted after the previous exact-name repair; the next package audit rejected `uiautomationcore.dll`. The ZIP and extracted-helper stages did not complete, and artifact upload was skipped.
- Commit `119d2a4` adds only the UIAutomationCore.dll OS component documented in Microsoft's [UiaHostProviderFromHwnd requirements](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationcoreapi/nf-uiautomationcoreapi-uiahostproviderfromhwnd). Normal and delayed imports are both checked; all unapproved import names and both binary rejection reasons are now reported together before package failure. Added three regressions, retaining unknown/dynamic-CRT/path/structure rejection. No DLL is bundled or new code dependency introduced.
- Integrated reviewed local-sample-browser `3e4335e` cleanly as **`abc9ec888a5b4c96ca229cc8544d15aabf3700ec`**, directly over the measured-meter checkpoint and package-only repair. Complete rerun on this combined source: `cargo test --offline --locked --no-default-features --all-targets` **901 passed / 0 failed / 0 ignored**; fmt, no-default all-target Clippy with `-D warnings`, and app debug build passed. Default test stack, real installed ALSA runtime, no-default feature boundary preserved. This is a real full-suite rerun after integration, separate from the feature-owner's prior run.
- Explicitly allowlisted `LOCAL_SAMPLE_BROWSER.md` and extended the joint feature-guide/link regression. Full Python packaging/helper discovery passes **56 tests** (42 packager + 14 helper); packaged source-document/link checks include all 16 documents. No arbitrary documentation directory copy or import allowlist relaxation.
- Actual Linux GUI probe at `4045cb24` created a native X11 window but Mesa `eglSwapBuffers` failed with `EGL_BAD_SURFACE` at `xcb_shm_attach_checked`. Repeated supported probes could not present the DAW. Create/edit/undo/save/reopen, native WAV picker/cancel, meter GUI and physical-device flows remain **BLOCKED / NOT RUN**. No physical audio device was present. Source/headless/callback tests are not recorded as real GUI acceptance.
- Canonical evidence now separates the green metering Windows checkpoint from the new sample-browser candidate. One new branch push will run the combined source and narrow packaging repair; no main merge, release, tag, deployment or binary artifact upload is included.


## 2026-10-09 04:58 UTC — Browser Windows gates pass; integrate production UI QA and layout fixes

- [Quality run 37884873705](https://github.com/wrench1997/DAW/actions/runs/37884873705) at `0ff5e6e9664e16108f9b3e74acda90ad7a0a985a` passes all Windows gates: **900 all-feature/all-target Rust tests**, 0 failed/ignored, helper target 0; fmt, strict Clippy, app/helper build, 14 Python harness regressions, actual helper smoke and no-default all-target check. Its executable source separately passes 901 Linux no-default tests. Counts are platform/feature-specific rather than additive guesses.
- [Preview run 37884873710](https://github.com/wrench1997/DAW/actions/runs/37884873710) passed 56 Python tests, 900 Rust tests, strict gates and actual optimized app/helper build. Combined import auditing accepted prior ComBase/UIAutomationCore repairs and reported only `bcryptprimitives.dll` in both binaries. No validated ZIP or extracted-helper smoke resulted; upload was skipped and the artifact API reports zero artifacts.
- `2aacc09` adds only the exact BCryptPrimitives.dll OS import documented by Microsoft's [ProcessPrng requirements](https://learn.microsoft.com/en-us/windows/win32/seccng/processprng), plus normal/delay regression. Unknown/dynamic-CRT/path/structure guards remain intact. Current full Python discovery passes **57 tests** (43 packager + 14 helper); fresh Windows package execution remains required.
- Integrated reviewed production UI QA `cb49961` as `1663f8f` and responsive/About follow-up `493cc89` as **`dda0ccadbb5e7351889f9d085727c9e344ff6119`**, with no conflicts or added dependencies. The harness runs real CitrusApp::ui, actual egui input, production directory/import workers and history, while deliberately excluding native picker/device/profile side effects. The feature owner's eight input flows and 17 genuine Vulkan offscreen checkpoints found and verified fixes for narrow Plugins/Mixer and Group/Snap overlap. About now reports actual build platform and observed host/offline state. These captures are not native desktop screenshots or hardware acceptance.
- Full rerun on integrated source: `cargo test --offline --locked --no-default-features --all-targets` **910 passed / 0 failed / 0 ignored**, default stack; fmt, no-default all-target Clippy `-D warnings` and app debug build passed. The feature owner also passed Windows MSVC no-default all-target cross-check; that is not Windows runtime evidence. No integration-source changes followed the aggregate run.
- Explicitly added `HEADLESS_UI_QA.md` to the reviewed package-document whitelist and joint feature-guide regression. All **17 packaged source documents** and relative links validate. Canonical documents distinguish source/test/offscreen/native/physical-device/package evidence and preserve the earlier actual X11 EGL_BAD_SURFACE blocker.
- The exact prior `0ff5e6e` source snapshot has a clean-original-base patch whose applied files match the Git archive byte-for-byte, plus an external terminal-validation receipt. This is a source deliverable, not an executable release. Current UI/packaging changes await the next independent-branch CI run; no main merge, release, deployment or binary publication is included.

## 2026-10-09 05:25 UTC — First full preview pass; integrate protected native VST3 editor source

- Exact checkpoint **`53494d518bfc4bad7304f25c127491a0c67d7cd8`** passed [Windows quality 37886481852](https://github.com/wrench1997/DAW/actions/runs/37886481852): **909 app Rust tests**, helper target 0, fmt, strict Clippy, app/helper builds, 14 Python helper regressions, actual helper protocol smoke and no-default check. The matching [preview 37886481857](https://github.com/wrench1997/DAW/actions/runs/37886481857) passed **57 Python tests**, all 909 Rust tests, optimized static-CRT app/helper build, exact PE/import auditing, ZIP/hash/provenance validation and actual extracted-helper smoke under sanitized PATH. This is the first complete preview-lane pass. Upload was skipped and the artifact API reported zero artifacts. It is not fixed gnullvm Release, clean-machine, native app GUI or hardware acceptance.
- Integrated reviewed native-editor feature commits as `30d327b`, `330f601`, `56fa97f` and `37ae191`; excluded duplicate prerequisites and kept workflow changes separate for the workflow-capable publication route. Windows VST3 native Open/Close controls now route to helper-owned containers with exact session/endpoint/instance/generation identity, dirty revision and durable capture. Saved generic parameter bases survive later saves, missing IDs fail visibly, and native edits detach/flush before stopped-state capture. Automation-linked instances (including stopped/unplaced lanes) are deliberately excluded. Generic editing, topology, Undo/Redo, transforms and project transitions respect native capture ownership. Linux and VST2 native editors remain explicitly unsupported.
- `df1ba2b` integrates the independently reviewed packaging change: only exact root identity and `vendor/vst3-host-0.9.0` MIT path override are allowed, with original LICENSE, CITRUS_PATCHES.md, CITRUS.patch and upstream VCS metadata pinned to their reviewed hashes. Arbitrary path/git/registry/replacement overrides, altered provenance and symlink/reparse escapes remain rejected. The original archive checksum is labeled as upstream input, not a hash of the modified dependency. `8c09f45` pins vendor checkout to LF and points the packaged guide to the exact integrated fixture source. Actual Windows-resolved Cargo metadata validates **234 packages / one reviewed vendor**; all **22 source-document/provenance inputs** have closed relative links.
- Fresh integration review discovered an additional **older Browser baseline bug**, not an editor-merge regression: a whole-Project generator replacement deferred for MIDI teardown could overwrite a just-completed WAV import. `9888264` adds one explicit project snapshot-transition predicate shared by native Open and import completion; result consumption still performs the existing generation/session checks afterward. The production-app regression covers successful and terminally failed replacement and separate import Undo. Transient replacement retries retain the barrier. Independent re-review found no remaining blocker. External teardown is injected at the completion seam and the success case uses the no-plugin path; no live MIDI/VST replacement acceptance is inferred.
- Final executable source **`8c09f457bc994271d9d8bff824b38960bb240131`** was rerun completely on Linux: `cargo test --offline --locked --all-features --all-targets` **936 application + 14 helper + 5 protocol passed**; `--no-default-features --all-targets` **934 application passed**, all zero failed/ignored. `cargo fmt --all -- --check`, both all-target strict Clippy configurations, and all-feature app/helper build passed. Default stack; debug symbols/incremental disabled for resources, real installed ALSA/XCB runtimes, no lint suppression added. One inherited dependency deprecation warning remains inside vendor `internal/data_exchange.rs`. Actual Linux helper smoke passed three replies, invalid-command recovery, explicit Shutdown and reaping. Full Python discovery passed **153 tests**.
- `2073863` strengthens the source-only MIT fixture harness with native dirty revision and stopped-state SaveState/restore, exact component/controller byte round-trip and restored Cutoff/project context. The legacy parameter drain happens only after state capture. The proposed Windows quality workflow builds only receipt-verified checked-in fixtures with locked/offline dependencies and executes the actual production helper with bounded deadlines. Native exit 77 remains explicitly UNSUPPORTED / NOT VERIFIED and non-passing. Local Linux native execution returned 77 as expected; proposed workflow syntax/source checks and Python tests are not Windows execution.
- Fresh Windows quality/native-fixture/preview runs for this combined source remain required. Native desktop presentation/file dialogs, real vendor plug-ins, physical audio/MIDI, sustained performance and clean-system release acceptance remain open. No main merge, Release, tag, deployment or binary artifact publication is included.

## 2026-10-09 05:42 UTC — Windows source gates pass; retain native paint failure and repair portable test input

- Workflow connector commits `5500a07` and `b76ba322` publish the reviewed fixture runtime steps and narrow preview source filters. The local checkout was fast-forwarded and both files byte-compared against the approved proposals. No artifact upload or release step was enabled.
- At **`b76ba3228053c5c5af63e5621df999553543a69d`**, [quality 37888700443](https://github.com/wrench1997/DAW/actions/runs/37888700443) passed **933 application + 13 helper + 5 protocol Rust tests**, fmt, strict Clippy, all-bin builds, 14 helper and 47 editor Python tests, actual ordinary helper smoke, no-default check and the fixture's strict Clippy/build/receipt verification. Native fixture execution passed visible/input-desktop prerequisites, actual helper-owned attach, exact 560×400 resize/content-scale request and Rust/Win32/CRT stdout isolation. It **failed** when PrintWindow returned false for the real native button. Exit 1 is a runtime paint failure, not unsupported-desktop exit 77. Later input/state/lifecycle cases were not reached. The overall workflow is failed; the successful source stages are recorded separately, not relabeled as full native acceptance.
- [Preview 37888700426](https://github.com/wrench1997/DAW/actions/runs/37888700426) failed before compilation in one of 153 Python tests: an absolute Windows path was inserted into a negative TOML fixture without string escaping, causing parser rejection before the intended packaging-policy check. The fixture now quotes the entire path, tests an explicit Windows backslash path on every platform and checks TOML parsing before requiring PackageError. Independent review accepted this test-only repair. Production dependency/path/DLL allowlists are unchanged.
- `ac73000` adds bounded failure-only diagnostics around the exact same native PrintWindow gate: cleared/read advisory last-error, validated live target identity/visibility, a same-process owned standard-button comparison and documented flags=0 probe. The probes share the existing five-second capture-child deadline and cannot satisfy acceptance; timeout stderr is bounded. No desktop permissions, security settings, unrelated windows or arbitrary plug-ins are accessed. Microsoft does not document a last-error contract for [PrintWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-printwindow), so the diagnostic code is not used to reinterpret success. This is a diagnostic rerun, not a claimed fix for the paint failure.
- The combined Python suite now passes **157 tests** locally; package document/provenance link closure and `git diff --check` pass. Application/helper Rust, Cargo inputs and vendored code are unchanged from the previously tested source. Fresh Windows preview and native diagnostic execution remain required. Broader native app, real vendor/device and release acceptance remain open.

## 2026-10-09 05:54 UTC — Native inspector actions wrap without changing editor safety

- Reproduced actual generator clipping and Mixer status/action crowding on `76232d9` using production `CitrusApp::ui`, real accessibility geometry and Vulkan offscreen captures. The generator's nonwrapping action row extended beyond the viewport; the Mixer's right-to-left action child extended over earlier status content. Oversized children also displaced the inspector's recorded bounds.
- Changed only presentation: generator actions wrap; Mixer status retains its own row and actions wrap below it. Native command dispatch, owner validation, runtime state access, automation exclusion and snapshot/save/topology barriers remain unchanged. A `cfg(test)` capability fixture is consumed only by the two inspector render sites; the test explicitly confirms no runtime native snapshot or plug-in chain exists.
- The added full-app regression checks the first requested 240/340-point frame, then settled visible/nonoverlapping action and status bounds, supported/open/closed/pending/unsupported/no-editor presentation, and repeated real Replace/LOAD → Escape/reopen with unchanged project data. Existing content constraints settle the 240 request to about 258 points in Rack and 270 in Mixer; this is documented rather than reported as a fixed 240-point inspector.
- Final Linux `--offline --locked` all-feature/all-target execution: **943 application + 14 helper + 5 protocol passed**; no-default/all-target: **941 application passed**. All have zero failures/ignored tests and use the default test stack. Formatting and both all-target strict Clippy configurations pass; the inherited vendored deprecation warning remains unchanged.
- The complete explicit Vulkan capture suite passes **12 tests** and records **23 genuine offscreen app renders**, four showing the clearly labeled native-editor UI fixture. All four new PPM-to-PNG conversions have identical RGB pixels, and before/after images were inspected. These are presentation regressions, not native desktop screenshots or Windows editor runtime verification; Linux native editors, physical hardware and real vendor acceptance remain unsupported/unverified as previously documented. No dependency/vendor changes or publication are included.

## 2026-10-09 05:58 UTC — Integrate FL-inspired design and verified narrow Inspector repair

- User requested an FL Studio-inspired appearance. Reviewed `e7966e0` integrates cleanly as `76232d9`: native egui transport/navigation, Browser, Playlist and Mixer hierarchy; real stored note/step previews, decoded waveforms and measured meter inputs; no copied Image-Line assets or decorative fake signal data. Source and visual input/render evidence are documented in [FL-inspired native theme](FL_INSPIRED_NATIVE_THEME.md). Independent integration review confirmed identical patch identity and preservation of native editor actions/ownership, automation/state capture and shared import-transition guards.
- Review then identified an older inspector layout risk. Actual full-app reproduction at requested 240/340 widths showed generator controls outside the viewport and Mixer actions overlapping status, with rows expanding their intended panel. `aa46f0f` integrates as **`37bfabab8c9a6f6c065702aa6ebfa5a7f3eb04a4`**, wrapping generator actions and separating/wrapping Mixer actions. The regression covers first/settled bounds, open/closed/pending/unsupported/no-editor presentation and repeated Replace/LOAD→Escape without Project changes. Its capability snapshot is cfg(test), instance-scoped and render-only; command/runtime guards read real state. Existing unrelated content minima may settle a 240 request to 258/270 points, explicitly documented. Independent review found no blocker. This is a newly reproduced layout correction, not native Windows execution.
- Complete final merged Rust rerun at `37bfaba`: **943 app  + 14 helper  + 5 protocol all-feature/all-target tests**, **941 no-default app tests**, zero failed/ignored; fmt, both strict Clippy configurations and all-feature app/helper build pass on default stack. Actual Linux ordinary helper smoke passes. The final Vulkan-only UI rerun passes **12 real app flows /23 genuine app renders**; lossless PNG conversion is RGB-verified. No source regression or fake desktop/device pass is inferred.
- Prior published **`8961529b02a04ade3ba11626f5118b0d7875d760`** now has terminal Windows evidence. [Preview 37889957208](https://github.com/wrench1997/DAW/actions/runs/37889957208) passes 157 Python tests,933 app  + 13 helper  + 5 protocol Rust tests, strict gates, optimized static-CRT build, exact vendor-license/provenance/PE/ZIP/hash validation and actual extracted-helper smoke. Upload skipped; artifacts API 0. This validates the path-fixture escaping repair and the complete vendor-aware package lane. [Quality 37889957193](https://github.com/wrench1997/DAW/actions/runs/37889957193) passes all source gates, 14 helper /51 editor Python tests, trusted fixture build/receipts and partial native attach/resize/stdout isolation, but fails native paint. Both the trusted helper control and same-process standard-button probes reject both PrintWindow modes; all advisory last-error values are 0. The full native acceptance result remains failed, not a proven helper-only defect or a successful GUI flow.
- `1fb0860` aggregates independent native acceptance evidence without weakening paint. Each stage reports PASS/FAIL/SKIP; any failure/skip keeps overall failure. Fresh trusted HWND/PID/class/ancestry/control-ID/geometry validation precedes native events. A non-paint prerequisite failure stops dependent actions; fresh-helper Shutdown/EOF/crash checks remain bounded. New state/lifecycle outcomes await actual Windows execution. No speculative driver/security changes or arbitrary vendor inputs.
- Current full Python discovery passes **163 tests**. The new visual guide is added explicitly to the package allowlist and joint link regression, without broad directory copying. All 23 packaged document/provenance inputs have closed relative links. Runtime Rust/Cargo/vendor source is unchanged after the final Rust/UI gates; follow-up changes are Python/docs only. New Windows runs for this visual/inspector/aggregate candidate are still required.
- The user also explicitly requested simultaneous FL-style editor windows. That shared-project internal floating-workspace work is proceeding separately and is not included or claimed in this checkpoint. Native OS window detach remains a separate capability. No main merge, Release, binary upload or commercial-ready claim.

## 2026-10-09 — Simultaneous internal editor windows

Implemented the production multiwindow workspace: Playlist, Channel Rack, Piano Roll
and Mixer open together, with movable/resizable/closable window chrome, F5/F6/F7/F9
show-and-focus, persisted versioned geometry/stacking/visibility/focus, offscreen
recovery, Arrange/Cascade and maximize/restore. A single Project, transport, audio
engine, plug-in runtime and history remain authoritative. Explicit editor context
prevents a focused Piano toolbar from changing Playlist tool/snap bindings.

Actual input regressions cover title/edge interactions without musical edits,
first-press background Mixer controls, shared channel/Pattern updates, active-editor
Delete/Duplicate plus global Undo, text-field isolation, interrupted note/clip
body/resize gestures, modal z-order and modal-owned drags, layout restore and
hidden-editor safety. The native plug-in lifecycle and shared save/import/transition
guards remain intact. Window sizing probes no longer reset the Piano pitch origin.

Final isolated-source gates: 954 app +14 helper +5 protocol all-feature tests; 952
no-default tests; both strict Clippy modes; format; all-bin build; Windows MSVC
no-default/all-target cross-check; 20 actual UI flows and 27 Vulkan offscreen frames.
PNG conversion is RGB-identical. Independent source review has no outstanding
high-priority findings. Detailed controls and acceptance boundaries are documented
in `docs/MULTIWINDOW_WORKSPACE.md`. This implements internal native-app windows,
not detached OS editor windows, and does not establish physical hardware or native
Windows desktop acceptance. No publication was performed from this worktree.

## 2026-10-09 06:20 UTC — Integrate simultaneous editor workspace and source-grounded fixture assertion

- The user explicitly requested FL-style simultaneous windows; the final genuine overview and independent feature/input-safety review were approved. Integrated only `8b9f2c7` as **`bffc6f48f47dfe241809a852951bb393ab3ea402`**, excluding its duplicate Inspector prerequisite. Runtime files merged cleanly; append-only HEADLESS_UI_QA/WORK_LOG conflicts were resolved by preserving both records. Independent integration review confirms the entire src tree matches the reviewed feature and retains native owner/state/automation/save/import guards and test-only Inspector presentation seam.
- Playlist, Channel Rack, Piano Roll and Mixer now coexist as real movable/resizable internal egui windows sharing one Project, transport, audio engine, plug-in runtime and history. Versioned geometry/visibility/stacking/focus, close/reopen, Arrange/Cascade and maximize/restore are connected. Centralized once-per-frame shortcut dispatch, visible focus, text ownership, modal stacking, interrupted-drag cancellation and cross-editor history boundaries are exercised by actual input. Native OS detach is not included. See [Multiwindow workspace](MULTIWINDOW_WORKSPACE.md).
- Full fresh integration gates at `bffc6f4`: **954 app + 14 helper + 5 protocol all-feature/all-target tests**, **952 no-default app tests**, zero failed/ignored; default stack; fmt, both strict Clippy configurations, all-feature app/helper build and actual ordinary helper smoke passed. Fresh Vulkan capture passed **20 real app UI flows /27 genuine app renders** with RGB-identical PNG conversion. Normal/minimum workspace pixels were inspected. This verifies actual shared app input/render code, not native desktop presentation, hardware or detached OS windows.
- Earlier exact **`2e4988348270c39a037ba042f753380fefc73ba3`** is now terminal on Windows. [Preview 37891374470](https://github.com/wrench1997/DAW/actions/runs/37891374470) passed **163 Python tests**, **940 app + 13 helper + 5 protocol Rust tests**, strict source gates, optimized static-CRT build, exact vendor/license/PE/ZIP/hash verification and extracted-helper smoke; upload skipped and artifacts API 0. [Quality 37891374461](https://github.com/wrench1997/DAW/actions/runs/37891374461) passed source gates, 14 helper /57 editor Python tests and fixture build/receipts. Actual native interaction/ordered gesture/dirty, repeat focus/owner rejection and Shutdown/EOF/crash cleanup passed. Paint before/after failed and repaint comparison was SKIP. SaveState/detach succeeded, but the old feedback-count assertion stopped the state stage; restore and later lifecycle were SKIP. Overall quality remains failed, not silently accepted.
- Source analysis establishes the trusted fixture's exact two legacy feedback records: DSP output echo first, then the GUI performEdit stash retained across zero-sample flush. `dd73939` integrates as `c5c1d43`, requiring precisely two typed identical Cutoff=0.25 records and exactly one revision increment per value/dirty callback. Regressions reject wrong counts, wrong IDs, divergent values in either position, malformed types, missing/reordered/extra gesture bounds, revision drift and changed restored state bytes. No host/vendor/fixture code changes or generic duplicate normalization; this contract is fixture-specific. Actual corrected Windows state restore remains pending, and paint remains separately failed.
- Current complete Python discovery passes **167 tests**. The workspace guide is explicitly allowlisted with a joint packaging-link regression; all **24 packaged source-document/provenance inputs** have closed relative links. Canonical docs now describe implemented internal windows and retain detached-OS/native/device limitations. Current source publication triggers new exact Windows checks; after terminal results, the user-facing source snapshot will be refreshed with an exact revision and validation receipt. No main merge, Release, binary artifact upload or commercial-ready claim.


### 2026-10-09 — Isolated Piano note clipboard

- Added focused Piano target-channel select-all and session-local Copy/Cut/Paste.
  Explicit Copy/Cut writes bounded, versioned Citrus note JSON through egui's
  platform output; semantic Paste validates that payload. The Paste notes button
  uses the typed local copy. Other editor canvas clipboard operations stay
  unsupported, and normal text/numeric editing retains its clipboard ownership.
- Preserves source channel, pitch, relative time, length, velocity and mute values;
  regenerates global note IDs and independent pattern-local groups; commits each
  Cut/Paste as one history transaction. PAT uses the snap-down local transport
  cursor, SONG uses Pattern beat zero, with the anchor visibly labeled. Project
  replacement invalidates both old text payloads and the local copy. No assets,
  source paths or MIDI interchange format is copied.
- Fourteen added tests cover bounded data, full-limit linear selection expansion
  and ten actual-app pointer/key flows. Exact final Linux gates pass 966 no-default
  app tests and 968 app +14 helper +5 protocol all-feature tests, fmt, both strict
  Clippy modes and app/helper build. Windows MSVC no-default/all-target cross-check
  and 167 Python tests pass. The inherited vendor deprecation warning is unchanged.
- Fresh final SwiftShader/Vulkan capture passes 30 production UI flows and emits
  30 genuine frames. Three new clipboard checkpoints were visually inspected at
  normal/minimum size; every PNG is RGB-identical to PPM readback. These checks do
  not claim a native OS clipboard round trip, native desktop/Windows execution,
  cross-DAW MIDI paste or physical device acceptance. Main and remote CI checkpoint
  are unchanged by this isolated worktree.

## 2026-10-09 — Compact simultaneous editor workspace

- Refined real egui surfaces into a compact two-column default workspace: contextual
  title bars, reduced duplicated chrome/padding, narrower fresh Browser and compact
  Rack rows with 24-point targets, real Mute/Solo labels/state and geometry-aligned
  beat headers. Existing v1 custom layouts keep their rectangles and legacy Inspector
  visibility; subsequent Inspector choices persist.
- Fixed reset placement using stale egui area sizes. Added bounded release-only
  workspace/visible-window edge alignment with Alt bypass, preserving native held
  movement and canceling stale gestures on sidebar/viewport changes.
- Continuous title/all-eight-edge pointer, snap-away/Alt/peer resize, minimum Rack
  scroll/hit-area, repeat-arrange, laptop split-boundary and migration regressions pass.
- Full isolated gates: 975 no-default; 977 app +14 helper +5 protocol all-feature;
  fmt, two strict Clippy modes, app/helper build, Windows source cross-check and 167
  Python tests pass. 26 harness entries pass and 30 genuine Vulkan frames convert
  losslessly; details and precise boundaries are in [COMPACT_WORKSPACE.md](COMPACT_WORKSPACE.md).
- Three same-profile baseline/refined CPU pairs show similar timings, not a general
  speedup. Geometry continuity is unchanged during held native gestures; alignment
  applies only after release. Native display smoothness and physical devices remain
  unmeasured. This slice does not substitute for the combined clipboard integration.


## 2026-10-09 06:38 UTC — Terminal multiwindow checkpoint and refreshed source delivery

- Exact published `c88c7fd2fc368ab1e358f1726b64cb72a10fcfda` reached terminal Windows results. [Preview 37893177401](https://github.com/wrench1997/DAW/actions/runs/37893177401) fully passed 167 Python tests, 951 application +13 helper +5 protocol Rust tests, optimized static-CRT app/helper build, strict vendor/license/provenance/PE/import audit, ZIP/hash/extraction and actual extracted-helper sanitized-PATH smoke. Upload was skipped; artifacts API returned zero.
- [Quality 37893177378](https://github.com/wrench1997/DAW/actions/runs/37893177378) passed all source gates, 14 helper/61 editor Python tests and the trusted source-built MIT fixture. The strict two-record feedback assertion, exact dirty revisions, stopped SaveState/detach/zero-sample flush, fresh-instance Project-context restore and byte-identical component/controller state now pass in actual Windows execution. Ordered native interaction, owner rejection, repeated close/open, WM_CLOSE, owner loss, unload/reload/no-editor, normal completion and independent Shutdown/EOF/forced termination also pass.
- Native paint before/after still FAILS, and repaint comparison is SKIP. Same-process and trusted helper controls both reject PrintWindow modes; overall quality remains failed. Source, package and independently safe native stages are reported separately. Physical input, actual DPI transitions, real vendors, desktop/device acceptance and forced detach/destruction failures remain unverified.
- Refreshed exact source snapshot contains all 141 Git-tracked files, terminal validation receipt and the full binary-safe patch from original main `9c159953163763a354634b3a9f95f84de174641b`. The patch was actually applied to the original tree and every resulting source byte matched; ZIP CRC and inventory passed. Archive SHA-256: `a55f43bceebe364f4faaa0b13e59c86fb6b6eb637f897b0dfcb4cf6336cb08e6`. This source delivery preserves the exact tested c88 checkpoint independently from later features; no executable release, main merge or GitHub artifact publication.


## 2026-10-09 07:12 UTC — Integrate compact workspace and bounded Piano clipboard

- User requested a more compact/fluid FL-style workspace. Approved compact feature `c6e8e32154ae593d8aa12838318deea75dc2067f` integrates as **`8d6b54c11fe07386b105de8d41e209bb0f45af5e`**, after reviewed clipboard `1d0d54f292306fb84786bc61a710ef776c343af7` integrated as `9a75c606`. Runtime files auto-merged; additive headless-test/doc conflicts retain both complete suites. Read-only independent integration review found no blocker and confirmed exact feature modules, native/import/snapshot barriers, semantic clipboard/text ownership, migration, gesture cancellation and toolbar reachability.
- Actual task surfaces are denser: contextual title bars, compact Rack/Mute/Solo/steps, narrower fresh Browser, useful default geometry and preserved v1 layouts/Inspector choices. Held movement remains native and continuous; bounded workspace/peer alignment happens once after release with Alt bypass. New-press/modal/sidebar/focus changes cannot replay stale alignment. Controlled same-profile debug CPU comparisons are mixed, so no general speedup, target FPS or native display smoothness is claimed.
- Piano Select all/Copy/Cut/Paste uses a bounded versioned session-local note payload with fresh stable note/group identities, source-channel/timing/velocity/mute preservation, explicit anchor, and one-step Cut/Paste Undo. Semantic events deduplicate raw keys, text/numeric fields keep clipboard ownership, replacements invalidate session data, and snapshot/gesture/modal barriers remain enforced. It is not cross-DAW MIDI interchange; actual OS clipboard round-trip remains unverified.
- Fresh full Linux gates at exact `8d6b54c`: **991 application +14 helper +5 protocol all-feature/all-target tests**, **989 no-default application tests**, zero failed/ignored, default stack. fmt, both strict all-target Clippy modes, app/helper build and actual ordinary helper smoke pass. Windows MSVC no-default/all-target cross-check and **167 Python tests** pass. Existing vendored dependency deprecation remains unchanged.
- Fresh combined actual-app Vulkan run: **36 entries passed** (35 input/flow checks plus the opt-in benchmark entry, timing disabled), **33 genuine renders**, all PNG conversions RGB-identical. Wide/minimum/clipboard layouts were inspected. Existing native Inspector, Settings/modal, import, history and hidden-editor safety flows pass alongside compact motion and clipboard cases.
- `docs/COMPACT_WORKSPACE.md` is deliberately added to the preview whitelist and joint guide-link regression. All **25 packaged document/provenance inputs** have closed relative links. Canonical current-state/roadmap/parity/native docs record terminal c88 evidence separately from the new merged candidate. This follow-up changes only docs and packaging inputs after the final executable-source gates; new exact Windows CI will run on publication. No main merge, Release or binary upload.

### 2026-10-09: context-scoped Piano melody keyboard slice

- Corrected Piano Ctrl/Cmd+D to deselect and Ctrl/Cmd+B to repeat the selected phrase to its right; Playlist Ctrl+D remains duplication. Added Shift-arrow snap/semitone movement, command-arrow octave transpose, Shift+D discard lengths, basic and starts-only quick quantize, and Alt+V ghost visibility. macOS basic quantize uses Option+Cmd+Q, leaving Cmd+Q untouched.
- Immediate operations share clipboard editor ownership and project-transition/lifecycle barriers; real menu commands bypass only their own popup. Active-Channel selected-or-all/group scope preserves ghost and unassigned notes. Finite/pitch/time/count checks, project-wide new note IDs, fresh copy groups, and inward-rounded common boundary movement are all-or-none. Arrow autorepeat commits discrete Undo steps; other added commands never autorepeat. No-op steps create no history.
- Updated Edit/Tools labels and help, README, parity/workspace guides, and added [Piano keyboard editing](PIANO_KEYBOARD_EDITING.md). Selected-extent repeat, independent local Piano snap, and bounds are explicitly Citrus policies; no separate repeat-time-range or undocumented FL rounding is claimed.
- Exact feature-source Linux gates: all-feature/all-target **1006 application + 14 helper + 5 protocol** passed; no-default/all-target **1004 application** passed, all zero failed/ignored. Both strict Clippy configurations and formatting checks passed. Nine pure boundary/scope regressions plus six production-egui key/pointer/menu/history/guard tests were added, and the earlier Ctrl+D Piano test now uses Ctrl/Cmd+B. Native OS keyboard/plugin focus, hardware input/audio and fixed Windows release remain separate acceptance work.
- Six new actual-app flows also passed in genuine SwiftShader Vulkan offscreen capture mode. Two captured frames (Tools menu and phrase repeat) were visually inspected; PNG conversion is RGB-identical to raw readback. Windows MSVC no-default/all-target cross-check passed; no Windows execution or native focus claim is made.


## 2026-10-09 07:27 UTC — Terminal compact/clipboard checks and source delivery

- Exact `42393c66567f5f362e19230eabbba71b8d41a3d8` completed [preview 37897795252](https://github.com/wrench1997/DAW/actions/runs/37897795252) successfully: 167 Python tests, 988 app +13 helper +5 protocol Rust tests, optimized static-CRT/provenance/license/PE/ZIP/hash checks and actual extracted-helper smoke. Upload was skipped and artifacts API returned 0.
- [Quality 37897795277](https://github.com/wrench1997/DAW/actions/runs/37897795277) passed all source gates and trusted native protocol/interaction/gesture/dirty, stopped-state exact restore, focus/owner/lifecycle and Shutdown/EOF/forced-termination cleanup. Before/after PrintWindow capture FAIL and repaint comparison SKIP keep overall quality failed; no new source regression or speculative renderer workaround.
- The refreshed exact source archive has 145 tracked files plus terminal receipt and original-main full-index patch. Actual patch application reproduces all source bytes; ZIP CRC/inventory pass. SHA-256: `4b5c0f3ffab56e7cf7ce47f4141a2df8c23dc22bafd3869123d555598a1c2430`. It includes compact layout and typed clipboard, with further Piano keyboard/mouse work explicitly excluded. No executable release, main merge or binary artifact upload.


## 2026-10-09 07:59 UTC — Integrate focused Piano keyboard and mouse composition

- Reviewed keyboard `26f682c9518a02131b741dde5d699b454721bca8` integrates as `89c3410`, then reviewed mouse `953011f3465dd08e34513e64163376ff4af9050a` as **`43fc5a388a292cd15503cc3759c24ba420fcb7f4`**. Only additive module/test-include conflicts occurred; both full suites remain. Independent integration review verifies exact approved helper/modules, production Undo/Redo guards, first-event modifier ownership, frontmost clipped note/grid hit testing, project-wide Paint/Stamp/clone IDs, legacy resize bounds and existing native/import/snapshot/text barriers.
- Added focused deselect/repeat, grid/semitone/octave movement, quick quantize, length reset and ghost toggle, with active-Channel selected-or-all/group scope. Pointer-down Draw, temporary Ctrl selection, Shift-clone, modifier-order axis locks, Draw-length drag and touched-note length inheritance support melody entry. All-or-none data bounds, note identities and explicit history are preserved. Selected-extent repeat, independent local snap, generic Shift and Alt Stamp are explicit Citrus policies, not undocumented FL equivalence claims.
- Mouse review discovered an existing safety gap: Undo/Redo could replace earlier history while a pointer preview retained replayable pre-press origins. Production `undo()`/`redo()` now return during active/interrupted held project gestures; tests check both held keys and ordinary menu actions after release. Further review fixes preserve first-event batching, global IDs and positive lengths for legacy notes beyond the interactive horizon.
- Fresh exact combined Linux gates: **1,028 application +14 helper +5 protocol all-feature/all-target tests**, **1,026 no-default application tests**, zero failed/ignored on default stack; fmt, both strict all-target Clippy modes, app/helper build, actual ordinary helper smoke, Windows MSVC no-default/all-target source cross-check and **167 Python tests** pass. The inherited vendored dependency warning remains unchanged.
- Fresh complete genuine Vulkan suite: **64 entries pass** (63 actual input/flow checks plus opt-in benchmark entry, timing disabled), **37 real app frames**, all PNGs RGB-identical. Merged Tools-menu, simultaneous cloned phrase and minimum drawn-length pixels were inspected. Both new input paths run together with workspace/clipboard/native Inspector/import/modal regressions. This does not establish native OS/plugin focus, physical input/audio, OS clipboard round-trip or complete FL parity.
- Both focused Piano guides are explicitly added to preview packaging with the joint link regression; all **27 packaged document/provenance inputs** have closed relative links. Canonical docs capture terminal 42393c evidence and distinguish the new source candidate. Post-gate edits are docs/packaging only. The new velocity-wheel/note-properties worker starts from the exact merged runtime but is not included in this publication or source delivery. New exact Windows checks will be monitored through terminal; native paint failure remains a failing acceptance gate. No main merge, Release or binary artifact upload.


## 2026-10-09 08:13 UTC — Terminal keyboard/mouse checks and exact source delivery

- Published `e6f117e18e2764f0c8b0e8ded3bee57fae46a3eb` completed [preview 37902412320](https://github.com/wrench1997/DAW/actions/runs/37902412320) successfully: 167 Python tests, 1,025 app +13 helper +5 protocol Rust tests, optimized static-CRT/provenance/license/PE/ZIP/hash validation and actual extracted-helper smoke. Upload was skipped and artifacts API returned 0.
- [Quality 37902412259](https://github.com/wrench1997/DAW/actions/runs/37902412259) passed all source and trusted native interaction/state/lifecycle/cleanup checks; only before/after PrintWindow paint stages FAIL, with repaint comparison SKIP. Overall quality remains failed. No new source regression, physical-device claim or speculative renderer change.
- Refreshed exact source snapshot contains 151 tracked files, terminal receipt and the full original-main patch. Actual patch application reproduces every source byte; ZIP CRC/inventory pass. SHA-256: `c115112e531997442100394e66a2cd694c4757984afb268e85954fdb45762e88`. It includes keyboard/mouse composition and explicitly excludes subsequent expression work. No main merge, executable release or binary artifact publication.


## 2026-10-09 08:29 UTC — Integrate relative velocity and transactional note properties

- Reviewed expression `a26a7d7e798ed0aa3752e2c56eaae5abfd78ad5e` integrates as **`9a4d37b0f30f7aff568aa4b66f7b7e2227cfd91e`**. Entire src/Cargo/vendor input is byte-identical to the approved feature. The sole conflict was the parity table; existing mouse details and the new expression entry were retained, with the stale pending label removed. Independent feature review and parent inspection of all three genuine property frames found no remaining blocker.
- Alt+wheel and Ctrl+Alt+wheel now apply bounded relative velocity changes over actual active-Channel note/group targets, with common bounds preserving differences. Raw phases, finite input, per-event Undo, no-op saturation, pointer-centered navigation and old scroll-tail suppression have regressions. Coarse/fine values and raw-event normalization are explicit Citrus policies; generic raw kinetic events are not mislabeled as a separately detectable OS momentum phase.
- Inspector and note-body/grip double-click open one private properties draft. Single-note existing model fields, multi-note common transpose/velocity, explicit mixed mute and Channel assignment support Reset/Cancel/Escape and one-step Apply. Target identity/session/Pattern/Channel staleness fails closed; newer unrelated Project data survives. Imported sub-minimum/legacy timing stays unchanged unless explicitly edited. Review fixes cover tiny-note grips, Stamp repetition, first-event modifiers, numeric focus and modal/save/import barriers. No invented per-note synthesis/MPE fields or schema change.
- Fresh integrated Linux gates: **1,046 application +14 helper +5 protocol all-feature/all-target tests**, **1,044 no-default application tests**, zero failed/ignored on default stack; fmt, both strict Clippy profiles, all-bin build, actual ordinary helper smoke, Windows MSVC no-default/all-target source cross-check and **167 Python tests** pass. Inherited upstream dependency deprecation remains unchanged.
- Fresh complete UI suite: **79 entries passed** (78 production-input flows plus opt-in benchmark entry, timing disabled), **40 genuine Vulkan frames**, all PNGs RGB-identical. Three pure candidate regressions are covered by the full Rust gates, not included in the UI-flow count. Merged single/group/minimum properties fields and action buttons were visually inspected; all preceding keyboard/mouse/clipboard/workspace and modal/import cases run together.
- The expression guide is explicitly added to the exact preview whitelist and joint guide-link regression; all **28 packaged document/provenance inputs** have closed relative links. Canonical docs capture terminal e6f117e evidence separately from this new candidate. Post-gate edits affect only docs/packaging. Independent time-range/snapping work starts from this runtime but is not included or allowed to delay its publication/delivery. New Windows checks remain required; known native paint acceptance is still a failing gate. No main merge, executable Release or binary artifact upload.

## 2026-10-09 09:00 UTC — Bounded Piano edit ranges and local fine/triplet snap

- Added the independent ruler edit/repeat range through Ctrl/Cmd-drag and genuine unmodified double-click-and-drag, plus accessible set-from-selection/move/clear controls. The range belongs to the Project session, Pattern and TARGET Channel; owner changes clear it and interrupted previews restore only their matching owner's prior view state. Range operations never dirty Project, enter Undo or change playback looping. Ctrl+D remains note-only.
- Ctrl+B uses exact range width with active-Channel selected-or-all/group scope, preserving notes outside/overlapping the interval, stable musical fields, fresh identities and single-step Undo. Paste now visibly targets the bar containing the viewport's left edge in both transport modes. All boundary/partial-bar interpretations and f32 repeated-copy precision limits are explicit Citrus policies in [Piano ranges and snap](PIANO_RANGES_AND_SNAP.md).
- Persisted enum-based local snap adds Off and fine/triplet choices without changing Playlist snap or Project schema. Pointer placement and displayed grid share rational timing; rendering is viewport-bounded, thinned only on the original lattice and capped at 2,048 lines. Off keeps raw time editing and independent scale lock, provides a finite 1/64 keyboard fallback and explains unavailable quick quantize. Indexed Chop/Arpeggiate/canonical nudges prevent accumulated on-grid triplet drift; valid minimum Slice/Chop boundaries remain editable.
- Review fixes cover Channel ownership, held-ruler transform snapshot contamination, stale double-click seeds, fine Slice lengths, final-minimum Chop counts and repeated triplet nudges. Narrow editors place infrequent actions in accessible NOTE EDIT and SCALE / CHORD menus: the actual 480×420 floating window keeps the original pitch zoom and a useful populated canvas/velocity lane, without raising its minimum.
- Exact final Linux gates pass **1,092 no-default application tests**, **1,094 all-feature application +14 helper +5 protocol tests**, both strict Clippy profiles, formatting, app/helper build, ordinary helper smoke, MSVC no-default/all-target source cross-check and **167 Python tests**. Both full test configurations were rerun after the test-only lint fix. The source's 29 packaged document/provenance inputs have closed relative links.
- Complete production-input Vulkan capture passes **105 entries** (104 flow/input checks + opt-in benchmark entry, timing disabled), producing **44 genuine app frames** with lossless RGB-identical PNG conversion. Captured binary is byte-identical to the final rebuilt core test binary. Wide triplet and populated 480×420/menu pixels were inspected independently. Windows execution, native OS/plug-in focus, physical MIDI/audio, audio playback-range parity and full FL parity remain separate acceptance boundaries. This work does not push, merge main, publish a release or claim those boundaries complete.


## 2026-10-09 08:48 UTC — Terminal expression checks and exact source delivery

- Exact `b57076ad990869f4a421cc816d67409b5c69b694` completes [preview 37905510775](https://github.com/wrench1997/DAW/actions/runs/37905510775) successfully: 167 Python tests, 1,043 app +13 helper +5 protocol Rust tests, optimized static-CRT/provenance/license/PE/ZIP/hash verification and actual extracted-helper smoke. Upload skipped; artifacts API returned 0. Initial read-only status watches hit transient HTTP 502 and were resumed; CI was not rerun or altered.
- [Quality 37905510854](https://github.com/wrench1997/DAW/actions/runs/37905510854) passes all source and trusted native state/interaction/lifecycle/cleanup checks, with only the known before/after PrintWindow failures and repaint comparison SKIP. Overall quality remains failed; passing source/package gates are separately reported.
- Exact expression source delivery contains 154 tracked files, terminal receipt and full original-main patch. Applied patch reproduces every source byte; ZIP CRC/inventory pass. SHA-256: `e23da9d31789ab4599cd293de5c509756a78e3c52102299a24750b6dcf086acb`. This snapshot includes velocity/properties and excludes later range/snap changes. No main merge, Release or binary artifact upload.


## 2026-10-09 09:16 UTC — Integrate guarded ranges and exact local snap

- Reviewed feature `9d2f2226de89b0ae955936e4c2da14cd78087e5d` integrates as **`37b0d0f65e0d7fe52139289b410cfea2eb8a3087`**. Runtime, Cargo, vendor and packaging inputs exactly match the reviewed source. Only additive evidence/document whitelist conflicts needed reconciliation; both expression and range guides remain packaged. Corrected the older workspace guide's now-obsolete PAT-cursor/SONG-zero paste text to the new viewport-bar policy.
- Fresh exact Linux gates pass **1,092 core application tests**, **1,094 app +14 helper +5 protocol all-feature/all-target tests**, zero failed/ignored on default stack; formatting, both strict Clippy profiles, all-bin app/helper build, ordinary helper smoke, Windows MSVC no-default/all-target source cross-check and **167 Python tests** pass. All **29 packaged document/provenance inputs** have closed relative links.
- The complete actual-app harness passes **105 entries** (104 input/flow checks plus the opt-in benchmark entry, timing disabled), with **44 genuine Vulkan frames** and RGB-identical PNG conversion. The copied capture binary matches the final tested binary byte-for-byte. Wide triplet, populated 480×420 floating Piano and NOTE EDIT popup pixels were inspected; all previous expression, mouse, keyboard, clipboard, native Inspector, import/modal and workspace flows remain in the suite.
- The range remains editing/repeat metadata, independent of Project dirty/history and transport looping. Exact local snap, finite Off fallback, rational triplet grid and explicit f32 repeat bounds retain the documented Citrus semantics. No native OS/device, performance or complete FL-parity claim follows from these checks. New Windows checks are required and the known native paint failure remains a failing acceptance gate.
- Canonical docs preserve terminal expression-checkpoint evidence separately from the new candidate. Scanner classification and inter-plugin MIDI routing are isolated next work and do not delay this source checkpoint. Their runtime changes are not included here. Post-gate edits are documentation only; no main merge, executable Release or binary artifact upload.


## 2026-10-09 09:34 UTC — Terminal range checks and exact source delivery

- Published `d9016e5066a8a974fc4be71d6fbc5cf147529010` completes [preview 37910288992](https://github.com/wrench1997/DAW/actions/runs/37910288992) successfully: 167 Python tests, 1,091 app +13 helper +5 protocol Rust tests, optimized static-CRT/provenance/license/PE/ZIP/hash validation and actual extracted-helper smoke. Upload was skipped; both run artifact APIs returned 0.
- [Quality 37910289005](https://github.com/wrench1997/DAW/actions/runs/37910289005) passes all source and trusted native interaction/state/focus/lifecycle/cleanup checks. Before/after PrintWindow still fail, the same-process standard-control diagnostic also fails and repaint comparison is skipped. Overall quality remains failed; no native paint or hardware acceptance is inferred.
- The exact source archive contains 159 tracked files, terminal receipt and the full original-main patch. Actual application reproduces every source byte; ZIP CRC/inventory pass. SHA-256: `d58b0f33f357cbd1f1a20febc94469e14a4b93e5e1e70faf285d1e01fef01270`. It includes range/snap/compact minimum controls and excludes subsequent scanner and MIDI routing work. No main merge, Release or binary artifact publication.


## 2026-10-09 09:53 UTC — Integrate authoritative scanner and real-plugin source receipts

- Reviewed scanner `1c673eb47ace331b4b05d147dd8d29747670d99e` integrates cleanly as **`e6bd216863f6180c2745de05c6701c6300d41cf2`**; src/Cargo/vendor bytes exactly match the approved feature. The runtime-selected default class supplies actual name/vendor/category/class UID and independent MIDI capabilities through the isolated helper. Legacy filename-only cache entries request rescan, failures preserve actionable hover details, bundle pruning and bounded cancellation are tested. VST2 discovery and saved project identities remain unchanged. Metadata loading executes candidate initialization in a helper, not an OS security sandbox.
- Fresh Linux gates pass **1,107 app +14 helper +5 protocol all-feature/all-target tests**, **1,103 no-default application tests**, zero failed/ignored on default stack, formatting, both strict Clippy modes, app/helper build, ordinary helper smoke and Windows MSVC source cross-check. Complete actual-app capture passes **106 entries** (105 input flows plus the timing-disabled benchmark entry), with **44 genuine Vulkan frames** and RGB-identical PNG conversion. New rescan/failure UI flow and all preceding editor/barrier regressions run together.
- A fresh portable metadata probe compiled against exact Cargo-resolved rlibs and this source loads actual official Surge XT/Effects, verifies Instrument/Fx classification, independent MIDI capabilities and cache round-trip, and shuts down cleanly. The newly built helper SHA-256 remains `ca91f242e409ab632427d5191c33ecae94bbd5172a8e7f1a98928d60f4f365fd`. This is metadata-only; it does not reattribute historical processing evidence or exercise a native editor.
- Added source-only scripts/receipts for the earlier exact b57076a controlled-offline Surge instrument/FX/audio chain, direct helper state/parameter/DSP checks, blank-default negative and configured positive Stochas MIDI output, and both corrected-scanner runs. Preserved raw measurement/event/state bytes and original source hashes. Portable source adaptations are labeled; published task-local paths use tokens with original raw hashes, while reversible audit maps/raw originals stay outside the repository. The empty historical compile log is omitted with its empty-byte hash/reason, rather than weakening preview input validation. No plugin binary, factory asset, audio render or compiled probe is bundled.
- The exact **41-file** QA bundle is all-or-none and its canonical inventory hashes are checked during package creation and verification. LF attributes protect textual hashes across Windows checkout; the owned opaque state explicitly bypasses newline conversion. Five new regression methods run in both existing package test classes; actual full discovery passes **177 Python tests**. All **72 packaged document/provenance/QA inputs** have closed relative links and valid QA inventory. Post-runtime-gate edits affect only documentation, source receipts and packaging.
- Canonical docs preserve terminal range evidence separately from this scanner candidate. Native paint remains a failing acceptance gate; downstream MIDI routing, Harmony Blueprint, native GUI, physical hardware and real-time deadlines are not implied by the receipts. The independent routing/Linux-editor work is excluded and does not delay this checkpoint. No main merge, Release or binary artifact publication.

- Independent final source-receipt/package review found no blocker. A staged checkout with Git `core.autocrlf=true` / `core.eol=crlf` preserved all 41 QA files byte-for-byte and passed the same strict inventory; opaque state bytes were unchanged. No source/runtime change followed the completed Rust gates.


## 2026-10-09 10:46 UTC — Terminal scanner checks and source delivery

- Exact `9782e62a3d92b939476727ff6bbff7a3d3e7c42f` completes [preview 37914387094](https://github.com/wrench1997/DAW/actions/runs/37914387094) successfully: 177 Python tests, 1,102 app +13 helper +5 protocol Rust tests, optimized static-CRT/provenance/license/PE/ZIP/hash validation and actual extracted-helper smoke. Upload skipped; both artifact APIs returned zero.
- [Quality 37914386973](https://github.com/wrench1997/DAW/actions/runs/37914386973) passes source, trusted fixture interaction/state/focus/lifecycle/protocol/cleanup checks. Before/after PrintWindow still fail; standard same-process control diagnostics also fail and repaint comparison is SKIP. Overall quality remains failed, with native paint acceptance explicitly open.
- The delivered exact scanner source contains 202 tracked files, terminal receipt and full original-main patch. Actual patch application reproduces every source byte; ZIP CRC/inventory pass. Source ZIP SHA-256: `5de1ff054f59c9fc3f5e97f996fc8516105a093c357212522448253c6d675f8d`. That snapshot excludes the later routing/Linux-editor/metronome slices.

## 2026-10-09 10:46 UTC — Integrate bounded MIDI routes and production graph evidence

- Reviewed implementation `e54a6e49fe02b78ff299ce56dddd781925f7fe3c` and acceptance docs `9f4a64b3ea10d4eec4eb451d51a67eea15a36c51` integrate cleanly as `d812b6c` / **`5e8ff0f622077e6a3817dce122f03ec6e33c1ab1`**. All src/Cargo/vendor/tests match the approved feature byte-for-byte; independent real-plugin binding also matches all 117 production files. New Project v12 persists MIDI ports/monitor, migrates v10/v11 ports Off and preserves backups as a compatibility prerequisite.
- Fresh exact Linux gates pass **1,131 app +15 helper +5 editor protocol +2 transport protocol all-feature/all-target tests**, **1,127 no-default app tests**, zero failed/ignored on default stack; fmt, both strict Clippy modes, all-bin build, ordinary helper protocol smoke and Windows MSVC no-default/all-target source cross-check pass. Complete actual-app capture passes **107 entries** (106 input flows plus timing-disabled benchmark entry), with **45 genuine Vulkan frames** and RGB-identical PNG conversion. The exact tested core binary owns the capture; current Inspector and prior UI/barrier regressions execute together.
- One producer can feed one or more exclusive numbered sink ports during constant-tempo playback, with Off separate from port0, independent source audio-monitor mute, stopped-only history-safe editing, bounded bus0 events and endpoint/epoch/latency attestation. Failures latch independently of full queues and require successful fresh-epoch reset. No stopped live chain, seamless loop, arbitrary multi-hop or automated sink is claimed.
- The separate exact-source official Stochas/Surge/Effects production graph suite passed **5/5**: callback128/256/512/2048 and60/120BPM, genuine FX latency fence/recovery, held/retrigger cleanup and deliberate overload/restart. All 50 source-only published receipt files retain raw-data identity and original imported inventory; only the reproduction guide was clarified. Import archive SHA-256: `d2256e435d1610f2ee7b8a65a3ed116ddf48fa87edbceb1bee0941ab3794433d`. No vendor binary, factory asset, compiled probe or WAV is redistributed.
- Kept negative evidence: a historical concurrent-build source batch loss has an unresolved cause despite same-binary quiet success; the old Off one-quantum baseline misses15/10; debug callback128 reached3.116ms against2.667ms. Two endpoint bridges add4352frames /90.667ms at48kHz before attack/FX. These results do not establish physical device deadlines, low latency, Harmony Blueprint, Windows execution or native plugin UI.
- Preview creation/verification enforces the new exact 50-file all-or-none QA whitelist and canonical hashes, preserving the immutable 41-file historical bundle. Full actual Python discovery passes **187 tests**; all **124 packaged document/provenance/QA inputs** have closed relative links. LF attributes pin receipt hashes. Root independently reviewed the verifier/publication/packaging and reran read-only117-file source validation with no blocker. Post-Rust-gate edits only affect docs, source receipts and packaging.
- New Windows CI remains required and native paint remains a failing gate. Linux native-editor and metronome implementations are separate approved/reviewed work and are not included here. No main merge, Release or binary artifact publication.

- Final publication checks reject six receipt-corruption/source-binding/negative-erasure cases. A staged Git checkout with `core.autocrlf=true` / `core.eol=crlf` preserves all **91 QA files** byte-for-byte and passes both canonical inventories. Original captured log endings are preserved with a log-only `blank-at-eof` whitespace exemption; no measured byte is trimmed to satisfy a style check.
## 2026-10-09 10:39 UTC — Linux standalone native VST3 functional preview

- Integrated standalone X11 plugin windows compatible with system XWayland while retaining
  the DAW's independent backend. Linux Open uses no owner; Windows HWND/PID checks remain.
- Added bounded main-thread helper pumping, factory and per-attachment frame IRunLoop,
  lifetime/reentry guards, WM_DELETE/focus-proxy XEmbed, resize and Xft.dpi handling.
  Unix protocol claims now fail closed. Original upstream MIT bytes and exact patch pins
  are preserved and reconstructed against the registry archive.
- Production source gates passed: 1,131 all-feature app +21 helper tests, 1,127 no-default
  app tests, both strict Clippy modes, Windows app/helper and fixture source checks,
  seven run-loop +one claim unit regressions, four live protocol tests, 64 harness and
  92 packaging tests. Actual source-built fixture mouse, key press/release, focus,
  callback retirement, state restore, native close/reopen and crash lifecycle passed.
- Genuine Surge and Stochas native UI edits survived fresh-instance state restoration.
  Surge generated finite nonzero PCM while its UI was active and edited; native resize
  caused a measured 346.9 ms request stall. This functional preview does not pass realtime
  continuity, hardware-device or mixed Wayland/XWayland acceptance. No sanitizer run.
- Exact executable identity and measured results: [Linux validation](LINUX_VST3_EDITOR_VALIDATION.md).
  No publication or release was performed by this worker.


## 2026-10-09 10:59 UTC — Integrate Linux native editors and opt-in metronome

- Preserved the prior route source as **`5e38b36e3ee2b172eda00bc3497d93f074120d85`**: 257 tracked files, exact original-main patch and explicit local-only/new-Windows-NOT-RUN receipt. Its source ZIP SHA-256 is `48dce1e32f545010209a13a18853b62fcec8e1c1588b9b312bf77eef5d81d74f`. No historical CI result was relabeled as execution of that candidate.
- Approved Linux editor commits `d4e4cd4` and `88c498c` integrate as `55381c0` / `146983e`; approved metronome `7be79ad` integrates as **`fcf57b0e417cc18d7649577c68ef031d026bc6d5`**. Only an additive WORK_LOG conflict needed resolution; pre-metronome src/Cargo/vendor/tests exactly matched the reviewed Linux feature. After metronome, Cargo/helper/plugin-runtime/vendor/fixture bytes remain identical to `88c498c`; app/audio changes are the reviewed metronome diff.
- Fresh combined Linux gates pass **1,137 app +21 helper +5 editor protocol +2 transport protocol all-feature/all-target tests**, **1,133 core tests**, zero failed/ignored, default stack; fmt, both strict all-target Clippy modes, all-bin build, actual ordinary helper smoke and MSVC no-default/all-target source cross-check pass. All **194 Python tests** pass with the fresh helper, including all four live Unix descriptor-isolation tests. Actual vendored source passes **seven run-loop tests plus one closed-stdout test** through an external test manifest; upstream manifests are not edited for those checks.
- Complete actual app capture passes **108 entries** (107 input flows plus the timing-disabled benchmark entry), producing **46 genuine Vulkan frames**; all PNGs preserve readback RGB. Inspected metronome ON and minimum Playlist layout pixels. Shared native/import barriers, MIDI port UI, Piano/workspace and repeated/interruptible flows remain in the same suite. Source capture is independent of native desktop/hardware acceptance.
- Fresh helper SHA-256 **`9cc00c81f07dc88c7514c8fb2b016cd1f080803fefbb281e76cc43b0a16a3ffd`** exactly matches the actual native GUI-tested executable. Linux source-built fixture and real Surge/Stochas painted interaction/state restoration remain attributed to that earlier exact helper/source, rather than claimed as a repeated app GUI run. Its **346.9 ms native-resize processing stall**, restored-run 145.1 ms and unverified mixed Wayland/hardware/DPI/sanitizer boundaries remain explicit. Windows paint remains a failing separate acceptance gate.
- The opt-in CLICK OFF/ON preference defaults Off, persists through app/device replacement, does not dirty Project or history, and uses callback-atomic source clearing without changing transport phase. Existing PDC/FX/device buffers can drain after Off. Offline WAV excludes clicks; realtime Master Capture includes actual enabled clicks. It does not fix general worker scheduling or the native resize stall.
- Pristine registry archive plus the exact pinned vendor patch reconstructs **all 41 upstream files**. Original MIT license/manifests/archive checksum remain unchanged; modified patch/manifest hashes are distinct. Added Linux editor, native-validation and metronome guides to the exact preview whitelist and link regression. A source-fixture README hyperlink exposed a real package closure gap; the guide now states its source-checkout path and developer-only exclusion, preserving the runtime package contract. All **127 packaged document/provenance/QA inputs** have closed links and valid inventories. The historical 91 QA files remain unchanged.
- Fresh Windows source/native/optimized-package execution is still required for this combined candidate. The later callback scheduling/control-runtime refactors are not included. Post-Rust-gate changes are docs/package whitelist only; no main merge, Release or binary artifact publication.


## 2026-10-09 11:04 UTC — Repair Windows synthetic Linux-receipt path regression

- Published combined checkpoint `21ea7264706ea849bbbb5ba22308145de163b874` started [quality 37921083514](https://github.com/wrench1997/DAW/actions/runs/37921083514) and [preview 37921083583](https://github.com/wrench1997/DAW/actions/runs/37921083583). Preview's full Python discovery exposed a Windows-only fixture defect before the Rust/package stages: the synthetic Linux receipt used host-dependent backslashes. The real fixture builder already emits the required portable slash path; strict production verification was correct and remains unchanged.
- Changed the test fixture to `Path.as_posix()` and added a positive portable-path assertion plus explicit rejection of backslash receipt paths. No skip, weakened validator, new permission or runtime change. Fresh full Linux discovery passes **195 tests**, including four actual Unix helper descriptor checks. Prior combined Rust/UI/native-helper evidence is byte-unchanged; exact new Windows runs are required. The earlier failure is preserved as evidence rather than erased by a rerun.
## 2026-10-09 11:21 UTC — Prepare synchronous VST3 control / processor ownership

- Based on `88c498c`, added a deliberately !Send/!Sync `MainThreadPlugin` helper entry point
  while preserving the legacy movable `Plugin` API. Linux/Windows helpers retain all plugin,
  GUI, lifecycle and reply work on the main thread. Split `ControlDomain` ownership from
  `ProcessorRuntime` state and use an exclusive, module-lifetime-bound `ProcessorLease` limited
  to process/setProcessing. There is no new unsafe Send, processor worker, wire change or
  performance claim. Existing GUI locks, growable parameter/event containers and metering
  remain explicit prerequisites for the later real-time split.
- Corrected behavior discovered by the extraction: prepared bus storage is invalidated and
  rebuilt after state/topology changes, including legal fallback changes after declined
  arrangements. Failed rebuilds cannot process old pointers. Reconfiguration checks actual
  deactivation; in-process public processing flags track state/restart failure. State restore
  retains surviving bus activation choices. The outer isolated client's separate cached-state
  recovery limitation remains documented.
- Independent source review found and closed module-lifetime error-unwind hazards: the
  initialization guard now consumes/releases its extra COM refs immediately at owner transfer,
  factory ownership is retained on every platform, and host context is created before module
  loading everywhere. Review closed without remaining blockers for this preparatory stage.
- Final source-linked vendor tests with exactly `cpal-backend,process-isolation` and default
  features disabled pass **270/270 available tests**, including 13 instrumented COM fixtures.
  Those fixtures verify alias-once lifecycle, no lease refcount churn, same-thread/module-last
  destruction, state/refused-arrangement/restart layout changes with actual sample writes,
  failure rejection/retry and transfer failure order. **Five facade doctests pass**, consisting
  of normal same-thread compilation plus four compile-fail ownership/escape constraints.
- Upstream development-only dependencies are unavailable offline. The temporary manifest
  points directly at every unchanged production source path and preserves production dependency
  entries; it does not remove tests or modify cfgs. The registry package also omits the upstream
  `test_plugins/Dexed.vst3` metadata fixture: its original full-run failure is retained, and only
  that test is explicitly filtered in the available-suite result. This is not a full upstream
  package acceptance result. One parallel rerun separately failed spawning a fake helper with
  ETXTBUSY; the raw log is retained, its cause remains unconfirmed, and the same available set
  passes serially without deleting that test or changing its implementation.
- Final production helper gates pass **21 helper +7 protocol tests**, strict helper/protocol
  Clippy, helper build, root/changed-vendor formatting and diff checks. Actual copied-helper
  smoke passes three JSON replies, invalid-command recovery, Shutdown with stdin open and
  child reap; it loads no plugin. Python helper/editor harness tests pass **78**, and packaging
  regressions pass **92**. The upstream deprecated atomic-method warning remains unchanged.
- Exact registry archive SHA-256 and original manifests/license/version remain intact. The
  cumulative vendor patch was regenerated, applied to a pristine extraction and byte-compared
  for every original/added file; patch/manifest pins and the new guide's package whitelist are
  updated. Source-linked manifest, lockfile, dependency-feature tree, per-source hashes, copied
  helper identity and raw gate logs are retained in the development validation receipt.
- Design and remaining worker/broker/state-fence requirements: [Processor domains](PLUGIN_PROCESSOR_DOMAINS.md).
  No native UI, real-plugin resize benchmark, physical-device acceptance, Windows/macOS runtime
  run, new installation, main merge, push or release is claimed by this commit.


## 2026-10-09 11:49 UTC — Terminal combined preview and verified source snapshot

- Exact `578a3ce9578f867c24fef129c1012f8a739d4d5e` completes [preview 37921482180](https://github.com/wrench1997/DAW/actions/runs/37921482180) successfully: 195 Python cases (191 passed, four Unix-only skips), 1,132 app +14 helper +5 editor protocol +2 transport protocol Rust tests, optimized static-CRT app/helper, strict vendor/license/PE/ZIP/hash and actual extracted-helper smoke. Both final run artifact APIs return zero; upload is skipped.
- [Quality 37921482220](https://github.com/wrench1997/DAW/actions/runs/37921482220) passes source and trusted native state/interaction/focus/lifecycle/protocol/cleanup checks. Before/after PrintWindow fail, same-process standard Button diagnostics also fail, and repaint comparison is SKIP. Overall quality remains FAILURE. The superseded 21ea726 quality run is service-labelled CANCELLED; its source/paint logs remain retained. The first preview's synthetic path failure is preserved. A transient watch HTTP502 was recovered read-only, without changing CI.
- The verified exact source archive has 265 tracked files and the full original-main patch; actual application reproduces every byte and ZIP CRC/inventory pass. SHA-256: `a0f1329ab9e12a77754a2aa3858fcf7ce746596c67fe2d57f7a6cc94a54c5248`. It includes MIDI routing, Linux editors and the metronome; it excludes the subsequent ownership preparation and callback timing work. Historical real-route and new native-editor evidence remain attributed to their actual helpers/sources.

## 2026-10-09 11:49 UTC — Integrate synchronous ownership preparation with partial state acceptance

- Independently approved `81fe8bf7267e1affc54fafa45d7b345770837407` integrates as **`3fb549a059916dca7b3af3eff00ff27fba0bddd4`**. Helper/vendor/Cargo/fixture match reviewed bytes; app/audio/UI/metronome remain unchanged from578a3ce. Only additive WORK_LOG/package-guide conflicts were reconciled, preserving all91 earlier QA receipt files and the strict Windows portable-path regression.
- Fresh Linux all-feature/all-target gates pass **1,137 app +21 helper +5 editor protocol +2 transport protocol**, with **1,133 no-default app tests**, zero failed/ignored and default stack. fmt, both strict Clippy modes, all-bin build, actual helper smoke, both Windows MSVC all-feature/no-default source checks and **195 Python tests** pass. The four Unix descriptor tests execute against the fresh helper. Existing app/headless tests rerun inside these aggregates; no new offscreen frames or full DAW desktop run is claimed for unchanged UI code.
- Source-linked vendor tests with exact production features pass **270 available cases serially**, with one explicitly filtered missing upstream Dexed metadata fixture; initial missing-fixture and parallel ETXTBUSY observations remain retained. **All26 doctests** pass, including five new facade contracts. The manifest omits unused upstream dev dependencies without modifying production source, cfg or vendored manifests. Pristine archive plus pinned patch reproduces **43 original/added files**; original MIT/license/manifests and archive checksum remain distinct and unchanged.
- The main-thread-only facade and exclusive processor borrow do not create a worker. Control lifetime/error unwind, state/topology storage rebuild and lifecycle failure reporting are hardened, but legacy locks/feedback remain synchronous. No latency, dropout or realtime-thread isolation gain is claimed. Fresh helper hash **`cb1899fd7b768e61ab3b5de747e4236e250a0387762058b1db12f4fe2bd5ca08`** is byte-identical to the independently tested native/headless regression helper.
- Post-freeze genuine fixture and fresh-instance Surge/Stochas paint/input/state checks pass. **Full restore acceptance remains partial:** both previous/new helpers lose Surge's first immediate note after reused-instance restore, and the host getter stays stale even after the component volume correctly applies. Actual native menu and component XML correct the earlier interpretation of permanent DSP-volume loss. Reused-state content913×569 can remain inside host1178×735 until detach/reopen; historical resize346.9ms request stall remains. Details and exact source/state identities are in [state limits](PLUGIN_STATE_RESTORE_LIMITS.md), independently reviewed before this checkpoint.
- Added ownership/state-limit guides to exact packaging and linked current native validation. All **129 packaged inputs** have closed links and strict provenance/QA inventories; original receipts are not rewritten. New Windows CI remains required. Callback timing, a real DSP worker and the separately proposed Surge compatibility transaction are excluded. Post-Rust-gate changes are documentation/packaging only; no main merge, Release or binary artifact publication.
### 2026-10-09 12:25 UTC — Bound Surge state restore to fresh instances

- Separate follow-on to ownership checkpoint `81fe8bf`; no DSP worker or latency improvement is
  claimed. Old/new real-plugin comparison found the same existing Surge XT 1.3.4 behavior: a
  used instance defers restored component state, can erase its first note, and leaves controller
  readback stale. Component XML/native UI eventually hold the correct volume; the initial 1.0
  getter was not proof of permanent DSP-volume loss.
- Rejected and archived an uncommitted 32-frame-settlement experiment after independent review
  found a real native preset-worker/halt_engine race. Successful Process is not a completion
  acknowledgment. No hidden processing, sleeps, fixed retries or opaque state parsing was added.
- Minimal guard matches only actual factory UID `ABCDEF019182FAEB566D624153675854` and version
  `1.3.4`. Positive Process attempts (including failures) and explicit native opening/attachment
  attempts are sticky history. Used instances receive an actionable fresh-instance error before
  COM/lifecycle/queue mutation. Linux/Windows helper preflight precedes editor detachment. Zero
  sample flushes remain eligible. Temporary createView metadata probes are not an assertion that
  plugin-side UI initialization never occurred. Metadata selects policy, not binary authenticity.
- Fixed the separately identified single-component alias case: an optional controller blob no
  longer reapplies component state through the aliased controller interface. Focused fixture
  verifies both controller-state calls are skipped while component state is applied once.
- Available exact-feature vendor suite passes **276/276**, including six new history/no-mutation/
  alias tests; the known missing upstream Dexed metadata fixture stays explicitly excluded.
  **5** ownership doctests, **22** helper tests, **7** protocol tests, strict root helper/protocol
  Clippy, build, **78** Python smoke-harness tests, **4** executed Unix descriptor tests, and
  **92** packaging tests pass. Actual copied-helper smoke verifies three replies/recovery/shutdown
  without loading a plugin. Source-linked harness constraints from the prior entry still apply.
- Copied helper SHA256 `9dd18d74783e2ed13e947008d4dc4d5b2bdc0032948680c1cac678daed9aae60`
  passes a **12-case** real Surge matrix at 17/47/128/256 frames: fresh authoritative restoration
  immediately plays its first note and agrees at controller/component volume; used active and
  stopped restoration is refused with byte-identical saved state before/after, preserved
  lifecycle and subsequent nonzero PCM. No caller restore warmup is used. Stochas again produces
  semantic pattern-preserving, balanced MIDI across reused/fresh state operations.
- Independent native checks pass fresh Surge numeric agreement (−6.28 dB / normalized0.869186),
  rejection of a different state with the SAME open XID/generation/1178×735 geometry/menu/value,
  closed-but-previously-opened rejection and later reopen. Trusted fixture passes. Stochas reused
  open-window state operations still save/reset/restore/re-export and repaint its UI-created cell.
  Native helper shuts down with exit0. Independent read-only review found no remaining blocker
  and verified exact source/diff/helper hashes.
- The current App uses fresh candidate replacement. The unchanged legacy public live-admin
  `PluginChainControl::load_state` still closes/faults its slot before backend rejection and is not
  called by the current App. This guard's direct helper/in-process preservation is not a claim of
  full-chain preservation through that legacy API. No Windows/macOS runtime, physical audio
  device, full upstream fixture suite, merge or push acceptance is claimed.
- Regenerated cumulative vendor patch replays byte-for-byte against the unchanged registry
  archive; package patch/manifest pins are updated. Full source/provenance, raw positive and
  retained negative traces, rejected-experiment receipt and native evidence are indexed by
  `surge-guard-validation-receipt.json` in the development QA artifacts.


## 2026-10-09 12:30 UTC — Freeze previous ownership checkpoint and integrate guarded rejection

- Exact published `5befb51301574d129cf33ed84d8908e90b665c0e` completes [preview37926251178](https://github.com/wrench1997/DAW/actions/runs/37926251178) successfully: 195 Python cases (191 passed, four Unix-only skips), 1,132 app +14 helper +5 editor protocol +2 transport protocol tests, optimized/static-CRT build, strict provenance/PE/ZIP/hash and extracted-helper smoke. Upload is skipped; both final runs have zero artifacts. [Quality37926251171](https://github.com/wrench1997/DAW/actions/runs/37926251171) passes source and independent native state/interaction/lifecycle checks, fails known before/after PrintWindow, skips repaint comparison and remains overall FAILURE.
- Its exact source archive contains269 tracked files and a full original-main patch; patch application reproduces every source byte and ZIP CRC/inventory pass. SHA256 `43a99163e9835d22f36fa13ef4c0222c542424b4df0f4a5bf67176462af0bac9`. Historical first-note/getter/content-size failures remain explicitly unpassed.
- Integrated only reviewed guard `c056dc5` as `60134a5cd668e90eb0801f6da4afc9aaca5abe1c`. Helper/vendor/Cargo/fixture bytes exactly match reviewed source; app/audio/UI/metronome remain unchanged from5befb51. Additive ownership-guide and work-log conflicts preserve both historical and new evidence. The exact package pins preserve strict provenance and all91 earlier source-only QA files.
- Updated canonical documents to distinguish newly blocked unsafe helper operations from the pre-existing deferred-state behavior. Fresh candidate restoration remains supported; no hidden settlement or universal restoration/latency improvement is claimed. Legacy public full-chain Admin closes the editor before backend LoadState and faults the slot when that call rejects. Timing changes are excluded. Full combined gates and new CI have not yet run; source/docs/package preparation is held during another candidate's coordinated build/quiet QA.
## 2026-10-09 — Preserve late plug-in drift evidence before Timeline suspension

Genuine plug-in QA against `f164e91` exposed a cold-start classification race: Surge FX
published latency 0 → 32 after activation, and a later render freshness check cleared the
Timeline binding without latching a plug-in fault. The resulting authorized-gap guard kept
processing in Priming with zero submissions. Read-only activation/callback traces reproduce
this on all four B2048 fixture variants; this was not a worker deadline failure.

The correction classifies already-observed coherent endpoint evidence against the active
identity table before generic Timeline cleanup. Same endpoint/new latency revision records
LatencyDrift with endpoint and expected sequence; physical identity loss records EndpointChanged.
Generic packet/ownership failures do not perform a new shared read or become plug-in faults
without positive identity evidence. All raw, normal-render and paused-monitor PDC freshness
failures use the same latch path. Paused safety/parameter helpers additionally refuse work
if a fault or suspension arose earlier in the same callback.

New deterministic tests cover all four profiles, ordinary/routed paths, prepare/precommit
identity reads, normal and paused refresh, exact fault identity, zero pre-fault submissions,
same-revision rejection, stopped Retry, cached drift versus unavailable-read negatives, and
full paused-monitor continuation with an unrelated admitted edit and armed safety budget.
These paths run inside callback allocation/deallocation guards. Prior genuine deadline
failures at B128/B256 remain evidence; this correction does not tune the guard or certify
any profile, hardware device, or native editor.

The first unrestricted-parallel aggregate invocation additionally exposed three older fixture
assumptions: `deferred_partial_q_midi_safety_runs_through_the_following_quantum`,
`midi_recording_start_rejects_an_input_route_still_in_panic_recovery`, and
`replacement_monitor_never_clears_another_generators_safety_service` directly installed
project-stamped mock endpoints before coherent worker readiness, then unwrapped missing
endpoint/route state. Their common factory now waits off-callback for an active coherent
publication. The ordinary mock instrument/effect factories also establish readiness before
immediate render. The same invocation recorded a two-second `wait_until` timeout in
`mock_instrument_routes_through_insert_and_fader_then_stops_on_midi`; the old log does not
identify which of its several predicates timed out, so it is retained without assigning an
unproven cause. `wait_until` now tracks the caller for any future failure. No processing block,
longer timeout, weakened assertion, or runtime-path wait was added.

## 2026-10-09 — Mark smaller plug-in timing profiles Experimental

After preserving the same-binary genuine quiet 13/16 and four-thread-load 10/16 matrix,
B2048 remains the default. The 128/256/512 choices now explicitly say Experimental;
Settings visibly warns that worker deadlines can fail even when callbacks fit the
ceiling and that a fault requires Retry. Focused pointer/AccessKit tests assert the
warning/default, retained admission rules and clickable footer/Retry at both sizes.
The timing contract, guard, device preferences and runtime processing are unchanged.
`PLUGIN_TIMING.md` preserves every final matrix failure and long-cleanup limitation;
optimized bounded-capture evidence remains future work.

## 2026-10-09 12:55 UTC — Combine reviewed timing correctness with the Surge guard

- Integrated only the approved timing commits `18478d2`, `f164e91`, `882a743`, `f68b9a0`, `f5d887d` onto the guarded source. Exact combined runtime is **`4fdfbc2ea5c828fd9c329f31e9be203810fa62c7`**. App/audio/timing/runtime source matches reviewed f5d887d byte-for-byte; helper/vendor/Cargo/fixture matches reviewed c056dc5. Only additive WORK_LOG conflict resolution was needed. The helper remains single-threaded; this is not the later DSP/control-thread split.
- Fresh Linux all-feature/all-target gates pass **1,170 app +22 helper +5 editor protocol +2 transport protocol** and **1,166 no-default app tests**, zero failed/ignored with the default stack. fmt, both strict all-target Clippy modes, all-bin build, actual ordinary helper smoke and both Windows MSVC source profiles pass. **276 available vendor cases** pass serially, with the same named upstream Dexed fixture exclusion; **26 doctests** pass. A shared-target stale vendor artifact initially failed API resolution; forcing the exact current source to recompile resolved it without code changes, deletion or weakened gates. The initial failure log remains retained.
- The fresh copied helper SHA256 **`9dd18d74783e2ed13e947008d4dc4d5b2bdc0032948680c1cac678daed9aae60`** is byte-identical to the independently tested guard helper. The core test binary was selected from Cargo compiler-artifact JSON, copied and hashed as **`e642ab72f537c66a7175f77cbfaad736a4eca12bf7cefa92810e07a5fe341394`**, avoiding ambiguous shared-target filename selection.
- Full combined actual-app QA passes **123 entries** (122 input flows plus the timing-disabled benchmark entry), with **49 genuine SwiftShader Vulkan frames** and RGB-identical PNG conversion. Fault/Experimental Settings were inspected at1040×728 and1498×936; labels, warnings, default2048 and Retry remain visible. These are surface-free app frames, not native desktop/device or performance evidence.
- Added [historical timing debug receipts](../qa/plugin_timing_debug/RESULT.md) as **250 byte-identical source/text files** from archive SHA256 `fb320f6289f7b5976b7d6d4cc996f7f22863404841f6b765d09ba45a74b0d1e0`. Pinned publication inventory SHA256 `4dc1d40b8e98fa092913eabb06e313077322d4465a322c7780daaacb24f65caa`, explicit all-or-none paths and regression checks reject changed/missing/extra data and rewritten/escaping inventories. All250 files survive Windows-configured CRLF checkout byte-exact. Original hashes, all six failed overall attempts, path-normalization custody and debug-only limits remain unchanged; no binaries, assets, states or audio are included.
- Full **203 Python tests** pass, including new packaging negatives and four real Unix descriptor checks. An initial strict-inventory check caught task-generated bytecode in the older receipt directory; the transient cache was moved outside that immutable tree and the full suite rerun. Existing91 QA files remain unchanged; all **380 actual packaged inputs** have closed links and valid provenance/inventories. The unchanged registry archive plus guard patch reconstructs all43 original/added vendor files.
- Current timing uses whole-callback admission, explicit endpoint/topology/epoch fences, independent visible fault latching, stopped Retry and tagged live-state/prevalidated configuration replacement. Default B2048 remains; smaller128/256/512 are Experimental. At48kHz one physical worker adds2432frames at default, so producer+instrument adds4864frames /101.3ms before plugin/downstream latency. Serial slots do not each add another worker bridge.
- Prior exact timing-only debug evidence is **13/16 quiet and10/16 four-thread load**, with all four2048 configurations passing both through actual native FX32 latency replan/stopped Retry and exact events/PDC. Smaller-profile DeadlineMiss, failed longer cleanup/retry, old no-fault-Priming source failure and corrected harness setup assertions remain visible. These numbers do not certify optimized latency, physical devices or solved dropouts. The historical346.9ms native resize stall remains. Fresh combined helper/default2048/fresh-state checks and new exact Windows CI are still pending at this entry; later optimized measurements are separate.


## 2026-10-09 12:59 UTC — Exact combined real-plugin check and publication freeze

- Combined4fdfbc2 + helper9dd18d74 passes all **four default2048** ordinary/routed × fixed/changing cases after real Surge Effects0→32 latency fence and actual stopped Retry/new revision/epoch. Every active endpoint completes375 blocks with357 exact +18 startup; exact PDC4896/7328 and events pass. Fresh Surge state restore immediately sounds the first note (sample60, peak0.21138866245746613 over1024frames), with controller0.8691863417625427 and component−6.27905654907227 dB unchanged.
- Matched preallocated **debug128 remains2/4**: fixed ordinary/routed cases fail FX DeadlineMiss at sequence151/1. No capture overflow occurs. Delivery PASS must not imply timing PASS: passing2048 changing cases still have14 callback-core interval overruns each;128 changing cases have18/24. The helper/harness/source binding and each raw non-audio receipt hash are in the separate [combined summary](../qa/plugin_timing_combined/README.md). It retains original summary bytes with SHA256 `57300eb4cac2e40eb81d7db9cb4228c475f11d7a12bfee82c63c1cf0b867d0e7`; its small publication is a summary, not a complete new reproduction harness.
- Added this distinct three-file summary with pinned inventory and strict missing/changed/extra/rewrite package regressions. Historical250-file debug publication and earlier91 files remain byte-identical and separately attributed. Full post-document/package Python verification is **207 tests**, including actual Unix helper cases. Actual package closure is **383 inputs**. New exact Windows CI is required after publication. The approved cold optimized experiment runs separately and is not a prerequisite or a claimed result of this correctness checkpoint.

## 2026-10-09 13:36 UTC — Bounded native-edit delivery and capture fence

Separate slice from published `244f622`; helper/wire/public Plugin/processor lease/App/timing and
Cargo sources are unchanged. Native performEdit values now use an existing-rtrb fixed-capacity
channel with generation/sequence tags, producer-only nonblocking synchronization and an exclusive
runtime consumer/preallocated staging. Display and gesture polling are separate. Checked COM
parameter admission plus each actual successful SDK Process is required to acknowledge a batch;
empty queues are not proof of delivery. Sticky loss/exhaustion and dirty-revision checks survive
polling, later successful blocks and state supersession. No new unsafe Send/Sync or worker.

Stopped polling is an intentional behavior fix: it no longer steals DSP edits and poisons a
subsequent save. Successful state application explicitly supersedes earlier native packets at the
existing queue-clear boundary, including setup-failure recovery; discarded edits are not called
applied. A contended/in-flight post-application fence fails explicitly and permanently closes
native input/capture instead of replaying stale edits. Rejected Surge preflight still leaves
queues, generation, lifecycle and history unchanged. Administration remains main-thread serialized.

Executed against the final frozen Rust source:
- 304 available vendor unit tests pass, including 19 channel tests and COM integration fixtures;
  26 doctests pass, including positive main-thread use and ownership compile-fail cases. The one
  named upstream metadata test remains excluded because the registry archive omits its Dexed
  fixture. The source-linked harness retains production source/features/dependency versions;
  original-manifest offline dev-dependency and prior-stage fixture limitations remain documented.
- 22 helper + 5 editor protocol + 2 transport protocol tests; 79 Python smoke-harness tests;
  4 executed Unix descriptor tests; 124 packaging tests; production helper build, root strict
  Clippy, rustfmt and diff checks pass. Direct vendor Clippy passes with explicit exceptions for
  unchanged deprecated-atomic and drain_collect diagnostics; the initial strict failure is kept.
  An initial pytest invocation failed because pytest is absent; the repository's unittest runner
  then executed all packaging tests successfully, without installing anything.
- Cumulative vendor patch replay onto the unchanged registry archive is byte-for-byte verified,
  with mechanical package provenance pins refreshed. No dependency/archive/license changes.
- Genuine copied-helper matrix passes 12 Surge fresh/used-active/used-stopped cases across
  17/47/128/256 frames plus Stochas state/MIDI. Fresh first-note PCM/getter/component state agree;
  used Surge restoration is safely rejected while the old instance remains usable.
- New real native Surge edit while stopped: volume 1.0→0.8691863417625427 (native −6.28 dB),
  then 20 rounds/80 requests polling dirty/value/changes/gestures, then SaveState with no positive
  Process. The newly produced 51,929-byte state, SHA256
  `011ded9e14952b8a631e9e9b443fad6f0997daf641919862e95f5e860f414272`, restores into fresh instances
  at all four profiles with correct getter/component state and finite first-note PCM without
  positive warmup. Peaks 0.194–0.214; this is functional evidence, not a latency measurement.
- Independent native verifier passes the complete trusted fixture, same-XID/generation/geometry
  Surge rejection before detach, closed-used rejection/reopen, and Stochas used/open restore of a
  native-created row115/step3 cell. Shutdown exits 0; no native windows remain.

Copied helper SHA256: `dca08353e3f23308d535a791c9fa2c89635ee683db29625fa8d2a3d0a988cbe8`.
Frozen Rust diff SHA256: `810e6a4f85c043c154a0ed90aa61110d660c6be98e377cf596f684646f90d2e6`.
Independent review verified all 41 source hashes and the helper and found no blocking issue.
Evidence index: `validation-receipt.json` in the native-edit development QA record;
the independent native record is `native-transport-receipt.json`. No raw task paths,
plugin states or executables are added to the source package.

The channel tests establish no per-operation allocation/free and consumer independence while
producer/display guards are held at least 350 ms, with a 3 s scheduling watchdog. They do not
establish whole-plugin real-time safety or remove the helper's single-threaded GUI resize stall.
Legacy COM parameter/event allocation/locks, data exchange/metering, hardware qualification and
Windows/macOS native execution remain separate work. The legacy full-chain Admin LoadState
closure/fault limitation is unchanged. No DSP latency/dropout improvement is claimed.


## 2026-10-09 13:44 UTC — Integrate native-edit transport after terminal timing checkpoint

- Previous exact `244f6220233a9a415f503b921da3b94b1130575c` completes [preview37933923565](https://github.com/wrench1997/DAW/actions/runs/37933923565) successfully:207 Python cases (203 passed/four Unix-only skips),1,165 app +15 helper +5 editor protocol +2 transport protocol tests, optimized/static-CRT build, strict package/provenance/PE/ZIP/hash and extracted-helper smoke. Upload is skipped and both artifact APIs return0. [Quality37933923608](https://github.com/wrench1997/DAW/actions/runs/37933923608) passes source and independent native interaction/state/lifecycle/cleanup, fails known before/after PrintWindow and skips repaint comparison; overall FAILURE remains.
- Its final source ZIP retains525 Git source files and original-main patch plus a separate post-freeze optimized receipt appendix. Final SHA256 `b187b32a4fbce7b7d36368bcc8a905f9334dfa95969b011ea90dd09df711c9b3`; source/patch/terminal receipt bytes unchanged from the pre-appendix archive. Original optimized archive SHA256 `9ffb53a5a2df46bb4e122d5bb4e6d0ad6603cc905101a0301faa3bc99f029d3b` retains199 text/source files, exact123-file4fdf/244 binding, all failed attempts,12/16 quiet and12/16 load delivery, and loaded2048 raw-interval overrun. It is not a new-helper performance qualification.
- Reviewed native-edit48d3f97 integrates without conflicts as **87ceb06ce3dc23d817b1623093c8a51fddc2bea3**. All41 source hashes match; App/audio/timing/helper dispatcher/wire/Cargo/fixtures are unchanged. Native queue/display ownership is the only runtime slice. The fresh copied helper is byte-identical to independent native/PCM helper **dca08353e3f23308d535a791c9fa2c89635ee683db29625fa8d2a3d0a988cbe8**.
- Fresh Linux all-feature/all-target tests pass **1,170 app +22 helper +5 editor protocol +2 transport protocol**, core **1,166**, zero failed/ignored/default stack. fmt, both strict root Clippy modes, all-bin build, ordinary actual helper smoke and both MSVC source profiles pass. **304 available vendor cases** pass with one explicitly filtered missing upstream Dexed fixture; all **26 doctests** pass. **207 Python tests** pass, including four actual Unix descriptor cases. No original vendor manifests/dependencies or lint policy were changed. The owner's separate direct-vendor lint exceptions remain disclosed rather than called an unqualified strict upstream pass.
- Pristine registry archive plus pinned patch reproduces **44 original/added files**, keeping original license/manifests/checksum. Existing **383 packaged inputs** have closed links/provenance and every historical QA payload remains unchanged. There is no new App/UI rendering claim: existing input suites rerun;123-entry/49-frame capture remains attributed to4fdfbc2.
- Current native outcome is stopped real edit→20 polling rounds→zero-sample SaveState→fresh numeric/component/first-note agreement, plus mutation-free Surge guard/Stochas/fixture checks. The350ms blocked-producer fixture tests transport independence only; GUI/DSP remain serialized and the346.9ms native resize stall is not solved. COM/event allocations/locks, data exchange/metering and full realtime qualification remain. New default2048/fresh-state regression and new exact Windows CI are still required at this entry.


## 2026-10-09 13:46 UTC — New-helper bounded correctness acceptance

- Exact87ceb06/new helperdca08353 passes four default2048 ordinary/routed × fixed/changing production-graph cases, including actual stopped Retry/native FX32 replan, exact delivery/events and PDC4896/7328. Routed notes remain8on/8off. Fresh-state immediate-note behavior also passes. Both invocations exit0; no new worker faults or capture overflow.
- Debug changing callbacks retain11/14 raw-core interval overruns. This bounded check does not renew the previous full profile matrix or certify hardware/native-GUI continuity. Historical optimized244f622 appendix remains byte-unchanged and separately attributed. New Windows checks are still pending until source publication.


## 2026-10-09 13:52 UTC — Preserve exact changed-helper regression receipts

- Imported52 source/text files byte-for-byte from the normalized archive SHA256 `b745b80f17572c51164839d89682caae9d4c607564d203beb860f6e5f8e94bf8` (134116bytes), under [qa/native_edit_regression](../qa/native_edit_regression/RESULT.md). Its own inventory retains raw versus normalized hashes; only task-local path prefixes differ from original records. No plugin binary, preset/state blob, executable, audio payload or private path is included.
- Exact124 production-file hashes bind to87ceb06, helper `dca08353e3f23308d535a791c9fa2c89635ee683db29625fa8d2a3d0a988cbe8` and test `876dfe3105289120693a807374fca2361b338783b62c42d273d76bb92130743a`. Four default2048 delivery cases and fresh restore exit0. The fresh note starts atframe28, peak0.2264193892478943, controller0.8691863417625427 and component−6.27905654907227dB retained. Both changing cases preserve11/14 raw-core interval overruns; this remains correctness-only acceptance.
- Packaging uses an exact52-path whitelist, pinned inventory bytes and every payload hash. Regressions reject missing/changed/extra/escaping/backslash paths and self-consistent rewritten inventories; the tree is pinned toLF on Windows. All historical QA trees and the separately delivered optimized244f622 appendix stay unchanged. Windows CI for the new publication is pending.
- Receipt/package regressions now pass **215 Python tests**, including all four actual Unix descriptor tests. Actual package closure contains **435 inputs**; all new52 files retain reviewed bytes. No Rust production file changed after the complete source/native gates.

## 2026-10-09 — Bounded parameter storage, checked admission and output-loss fence

Separate parameter-only continuation from `48d3f970`; helper dispatch/wire, application/timing,
event payload/container and processor-lease production sources are unchanged. Native transport
changes are confined to the cfg(test) allocator-accounting helper.

- Prepared stable COM queue wrappers and one total-point arena per container: input 8192 queues /
  8192 points, output 4096 / 4096. Safe mutexes, borrowed interface returns and explicit COM
  retention remain. A shared ordinal index plus an ordered populated-slot index retain first-seen
  queue order and every equal-offset value without per-operation allocation or COM ownership churn.
- Checked host admission now refuses the pending 4096 limit before controller mirroring. Every
  point admission is checked before SDK Process; failed admission clears staged contents without
  phantom native acknowledgment. Native acknowledgment still follows each actual SDK result.
- Output write/storage failure becomes a separate sticky runtime fault, including ignored plugin
  failures and reentrant getState writes. Process/SaveState refuse until a fresh instance; ordinary
  stop/start/state/reconfigure cannot erase it. Invalid read probes do not invent delivery loss.
- Constructor measurements: input 1,048,696 requested bytes / 8,198 allocations; output 524,408 /
  4,102. These exclude two outer runtime COM wrappers, allocator metadata and RSS. The populated
  arrays add 98,352 bytes versus the superseded dense-index version. Counted cold/high-water/reuse/
  error regions allocate/free zero bytes. Retired queues' final plugin-owned Release remains an
  explicitly separate lifetime/no-free boundary.

Comparative evidence drove corrections before acceptance. Original linked-cursor random reads,
lazy-index alternating rebuilds, linked random-insertion rank search, and dense empty-queue-offset
updates all showed significant regressions. Their exact sources, raw results and the superseded
`ee45bb1c` helper/native partial run are preserved; none is final acceptance. The accepted sparse
version retains explicit read/check overhead and global populated-suffix movement costs rather
than claiming universal speedup. Optimized 8192-point baseline→final medians: random insertion
8.192→2.005 ms; non-final interleave 9.747→0.455 ms; random reads 0.0473→0.0666 ms; large populated
single/multi suffix edits 0.657/0.744→1.293/1.678 ms. The previously unacceptable 8192-empty-queue /
128-edit result falls from 1.218 ms to 0.004216 ms (baseline 0.006560 ms). Five-sample cold/reused,
debug/release, 32/128/512/4096/8192-point and sparse cases use identical actual-value checksums.
The guide records smaller-case slowdowns and remaining aggregate quadratic worst cases.

Final source-linked gates: 339 available vendor tests, 26 doctests, 22 helper tests, 5 editor-wire
and 2 transport-wire tests; root strict Clippy, vendor Clippy with the existing deprecated and
intentional drain_collect exceptions; formatting/diff checks; 124 package tests, 79 Python smoke
tests, 4 Unix descriptor tests and direct helper smoke. The one upstream SDK metadata fixture is
still absent from the registry archive and explicitly filtered. The temporary vendor manifest
points directly to production source with cpal-backend/process-isolation and unchanged dependency
versions; this is not a claim that the omitted fixture or the original unavailable dev-dependency
manifest ran. Initial test-fixture ordering/import and mechanical Clippy failures are retained.
The cumulative vendor patch replays byte-for-byte against the original checksum-verified archive;
package provenance changes are hash pins only.

Genuine copied-helper regression passes all 12 Surge XT 1.3.4 fresh/used-active/used-stopped cases
at 17/47/128/256 frames, plus Stochas 1.3.13 state/MIDI. A newly produced stopped native Surge
edit/poll/save blob from the final sparse-helper run restores with normalized volume
0.8691863417625427 and component volume −6.27905654907227 dB before/after first-note processing at
all four profiles, without positive warmup. PCM is finite/nonzero; first nonzero samples in this
run are 60/18/73/55, respectively. These are functional results, not latency qualification.

Final copied helper: `a29e4942b5fe027e9891a28e5d7f7611d2c5b2f12ef5f283599d15c4278aee64`.
Frozen Rust diff: `b77e86e385579a919961e1a9600dd4dd77f25d0b96fe08446070633ef27fedcd`.
Independent source and comparative-cost review verifies all 41 source hashes with no blocker.
Full evidence: the parameter-storage `validation-receipt.json`; matched
comparisons: `parameter-comparison/populated/` within that QA directory. Earlier rejected results
remain alongside it. Exact new native state: the retained native `surge-native-polled.state` (not distributed).

The helper remains single-threaded and its historical 346.9 ms native-resize stall is unresolved.
Locks, arbitrary plugin work, event ownership, data exchange/metering and whole-Process allocation/
free behavior remain outside the parameter-storage guarantee. Capture still requires serialized
main-thread administration. Legacy full-chain live-admin LoadState closure/fault behavior,
Windows/macOS runtime, hardware and mixed-Wayland qualification are unchanged limitations.

Final independent native acceptance on the sparse helper also passes the complete trusted fixture,
same-XID/generation/geometry Surge rejection before detach, closed-used rejection/reopen, and
Stochas used-instance empty reset then exact native-created row115/step4 cell restore/re-export and
repaint (probability 20, velocity 127, length/offset 0). All windows close and Shutdown exits 0.
Native receipt directory: the separately retained `parameter-storage-sparse` QA directory.
The prior dense-index native run remains explicitly superseded and is not counted here.


## 2026-10-09 15:02 UTC — Integrate reviewed parameter storage

- Prior exacte487a50 completes preview37940326948 successfully:215 Python cases (211 passed/four Unix-only skips),1,165 app +15 helper +5+2 protocol tests, optimized/static-CRT/provenance/PE/ZIP and extracted-helper checks. Quality37940326370 retains only before/after native paint failure; repaint comparison skipped, source/state/control/lifecycle pass. Upload skipped and both artifact APIs0. Delivered-source candidate578 tracked files plus separately historical optimized244f622 appendix has SHAac79bb1d60c4ab2fc151607e5aacf116cd2e035e0b0babbe425d3b1ad32101e2.
- Only reviewedbf573a4 integrates asfb7b91a82d31226485a22e9e5e0b73d1f9a1fe3f. Additive docs conflicts preserve both histories; production/vendor/Cargo/tests byte-match the approved feature. All41 receipt hashes and native receipt450ee6a6 independently verified. Original registry archive + cumulative patch reconstruct44 files; original license/manifests unchanged. Complete combined gates and a new-helper2048/fresh-state regression are now running, independently of older optimized evidence.


## 2026-10-09 15:05 UTC — Exact combined parameter source gates pass

- Fresh1170 app +22 helper +5+2 protocol and1166 core tests pass, zero failed/ignored on default stack. Both strict root Clippy modes,fmt,all-bin build,actual helper smoke,both MSVC source profiles pass. Actual vendor harness passes339 available cases with one known missing upstream fixture excluded and all26 doctests; no source/cfg/lint relaxation.215 Python tests pass, including four actual Unix descriptor cases.
- Copied combined helper SHAa29e4942b5fe027e9891a28e5d7f7611d2c5b2f12ef5f283599d15c4278aee64 matches the independently reviewed/native-tested final sparse helper. No production source changed after those gates. Existing435 package inputs close links/vendor/historical QA hashes; new source-only cost/regression receipt publication will be checked separately. Shared build target handed to the bounded default2048/fresh-state validator; no new performance matrix implied.


## 2026-10-09 15:07 UTC — New parameter helper bounded correctness pass

- Exactfb7b91a/helpera29e4942 passes four default2048 production-graph ordinary/routed × fixed/changing cases plus fresh-state immediate-note; both invocations exit0. Actual stopped Retry/new timing revision/epoch after genuine FX0→32 change retains exact4896/7328 PDC, routed8on/8off, finite nonzero PCM and exact worker delivery. No new worker fault or capture overflow.
- Changing debug ordinary/routed callbacks retain13/14 core and15/14 outer interval overruns. The new source-linked receipt is correctness-only; no old optimized result is relabeled for this changed helper. Root approved a separate immutable full comparative-history appendix and concise hash-bound summary/raw-cost index in Git. Original rejected sources/observations remain preserved; no binaries/assets/captured state/audio are included in that new evidence.


## 2026-10-09 15:10 UTC — Parameter helper correctness receipt publication

- Preserved52 source/text files byte-exact from archivea02b5d2deee6793902a067897ad36b025728a50f237f47a3da65417921e1cc37 (134041bytes), under [qa/parameter_storage_regression](../qa/parameter_storage_regression/RESULT.md). All124 production hashes matchfb7b91a; testd09d2d72e4a5c76adad988c475247911713075e5a1d96570e1b396df96836362 uses exact helpera29e4942 and unchanged raw harnessc38f30c4. Original/published hashes and all observations remain; only task-root prefixes are normalized.
- The separate combined fresh-state test uses captured input104bc8ab… and first nonzero sample24/peak0.22329643368721008. This is not the source-owner's newly native-created011ded9e… state with per-profile60/18/73/55 onsets. Both are independently successful and separately attributed.
- New exact whitelist/pinned-inventory packaging tests and all other Python suites pass223 cases; actual package input closure is487 before the concise comparative-cost set. Root/vendor/runtime bytes remain unchanged. Historical QA and old optimized244f622 evidence are not relabeled.


## 2026-10-09 15:15 UTC — Preserve full comparative history separately

- Approved immutable source-only appendix `parameter-storage-bf573a4-source-evidence.zip`:5092126bytes,SHA1bd33df7c1ec9afa58cd5b6a508b7af12c4a84f68ce4ad12cda5dca3f388773c. Content-addressed435 text objects reconstruct1677 logical files, with450 members verified,28 source/license associations and no binaries/assets/state/audio/cache payload. All rejected drafts and original observations remain; the archive avoids redundant copies without losing reconstructability.
- [Nine concise Git records](../qa/parameter_storage_cost/README.md) preserve complete final matrices, constructor/native/source results,105-entry raw-cost inventory and exact full-archive identity. Raw/published hashes and normalization counts remain explicit; the private reverse map is not published. The two original CSVs keep their CRLF bytes with exact file-specific attributes; every other new text file usesLF. Strict whitelists/inventories reject tampering and partial sets.
- Exact final Python aggregate passes **231 cases**, including four actual Unix descriptor tests. Real package closure is **496 inputs** with all existing and new provenance/link checks; all61 new correctness/cost files retain publication bytes, including raw CRLF CSVs. Production source remainsfb7b91a and helpera29e4942, with no post-gate runtime edits.

## 2026-10-09 — checked event admission and note bookkeeping

A separate bounded correctness slice on `bf573a4` checks event-header/payload admission before
raw deep copy and propagates enqueue failure through MIDI, owned-event, expression and tracked
voice APIs. Existing limits are 4096 headers, 8 MiB total payload, 1 MiB individual data and
16,384 UTF-16 units per text event. Rejected admission preserves queued contents and budgets;
callbacks use bounded loss evidence rather than formatting admission errors.

Ordinary counts and tracked voice IDs commit after successful admission. Full tracking rejects
instead of evicting an active voice; full ordinary counters and wrapped-ID collisions reject
without mutation. Panic commits only its admitted release prefix, retains remaining tracked and
ordinary obligations, and bounds each call to 4096 combined releases. Controller-parameter
failure after successful releases cannot undo or replay that prefix. These are explicit behavior
fixes. Admission remains distinct from SDK processing or native-edit acknowledgment.

Source-linked available vendor suite: 362 passing tests, one known absent upstream SDK metadata
fixture filtered; 26 doctests; 22 helper, 5 editor-protocol and 2 transport-protocol tests. Strict
root Clippy passes; direct vendor Clippy retains only the established deprecated and intentional
drain_collect exceptions. Formatting/diff checks, 124 packaging tests, 79 Python helper-smoke
unit tests, 4 Unix descriptor tests and direct copied-helper smoke pass. Dependency versions and
actual enabled cpal-backend/process-isolation features remain recorded in the temporary harness;
its lib.path points directly to unchanged production source files. The original unavailable
upstream dev-dependency manifest and missing fixture are not claimed as executed.

New focused tests cover every supported scalar and payload boundary, malformed metadata,
no-copy rejection with unread payload pointers, exact aggregate limits, deep-copy/FIFO ownership,
poison, scalar first-use/high-water/retry with zero measured allocation/free, and transactional
note/panic rejection. A successful SDK mock that ignores output-event overflow still acknowledges
its native input, succeeds and exposes the existing loss-aware event drain; that loss does not
become the separate permanent output-parameter fault. All existing native/state/Surge/split and
failed-SDK cleanup regressions run in the same suite. Initial fixture errors (using valid legacy
MIDI tag 65535 as an unknown tag, and output-fixture constructor/import mistakes) are retained in
raw logs; they required no production workaround.

Payload Vec allocation/free, Mutex synchronization, legacy void staging failure handling and
vendor legacy playback/realtime ignored results remain outside this slice. Active App fault
propagation was inspected but not changed. Helper/wire/application/timing/lease/native transport
and parameter storage behavior remain unchanged; no worker or hidden state-settlement Process
is introduced. The historical 346.9 ms native-resize stall remains unresolved. Actual-plugin
and native-window acceptance is recorded separately after immutable source/helper freeze.

Frozen copied helper: `59b6bcbdb7a90b08fe5c8ebb2da2ba3ffe368c1089d70a6c7d4e7ab28b90d086`.
Rust diff: `482031cc4050ca3747db2b276a5b2957b0a55d32f98a07c684eb408683584e23`.
The cumulative patch replays against the checksum-verified original crate with every source file
byte-compared. Package script/test changes are only refreshed patch/manifest pins. The new helper
passes all 12 fresh/used-active/used-stopped Surge cases at 17/47/128/256 frames and Stochas
state/MIDI. No caller warmup or state-guard bypass is used. Exact native-produced-state first-note
and window checks remain separately recorded. Evidence root: the retained event-admission QA record.

The newly produced event-helper native stopped-edit/poll/save blob restores on the same frozen
helper with normalized volume 0.8691863417625427 and component volume −6.27905654907227 dB before
and after first-note PCM at 17/47/128/256. PCM is finite/nonzero without positive warmup; observed
first nonzero frames 25/46/24/23 are functional evidence, not latency qualification. Independent
exact-helper default2048 checks pass four routed/ordinary fixed/changing cases plus authoritative
fresh-state first note, with correct PDC and 8 note-on / 8 note-off routed pairs, no worker fault or
capture overflow. Changing debug cases retain 12/14 core interval overruns and do not establish
performance acceptance. That separate receipt root is
the separately retained `checked-event-admission-regression` record.

Independent native acceptance on the frozen event helper passes the complete trusted fixture,
new stopped Surge edit plus 20 display/gesture poll rounds and zero-sample SaveState, fresh native
numeric agreement, same-window preflight rejection before detach, closed-used rejection/reopen,
and Stochas used-instance empty reset then exact native-created row115/step5 cell restoration,
re-export and repaint. Shutdown exits 0 with no native windows remaining. All 41 source hashes
are independently verified before/after; 113 raw wire exchanges and new captures are retained.
Receipt: the retained `native-event-admission-receipt.json`,
SHA256 `835ccf39085011738adf12cd5587ad4cac8d3968a0d0318023b1741a1b0dfc25`.
The independent review binds the final Rust/helper bytes with no source blocker. GUI results do
not imply DSP-thread isolation, resize performance, hardware or cross-platform qualification.


## 2026-10-09 15:45 UTC — Prepare checked-event checkpoint independently

- Prior publishedb92c394 preview37950671153 completes successfully:231Python cases (227 passed/four Unix skips),1,165app+15helper+5+2protocol,optimized/static-CRT/provenance/PE/ZIP/extracted smoke; upload skipped,both artifact APIs0. Quality37950671033 has only native paint before/after failure and repaint comparison skipped, with independent source/state/control/lifecycle passes. The639-file source ZIP plus unchanged current-cost/historical-optimized appendices verifies SHA341fee9ac3b6f6ffb01014e7d10d0ec931a27c03d71d129fb7ddaafa11c64351.
- Only event7d8a416 integrates as7d7294b36df8f877c30a16a14afb98cf0f1ac047; additive docs histories retained. src/vendor/Cargo/tests byte-match reviewed feature; all41 source hashes and native835ccf39 receipt verify. App/reset remains unchanged. Existing496-input package links/vendor/QA provenance pass read-only inspection. No new combined build/test run or publication is claimed at this entry.
- Source-owner final review closes with no blocker. Default2048 source-only archive4e696243f9782fd6a623f89010d1b65e58e5af109891a6183498c334516a7da1 (152229bytes,55text/source files) preserves basebf573a4+exact482031cc diff and separate final7d8a416 equivalence, not a false base-only claim. Four delivery cases plus fresh state exit0;12/14 core overruns remain. Complete combined gates and any separately reviewed reset-policy work remain pending; no hypothetical reset change is included.


## 2026-10-09 15:50 UTC — Event-only gates and receipt preparation

- The checkpoint proceeds independently; reset policy remains unchanged. Imported55 original portable files under [qa/event_admission_regression](../qa/event_admission_regression/RESULT.md), with exact path whitelist and pinned inventory9bf3d8af3a911286a766b628f547ee4579e2d40ef582b38b6c36254d0e9b8b88. Every124 tested production hash matches both reviewed7d8a416 and integrated7d7294b. No base-only provenance claim or renewed optimized-performance claim is made.
- Existing/new551 package inputs close links/vendor/all QA inventory checks; official archive+patch reconstruct44 files, preserving original license/manifests. Fresh all-feature1170app+22helper+5+2protocol already passes; remaining aggregate gates continue. No publication or complete combined acceptance is claimed before those gates finish.


## 2026-10-09 15:52 UTC — Final event-only combined source gates pass

- Complete all-feature/all-target1170app+22helper+5editor+2transport and no-default1166core pass, zero failed/ignored on default stack.362 available source-linked vendor cases pass with one explicitly excluded missing upstream Dexed fixture, plus26doctests. fmt,bothstrictrootClippy modes,all-bin build,actual helper smoke,andbothMSVCsource profiles pass. Full239Python cases pass including four actual Unix descriptor tests.
- Fresh combined helper SHA59b6bcbdb7a90b08fe5c8ebb2da2ba3ffe368c1089d70a6c7d4e7ab28b90d086 is byte-identical to independent native/headless/default2048 evidence. All124 tested source hashes match7d7294b; App/reset/timing/wire remain unchanged. Existing exact-source native results are bound through equivalence, not relabeled as a new measurement.
- New55-file exact whitelist/pinned inventory and provenance regressions pass; all551 actual source-package inputs have closed links and exact vendor/historical/newQA hashes. Read-only parent review found no blocker. Publication/WindowsCI remain pending at this checkpoint entry; no reset proposal is included.

## 2026-10-09 — private scoped domain-session checkpoint 1

A separate worktree based on reviewed event commit 7d8a416 introduces the approved private scoped
ControlOps/ProcessorOps seam and the AtomicBool-only data-exchange RAII gate. Owner/module/COM
ownership remains in place and exclusively borrowed until rejoin; both facades remain !Send/!Sync.
Only runtime-independent operations are factored. Mixed controller setters, cache refresh,
state/lifecycle/metadata and the separately designed reset operation remain aggregate-only.
No helper/broker/thread/default-policy change is introduced. Full source/helper/native gates and
private positive/negative compiler contracts are recorded in the final receipt.

Final source gates pass 374 available vendor tests (one known missing upstream SDK metadata
fixture remains excluded), 26 doctests, 22 helper tests plus 5 editor and 2 transport protocol tests,
strict root Clippy and direct vendor Clippy with only existing deprecated/drain_collect exceptions,
format/diff checks, 124 packaging, 79 Python helper-smoke tests, 4 Unix descriptor tests and direct
helper smoke. Four new RAII gate tests and eight real scoped-session tests complement all retained
state/native/parameter/event regressions. The checked-in private compiler runner passes 47/47,
including a real private positive baseline and 46 diagnosed negative cases, with stable source and
manifest/lock hashes and no custom flags/compiler wrappers. Exact runtime dependencies/features
are recorded; this does not claim the unavailable original upstream dev manifest ran.

One initial Clippy failure exposed cross-worktree shared-target metadata: the unchanged session
helper saw another branch's reset command variants. The raw failure is retained. An exclusively
coordinated timestamp-only source refresh forced this worktree's vendor compile; verbose rustc
commands identify the current source and the helper's exact linked rlib, with before/after source
hashes and copied dep-info. A second forced build reproduced the identical helper. No cross-branch
cache deletion, reset imports or permissive helper match-arm workaround was used.

Frozen helper: `e094690aa0de2788c2a8cea64817859aac44332cd6290a45410e16deea5a944b`.
Frozen Rust/compiler-runner diff: `51f9549fe5e93c52765879b9e8e413f02887b1d325617a21eb379783f6763dbf`.
The 44-file manifest and forced-artifact binding are independently reviewed without a source
blocker. The cumulative vendor patch replays byte-for-byte against the exact original crate;
packaging changes are provenance pins only. New copied-helper headless regression passes all 12
Surge fresh/used-active/used-stopped cases at 17/47/128/256 plus Stochas state/MIDI. Current receipt
root is the retained domain-session QA record; genuine native-window and newly produced GUI
state/first-note results are recorded separately after this immutable freeze.

The newly produced domain-session native stopped-edit/poll/save blob restores on the exact frozen
helper with normalized volume 0.8691863417625427 and component volume −6.27905654907227 dB before
and after first-note PCM at 17/47/128/256. All four renders are finite/nonzero without positive
warmup. Observed first nonzero frames 64/44/64/54 describe these functional traces only. The
new-run blob hash is `011ded9e14952b8a631e9e9b443fad6f0997daf641919862e95f5e860f414272`;
its deterministic bytes match earlier captures, but the actual drag, 20 stopped poll rounds and
capture were repeated with this helper. Exact result and command traces are retained under the
current receipt root. No default2048 or timing result from an earlier helper is relabeled here.

Independent new native acceptance passes the complete trusted fixture, stopped Surge edit and
20 polling rounds followed by zero-sample SaveState, fresh native numeric agreement, same-window
and closed-used preflight guards, and Stochas used-instance empty reset then exact seventh-column
C3 cell restoration (row115/step6/probability20), re-export and repaint. All 44 frozen source hashes
match before/after; 113 exact wire exchanges and six current-run captures are retained. Shutdown
exits 0 and no plugin window remains. Receipt:
the retained `native-domain-session-receipt.json`,
SHA256 `3b69b430879369a14ed6f589a752be60d0b57c753c910403ac8549b1f3669653`.
These source-bound Linux results complete this same-thread ownership checkpoint's functional
regression gates; hardware, cross-platform native runtime and processing-deadline acceptance
remain separate.

## 2026-10-09 — Explicit stopped reset-origin quantum (isolated implementation)

The reviewed reset design replaces the DAW's exceptional one-frame cleanup with owner-only
Q128 origin processing, preserving the old event/parameter ordering rather than changing only
the block length. Capability/preflight precedes safety input; partial admission, output loss,
poison and uncertain IPC remain visible failures. Native UI feedback and actual SDK input
acknowledgments are preserved. Standalone vendor/helper small blocks remain supported; the
minimum128 is specific to the DAW backend. See [reset-origin contract](PLUGIN_RESET_ORIGIN.md).
Historical cold-start/latency traces and timing-profile failures remain immutable. Source gates
and genuine acceptance are recorded separately; no GUI threading or hardware qualification is
implied by this reset correction.

Reset source gates: copied, Cargo-identified all-feature artifacts pass **1,175 app +25 helper
+6 reset protocol +5 editor protocol +2 transport protocol** tests. Core-only passes **1,167**.
The source-linked vendor suite passes **382 available cases**, with the same named upstream
Dexed metadata-fixture exclusion, plus **26 doctests**. Both root strict Clippy profiles and
vendor Clippy with the pre-existing deprecation/drain-collect allowances pass; formatting and
**124 packaging tests** pass. A source-linked initial test used truncation instead of the
existing rounded system-clock increment; its 1ns assertion failure and corrected passing run
are retained. Verbose forced compilation proves the reset worktree supplied the linked vendor
rlib, and source hashes stayed unchanged across link; copied executable hashes and logs are in
the retained reset-origin QA record. Genuine reset/profile acceptance remains separate.
Both Windows MSVC feature profiles pass source-only checks, and the exact copied production
helper passes ordinary no-plug-in protocol smoke. These checks are not native Windows execution.


## 2026-10-09 16:38 UTC — Combine scoped sessions and reset origin

- Previous20a9dfc terminal preview37955606487 passes239Python cases (235/four Unix skips),1165app+15helper+5+2protocol,optimized/static-CRT/provenance/archive/extracted smoke; upload skipped and both artifact APIs0. Quality37955606457 fails only the established native paint before/after checks, with repaint skipped and independent state/control/lifecycle passes. The694-source-file bundle and two unchanged historical appendices verify SHA3a5df4f6aa6b8ecf5f4c05a3497398dc4abeae41b774c5e41614634e96fe8769.
- Session96410ec integrates as13b5aa0; resetfd6f5b1 combines as1f021b154c436d2910c15a4a9ddfea5617f8c792. Independent semantic review binds the resolved runtime: ordinary/reset SDK-only restricted RAII gate, checked origin ordering, discard/loss/native acknowledgments, strict metadata and owner-only reset after session rejoin. No facade gets administrative authority.
- A new COM test observes gate entry inside SDK and clearing on success/SDK failure, with no entry on defensive staging failure. Six reset-negative compiler cases plus positive owner-after-rejoin extend the original47 contracts to53. Official cumulative patch/pins replay; reset guide is explicitly included in package-link closure. Original source/negative receipts and old parameter/timing appendix identities remain unchanged.
- Forced verbose current-vendor compile and helper link produce421a8d7dc8c793cc5e8ecd81e0c74392b24383eed08ed21a4d64fee8f9473c41 with source bytes stable. Full merged gates/compiler contracts and fresh combined-helper graph/native state smoke remain pending at this entry; isolated helper results are not reused as merged acceptance.
- Reset validation identified a retained old/new immediate held-note transient after stop/start; fresh no-reset sustain and ordinary graph first/replay are different contracts. Initial failed onset/tail assumptions and advancing blank-source controls remain in the source-bound receipts. No universal retrigger, tail clearing, resize-stall or hardware qualification claim is made.


## 2026-10-09 16:51 UTC — Baseline combined gates pass; exact Surge correction held separately

- Exact1f021b1 passes1175app+25helper+6reset+5editor+2transport all-feature tests,1167core,395available vendor cases (same one explicit missing fixture),26doctests,53/53 private compiler contracts with stable source/manifest/lock,fmt/both strict rootClippy/build/helper smoke and both MSVC source profiles. Official patch replay reconstructs46 files. The final Python rerun executes all239 cases including four real Unix descriptor checks with the copied helper; no skips. Initial default-path discovery235 cases/one class skip remains recorded. Current552 package inputs close all links/vendor/QA inventories.
- Source-bound interimhelper421a8d7d is independently linked from current vendor and copied byte-exact. Fresh combined native graph/UI acceptance is intentionally NOT RUN while a narrowly reviewed reset correction is prepared. Direct vendor Clippy will run on the final corrected combination; this entry does not expand strict root Clippy to an unexecuted vendor check.
- Historical fd6 reset diagnosis archive3ff69eaba1a11b8640e6b83534768d13f395430be92be6ec9085818b97e4320d (814776bytes,335files/24directories) preserves all16 invocations and failed onset/silence hypotheses. Four after-decay arms sustain a distinct G4 after genuinely advancing four seconds. A matched control adding onlyCC120 reproduces the immediate truncation; an explicit diagnostic256 reset retaining CC120/panic sustains. No omission of safety CC or global reset256 policy was authorized.
- Root approved only exact loaded Surge XT instrument factoryUID/version1.3.4 for one reset256 operation, others128, with prepared maximum rejection before state/CC/lifecycle. The correction, final source gates and new genuine acceptance remain separate from this baseline; publication is held.

## Exact Surge XT 1.3.4 reset cleanup policy

A narrowly scoped follow-up to the reviewed reset-origin/session combination selects one
256-frame owner-only reset for actual loaded instrument UID
`ABCDEF019182FAEB566D624153675854`, version exactly `1.3.4`. All other identities (including
Surge FX) retain128. Normal worker Q128, device/prepared maximum, wire, safety CCs, lifecycle,
metadata reads, output discard/loss policy, and restricted session data-exchange gate are unchanged.
Capacity refusal precedes pending-state/prepare mutation and every worker's first safety message;
prepare checks both requested configuration and the helper's authoritative existing capacity.

The prior source/helper-bound diagnostics remain preserved under
`reset-origin-regression/runs/run011-post-reset-note-contrast` through
`run016-single-cc120-contrast`. Both legacy1 and reset128 truncated an immediate new held note;
lifecycle reorder alone also failed. The matched CC120-only contrast and full-safety256 positive
control agree with official source's eight32-sample deferred all-sound-off blocks. Scheduled
playback success is not represented as proof of immediate-note sustain. See
`docs/PLUGIN_RESET_ORIGIN.md` for identity scope, source links, internal-state advance, and limits.

Validation for this correction is recorded separately; historical receipt bytes are not modified.

The seven focused fake-helper/backend regressions passed on the corrected source. The matrix
also verifies exactly one reset request, unchanged stopped sample/PPQ position, all48 safety
messages, and no ordinary Process/SaveState/merged feedback poll. Two setup-only command
mistakes (wrong binary name, then a zero-match filter) are retained in the separate QA directory;
the final filter executed seven tests. Aggregate/source-bound genuine gates follow separately.


## 2026-10-09 16:54 UTC — Integrate reviewed exact-identity reset correction

- Reviewed236f7846ff869f38815ec66ca138d7387adc4c1b integrates asf08617092f74903862a9e7bf04698c2db49b523c. All136 frozen Rust/Cargo hashes match; src/vendor/tests/private-contract runner are byte-identical. Only additive WORK_LOG reconciliation was required. The merged RAII/owner-only reset invariant remains unchanged.
- Only actual loaded Surge XT instrument UID/version1.3.4 selects one256-frame reset. Others, includingSurgeFX, retain128; selected capacity is checked before state/safety CC/lifecycle. No ordinary warmup, dropped safety message, PDC/worker/wire or metadata policy change is added. Rebuilt helper421a8d7d remains identical because the correction resides in the App backend policy.
- Owner aggregate gates and corrected source-bound genuine checks run separately. Main final gates await their coordinated target/quiet handoff. No new native acceptance or publication is claimed before those results; baseline1f and historicalfd6 diagnostic records remain intact.


## 2026-10-09 17:32 UTC — Corrected session/reset checkpoint acceptance closes

- Exact main runtimef086170 passes1,179app+25helper+6reset+5editor+2transport all-feature tests and1,167core, all zero failed/ignored. Both strict root Clippy profiles, fmt/build/actual helper smoke and both MSVC source profiles pass. Vendor395 available cases (one named missing Dexed metadata fixture excluded),26doctests and direct-vendor Clippy with only baseline deprecated/drain_collect allowances pass.
- An interrupted final compiler run stopped without a terminal report. Its raw log and run0001 are retained; completed Rust gates were not repeated. Run0002 completes53/53 private contracts with stable source/manifest/lock. Final Python239/0 skips includes four actual Unix descriptor cases; official archive+patch reconstruct46 source files and existing552 package inputs close document/vendor/all QA checks. Forced current-vendor rustc/link binds unchanged copied helper421a8d7d.
- Corrected-source archivefe46a506eb1a936fcf0fc13f8f3ed571f31ff5c1e3339e5563c29a77578b70f5 (339092bytes,129files) passes all five actual-plugin invocations. Automatic exact-identity256 reset sustains new G4 at offsets0/1/127, held/future+sustain cleanup leaves192000 samples zero, state/host position are unchanged, genuine max128 refusal works, eight normal graph phases keep FX0/no initialRetry, deliberate ordinary1 still causes strict FX32 drift/actualRetry, and advancing blank-tail/fresh-state controls pass. Core interval overruns12/14 ordinary and15/17 routed remain. All136 frozen owner hashes match236f784/f086170;127 runtime snapshot files and appended harness modules are separately bound.
- Fresh native smoke verifies136 hashes before/after: newSurge edit/poll/save/fresh numeric, unchanged guard window/value, Stochas step7 used-state restoration/repaint and cleanShutdown. The exact newGUI blob011ded9e passes fresh17/47/128/256 first-note PCM without positive warmup, onsets11/44/64/40 and unchanged controller/component. Raw receiptbd6365eb84026aa96050a0089663a736e1b2f3534d74f6e225c3fc6b7cb2a138, normalizedde96e3194ea881bcc0589fb68be3c213aba35efc7d53634c34dfae742fa9456c. Minimal smoke does not borrow prior fixture/EOF/crash or hardware results.
- Historical fd6 diagnostic archive3ff69eab remains separate, including failed onset/silence assumptions, old/new128 immediate truncation and causalCC120/256 controls. Historical parameter1bd33df7 and optimized244f622/9ffb53a5 archives remain unchanged. New receipts establish correctness only; the256 reset advances internal DSP5.333ms at48k without timeline/PDC advance. Windows publication/checks remain pending at this freeze; no main merge or binary release.


## 2026-10-10 — Checked replacement metadata, unvalidated source checkpoint

The first resumed patch starts from accepted `cbbbe1f`, independently of the unavailable
unpublished dispatcher checkpoint. Isolated latency/tail getters previously converted failed
IPC polls into zero; production readiness and later metadata publication could accept those
zeroes as genuine answers. New checked getters preserve transport, Error and wrong-response
failures without automatic helper recovery. The worker consumes one fallible metadata pair;
neither value is accepted unless both reads succeed. Legacy infallible APIs remain compatible.
Existing helper response variants and main-thread ownership are retained.

Source tests cover valid zero/nonzero/infinite tail, error/wrong replies, helper death with
legacy recovery opt-in, initial readiness rejection, later sticky faults and old candidate
state/endpoint retention. Timing Retry or a new transport epoch cannot revive a faulted helper;
only a separately prepared fresh candidate with successful checked metadata can become ready.
No automatic reload, state discard, silent warmup, worker separation or SHM workaround is added.

Status at this archival checkpoint: **UNVALIDATED, tests not run**. The replaced environment
has no Rust toolchain; the source archive is persisted before toolchain bootstrap. Formatting,
compilation, test execution and mechanical vendor patch/hash regeneration remain pending.
The adjacent AudioBusLayout fallback is a separate approved follow-up and is unchanged here.

Before the first compile, official task-local Rust/Cargo 1.99.0 was restored (rustc
b940084d7, LLVM23.1.1), matching the accepted version. Additional review cases cover raw
malformed JSON, valid-latency/failed-tail in the real VST3 backend, and a full fault-event ring.
Touched-source rustfmt and exact46-file upstream archive+patch replay now pass; compilation
and test execution remain pending in the next UNVALIDATED recovery archive.


### Executed checkpoint 01 validation (2026-10-10)

On the recovered Linux cloud workspace, the restored official Rust/Cargo 1.99.0
(rustc `b940084d7`, LLVM23.1.1) compiles the patch. All 1,185 all-feature App tests,
1,171 core-only App tests, 25 helper tests and 13 protocol tests pass. The directly
source-linked vendor harness passes 400 available tests and 26 doctests with
`cpal-backend,process-isolation`; its original full run records one failure because
the upstream Dexed moduleinfo fixture is absent. That failure is retained, and the
subsequent available-case run explicitly filters only that named fixture. No Rust
test source or cfg was removed; the external harness omits unused example/dev-only
dependencies and records its exact manifest, lock and source hashes.

Both strict root Clippy profiles, formatting and upstream archive+patch replay
(46 files) pass. Python reports 235 tests with one missing-helper skip, followed
by all four real copied-helper Unix descriptor cases passing with the helper path
set. The initial candidate test invocation accidentally selected zero tests; its
log is retained, and the corrected named invocation passes one test. Metadata
failure tests cover malformed JSON/timeout, helper death, wrong responses, an
actual valid-latency/failed-tail backend pair, saturated fault-event delivery and
old-model/endpoint retention. No missing reply becomes a healthy zero.

This is source/mock/helper-protocol validation, not a new genuine-plugin, native
editor, Windows runtime or performance acceptance. The stable accepted release is
unchanged. Timing Retry remains a timing-plan operation and does not revive a dead
helper; there is no automatic state reload. The initial UNVALIDATED archive entries
above are historical checkpoints, not the current test result.


## 2026-10-10 — Checked bus-layout admission, checkpoint 02 (UNVALIDATED)

The production VST3 backend now propagates `AudioBusLayout` query failure instead
of guessing stereo channels. A valid reply retains the existing sum of active
bus channels, including empty, mono and multibus layouts. This change occurs
after helper LoadPlugin initialization, but before backend construction, pending
state restoration, DAW prepare/reconfigure/start/reset or endpoint promotion.
The existing disabled-auto-recovery policy and wire format are unchanged.

New tests exercise Error/wrong-response/helper-death refusal, valid channel counts,
no candidate readiness on layout failure, and the existing App old-model/endpoint
retention path for load failure. At this source archival boundary these added
tests have not been compiled or run. The preceding metadata checkpoint remains
separately committed and source-tested at `6fdfb8f`.

The first checkpoint02 compile found a missing qualified test-only
`PluginPrepareConfig` path in the App fixture. The diagnostic is retained; the
fixture now uses the same fully qualified type as its production loader. Its App
case proves generic nonexistent-plugin loader-failure retention; the direct VST3
backend/candidate tests separately prove layout-specific refusal. These are not
an end-to-end App VST3-layout integration test. No production correction was
needed for this compile error.


### Executed checkpoint 02 validation (2026-10-10)

After the test-only path correction, all three direct VST3 layout regressions
and the expanded App retention case pass. Full current-source runs pass 1,188
all-feature App tests and 1,171 core tests; both strict root Clippy profiles and
formatting pass. All 239 Python tests pass with the copied helper configured,
including its four actual Unix descriptor checks.

The vendor, helper and protocol inputs are byte-identical to checkpoint01 across
68 recorded files; those 400 available vendor tests, 26 doctests, 25 helper tests
and 13 protocol checks are reused results, not claimed as new executions. The
build reproduces the same helper SHA256
`000195915ef88f8ab2e212b97f87f26bb41eb9a7d54a36540fdb470a61e409cd`.
The initial compile failure, absent upstream fixture, and prior checkpoint
qualifications remain in the evidence. Genuine-plugin/native/Windows runtime
acceptance and release are outside these source-only checkpoints.


## 2026-10-10 01:58 UTC — Integrate reviewed metadata/layout refusal checkpoints

- The clean recovered development branch fast-forwards through reviewed `6fdfb8f39a130dfb02f0707d3e0cc5b8eeaefb64` and `959958f6bcb1f5ac99b297b94eee6224eae9ebba`. All 700 tracked files match the final executed source manifest before additive status documentation. No unavailable former worktree or artifact is used as current evidence; the unrelated dirty checkout is untouched.
- Exact owner receipts bind current 1,188 App/1,171 core, both strict root Clippy profiles, fmt and 239 Python. The unchanged 68 vendor/helper/protocol inputs explicitly reuse checkpoint01's 400 available vendor, 26 doctest, 25 helper and 13 protocol results. The absent Dexed fixture, zero-match first filter, initial helper-env skip and corrected test-only type-path failure remain preserved.
- Integration verifies copied helper SHA256 `000195915ef88f8ab2e212b97f87f26bb41eb9a7d54a36540fdb470a61e409cd`, replays the official crate plus cumulative patch for all 46 files and independently reruns Python/package checks. Only documentation changes follow; tested production bytes stay exact.
- This is bounded error propagation/admission, not new genuine-plugin, native editor, Windows runtime, thread or performance acceptance. Historical cbbbe1f source and its approved appendices retain their original attribution. New exact-commit CI and final bundle integrity are recorded separately after publication; no main merge or executable release.


## 2026-10-10 03:16 UTC — Integrate short-note properties and exact numeric round trips

- Fast-forward only reviewed `96223cc1ca63e2bac0e6089653b1f108a9dbd4de`, `3adc585fc9dc743dbb9ffbda7afcdfbc0b591312`, `707e0ea77034ed0d276b906464e5d3283afff7d1` and docs-only `97be40bb9397a835bda0ffda20690509fc2d4ebc` onto3018b41. All700 tracked sizes/SHA256/Git blobs match the final frozen manifest. Final implementation/tests are byte-identical to tested707e0ea; helper/vendor/protocol/timing remain unchanged. Experimental runtime work and the unrelated dirty checkout are excluded.
- Properties share the canonical1/64 timing minimum and precise horizon-end validation. Shortest round-trippable f32 text prevents untouched focus/Enter/Apply from changing1/64,1/24 and last-legal-start timing. Production input checks demand exact Project, dirty and Undo/Redo preservation.
- Exact final execution passes22 focused expression checks,1175 core,1192 App+25 helper+13 protocol, both strict all-target Clippy profiles,fmt,all-bin build,ordinary helper smoke and239 Python with zero skips. The earlier235 Python/one skip and3adc585 focused21-pass/one saved-baseline fixture failure remain separate. The final test fix initializes saved identity; runtime is unchanged.
- Integration independently verifies source equivalence and reruns Python plus real package document/vendor/QA closure; no concurrent Cargo build is used. Historical source/evidence appendices retain old identity and all negatives. New Windows CI and final source ZIP are tracked independently after publication. No new rendered/native/physical/genuine-plugin or performance acceptance, main merge or binary release is claimed.

## 2026-10-10 Native volume/pan automation workflow

Added Channel Rack and Mixer volume/pan context-menu create/open, stable canonical targets, safe free-track placement, flat current-value envelopes and atomic creation history. Repeated actions reveal existing placements without editing them. Ambiguous/global-unplaced lanes and plug-in-backed Channel targets fail visibly; existing plug-in AUTOMATE is unchanged. See [workflow and limitations](NATIVE_CONTROL_AUTOMATION.md).

Rust/app checkpoint `097ff1f`: 1,200 all-feature app, 1,183 core, 25 helper and 13 protocol tests; both strict Clippy profiles, fmt, all-bin build and helper smoke pass. Preceding `35620ff` had two stale accessibility-label fixtures; failures remain in external evidence and the full corrected suite passed. Packaging-only follow-up adds the guide allowlist; 553 actual package inputs and 239 Python tests pass. No new native/audio-device, rendered capture, real-plugin or Windows acceptance, publication or release claim.
