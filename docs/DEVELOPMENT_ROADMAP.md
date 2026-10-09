# Development roadmap

Updated: 2026-10-09 17:32 UTC. Current priorities are detailed Piano composition and verified real-plugin usability/routing, while the full song outcome remains: **make a song, edit it without unintended changes, save it, recover its media, export the supported mix, and reopen it without losing work**. This document tracks concrete acceptance rather than promised dates or an undifferentiated feature list.

## Baseline and evidence rules

- Development branch: `ci/windows-reliability-20261009`. Last fully validated Windows checkpoint: **`53494d518bfc4bad7304f25c127491a0c67d7cd8`**, including Browser, measured meters, v11 split fidelity, WAV/media/export controls and responsive UI QA. [Quality 37886481852](https://github.com/wrench1997/DAW/actions/runs/37886481852) passed **909 Rust tests**, 14 helper-harness tests and actual helper smoke. [Preview 37886481857](https://github.com/wrench1997/DAW/actions/runs/37886481857) passed optimized static-CRT builds, 57 Python tests, PE/import audit, ZIP/hash checks and extracted-helper smoke; no artifact upload.
- Application **0.5.0-alpha.1** uses Project **v12**. v10/v11 inputs load with ports Off and source audio monitoring enabled; new saves need a v12 build. Preserve original projects before evaluation.
- Historical ownership-preparation source **`3fb549a059916dca7b3af3eff00ff27fba0bddd4`** passes 1,137 app +21 helper +5 editor protocol +2 transport protocol Linux all-feature tests, 1,133 core tests, both strict Clippy modes, fmt/build/helper smoke, both MSVC source profiles and 195 Python tests. The available vendor suite passes 270 cases serially with one explicit missing upstream fixture exclusion; all 26 doctests pass, including five new facade cases. App/UI/audio are unchanged. Its exact published checkpoint 5befb51 completed its own Windows runs; no worker or latency improvement is claimed.
- At **`20a9dfc`**, Windows source gates pass 1,165 app +15 helper +5 editor protocol +2 transport protocol tests. [Preview 37955606487](https://github.com/wrench1997/DAW/actions/runs/37955606487) passes the optimized/provenance/package/extracted-helper lane, with 239 Python cases (235 passed, four Unix-only skips) and upload disabled. [Quality 37955606457](https://github.com/wrench1997/DAW/actions/runs/37955606457) passes independent native state/interaction/lifecycle but still fails before/after paint; repaint is SKIP. Overall quality is failed.
- The integrated [simultaneous workspace](MULTIWINDOW_WORKSPACE.md) is a real shared-project, internal-window implementation. The latest requested [compact refinement](COMPACT_WORKSPACE.md) keeps useful task density, persistent migration, continuous held gestures and bounded release snapping; controlled unoptimized CPU comparisons do not establish a general speedup or displayed FPS. Focused Piano session-local note clipboard now has bounded data, semantic input/text isolation and explicit Undo. Offscreen frames validate app paint/layout rather than cloud X11 presentation, OS-detached editors, device audio or native VST3 editor paint.
- Evidence levels stay distinct: **implemented/source-inspected**; **executed code tests** at an exact revision; **GUI/device scenario passed** with artifacts; **release candidate accepted** on the intended package; **commercial maturity** from broader workflow, compatibility and sustained-use coverage. None implies the next.

- Follow-on guarded state source **`60134a5`** is locally integrated from reviewed `c056dc5`. It rejects used/editor-opened exact Surge XT 1.3.4 before helper detachment or mutation, with fresh-instance/native rejection evidence. This guard later passed combined gates and exact Windows source/preview at244f622. The [state report](PLUGIN_STATE_RESTORE_LIMITS.md) retains previous failures, the legacy Admin caller limitation and absence of silent settlement. It now combines with reviewed timing source at **4fdfbc2**, whose fresh aggregate gates now pass1,170 app +22 helper +5 editor protocol +2 transport protocol,1,166 core,207 Python,276 available vendor tests (one named missing-fixture exclusion),26 doctests, both strict Clippy modes and both MSVC source profiles. Combined default2048 four-case delivery and fresh-state checks passed with helper9dd18d74; its exact244f622 Windows results are recorded above.

## Current scoped-session and reset-origin integration

Reviewed sourcef086170 combines private borrowed control/processor sessions with
owner-only reset-origin processing. Independent semantic review confirms SDK-only
RAII in both normal and reset paths, checked FIFO origin staging, output discard,
actual native acknowledgment and reset exclusion from both facades. A combined COM
test covers success, SDK error and pre-SDK staging failure; compiler contracts add
six reset exclusions to the original47 cases. Full merged gates pass 1,179 app +25 helper +13 protocol, 1,167 core,395 available
vendor cases,26 doctests,53 private contracts and239 Python cases. New source-bound
native graph/UI/state checks also pass; exact new Windows CI remains separate.

The correction targets host-induced one-frame Surge FX mode changes, preserving
strict metadata and faults. Standalone small blocks remain valid; only the DAW
requires prepared maximum128..=2048 normally. Exact loaded Surge XT instrument1.3.4
selects one reset256 and requires maximum256 or higher; no safety CC is omitted.
Corrected-source immediate notes at offsets0/1/127 sustain in the production path.
Fresh graph playback/replay
slice evidence does not establish universal tail clearing: immediate held NoteOn
after stop/start is transient in both old and new reset traces. Keep that negative,
old performance matrices and historical resize stall. See [reset contract](PLUGIN_RESET_ORIGIN.md).
No worker/thread migration or helper latency improvement follows from the private
session factoring.

## Historical bounded native-edit preparation

Reviewed48d3f97 integrates as **87ceb06** with App/timing/helper wire unchanged.
The fixed-capacity native edit channel separates display polling from DSP input,
requires complete admission and successful SDK Process acknowledgment, and refuses
state capture on sticky loss, exhaustion or an unstable dirty revision. Successful
state application supersedes earlier native packets without pretending they were
applied. Rejected Surge eligibility remains nonmutating. Current-source gates pass1,170 app +22 helper +5+2 protocol,1,166 core,215 Python after receipt packaging,
304 available vendor cases (one named fixture exclusion),26 doctests, both strict
root Clippy modes and both MSVC source profiles. The bounded new-helper default2048/fresh-state check passes; new exact Windows CI
completed at e487a50 with source/preview pass and retained paint failure.

Actual stopped Surge edit,20 polling rounds, zero-sample SaveState, fresh numeric/
component/first-note behavior and unchanged same-window rejection passed on the
reviewed helper; Stochas and native fixture also passed. The350ms blocked-producer
fixture is transport independence only. GUI/DSP remain single-threaded; COM/event
allocation, native resize346.9ms stall, hardware and broader realtime safety remain
open. Prior optimized12/16 quiet and12/16 loaded results belong to4fdf/244 and its
older helper; they cannot qualify this changed helper.

## Historical bounded parameter-container preparation

Reviewedbf573a4 integrates asfb7b91a. Stable prepared COM queues, finite shared
point storage and checked input admission replace per-operation queue allocation;
output loss remains latched across administrative lifecycle operations and blocks
Process/SaveState until a fresh instance. Native channel/Surge guard semantics and
App/timing/wire are unchanged. Source/native/comparative review is clear; combined aggregate gates pass1170 app,
22 helper +5+2 protocol,1166 core,339 available vendor cases/one explicit fixture
exclusion,26 doctests and231 Python, plus fmt/strict root Clippy/build/cross checks.
New-helper default2048 four cases plus fresh state pass (both exits0), retaining
13/14 changing debug core overruns. Exactb92c394 Windows source/preview passed;
known native paint remained failed.

Constructor requested bytes total1,573,104, excluding outer wrappers and allocator
metadata. Tested cold/reused container operations allocate/free zero while the host
owner exists. Final retained-after-owner Release may free on the plugin's thread.
Sparse populated-queue indexing fixes measured draft regressions; large populated
suffix movement and some read/small cases remain slower, with aggregate quadratic
worst cases. Whole-Process no-allocation, mutex/event/metering removal, independent
GUI/DSP execution and the346.9ms resize stall remain open. See [exact scope and
comparative costs](PLUGIN_PROCESSOR_DOMAINS.md#prepared-bounded-parameter-com-storage).

## Current checked-event correctness preparation

Reviewed7d8a416 integrates as7d7294b with41 exact source hashes and no App/helper
wire/timing/reset changes. Event capacity/metadata checks precede payload copy;
note counts/IDs commit after successful enqueue. Rejected releases retain their
obligations, and panic commits only its accepted prefix (at most4096 per call).
Invalid raw pointer memory remains the foreign caller's responsibility.

Source-owner native/headless/default2048 checks pass. New full combined gates pass
1170app+22helper+5+2protocol,1166core,362availablevendor/one explicitfixture exclusion,
26doctests,239Python,fmt/two strictrootClippy modes/build/helper smoke/bothMSVCprofiles.
Fresh helper59b6bcbd matches the independently tested source; exact Windows CI remains pending. No payload no-allocation, worker,
reset-policy correction, low-latency or hardware qualification is implied. Existing
legacy ignored-result callers and read/clear/reset diagnostics remain documented.

## Current product priority — detailed Piano melody composition

The reviewed [keyboard slice](PIANO_KEYBOARD_EDITING.md) and [mouse slice](PIANO_MOUSE_WORKFLOW.md)
are now integrated: context-scoped deselect/repeat, time/pitch edits, quick quantize,
length reset and ghost toggle; pointer-down Draw, temporary selection, phrase clone,
modifier-order axis locks and remembered/dragged length. These share one project,
explicit history boundaries, bounded note identity/data and native/import barriers.
Production Undo/Redo now waits for held/interrupted note gestures, preventing an
older history entry from being replaced underneath an active preview.

Citrus-specific selected-extent repeat, independent local snap, per-note scale lock,
Alt Stamp and generic-Shift policies are documented rather than claimed as exact FL
parity. Actual app input/render regressions execute both slices together. Native OS
focus/clipboard and physical-device behavior remain separate acceptance work.
[Velocity-wheel/fine adjustment and supported-field note properties](PIANO_NOTE_EXPRESSION.md)
are now integrated: relative dynamics with common bounds, single/multiple target drafts,
Reset/Cancel and one-step Apply, preserving imported timing and newer unrelated data.
Wheel residue, stale/frozen targets, numeric ownership and modal/import barriers have
actual input regressions. [Independent Piano time-range/snapping](PIANO_RANGES_AND_SNAP.md) is now integrated:
session-owned edit/repeat intervals, exact range-width copying, left-visible-bar
paste, safe Off/fine/triplet grids and compact narrow-window controls. Ranges do
not alter playback loops, transport, Project serialization or Playlist snap.
MIDI interchange, playback-range looping and broader commercial workflow remain open.

## Current plugin routing slice

[Numbered MIDI ports](PLUGIN_MIDI_ROUTING.md) now connect one producer to one or more
exclusive-input instruments during constant-tempo playback, with stopped-only route
edits, endpoint/epoch attestation, bounded event quotas and latched failure cleanup.
The [separate source-pinned real-plugin receipt](PLUGIN_MIDI_ROUTE_VALIDATION.md)
passed five historical e54a6e4 graph tests with Stochas/Surge/Effects, including
latency reactivation and overload/release safety. That historical plan used
4,352 bridge frames /90.667 ms for source plus synth at48kHz; no seamless
loop, stopped live chain or Harmony Blueprint qualification. The retained concurrent
source-loss negative is unresolved, the old Off path has misses and a measured debug
callback exceeds its period. The new [common timing plan](PLUGIN_TIMING.md) uses
explicit callback ceiling B: default2048 adds2432 frames per physical worker at48kHz,
or4864 frames /101.3ms for source plus synth, before reported/downstream latency.
Settings marks128/256/512 Experimental. Prior exact timing-only debug runs passed
13/16 quiet and10/16 under controlled four-thread load; all four2048 combinations
passed both with real stopped Retry and coherent PDC/events, while smaller-profile
deadlines and longer cleanup/retry negatives remain. Callback admission, visible
faults and stopped configuration/state transactions improve correctness, not helper
GUI/DSP isolation or established low-latency/device reliability.

## Current Linux editor and metronome slice

[Standalone Linux VST3 editors](LINUX_VST3_EDITORS.md) use the helper's X11 container,
main-thread factory/frame run loop and existing state/snapshot guards. Actual fixture,
Surge and Stochas paint/input/state restoration passed on Xfce/X11. [Native evidence](LINUX_VST3_EDITOR_VALIDATION.md)
retains a 346.9 ms resize processing-request stall: functional native UI is established,
realtime continuity is not. Mixed Wayland/XWayland, hardware and fractional DPI remain open.
The separate [metronome control](METRONOME.md) defaults Off, persists without dirty/history
changes, clears the active click source with a callback atomic and retains existing
buffer/FX drain semantics. It does not fix general plugin scheduling or native GUI stalls.

## Current synchronous ownership preparation

[Control/processor ownership](PLUGIN_PROCESSOR_DOMAINS.md) is separated without
starting a DSP worker. A main-thread-only facade and exclusive borrowed processor
lease clarify lifetimes; state/topology rebuild and loader unwind paths are checked.
The ownership-preparation helper matched the separately tested native binary. Real
fixture and fresh-instance vendor state passed, while historical [reused Surge
restore](PLUGIN_STATE_RESTORE_LIMITS.md) lost the first note and left controller
values stale on both old/new helpers.
Later component volume is correct; no permanent-volume-loss claim is warranted.
Content/container sizing, native resize stalls and general state consistency remain
open. The follow-on exact-class/version guard refuses used/editor-opened loads
before helper mutation or detachment. It does not make reused restoration succeed;
the newly integrated timing configuration path adds tagged live-state capture and
prevalidated replacement retention. That exact combined path has passed source and
bounded state/delivery gates, most recently published atb92c394; hardware/paint and
helper GUI/DSP concurrent processing remain separate work.

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

The [local WAV Browser](LOCAL_SAMPLE_BROWSER.md) is integrated from reviewed source `3e4335e` as `abc9ec8`: explicit folder selection, bounded single-level listing, current-list search, Up/Refresh/Cancel and shared File-menu/Browser Playlist import. One worker and a latest-request mailbox prevent scan pileups; imported paths must fit the existing UTF-8 Project persistence contract. Asynchronous import completion waits for snapshot-owning edit/lifecycle barriers and commits an independent Undo transaction. Placeholder samples/waveform are removed; audition is explicitly unavailable. The exact integrated source `abc9ec8` also passed all **901 native no-default/all-target tests**, fmt, strict Clippy and the app build on the default test stack. The browser checkpoint also passed all Windows gates at 0ff5e6e with 900 tests. Native picker, rapid GUI navigation, import/undo/save/reopen and actual playback still require a functioning graphical/audio environment.

## P1 — Exercise the actual app UI and fix visible layout defects

[Display-independent UI QA](HEADLESS_UI_QA.md) uses production app drawing and actual egui pointer/key input. Eight focused flows cover navigation, six Settings pages, export review/back/cancel, media dialogs, browser filter/navigation/import errors and success, Undo/Redo, and dirty-project Cancel. Optional genuine Vulkan offscreen rendering produced 17 checkpoints; these are explicitly not desktop screenshots. Observed narrow-window Plugins/Mixer and Group/Snap overlaps are fixed with bounded/wrapped toolbar groups, and About now reports the build platform and observed host or offline status. Native desktop presentation remains blocked by the observed cloud Mesa/X11 failure; native dialogs, real projects/save/reopen and physical hardware require separate acceptance. Exact integrated source gates are recorded in DEV_STATE/WORK_LOG.

## P1 / M4 — Install and complete the workflow on Windows

**Status:** release-candidate acceptance **not run**. Windows MSVC debug CI/helper smoke passed; it is not a pinned gnullvm Release or a clean-machine launch.

**Integrated validation lane:** the [Windows preview-package workflow](WINDOWS_PREVIEW.md) first passed end to end at `53494d5`: Windows 2025, Rust 1.99.0 MSVC/static CRT, optimized all-feature app/helper, exact PE/import checks, ZIP whitelist, checksums/provenance and actual extracted-helper smoke. The narrow reviewed OS import allowlist repairs are validated. Upload remained disabled; this does not establish clean-machine/GUI/hardware acceptance or change the pinned gnullvm release contract. The later `8961529` preview also passed its exact source-vendored native-editor provenance checks; native paint acceptance remains separately failed.

**Acceptance:**
1. Build the exact candidate with the pinned toolchain and locked inputs in [BUILD_AND_RELEASE.md](BUILD_AND_RELEASE.md); run all required tests/lints and the helper smoke against that Release helper.
2. Verify package contents, required runtime/helper files, hashes, licenses and documentation links; launch on clean Windows 10/11 x64.
3. Complete M1–M3 as one song workflow, with save/reopen at the end. Record screenshots, device/driver and plug-in versions, output files and failures for the candidate.
4. Interrupt transport, plug-in and device operations; test unplug/replug, rollback, shutdown/finalization and sustained use. A plugin-free helper smoke does not test vendor plug-in compatibility or physical audio.
5. Record remaining issues and their user impact. Do not label a package commercially ready because it compiles, passes unit tests or completes a single demo.

## P1 — Restricted Windows VST3 native-editor workflow

[Native editor controls](NATIVE_VST3_EDITORS.md), helper-owned lifecycle and exact dirty/state/base capture are integrated. Automated instances are deliberately excluded; generic parameter/automation workflows remain available. Shared project-snapshot barriers prevent native Open and queued import from crossing replacement snapshots. Windows source-built MIT fixture execution now validates attach/resize/events/close/reopen, exact stopped-state round-trip and protocol stdout isolation on the production helper. Windows native paint remains failed, so complete Windows native acceptance is still open. Fixtures are bounded test inputs, not broad vendor compatibility evidence. Linux standalone native paint/input/state is now functionally verified with the fixture and Surge/Stochas, with a material resize processing stall; mixed Wayland, broader physical input/DPI/audio/vendor coverage and native gesture automation remain open.

## Independent real-plugin usability and routing work

[Isolated metadata scanning](VST3_SCANNING.md) is now integrated. The production
helper reports the default audio class's real name/vendor/category and event-bus
capabilities. Old filename-only VST3 cache entries request a rescan; timeout/load
failures remain explicitly unverified and actionable. Official Surge XT Effects
is correctly classified as Fx. No additional class selector or project migration
is implied.

[Source-only real-plugin receipts](REAL_VST3_VALIDATION.md) preserve controlled
offline QA at `b57076a`: genuine Surge instrument, Effects and ordered audio chain,
plus separately configured Stochas MIDI generation. The original wrong scanner
classification remains in its historical log; the corrected metadata rerun is
separate. These results do not establish real-time audio deadlines, native GUI,
Harmony Blueprint, Windows or onward VST3 MIDI/event routing. The larger routing
feature proceeds independently and is not included in this scanner checkpoint.

## After the first reliable song workflow

Prioritize subsequent work from observed workflow failures and reproducible acceptance cases. The next product programs are dependable audio/MIDI recording (arming, latency, punch/loop/takes), plug-in usability and safety (editors, presets, isolation/compatibility), unified rendering, and large-session performance/recovery. Broader advanced editing, automation, routing, accessibility, localization and distribution work remains in the [capability matrix](FL_STUDIO_PARITY.md). These programs are not all active or complete; each needs a scoped milestone and evidence before implementation claims change.

## Documentation contract

Update [DEV_STATE.md](../DEV_STATE.md) and the README at material implementation/integration/test changes. Append aggregate dated [WORK_LOG.md](WORK_LOG.md) entries without rewriting historical outcomes. Record exact revision or worktree, command and result as **passed**, **failed**, **blocked before execution**, or **not run**. Never convert test attributes, planned cases, source inspection or another revision's results into fresh passing evidence.
