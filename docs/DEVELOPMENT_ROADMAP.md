# Development roadmap

Updated: 2026-10-09 04:37 UTC. Development continues beyond the passing CI milestone. The near-term product outcome is: **make a song, edit it without unintended changes, save it, recover its media, export the supported mix, and reopen it without losing work**. This document tracks concrete acceptance rather than promised dates or an undifferentiated feature list.

## Baseline and evidence rules

- Development branch: `ci/windows-reliability-20261009`. Last fully validated Windows source: `0b9cfa1a5d7e8c3384bc3ac435bcedc834c005af`, including measured meters, v11 split fidelity, WAV options, media and export controls. [Run 37883317515](https://github.com/wrench1997/DAW/actions/runs/37883317515) passed all gates with **880 Rust tests**, **14 helper-harness tests** and actual helper smoke.
- Audio split fidelity is now integrated as `ba6a27f` + `21b716b`, with explicit application version **0.5.0-alpha.1** and project format **v11**. v10 inputs still load; new v11 saves require a new build. Preserve original projects before evaluating this prerelease.
- Merged source `bb5817118b16fde04689d21131fdf1510a177713` passes real Linux no-default all-target typecheck and **860/860 Rust tests** on default stack, including the packet-construction regression and combined media/split/export/options tests. All-feature Windows gates passed separately with862 tests. Preview's fixture and VS initialization repairs now pass on Windows. Run 37883317601 passed the ComBase.dll repair, all 880 Rust tests and optimized static-CRT build, then rejected the OS UIAutomationCore.dll import during packaging. Its narrowly documented allowlist repair and expanded diagnostics need fresh Windows execution; archive/helper-from-ZIP validation remains pending. Linux is the primary ongoing development/test path; platform/GUI/hardware acceptance remains distinct.
- New metering source `4045cb24` passes **878 Linux no-default/all-target tests**, formatting, strict Clippy and application build. This metering checkpoint also passes the Windows all-feature gates at 0b9cfa1. The app creates an X11 window, but Mesa EGL_BAD_SURFACE at X11 shared-memory attachment blocks presentation. No GUI/device acceptance is inferred.
- Evidence levels stay distinct: **implemented/source-inspected**; **executed code tests** at an exact revision; **GUI/device scenario passed** with artifacts; **release candidate accepted** on the intended package; **commercial maturity** from broader workflow, compatibility and sustained-use coverage. None implies the next.
- Local Linux typechecking now uses the verified already-installed ALSA runtime through truthful linker metadata. It does not certify Windows, all-feature VST hosting, GUI or physical hardware.

## P0 / M1 — Edit a song without changing its meaning

**Existing:** Pattern/Piano Roll editing, audio import, grouped Playlist gestures, Slip, equal-power fades and two-Clip Crossfade are connected. Arithmetic hardening and dependent-state undo/redo regressions passed in the baseline suite (`playlist.rs`, `app.rs`).

**Verified gap:** splitting an Audio Clip inside an existing fade copies normalized fades to both pieces instead of retaining one continuous original envelope. Splitting also rounds the native-source offset to an integer, which can alter resampled PCM at a non-native-rate boundary. These defects are addressed by the integrated source and have passed the complete Linux and Windows candidate code gates. Real-media GUI/device acceptance is still required.

**Active slice:** preserve Audio Clip split-envelope origin/range and source phase across model persistence, migration, realtime Timeline/audio, offline export and visual editing. Pattern splitting is outside this implementation slice. Status: **integrated in the v11 prerelease candidate; 200 focused source tests and seven real callback/App tests passed before integration, merged native typecheck and complete860-test suite passed; Windows gates passed; GUI acceptance pending**.

**Acceptance:**
1. Compare an unsplit reference against Audio splits inside fade-in, fade-out and overlapping fades; the combined output must retain the original envelope and source phase rather than restart, renormalize or shift resampled PCM. Include source/output-rate differences and tempo changes.
2. Repeat the split, move the right piece to beat zero, resize to reveal content and save/reopen the project; identities, routes, offsets and defined retained-envelope behavior survive. Test legacy project migration explicitly if the schema changes.
3. Undo and redo restore Clips, automation dependencies and audio routing as one edit. Rejected geometry or invalid numbers leave the project unchanged.
4. Run focused numeric/persistence/renderer regressions and the complete locked gates. Then exercise real-media slice/fade/Slip/Crossfade with the GUI at multiple tempos. Source/unit evidence alone does not close the GUI/audio check.

**Exit:** integrated implementation and exact passing regression evidence, followed by the recorded GUI/media acceptance. Fixes can ship as a development checkpoint before the broader milestone is accepted, with that boundary explicit.

## P0 / M2 — Save, recover media and reopen the same song

**Existing:** blank projects, Save/Save As, guarded New/Open/Quit transitions, tagged plug-in-state save barriers, synchronized atomic project replacement, autosave and a restore/discard dialog exist in `app.rs` and `model.rs`. Save finite-number preflight preserves old files on invalid data; its regressions passed in the baseline suite. Autosave is not a new feature planned here.

**Baseline gap addressed in source:** failed/missing WAV loading previously had transient notifications and runtime diagnostics without a persistent project-media inventory or relink action.

**Integrated slice:** a [Project media view](PROJECT_MEDIA.md) with persistent paths/load failures, background validation, explicit Apply/Cancel and undoable relinking by stable asset ID. Replacement must match sample rate, channels and frame count to preserve existing native-frame offsets; stale session/path/generation results are rejected. Status: **integrated and all Windows gates passed at `36d2577` with 792 Rust tests (including 15 media and 2 App additions); manual GUI validation pending**. No schema change.

**Acceptance:**
1. Create a song with imported WAVs and an offset/faded Clip, save, close and reopen; verify arrangement, routing and asset metadata.
2. Move or make a source WAV unreadable, reopen, and find the unresolved path and cause in a persistent view. No silent substitution or claim of successful media recovery.
3. Choose a compatible replacement and Apply; all Clips sharing the asset keep identity/geometry/routing and become playable. One undo/redo restores the old/new reference. Save/reopen retains the chosen path.
4. Cancel, invalid WAV, metadata mismatch and results arriving after project changes make no project mutation. Repeat selection and dismiss/reopen the dialog to test interruption safety.
5. Separately test unsaved-edit recovery: produce a recovery point, restart, Restore, explicitly save, and reopen. Corrupt recovery or failed write must not destroy the named project. Preserve unsaved status when plug-in state is stale.

**Exit:** integrated media workflow with regression evidence and a recorded create/save/missing-media/relink/reopen GUI scenario. Crash/power-loss robustness and portable self-contained project packaging require separate evidence; path relinking alone does not establish them.

## P1 / M3 — Export a usable deliverable from that song

**Existing:** background stereo PCM24 WAV arrangement export for Pattern and WAV Audio Clips through the plugin-free MainInput DAG; staged atomic replacement; explicit refusal when enabled plug-ins or active sidechains would be omitted (`export.rs`). Realtime Master Capture records the rendered Master through a bounded queue and background writer (`master_capture.rs`, `audio.rs`, `app.rs`). The final-frame drain fix and invalid export-rate/audio regressions passed in the baseline suite.

**New integrated slice:** [export progress/cancellation and fidelity preflight](OFFLINE_EXPORT_WORKFLOW.md) now refuses active unsupported non-Tempo automation, surfaces persistent errors, limits work to one background job, rejects stale-session results and atomically arbitrates Cancel versus final file commit. The progress/cancellation slice passed all Windows gates at `43e7e41`. Newly integrated [WAV export options](WAV_EXPORT_OPTIONS.md) add PCM16/PCM24/float32, file sample rate and explicit legacy peak attenuation versus preserve-level review. Defaults preserve legacy PCM24/0.95 behavior; the combined options/v11 candidate passed full Windows verification at `a760313d`. Non-Tempo automation rendering remains unsupported.

**Next acceptance work:** the integrated cancellation/preflight/options source has passed complete Windows and Linux code gates. Exercise its real GUI flow and follow M1/M2 with a reproducible song fixture and export/reopen evidence. No new full-mix offline plug-in renderer is claimed in this checkpoint.

**Acceptance:**
1. Export the supported arrangement at documented rates; inspect WAV header, frame count/duration, channel count, nonempty audible content and source offsets/fades. Reimport/play the result.
2. Compare known reference sections before/after splits and media relinking. Record numeric tolerances and the render scope rather than claiming arbitrary realtime/offline equivalence.
3. Test invalid rate/audio, missing source and write failure; errors must be visible, previous destinations preserved where atomic replacement promises apply, and partial output not reported as complete. Closing the file picker must not start a job. Test the new running-job cancellation in real Windows UI, including the noninterruptible operation and final-commit boundaries.
4. For Master Capture, verify start/stop receipts, final frames, no-clobber publication and gap/overflow diagnostics. Exercise live plug-in audio on real hardware and retain the output/evidence.
5. Offline plug-in/sidechain refusal stays explicit; unsupported automation must either render with verified semantics or fail visibly before output publication. Unified plug-in-aware rendering, automated parameters, tails, PDC equivalence, stems and freeze/bounce are later renderer work, each needing its own fixtures.

**Exit:** the supported export and live-capture workflows are reproducible with playable outputs and clear failure handling. This does not claim complete commercial bounce facilities.

## P1 — Truthful mixer feedback

[Measured mixer meters](MIXER_METERING.md) replace synthetic animation with actual post-fader stereo peaks, a pre-protection Master tap, dBFS labels, one-second hold and resettable CLIP/fault status. The callback uses bounded snapshots; graph/epoch/stable track identity, queue saturation, stale measurement and paused live-MIDI cases are covered. Meter activity does not alter Project or history. Integration commits `21989d0` and `ad565d7` retain the reviewed DSP/PCM behavior. Native GUI/device acceptance is pending; RMS/LUFS/true-peak analysis is outside this slice.

## P1 — Import real samples from a local folder

The [local WAV Browser](LOCAL_SAMPLE_BROWSER.md) is integrated from reviewed source `3e4335e` as `abc9ec8`: explicit folder selection, bounded single-level listing, current-list search, Up/Refresh/Cancel and shared File-menu/Browser Playlist import. One worker and a latest-request mailbox prevent scan pileups; imported paths must fit the existing UTF-8 Project persistence contract. Asynchronous import completion waits for snapshot-owning edit/lifecycle barriers and commits an independent Undo transaction. Placeholder samples/waveform are removed; audition is explicitly unavailable. The exact integrated source `abc9ec8` also passed all **901 native no-default/all-target tests**, fmt, strict Clippy and the app build on the default test stack. Fresh combined Windows gates remain pending. Native picker, rapid GUI navigation, import/undo/save/reopen and actual playback still require a functioning graphical/audio environment.

## P1 / M4 — Install and complete the workflow on Windows

**Status:** release-candidate acceptance **not run**. Windows MSVC debug CI/helper smoke passed; it is not a pinned gnullvm Release or a clean-machine launch.

**Integrated validation lane:** an opt-in [Windows preview-package workflow](WINDOWS_PREVIEW.md) has executed through optimized build but is still resolving package-validation findings: Windows 2025, Rust 1.99.0 MSVC/static CRT, optimized all-feature app/helper, PE/import checks, ZIP whitelist, checksums/provenance and extracted-helper smoke. The latest run rejected the known OS UIAutomationCore.dll import after the ComBase.dll repair passed; a narrow allowlist/diagnostic repair is ready. It has not produced a verified Windows archive yet. This preview lane does not replace or silently change the pinned gnullvm release contract.

**Acceptance:**
1. Build the exact candidate with the pinned toolchain and locked inputs in [BUILD_AND_RELEASE.md](BUILD_AND_RELEASE.md); run all required tests/lints and the helper smoke against that Release helper.
2. Verify package contents, required runtime/helper files, hashes, licenses and documentation links; launch on clean Windows 10/11 x64.
3. Complete M1–M3 as one song workflow, with save/reopen at the end. Record screenshots, device/driver and plug-in versions, output files and failures for the candidate.
4. Interrupt transport, plug-in and device operations; test unplug/replug, rollback, shutdown/finalization and sustained use. A plugin-free helper smoke does not test vendor plug-in compatibility or physical audio.
5. Record remaining issues and their user impact. Do not label a package commercially ready because it compiles, passes unit tests or completes a single demo.

## After the first reliable song workflow

Prioritize subsequent work from observed workflow failures and reproducible acceptance cases. The next product programs are dependable audio/MIDI recording (arming, latency, punch/loop/takes), plug-in usability and safety (editors, presets, isolation/compatibility), unified rendering, and large-session performance/recovery. Broader advanced editing, automation, routing, accessibility, localization and distribution work remains in the [capability matrix](FL_STUDIO_PARITY.md). These programs are not all active or complete; each needs a scoped milestone and evidence before implementation claims change.

## Documentation contract

Update [DEV_STATE.md](../DEV_STATE.md) and the README at material implementation/integration/test changes. Append aggregate dated [WORK_LOG.md](WORK_LOG.md) entries without rewriting historical outcomes. Record exact revision or worktree, command and result as **passed**, **failed**, **blocked before execution**, or **not run**. Never convert test attributes, planned cases, source inspection or another revision's results into fresh passing evidence.
