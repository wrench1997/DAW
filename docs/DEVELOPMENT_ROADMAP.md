# Development roadmap

Updated: 2026-10-09 03:09 UTC. Development continues beyond the passing CI milestone. The near-term product outcome is: **make a song, edit it without unintended changes, save it, recover its media, export the supported mix, and reopen it without losing work**. This document tracks concrete acceptance rather than promised dates or an undifferentiated feature list.

## Baseline and evidence rules

- Development branch: `ci/windows-reliability-20261009`. Last fully validated executable/CI baseline: `acdcf236d49cd3e5cd4d09506856c935b79eb12a`. Project media recovery is newly integrated at `4174649` on top of documentation checkpoint `598cd48`; fresh full Windows validation is pending.
- [Windows MSVC development CI](https://github.com/wrench1997/DAW/actions/runs/37873807018): fmt, **775 Rust tests**, Clippy with denied warnings, app/helper debug build, **14 Python harness tests**, actual plugin-free helper protocol smoke and no-default-features check **passed**. Earlier failure/correction history is retained in [WORK_LOG.md](WORK_LOG.md).
- Project media recovery has 15 passing focused production-module regressions and passing subset Clippy, and is now integrated pending full Windows and manual GUI validation. Split preservation and the preview-packaging lane remain separate/unintegrated. None can inherit the older 775-test result.
- Evidence levels stay distinct: **implemented/source-inspected**; **executed code tests** at an exact revision; **GUI/device scenario passed** with artifacts; **release candidate accepted** on the intended package; **commercial maturity** from broader workflow, compatibility and sustained-use coverage. None implies the next.
- Local Linux compilation was blocked at the missing ALSA native dependency before project compilation. Windows passing results do not certify Linux.

## P0 / M1 — Edit a song without changing its meaning

**Existing:** Pattern/Piano Roll editing, audio import, grouped Playlist gestures, Slip, equal-power fades and two-Clip Crossfade are connected. Arithmetic hardening and dependent-state undo/redo regressions passed in the baseline suite (`playlist.rs`, `app.rs`).

**Verified gap:** splitting an Audio Clip inside an existing fade copies normalized fades to both pieces instead of retaining one continuous original envelope. Splitting also rounds the native-source offset to an integer, which can alter resampled PCM at a non-native-rate boundary. Both remain defects in the integrated baseline.

**Active slice:** preserve Audio Clip split-envelope origin/range and source phase across model persistence, migration, realtime Timeline/audio, offline export and visual editing. Pattern splitting is outside this implementation slice. Status: **in progress in a separate worktree; not integrated or test-accepted**.

**Acceptance:**
1. Compare an unsplit reference against Audio splits inside fade-in, fade-out and overlapping fades; the combined output must retain the original envelope and source phase rather than restart, renormalize or shift resampled PCM. Include source/output-rate differences and tempo changes.
2. Repeat the split, move the right piece to beat zero, resize to reveal content and save/reopen the project; identities, routes, offsets and defined retained-envelope behavior survive. Test legacy project migration explicitly if the schema changes.
3. Undo and redo restore Clips, automation dependencies and audio routing as one edit. Rejected geometry or invalid numbers leave the project unchanged.
4. Run focused numeric/persistence/renderer regressions and the complete locked gates. Then exercise real-media slice/fade/Slip/Crossfade with the GUI at multiple tempos. Source/unit evidence alone does not close the GUI/audio check.

**Exit:** integrated implementation and exact passing regression evidence, followed by the recorded GUI/media acceptance. Fixes can ship as a development checkpoint before the broader milestone is accepted, with that boundary explicit.

## P0 / M2 — Save, recover media and reopen the same song

**Existing:** blank projects, Save/Save As, guarded New/Open/Quit transitions, tagged plug-in-state save barriers, synchronized atomic project replacement, autosave and a restore/discard dialog exist in `app.rs` and `model.rs`. Save finite-number preflight preserves old files on invalid data; its regressions passed in the baseline suite. Autosave is not a new feature planned here.

**Baseline gap addressed in source:** failed/missing WAV loading previously had transient notifications and runtime diagnostics without a persistent project-media inventory or relink action.

**Integrated slice:** a [Project media view](PROJECT_MEDIA.md) with persistent paths/load failures, background validation, explicit Apply/Cancel and undoable relinking by stable asset ID. Replacement must match sample rate, channels and frame count to preserve existing native-frame offsets; stale session/path/generation results are rejected. Status: **integrated at `4174649`; 15 focused production-module regressions and subset Clippy passed; 2 App regressions/full Windows gates/manual GUI validation pending**. No schema change.

**Acceptance:**
1. Create a song with imported WAVs and an offset/faded Clip, save, close and reopen; verify arrangement, routing and asset metadata.
2. Move or make a source WAV unreadable, reopen, and find the unresolved path and cause in a persistent view. No silent substitution or claim of successful media recovery.
3. Choose a compatible replacement and Apply; all Clips sharing the asset keep identity/geometry/routing and become playable. One undo/redo restores the old/new reference. Save/reopen retains the chosen path.
4. Cancel, invalid WAV, metadata mismatch and results arriving after project changes make no project mutation. Repeat selection and dismiss/reopen the dialog to test interruption safety.
5. Separately test unsaved-edit recovery: produce a recovery point, restart, Restore, explicitly save, and reopen. Corrupt recovery or failed write must not destroy the named project. Preserve unsaved status when plug-in state is stale.

**Exit:** integrated media workflow with regression evidence and a recorded create/save/missing-media/relink/reopen GUI scenario. Crash/power-loss robustness and portable self-contained project packaging require separate evidence; path relinking alone does not establish them.

## P1 / M3 — Export a usable deliverable from that song

**Existing:** background stereo PCM24 WAV arrangement export for Pattern and WAV Audio Clips through the plugin-free MainInput DAG; staged atomic replacement; explicit refusal when enabled plug-ins or active sidechains would be omitted (`export.rs`). Realtime Master Capture records the rendered Master through a bounded queue and background writer (`master_capture.rs`, `audio.rs`, `app.rs`). The final-frame drain fix and invalid export-rate/audio regressions passed in the baseline suite.

**Verified export gaps:** non-Tempo automation is not rendered by the static offline mixer/event path and is not covered by the existing plug-in/sidechain refusal. The background export has start/result notifications but no running-job progress/cancel controls. The current exporter also applies whole-song gain reduction when the peak exceeds 0.95; this must be included in output expectations rather than assuming an unchanged Master level. Tempo-map support does not establish support for all automation families.

**Next acceptance work:** follow M1/M2 with a reproducible song fixture, explicit unsupported-automation handling and export/reopen evidence. No new full-mix offline plug-in renderer is claimed in this checkpoint.

**Acceptance:**
1. Export the supported arrangement at documented rates; inspect WAV header, frame count/duration, channel count, nonempty audible content and source offsets/fades. Reimport/play the result.
2. Compare known reference sections before/after splits and media relinking. Record numeric tolerances and the render scope rather than claiming arbitrary realtime/offline equivalence.
3. Test invalid rate/audio, missing source and write failure; errors must be visible, previous destinations preserved where atomic replacement promises apply, and partial output not reported as complete. Closing the file picker must not start a job. Add and test running-job cancellation before claiming that capability.
4. For Master Capture, verify start/stop receipts, final frames, no-clobber publication and gap/overflow diagnostics. Exercise live plug-in audio on real hardware and retain the output/evidence.
5. Offline plug-in/sidechain refusal stays explicit; unsupported automation must either render with verified semantics or fail visibly before output publication. Unified plug-in-aware rendering, automated parameters, tails, PDC equivalence, stems and freeze/bounce are later renderer work, each needing its own fixtures.

**Exit:** the supported export and live-capture workflows are reproducible with playable outputs and clear failure handling. This does not claim complete commercial bounce facilities.

## P1 / M4 — Install and complete the workflow on Windows

**Status:** release-candidate acceptance **not run**. Windows MSVC debug CI/helper smoke passed; it is not a pinned gnullvm Release or a clean-machine launch.

**Parallel development lane:** an opt-in Windows preview-package workflow is in progress: Windows 2025, Rust 1.99.0 MSVC/static CRT, optimized all-feature app/helper, PE/import checks, ZIP whitelist, checksums/provenance and extracted-helper smoke. It has not produced a verified Windows artifact yet. This preview lane does not replace or silently change the pinned gnullvm release contract.

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
