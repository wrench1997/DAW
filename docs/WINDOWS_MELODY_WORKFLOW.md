# Windows melody workflow — support and verification

Updated: 2026-10-15 (Asia/Shanghai). Development branch:
`ci/windows-reliability-20261009`; this effort started from `b0d8d5c0`.
Citrus Studio remains a development DAW. The complete melody workflow is not yet qualified.
This document separates implemented behavior, actual local acceptance, failures and untested work.

The current generic-base restore candidate is not yet compiled or tested.
Ordinary Open formerly sent saved bases once through a 64-entry untagged admin
queue; rejected enqueues were not retained, yet SlotReady could publish Loaded.
The candidate retains exact project/session/endpoint/slot/control ownership,
uses existing tagged parameter acknowledgments, and withholds readiness for
nonempty maps until replay completes. Backpressure retries the retained base;
rejection or the existing 30-second inactivity budget latches an explicit retry.
Transport, save and export gates cover the Installed-before-Ready interval.
Empty maps keep the old Ready/request-zero State contract. Eight real-worker/
bounded-queue tests and a Loaded-identity guard assertion are written; only
formatting and diff checks have run. Native05/06 use the previous frozen bins.

Latest backend qualification: Source87 (`769525d`) quality actually passed 32
factory and 2321 App + 52 helper + 13 protocol = 2386 root cases/12 ignored, including
both numeric-entry cases and three new failure-evidence regressions. Strict
all-feature/all-target Clippy, both bins, protocol/harness, fixture and
no-default-feature gates passed. Its only failure remains independent before/
after native paint PrintWindow capture. Both factory child paths emitted all
phase markers; the old Deadline did not reproduce, which does not establish its
cause or repair. Preview actually completed success: 344 harness cases, the same
2386 root passes/12 ignored, strict Clippy, no-default-feature checks, static-CRT
release, final ZIP verification and extracted sanitized-PATH helper smoke passed.
No build artifact was uploaded. Preserve both
`ci-769525d-windows-{quality,preview}-01.log` and Source87 actual terminal JSON.
The docs-only Source88 (`25567fb`) quality repeat also completed: 32 factory,
2386 root/12 ignored, strict Clippy and all backend gates passed; only independent
native paint PrintWindow failed. It triggered no preview. Its logs and Source88
actual terminal JSON do not qualify the new restore candidate.

Native05 subsequently completed an ordinary GUI cold-open and Whole offline WAV
export from the saved NOIRE Pure validation project. Independent PCM decoding
confirms 10.500s of stereo 48k/24-bit audio, nonzero signal in all eight onset
windows, no clipped samples and digital zero in the last 20ms. Clone, source
helper and App all exited 0. Native06 repeated that cold path without exact
timing and also completed, but its first note is exactly 384 frames/8ms later
over the first 400ms. Later audio is identical. Native04's preceding warm-path
cleanup failure and the new onset difference remain open. Details follow below.

## Current implementation and export boundaries

| Workflow | Implementation | Actual NOIRE status |
| --- | --- | --- |
| Real-time Master recording | Records the Master audio produced during live playback | Pure was heard and captured locally; not an offline export |
| Selected MIDI occurrence to audio | Experimental Linux/Windows strict clone, capture validation, native publication and history fences | Actual recapture mismatch refuses replacement; original MIDI is retained |
| Whole-song offline WAV | Built-in sources; experimental single-VST full arrangement/static routing/Master automation | Native05/06 ordinary GUI cold-open→10.5s PCM24 WAV completed with/without exact timing; first-note offset, warm-path cleanup, listening and full state/automation remain open; earlier short/405-note evidence is separate |

These paths must retain their own receipts and must not be substituted for each other.
The ordinary build explicitly refuses unsupported active-VST offline export.
Experimental VST whole export requires `experimental-fresh-helper` on Linux or Windows.
It currently admits one active VST source and no insert effect slots; additional sources,
unsupported automation and unsupported graphs are rejected rather than silently omitted.
Master volume/pan/mute automation is wired through the complete arrangement path.
A fixed tail window requires explicit review; unknown/infinite tail retains its warning.
Effects, arbitrary graph latency compensation and complete natural tail release are not qualified.

Windows conversion uses an actual overlapped named-pipe helper and native file identity/publication.
It preserves MIDI until verified success, handles cancellation/failure and retains restoration owners.
Removing a platform condition was not treated as a Windows implementation.
Whole export shares the stopped-source capture actor but does not create a conversion history item.
Project/session, Timeline, asset, parameter and save fences remain active during pending work.
Conversion recaptures the original source after rendering and requires exact
opaque/parameter equality before replacing MIDI and preparing Undo. The actual
NOIRE run08 failed that second-capture comparison (177 changed bytes in13 NI
component ranges). Whole export consumes one capture and does not replace the
source, so that conversion failure does not block the next ordinary Whole test.
Earlier Whole capture-to-clone byte equality records the LoadState input, not a
second serialization of either the original source or clone.

## Ordinary scan reliability in this commit

Source75 connects ordinary UI `start_scan` and synchronous `scan_catalog_checked`
to the retained factory-only request. They share the original walker, Probe,
one-attempt seal, five-second exchange window, cancellation, owned pipes and
cleanup/quarantine. No component LOAD or automatic loaded-default fallback is
issued by catalog discovery.

One module keeps one descriptor with the same ID/path, independent factory
declarations, `verified=false`, no loaded-default metadata and unknown runtime
capabilities. Catalog-only modules are visible in both target pickers; Browser
offers explicit Channel/Mixer selection instead of guessing Effect from unknown
classification. Project role remains Unknown and actual LOAD/save/reopen retains
the original None-class behavior. Valid catalog caches are additive; old VST2 and
actual metadata remain supported, while legacy verified-only entries still need
rescan.

Catalog publication requires successful natural child exit, concrete reap,
complete owned pipes and no cleanup error. A fully healthy-reaped single-module
metadata failure remains Unknown/unverified with its persistent scan_error and
does not block other modules. Cancelled, busy, pending, killed or unclean scans
stop before publishing a partial index and preserve the old index/rescan flag/
exact cache bytes. The persistent UI message survives the toast. A rescan advances
the same abandoned cleanup owner before another spawn; Pending and Busy cannot
authorize a replacement child.

Existing `scan_checked`/`scan` retain explicit loaded-default verification with
None class,48k/Q128 settings and strict metadata consumers. They are not a catalog
fallback. Strict clone gates/class selection/state/budgets remain unchanged.
Source75's cloud quality job passed32 factory regressions,2342 root tests with
12 ignored, strict Clippy, both bins, protocol/harness smoke, no-default-feature
and fixture builds. The workflow failed native paint capture; the new
production-catalog selector has not run against NI. Source74's lower-level
factory result remains separate evidence.

Source77 adds the live-load identity bridge needed by catalog-only Unknown
sources. After the actual default LOAD, full prepare and initial timing succeed,
the original Ready event carries actual class/version/category and active MIDI
identity tied to the current generator's session/channel/endpoint/instance/slot/
control. Ready/Installed order and restoration's already-consumed unique Ready
retain the original owners. Identity-query failure leaves ordinary Ready usable
with an explicit export-ineligibility reason; it does not make ordinary loading
fail or guess MIDI support.

Only Unknown sources acquire this new Whole Review anchor, requiring an actual
Instrument token and active MIDI input. Classified Instrument callers retain
their original export conditions even when optional identity is available.
Save return and Stop/Capture stages recheck the bound source; captured class or
version drift refuses the new path before starting the clone. Project role,
history, dirty state and catalog verified are not promoted. Existing stopped
capture, full state/parameter/bus checks, strict clone, cancellation and actual
Join remain mandatory. Source77's initial all-feature main test artifact actually
compiled and 17 new Windows cases passed: App5, Whole10, restoration1 and
eligibility1. Three backend cases originally used a Unix-only shebang fixture,
so their two Windows filters ran 0 tests. They have now moved into a Windows/Unix
module and shared protocol fixture without changing production helper launch.
Windows holds the exact Python process handle before Shutdown and requires its
actual exit0 after worker Join, separately from CMD. All three migrated cases
subsequently ran and passed in both cloud Windows jobs. Preview completed 2364
root passes with 12 ignored, then strict Clippy refused the 208-byte Ready
variant in `GeneratorReadiness`; release and ZIP checks did not run. Quality
first failed the fresh-source guard's `fresh Guard Drop lost its exact held
worker` assertion and finished with 2162 passed, 137 failed and 12 ignored,
including later sticky fixture refusals. The intentionally injected backend
shutdown panic remains part of the test, not a failure to suppress.
Evidence is `source77-loaded-identity-targeted-01.log`, its terminal JSON,
`ci-da68aaf-windows-preview-01.log`, `ci-da68aaf-windows-quality-01.log` and their
actual terminal run/jobs JSON. The repair candidate requires actual checks;
Source77 native preparation remains unexecuted. Catalog discovery and mock
identity tests do not qualify a real NOIRE workflow.

Source78 addresses the Clippy payload size with boxed Ready identity storage,
preserving the public getter type and current-owner checks. The retained-worker
timeout remains under investigation. Its test now reports mock Drop/panic-unwind
progress and the original worker's actual is_finished/Join/observer-call progress
before the unchanged five-second assertion. No additional poll, Join, owner or
deadline is introduced; production builds exclude the runtime observations.
Healthy and injected-panic successes check that the diagnostic terminal stage
matches the actual Join outcome. Local Windows actually passed 21 targeted
regressions, strict root all-feature/all-target Clippy and matching main/helper
builds. Cloud quality passed 32 factory and 2364 root tests with 12 ignored,
strict Clippy and every backend/build/protocol gate. Only independent native
paint before/after capture failed PrintWindow. Preview completed success: 344
packaging/protocol harness cases, 2364 root passes/12 ignored, strict Clippy,
no-default-feature check, static-CRT release, final ZIP verification and extracted
helper smoke. Upload was skipped. Preserve `ci-78d5986-windows-{quality,preview}-01.log`
and Source78 actual terminal run/jobs JSON. The original guard timeout did not
reproduce; its cause is still unproved, not fixed by the diagnostic.

The coordinated Source78 native session used matching all-feature binaries and a
new isolated data directory, without copying a user project. GUI New project and
ordinary Plugin Manager Scan completed with nine published entries. The cached
Kontakt catalog reports version8.4.1, expected Instrument declaration UID and the
Compatibility class while retaining Unknown/unverified/no loaded-default metadata.
This is actual production UI catalog evidence, not execution of the separate
ignored catalog QA selector or proof of component retirement. The effective
output displayed `扬声器 (2- CB4ND HW 1/2)`,48000Hz/480frames/XRUN0; no notes were played.

Selecting Channel01 and checking experimental dry-bounce source then clicking
Kontakt LOAD failed with `Rescan this VST3 to obtain its actual factory class
before fresh-source preparation`. Catalog discovery cannot supply that runtime
default class, so this old preflight blocks the new explicitly opted-in path.
The repair must obtain/validate the actual default class after the real None-class
LOAD and retain full fresh preparation and cleanup, not select a catalog class
or remove the class guard. This first session did not load NOIRE or qualify
save/cold-open/export. Normal
Alt+F4 ended the independent blank App; its actual PID disappeared with no direct
children. Evidence is in `normal-noire-workflow-20261015-01/catalog-after-real-ui-scan-01.json`
and `native-observation-and-close-01.json`.

The second session used ordinary None-class LOAD without the fresh checkbox.
Actual Kontakt opened at a usable full size; the registered library's Pure
preset showed `NOIRE PURE` and about315.4MB instrument memory. Color was moved
from noon to upper-left and the editor closed without removing the source.
Eight Piano Roll notes span four bars and one16-beat Playlist Pattern. SONG
playback crossed it and Escape returned to zero. Listening confirmation and
digital PCM are still pending; a moving transport is not sound proof.

Ordinary Save showed SAVED without a stale warning. The file contains all eight
notes, one clip, Unknown role,591680 opaque-state bytes and Channel→FX1 routing.
It remains128BPM/64beats with zero automation lanes; the attempted range input
did not apply. Native save coordinates were rejected as15×15. UIA filename
set_value displayed the intended name, but Alt+S created
`desktop-song-14/Untitled.citrus` instead. That new test file overwrote neither
existing native13 nor native14 and was copied unchanged with overwrite=false
to the isolated `First-Light-NOIRE-Pure.citrus`. The actual result path, not the
edit display, determines the artifact. See `normal-save-model-audit-01.json`.

Both exact process handles were held before normal Alt+F4: App13528 and
helper36632 actually exited0, the exit observer ended, and fresh heavy0 preceded
explicit peer handback. Preserve `normal-close-process-terminal-02.json` and
`source78-resource-handback-01.json`. This runtime result does not replace the
old component-scan ExitDll failure. New cold-open, automation and audio-export
acceptance remain open.

Source79 (`56e9c89`) registers catalog intent as LoadedDefault,
validates the real None-class LOAD's actual UID/version, then enters unchanged
full fresh preparation. Exact callers, complete bus/initial timing checks,
original owners/restoration and captured-CID strict clone remain guarded. Five
new cross-platform backend tests and an App no-fallback matrix are present;
formatting/source review passed. Its actual cloud quality passed32 factory tests
and preview passed344 packaging/protocol harness tests. Both then failed root
test compilation with
`E0603: module catalog_tests is private` at
`src/app/clip_render_glue_tests.rs:730`. Root regression, strict Clippy,
release/ZIP and native checkbox acceptance have not run. Preserve
`ci-56e9c89-windows-{quality,preview}-01.log` and Source79 actual-terminal JSON.
The ordinary Source78 session is not evidence for the new branch.

Source80 (`9f3b608`) shares the existing two-class catalog construction through
the test module's private `fresh_catalog_descriptor`, removing the invalid
private-module call without exposing production APIs or removing assertions.
Local12 targeted cases, strict all-feature/all-target Clippy and matching main/
helper builds passed. Cloud quality passed32 factory and2370 root tests/12 ignored
plus all backend gates; only independent native PrintWindow capture failed.
Preview passed344 harness tests, then2304 App tests passed and one failed:
`stopped_epoch_safety_discards_inflight_old_output_and_rejects_queued_old_events`
at `audio_stopped_epoch_safety_tests.rs:1048`, asserting
`paused_work_quiescent(SAFETY_REVISION,2)`. Its Clippy/release/ZIP did not run.
All nine fresh/default backend and App-intent cases passed in both lanes. Preserve
`ci-9f3b608-windows-{quality,preview}-01.log`, Source80 actual-terminal JSON and
`source80-local-gates-terminal-01.json`.

The matching isolated Source80 binaries normally opened the actual saved
First-Light project without reloading an NKI. The native1350×680 editor showed
NOIRE Pure and the previously moved upper-left Color. FIT NOTES showed all eight
notes; the Channel→FX1 route and Icon48k/XRUN0 remained. Normal Save succeeded;
an overwrite=false copy preserves the input. All top-level model fields except
plugin_instances remained equal. Both generic parameter maps are empty and opaque
state changed591680→591683bytes; this is not byte equality or an all-parameter
audit. No automation was added and new physical listening/PCM remain pending.

FILE export admitted the experimental Whole Review. Actual settings were48k
Float32/Preserve level/0ms extra tail, explicitly confirmed. Attempts to set
song length16 and tail3000 did not apply: the project remains64beats (30seconds)
and the actual tail is0. After choosing a new WAV path and Alt+S, the host reported
`Whole-song export cancelled; destination was not changed; playback remains paused`.
No WAV or clone helper resulted. No Cancel, Escape or Stop was sent after Save.
Source investigation found that ordinary App update calls
`dispatch_armed_window_close` each frame, whose original fence branch cancels
without first checking close/exit intent. The frozen Source81 fix checks exit
intent before that branch. Seven new real App::ui cases cover ordinary frames
through held capture and a complete mock WAV, explicit cancellation and late
receipts; the held-clone close case also uses the production UI entry point.
First-accepted cancellation source/phase is retained, displayed at cancellation
and logged once under the existing diagnostics switch with frozen source/session
identity, without state contents.

After another ordinary SAVED result, held exact App19460/helper93060 handles both
confirmed natural exit0. Exit observer16196 and resource observer83574 ended0;
fresh heavy0 preceded explicit peer handback. Preserve
`cold-open-and-export-attempt-source80-01.json` and
`normal-close-process-terminal-03-source80.json` in the isolated workflow folder.
The Source81 stopped-safety test candidate retains the original successful full
snapshot rather than resampling a worker that can become busy again. A real admin
gate counterexample is added without changing production predicates or deadlines;
the original failure's varying field was not logged. Both new repairs await
compilation/regression and another native export; the full workflow remains open.

Source81 (`f807fb0`) subsequently failed root test compilation in both cloud
lanes: `ParameterEditFailure` in the new cancel fixture omitted `was_pending`
(`whole_song_export_cancel_tests.rs:263`,E0063). Quality's32 factory and preview's
344 harness cases passed, but no root test/Clippy/build pass follows. Preserve
`ci-f807fb0-windows-{quality,preview}-01.log` and Source81 terminal JSON.

Source82 (`098e3f5`) supplies `was_pending=true` for a queued edit's late timeout.
Local compilation passed and the new cancel group actually reported6 passed/
1 failed: held capture followed by Escape and repeated Stop ended Failed instead
of Cancelled. The remaining old/25 safety tests, Clippy and bins did not run.
Runner89774 exited1/Cargo101, observer35329 exited0 and fresh0 preceded peer CPU
handback; there was no NI/CU. Preserve `source82-headless-targeted-01.log`,
`source82-local-gates-terminal-01.json` and its resource handback receipt.

Source83 (`857d17b`) reuses the original actor's pause only while it is
the same engine/project's current mailbox request and App/clock/callback all
remain stopped. Ordinary Escape/repeated Stop no longer invalidates that exact
request. A foreign pending Play or real playback still causes a new pause; the
stop-intent, state, capture and cleanup fences are unchanged. Whole/selected-clip
tests retain Escape/repeated Stop and assert request identity. A new foreign Play
case requires an override even before the callback sees playback; a genuinely
invalidated contract remains Failed with capture ownership retained until ACK.
All8 cancellation cases actually passed in both cloud lanes, including ordinary
App::ui frames publishing a real mock WAV. Quality passed32 factory cases and
2314 App+52 helper+13 protocol=2379 root cases/12 ignored, strict all-feature/
all-target Clippy, both bins, protocol/harness, fixture and no-default-feature
gates. Its only failure was independent native paint: before/after PrintWindow
and the same-process button diagnostic returned false; native control, state,
lifecycle and cleanup passed. This does not establish a local Kontakt UI defect.
Preview passed344 harness cases and2313 App cases/12 ignored, but one old test
failed: `actual_whole_forced_app_drop_keeps_the_cancelled_capture_and_exact_source_until_worker_join`
stopped in `settle_initial_source` before starting its export. Preview Clippy,
release and ZIP did not run. The new same-snapshot admin-gate safety regression
passed in both lanes. Preserve `ci-857d17b-windows-{quality,preview}-01.log` and
`source83-*-actual-terminal-01.json`. Repaired native NOIRE export remains untested.

Source84 changes tests only. The shared `Fixture::until` previously sampled its
condition again after the successful read that ended the loop. Momentary Ready
is not a lease on the next observation. The fix asserts that same successful
sample and retains the five-second budget, ticks and cleanup owner. Two new
deterministic cases cover a one-shot true condition and actual Ready followed by
a superseding foreign transport request; the latter also verifies that old
readiness does not authorize the new request. Failure diagnostics add actual
elapsed time, sample count and settlement operands. The historical Source83
failure lacks those quantities, so its exact cause remains unproven. Source84
(`42bbf9e`) quality actually passed32 factory and2316 App+52 helper+13 protocol
=2381 root cases/12 ignored, including both new cases and the previously failing
forced-App-drop test. Strict all-feature/all-target Clippy, both bins, harness/
protocol, fixture and no-default-feature gates passed. Quality still failed only
independent native paint's before/after PrintWindow capture, while native control,
state, lifecycle and cleanup passed. Preview actually succeeded:344 harness
cases, the same2381 root passes/12 ignored, strict Clippy, no-default-feature
check, static-CRT release, final ZIP integrity and extracted sanitized-PATH
helper smoke passed. No build artifact was uploaded. Preserve both
`ci-42bbf9e-windows-{quality,preview}-01.log` and Source84 actual terminal JSON.
No repaired native NOIRE export has run.

Source85 adds two test-only actual App::ui keyboard-entry cases: Song Length
64→16 and Release Tail0→3000. They use the real TextEdit focus, Ctrl+A,
individual digit key/text events and Enter, then inspect the actual project or
Review options and retain arrangement/playback side-effect checks. Neither
AccessKit SetValue nor direct target-field assignment performs the edit. No
production input control changed and no ordinary-keyboard defect is established.
Formatting/diff passed at freeze. Both cases subsequently passed Cargo and
strict Clippy in both Source87 cloud jobs. Native individual-digit entry is not
qualified by those headless results. Source84's preview actually ended before
pushing this candidate for cloud execution.

Source86 (`1200196`, including those tests) quality stopped before root tests:
31 factory cases passed and `factory_and_default_load_share_one_attempt_seal_in_both_directions`
failed at `default_class_probe_tests.rs:159`. Its first ProbeFactory returned
`Failure { kind: Deadline, delivery: MayHaveApplied }`; root, Clippy and build
did not run. Preview passed344 harness cases and2317 App cases/12 ignored, but
the tail test failed its Review text observation. Song Length passed. Helper/
protocol tests, Clippy, release and ZIP did not run. Preserve both
`ci-1200196-windows-{quality,preview}-01.log` and Source86 actual terminal JSON.

The Source87 test candidates retain the real failures. Factory diagnostics add
child script/import/read/validation/flush phases and parent PID, elapsed time,
original Failure and actual same-owner cleanup, without changing the three-second
deadline, single attempt or failure. The old log cannot distinguish those phases.
The complete vendor patch was replayed from41 upstream to74 current sources:
48 changed paths,33 additions and0 deletions; explicit bundle integrity and the
one existing provenance-constants test passed. Rust tests were unrun at freeze;
current quality results are recorded at the top of this document. The tail
test read the wrong AccessKit field: egui Role::Label text uses value(), not
label(). Its candidate checks the actual role/value and retains independent
digits, focus, Enter and real destination options==3000 assertions. It subsequently
passed in Source87 quality.

### Native04: ordinary GUI rendering reaches clone cleanup, then fails

Actual Cargo all-feature main/helper artifacts built successfully with one jobs1
owner; pending changes were docs, package provenance and a cfg(test) vendor test,
not production Rust. Normal Open loaded the independent First-Light validation
copy without an NKI reload. Kontakt was Loaded, Channel→FX1,2304 samples latency,
Icon48k/XRUN0. SONG playback moved through the eight-note clip; no new listening
confirmation was received. Ordinary Ctrl+S showed Saved at the exact copy path.
Song length actually became16 after typing only1 and clamping from64; this does
not qualify two-digit native entry. Tail text entry3000/Return reached Review.
The saved model retains eight notes, one clip and all routing; only song length
and opaque state changed. Generic parameters remain empty and opaque bytes differ
(591683→591681). Native05 subsequently cold-opened this saved copy; complete
parameter and automation restoration remain unqualified.

The actual Whole review used48k,24-bit PCM, peak attenuation and an explicitly
accepted fixed3000ms tail. After native Save, clone41712 started and the UI entered
`Rendering full master / disposing clone`, with no subsequent Escape/Stop/Cancel.
Cleanup then reported retained ownership, required forced termination and grace
expiry before acknowledgement. The final modal was `Strict clone did not terminate
gracefully`. No WAV was published, so PCM duration, tail and click checks cannot
run. The filename control showed the requested new full WAV path, but the host's
progress path was `Untitled.wav` in the same isolated folder; keep this as a
tool/native-dialog submission discrepancy, not an established product defect.

Normal saved Alt+F4 ended App23408 and original helper112504 with actual exit0;
clone41712 actually exited1. Passive observer4724 exited0 and fresh heavy0 preceded
explicit peer handback. The observer issued no kill; the product reported forced
clone cleanup. Receipts: `cold-open-and-export-attempt-native04-01.json`,
`normal-save-model-audit-04.json`, `normal-close-process-terminal-04.json` in the
isolated workflow directory, and `native04-resource-handback-01.json` at its parent.
The subsequent exact exit-timing observation is Native05 below; retain this
failure rather than treating one later success as a cleanup repair.

Source87 retains the original primary first line in the Whole terminal error and
adds the existing backend/cleanup typed context and cleanup details. The existing
diagnostics switch emits one `WHOLE_EXPORT_RENDER_FAILED` containing typed facts,
not plug-in text, paths, state or PCM. Shutdown failure receives `dispose.shutdown`;
the first forced-exit failure receives `dispose.wait_for_exit`. Render-first-error,
cancellation, ownership, fences and budgets are unchanged. Three new tests cover
actual App/mock-renderer preparation and cleanup failure, payload-free diagnostics
without invented delivery, and retained cleanup-first-error; the existing held-exit
test also checks its stage. Both Source87 CI jobs subsequently passed those
regressions and Clippy. Source inspection places Native04's cleanup-only primary
after successful render checks, but no WAV was published in that failed run.

### Native05: ordinary GUI cold-open and Whole WAV complete once

Matching all-feature Source87 main/helper bins actually built with one jobs1
owner. The normal Open dialog loaded `First-Light-NOIRE-Pure-validation-02.citrus`,
the independent file saved by Native04, without an NKI reload. The UI showed
four bars/16 beats, eight notes/one clip, Saved, Kontakt Loaded, Channel→FX1,
2304 samples latency and `扬声器 (2- CB4ND HW 1/2)` at 48k/XRUN0. There was no
playback or new project Save in this session; it does not re-qualify monitoring.

Ordinary FILE→Export WAV→Review used stereo 48k/24-bit PCM, peak attenuation
above 0.95 and an explicitly accepted 3000ms/144000-frame fixed tail. Before native
Save, the default `Untitled.wav` did not exist. It was accepted without trying to
change that dialog's filename. Save created that exact file; no Escape/Stop/
Cancel followed. The final UI reported 7.500s music plus 3.000s fixed tail,
10.500s total and unity gain/peak 0.046039. This is Whole offline export, not
Master recording or selected-MIDI replacement. The original native destination
is retained; an overwrite=false copy is named
`First-Light-NOIRE-Pure-ordinary-whole-native05.wav`.

Independent bounded PCM24 decoding, without renderer or plug-in imports, found:

- Complete RIFF: 3,024,044 bytes, 504000 stereo frames, 48k/24-bit, 10.500s.
- Nonzero signal in all eight note-onset windows; all samples finite, no clipped
  samples. Peak 0.046038747 (−26.74dBFS); this does not establish target loudness.
- Tail first 500ms RMS about −93.29dBFS; last 500ms peak about −119.39dBFS.
  Last 20ms and the final stereo frame are exactly zero. At −100dBFS, trailing
  quiet spans about 1.959s.
- Maximum adjacent sample delta 0.002964 at 2.851708s is a diagnostic cue.
  This WAV has not been listened to, so click-free playback is not qualified.

The plug-in reports unknown/unbounded tail. The explicit fixed-window warning
remains; this one quiet ending does not establish complete natural decay for
arbitrary settings. Generic parameter maps remain empty, the project has no
automation, and Felt/full parameter restoration remain untested.

Only the existing exact
`CITRUS_VST3_HELPER_LOAD_DIAGNOSTIC=windows-load-response-timing-v1` was added to
the current diagnostics switch; `SHUTDOWN_MESSAGE_PASS` was explicitly absent.
Clone 112960's reader decoded/enqueued Shutdown, main consumed it and exited the
loop, and editor close/dispatcher drop/component terminate (SDK 0)/plug-in Drop
returned. `ExitDll Before→Returned` took about 751ms; Windows run returned at
about 915ms. Held actual OS handles confirmed clone 112960, source helper 84196
and App 69668 all exited 0 after normal Alt+F4. Observer 66907 exited 0 and fresh
heavy=[] preceded explicit resource handback.

Native04→05 also changed the play/Save prelude, cold-start state and diagnostic
source. Source87 changed failure evidence, not cleanup order or budgets.
Neither diagnostic timing nor Save is established as the failure's cause.
Native06 below reused those bins/input/cold sequence/options with exact timing
off. Warm play/Save still needs a separate comparison; do not extend deadlines
or make forced termination count as successful export.

Evidence in `target/backend-reliability-20261014/normal-noire-workflow-20261015-01/`:
`launch-05.json`, `cold-open-and-export-native05-01.json`, `stderr-05.log`,
`ordinary-whole-native05-pcm-audit-01.json`, `audit_native05_pcm24.py` and
`normal-close-process-terminal-05.json`. Its parent contains the actual build
terminal receipt, `native05-resource-handback-01.json` and
`source87-continuation-checkpoint-02.json`. Audio, opaque project state and raw
runtime evidence stay outside Git.

### Native06: cold export does not require exact timing; first onset differs

This session reused the exact frozen Native05 main/helper, the same validation02
file and isolated App data, without a rebuild, playback, editor opening or new
project Save. The starting App preference files were retained. Exact timing was
absent at process creation, plugin diagnostics remained1, and message-pass was
absent. Normal Open showed Saved/16beats, Kontakt Loaded, Channel→FX1,
2304samples and Icon48k/XRUN0.

The actual ordinary Whole review was48k/stereo/24-bit PCM, attenuation above0.95
and accepted3000ms fixed tail. The native Save address bar was navigated with
Ctrl+L, indexed address SetValue and Return; the actual new folder was verified,
while the default Untitled filename was unchanged. The destination did not
exist before Save. No Stop/Escape/Cancel followed. The UI and actual output
reported10.500s/504000frames,7.500s music plus3.000s tail, unity gain and peak0.046039.

Independent decoding confirms complete3,024,044B PCM24, nonzero signal in all
eight onset windows, no clipped samples, peak−26.74dBFS, the same quiet tail and
last20ms exact zero. Clone40840, original helper89188 and App110476 had held
actual exit0. Normal Alt+F4 completed observer62451/30376 exit0, no binding errors,
fresh process/window0, then explicit peer handback. No new listening occurred.

Direct sample comparison with Native05 found66023 differing samples in33019
frames, confined to frame129..33147. Over the first400ms, shifting Native06
back384frames (8ms, three Q128 blocks) gives exactly zero sample difference;
all later audio is identical. This establishes a first-note timing difference,
not its cause. Neither NI readiness, PDC nor the trace switch is yet established
as responsible. Follow the actual clone prepare/reset/MIDI/latency path before
choosing a repair; two successful publications do not qualify onset consistency
or fix Native04's warm-state teardown failure.

Evidence in the same isolated workflow directory:
`launch-06.json`, `cold-open-and-export-native06-01.json`, `stderr-06.log`,
`normal-close-process-terminal-06.json`, `native05-native06-pcm-comparison-01.json`
and `native06-no-exact-timing/ordinary-whole-native06-pcm-audit-01.json`.
The parent contains `native06-resource-handback-01.json`. The parameter-restore
candidate is separate, unbuilt source and did not enter this native comparison.

Source57 actually passed 306 targeted Rust tests (187+5+12+45+26+31), root strict
Clippy in default/all-feature/no-feature profiles, exact preview feature checks,
ordinary debug binaries and root/vendor formatting. The actual locked inventory
contains 483 packages with root vst2,vst3 and no experimental vendor feature;
this does not qualify a release preview artifact. Source58 preserves production
bytes while fixing the default-probe test helper's unused result and a Unix-only
fixture assertion. Default Clippy and all 12 Probe tests passed again. The SDK
cleanup-result use lint and final root/vendor/fixture formatting passed;
existing vendor style warnings are retained, not claimed as a warning-free SDK. Its full
73-file vendor patch replay is byte-exact, and all 170 package tests passed with
the correct unittest discover entry. Strict rendering stays experimental.

Original failed checks remain in local evidence: large Err/style lints, test
err_expect, offline metadata with an uncached dependency, the wrong unittest
import entry and rustc's refusal to accept extra arguments for multiple targets.
The intentional strict fixture's late OSError errno22 and existing platform
warnings remain in logs. No failure or synthetic process is claimed as NI audio.
New Kontakt/NOIRE, Linux/macOS, hardware and native UI acceptance is unrun.
Earlier Source52 native audio remains bound to its original source cohort.
Local source57/source58 receipts, recordings, state and build artifacts remain
outside Git; protected initial files and the original Saved13 were unchanged.

## Preservation and evidence policy

The initial uncommitted files were inspected and copied before editing:
`src/main.rs`, `scripts/qa_windows_kontakt.py`, `src/windows_kontakt_qa.rs`.
Their exact originals remain in the local `preexisting-changes` archive.
The two Rust files subsequently received reviewed formatting/window/QA wiring changes;
retaining their originals is not a claim that current Rust bytes never changed.
The original Kontakt script is preserved. No reset, work overwrite, security change or main merge
was performed. Test projects and new output directories were independent of user compositions.

Local evidence paths in this document belong to the original validation machine.
`target/` receipts, native projects, plugin state, PCM and binaries are not source-distribution inputs.
A clean clone is not expected to contain them; their absence is not a test failure or reproduced pass.
The pre-consolidation documents were archived byte-for-byte under
`target/backend-reliability-20261014/pre-push-docs-archive-01` with an exclusive manifest.
No NI opaque payload or private instrument-library download path is included here.

## Actual local plugin and audio results

| Check | Actual observation | Remaining limitation |
| --- | --- | --- |
| Scan/load | Kontakt 8.4.1 metadata and approximately 3-second empty-instance load passed | Empty-instance silence does not prove NOIRE audio |
| Native editor | User saw NOIRE Pure; old build opened, closed and reopened the separate Kontakt editor | Latest build and resize/playback-stall acceptance pending |
| Monitoring | User heard piano and reported normal volume | Specific sound-card model is not confirmed |
| Output identity | `扬声器 (2- CB4ND HW 1/2)`; manufacturer Icon ProAudio | Earlier default NVIDIA U2790B is not the intended output |
| Minimal persistence | Muted default-Kontakt project passed model save/reopen checks | Not NOIRE instrument or native restart verification |
| Pure persistence | Saved13 was normally saved, then cold opened in native14 | Not full internal NI parameter or Felt qualification |
| Song audio | Independent original song has actual Master recording and listening derivatives | These are real-time recordings, not conversion/offline receipts |

Computer Use succeeded for some local input/window observations and failed for particular cached
controls, click geometry and helper captures. These failures do not establish global local unavailability.
Cloud PrintWindow failure does not establish a broken plugin editor; native visibility is checked locally.
No new native window, listening or hardware operation is inferred from backend tests.

## Melody editing changes

Channel Rack layout/step visibility, Piano Roll horizontal and vertical view navigation,
Playlist ruler positioning/clip placement and button feedback have been adjusted in source.
Black/white key geometry, pressed/released state and playback feedback use actual audition state.
Audition cancellation, playback preparation and loop restart wait on their actual acknowledgements.
Existing copy, quantization, velocity, chord, snap and Undo/Redo functionality is retained.
These changes have source/headless regressions; latest-build FL-like interaction and compact layout
still require native use. A passing geometry or egui input test is not a physical playback acceptance.
For established editing behavior see [keyboard editing](PIANO_KEYBOARD_EDITING.md),
[mouse workflow](PIANO_MOUSE_WORKFLOW.md), [ranges/snap](PIANO_RANGES_AND_SNAP.md)
and the [FL parity inventory](FL_STUDIO_PARITY.md).

## Short actual whole-song qualification

Whole run02 used a real source, production App/controller and StrictBackend, with no mock renderer.
Its independent project has two patterns/two placements, two musical beats at 84 BPM and 48 kHz,
MasterVolume `.85 → .35`, and a reviewed three-second fixed tail.
The original Saved13 five-pattern/405-note/160-beat song was not rendered by this test.

The clone returned 1661 Q128 blocks. Natural exit0, concrete reap and destruction preceded typed
Complete/publication; actual source Join and cleanup/input pins passed.
The project, MIDI, history, dirty flag and saved path remained intact; playback remained paused.
The final Float32 stereo WAV contains 212571 frames (4.4285625 seconds).
An independent, pre-frozen analytical audit compared all 425142 final samples.
Maximum absolute error was approximately `2.05e-9` against fixed `2e-6` tolerance; no sample exceeded it.
Float32 bit differences were reported rather than called byte-identical.
All retained samples were finite, clipping count was zero, and all 37 padding frames were zero.
Tail audio was nonzero and the last 100 ms were silent; an internal exact-zero run was measured.
These numerical facts do not certify listening quality, absence of audible clicks or every natural release.
Plugin-reported tail remained unknown and its metadata warning was retained.

Evidence on the original machine:
`target/backend-reliability-20261014/windows-noire-whole-run-02/whole-song-master.wav`,
its immutable run result/terminal receipt and `whole-noire-independent-pcm-audit-01` artifacts.
The short test does not prove latest GUI, cold native save, clip Undo, Felt or physical monitoring.

## Complete original offline qualification

The subsequent Source52 full-song run02 exported the unchanged original Saved13 through the real production App/controller/StrictBackend and the newly built helper. It retains 405 notes, five patterns/placements, 160 beats including the empty 128–160-beat coda, direct MASTER routing and the original Master automation.
The actual clone returned 43983 Q128 blocks; 662248 records occupy 161370676 transcript bytes. Transcript/state writers joined, one source-worker Join is observed, and clone disposal/destruction precedes the typed report. The final 48k Float32 stereo WAV has 5629714 frames, 117.285708 seconds and 45037770 bytes; SHA-256 is `60f1c023b139114c10764e47b1b0d499f4f1a8f89184913e1528ce150076a396`.
Independent verification matches all 782 MIDI rows and 11259428 final samples, with maximum absolute error `3.72529e-9` below frozen `2e-6`. Music is nonzero, nonfinite/clipping samples are zero, maximum same-channel adjacent delta is approximately `.001802`, and the fixed three-second tail and last100ms are silent. Approximately 23 seconds of final silence includes the original empty coda. These numerical facts do not certify audible-click absence or complete natural release for the retained unknown tail.
Source sidecar loading matches the original 591772 bytes, while captured/clone-loaded 591761-byte payloads match each other exactly; this is byte consistency, not NI state semantics. Project/history/routes/automation and binary pins remain unchanged.
The actual test process PID3496 exited0 after 396.064 seconds (395.07s test). Crossing the external 280-second observation threshold only recorded Pending; the same process was observed to actual termination without a restart or budget expansion. This does not qualify total execution performance. It is single-VST, no insert effects, observed latency0 and an explicitly reviewed fixed tail. Scan/source helper independent strict reap, new cold save, actual listening/editor behavior, Felt, conversion Undo and effects/general PDC remain unqualified. The successful scan does not explain source50's timeout.
The immutable audio and receipts are local under `target/backend-reliability-20261014/windows-noire-full-song-run-02`; no user composition or source-distribution audio was overwritten.
The result's production_export section is the publication-stage snapshot, where audio audit was still pending. Final result.sound_audit_passed and the independent audit are true; the frozen earlier-stage field is retained rather than rewritten.

## Conversion and opaque-state refusal

Actual run07/08 strict clones produced short audio and naturally exited/reaped, but conversion
was refused by `OccurrenceCaptureAttestation::match_recapture` before replacing the original MIDI.
Actual error: `The second actual source capture differs from the consumed render capture`.
In run08 the captures had equal host envelopes and lengths but 177 differing NI-component bytes.
Their source identity, metadata, native generation, dirty revision and generic bases were unchanged;
no original-source positive Process/ResetOrigin call occurred between captures.
A separate no-pump three-capture diagnostic also observed adjacent opaque differences.

The cause and semantic significance of NI serialization differences are not established.
Host generic parameter count zero does not mean NOIRE has no internal settings.
Do not interpret or canonicalize opaque NI fields merely to bypass this admission guard.
Original MIDI/project/state/history and output evidence remain preserved.
Actual NOIRE conversion restoration/Undo/Redo is still unqualified.
Source regressions cover refusal for opaque-only, generic-base-only and combined drift.

## Initial-source failure and retained fresh ownership

Whole run03 failed before source-ready with
`qualification poll/pump deadline or bound exceeded` (actual 55.33 seconds, exit101).
Only 18 observations existed: zero capture, strict clone, Process and output WAV.
Cleanup lacked a concrete successful source Join; later OS process absence does not supply one.
No valid processing-time estimate or full-song budget conclusion can be derived from this run.
Its result, log and terminal receipt are retained; the actual NI cause remains unknown.

Source29/31 fixes retain the exact marked fresh guard across installation queue refusal,
actual callback Installed=false, running/pending retirement, engine loss, session replacement and Drop.
New/Open clearing registries or reusing IDs does not misclassify an unrelated ordinary worker.
Early-return guard Drop hands the actual worker owner to retained polling; Pending never detaches as success.
Actually Joined/panicked terminal owners settle; ordinary legacy behavior is preserved.
Thread-creation failure keeps an unjoined owner and does not report successful cleanup.
Audio teardown is an attempt: timeout-retained callback resources are not claimed destroyed.
Bounded initial diagnostics retain existing call/runtime/owner outcomes without new SDK operations.
No production success predicate or 35/280/300-second budget was widened to hide the failure.

## Executed source checks and current gate

| Source/check | Actual terminal | Scope |
| --- | --- | --- |
| source22 historical complete all-target cohort | 1963 passed, 0 failed; main 1932, helper/protocol 31; five NI ignores | Historical source; not acceptance of later changes |
| source29 clip-glue | 50 passed, 1 failed; all 12 new fresh cases passed | Original full-ring fixture failure retained |
| source30 compile | E0425 /Cargo101 | Diagnostic helper was private in a sibling module; no tests ran |
| source31 corrected clip-glue | 51 passed, 0 failed in 5.30 seconds | Entire corrected group, not whole current suite |
| source31 quality | Strict ordinary/all-feature all-target Clippy, root/include fmt and diff exit0 | Exact 243 source inputs; later changes excluded |
| Private spool/sidecar | 24 +24 selected tests passed; formatted copies passed fmt | Std-only foundations, not root/SDK/native integration |
| source32 compile | Four E0433 errors /Cargo101 | Missing test-only `strict` import; no tests/Clippy ran at that checkpoint |
| source33 regression | Compile passed; strict backend 29 passed /2 failed | Stale standalone IPC assertion and unrecorded Windows mock stdout error |
| source34 complete root all-target cohort | 2015 passed, 0 failed; main 1977, helper 25, protocols 6/5/2; seven explicit local/NI ignores | Actual terminal exit0; unchanged main/protocol results reused later, not native acceptance |
| source34 unfiltered vendor first run | 574 passed, 1 failed | Missing external upstream Dexed metadata fixture; original failure retained |
| source34 vendor single-exclusion rerun | 575 passed, 0 failed; one exact Dexed test filtered; probe has zero tests; exit0 | 574 library +1 legacy helper; not an unfiltered pass |
| source34 fixture | Check exit0; separate fmt-check exit1 | Three formatting blocks in native_windows.rs; original failure and bytes retained |
| source35 completed gates | Fixture check/fmt exit0; Python 259 passed (package 170, extraction 28, editor 61) | Actually executed here; root 2015/vendor 575 still belong to source34 |
| source35 strict all-feature Clippy | exit101: two result_large_err diagnostics | src/bin/strict_metadata_bundle/mod.rs:50/67; Err HostResponse at least 160B; original failure retained |
| source36 current helper/quality | Helper 25 passed, 0 failed; strict all-feature/default Clippy, root/vendor/include fmt and diff all exit0 | Compact internal Failure representation tested; these 25 are not added again to source34's 2015 |
| source36 actual Saved13 load-only | 1 passed, 0 failed; body 0.40s, including compile 18.79s | Reads the intact original project; no SDK/NOIRE instance, native reopen or export |
| source37 full owner | 14 targeted tests passed; strict Clippy exit101 (`new_ret_no_self`) | Original failure retained; model/binding/retention-policy tests, not a real NOIRE worker |
| source38 current owner/quality | 14 passed, 0 failed; body 1.76s; strict all-feature/all-target Clippy, root/include fmt and diff exit0 | Constructor renamed and callback released before App; 248 source inputs pinned, no original-song native claim |
| Private auditors candidate02 | PCM/WAV 35 and state sidecar 28 tests passed | Actual Windows file fixtures; not SDK, writer Join or NOIRE sound acceptance |
| source39 full-source fixture | 8 evidence tests passed; Clippy exit101 with five `cloned_ref_to_slice_refs` findings | Original failure preserved; no native source acceptance |
| source40 source fixture/quality/build | 8 passed; strict all-feature/all-target Clippy, fmt/diff and production helper build exit0 | Borrowed scan path/single-record slices; 249 source inputs frozen |
| source40 full-source native run01 | Actual 28.4428s, exit101; production scan timed out after 5s | Zero source workers/StateLoad/clone/Process/WAV; intact original 405 notes/160 beats and binary/project SHA unchanged |
| source41 pre-commit regression | 34 full-project tests passed, 2 ignored; 35 helper tests passed; Clippy exit101 (`write_literal`) | One format reference in a new diagnostic test; original failure preserved, no NI |
| source42 commit gates | 35 helper tests passed; strict default/all-feature/all-target Clippy, root/include fmt, diff and helper build exit0 | Only the test's dynamic format reference changed; source41's unchanged 34 full-project tests retained, not a whole-suite or native audio acceptance |
| source42 complete source native run02 | Actual 57.10s, exit0; 102 observations, original state acknowledged, one actual source worker Join | Intact 405-note/160-beat model/history/routes/automation and file/binary pins; no listening, new cold save, conversion or WAV acceptance |
| source43 source stream foundations | 115 passed (1.26s); strict Clippy exit101 with five decoder/style/enum findings | Original failure retained; no native throughput or complete-song claim |
| source44 stream foundation fixes | 116 passed (1.27s); strict default/all-feature Clippy, root/include fmt and diff exit0 | 257 source pins; fixed 64×2KiB pool, 64KiB whole operand, healthy backpressure and actual writer Join tested; active streaming sink not installed |
| source45 first complete stream-module gate | 198 passed (1.57s); strict Clippy exit101 with 15 unused-code/style findings | 263 source pins; instance dictionary, state sidecar and stream sink registered as test modules; original failure logs retained, no NI/full-song acceptance |
| source46 foundation commit gates | 200 passed (1.46s); strict root default/all-feature/all-target Clippy, root/include fmt and diff exit0 | Removed the unused FinalSource phase; actual cancel/recovery/same-writer Join regressions; 263 source pins, no NI/full-song acceptance |
| source47 independent full audit source port | 42 passed (1.14s); complete 405-note/810-boundary/782-wire input and 35 file/PCM regressions | Frozen builder bytes retain their original SHA; silent matching music is not NOIRE sound; no actual complete raw/WAV supplied |
| source47 first active stream integration compile | exit101: `E0063`, missing `WavExportOptions.sample_rate` in the full-song entry | 274 source pins; new Rust regressions and NI not run; failure retained and explicit 48k configuration added separately |
| source48 integration Clippy | exit101: unused replay-test helper and raw-reader loop style | Compilation passed; new Rust tests had not run; original failure retained |
| source49 active integration gates | 235 stream +34 full-owner tests passed, 3 explicit native cases ignored; strict default/all-feature/all-target Clippy, root/include fmt and diff exit0 | Default caps, all-kind replay and actual whole-owner Drop/recovery/Join fixtures; separate total auditor 16 passed, no native audio acceptance |
| source50 complete original native run01 | Actual 27.767s, test exit101; production scan again timed out waiting 5s for the helper response | No source/clone/stream/WAV started; complete model/history/592KB original state and binary pins unchanged; no full-song export pass |
| source51 vendor diagnostic replay/package | Verified upstream patch reproduces all 70 source files byte for byte; actual apply/check exit0, Python 170 passed | Original CRLF summary-parser failure retained; test process exited0 and only its frozen log binding was corrected, with no test rerun or NI/runtime claim |
| source51 first diagnostic Clippy | Actual exit101: dangling-pointers-from-temporaries | A temporary Box.as_ptr in the new helper test; Rust tests/NI not executed and original log retained |
| source52 diagnostic gates | 39 helper +5 SDK +34 full-owner passed, 3 explicit local cases ignored; nine quality/build gates exit0, Python package170 passed | Only test-returned Box lifetime fixes; production diagnostic prefix unchanged, full 70-file upstream patch replay passes |
| source52 complete original native run02 | Actual 396.064s exit0; complete input,782 MIDI and all PCM/WAV/stream-state Join audits pass | External 280s observation threshold crossed, no performance claim; single VST/no inserts/observed L0/reviewed fixed3s, complete workflow still unqualified |

The source34 root command actually completed in 805.36 seconds; main tests took 635.87 seconds.
The first source42 runner mistakenly retained the source41 manifest reference and failed its precheck before Cargo started. That failure is preserved; the corrected runner passed. Source42 gates cover its helper/quality and unchanged source41 full-project regressions. Source49 is the current active-integration gate; earlier full-suite results remain attributed to their original source batches.
The vendor failure is `internal::module_info::tests::reads_sdk_generated_bundle_metadata`, which requires the absent external `vendor/test_plugins/Dexed.vst3`; production read returns `Ok(None)`.
The existing [processor domains](PLUGIN_PROCESSOR_DOMAINS.md) documents this single-fixture boundary. That boundary is not evidence of an unfiltered vendor pass or an exact historical command invocation.
Source35 changes only fixture whitespace and the `MB_OK` call's trailing comma; root/vendor source is unchanged. Source34's actual complete results are retained without claiming a new full-suite rerun.
Source35 also verifies 49 historical MIDI payloads (inventory only); Kontakt `--help` exits0 without spawning a plugin.
Source36 changes only the helper's internal Failure representation and preserves wire/query order/admission guards.
The unchanged source34 implementation coverage, source35 fixture/Python and source36 helper checks retain their original attribution. Source38 adds the targeted full-owner regression and current all-feature quality gates; it does not claim a new complete source34-suite execution. Its initial runner referenced the old manifest, so preflight exited1 before any Cargo command; the corrected runner then completed all checks.
None of these checks is new acoustic/NI acceptance.

Source29's isolated full-ring retry passed but did not erase the original group failure or prove its cause.
Source31 additionally inherits unchanged source29 retained-shutdown (7) and pure NOIRE (62) checks; those cohorts were not unnecessarily rerun. Their actual NI cases remained explicitly ignored.
Current exact receipts are `source31-quality-receipt-01.json`, `source32-strict-backend-tests-terminal-01.json`, `source33-strict-backend-tests-terminal-01.json`,
`source34-pre-push-root-all-target-tests-terminal-01.json`, `source34-pre-push-vendor-all-target-tests-terminal-01.json`, its precisely filtered rest receipt,
`source36-pre-push-*-terminal-01.json` and `pure-stream-foundations-quality-03.json` under the local backend directory. Each receipt retains its actual source/stage attribution.
New owner receipts are `source37-owner-tests-terminal-01.json`, its failed Clippy receipt, and `source38-*-terminal-01.json`. Auditor receipts are `independent-full-pcm-audit02-tests-terminal-01.json` and `independent-state-sidecar-audit02-tests-terminal-01.json`.

## Newly applied and remaining full-song foundations

Selected metadata bundle code admits the actual selected Kontakt 8.4.1 factory identity and preserves complete bus/event/latency/tail snapshots, failure fields and query order.
Source-level accounting changes wrapper IPC from 11 to 5 per Q128 while retaining 149 SDK calls.
This is not a measured speed-up; source regressions, current quality and package gates pass, while actual native processing/cancellation timing and full-song feasibility remain unmeasured.
The separate legacy helper explicitly refuses the new request rather than silently handling it.
Vendor replay/provenance and packaging pins are refreshed independently of native acceptance.

The additive full-input loader preserves the complete pinned Saved13 through production Project::load and refuses normalization changes; the short scratch path stays separate.
Original 405 notes, five placements and 160 beats including coda remain intact.
The source routes directly to stable MASTER, channel `.55`, Master base `.7`, global `.7 → .56`.
The driven Master value replaces its base; it must not be multiplied by a second `.7`.
The explicit Windows test module now has a full-project owner using production source initialization, actual callback installation and stopped-source readiness checks. Its model/history image preserves every persisted field and skipped selection/waveform/migration data, allowing only the live source's session runtime status to change.
The owner requires an actual caller-supplied scan descriptor with an exact `Instrument` category token. A retained wrapper keeps the entire owner in failure quarantine on unfinished cleanup; this has no continuation/recovery interface yet and is not successful retirement. Resource completion is distinct from final model equality. Complete images are checked at stage boundaries rather than copied every 8ms while loading.
Source42 native run02 initialized the intact original project with a real source instance without rendering it; subsequent source52 now passes the intact complete offline render within the separately stated scope.
The source40 init-only fixture was actually attempted once using private frozen test/helper binaries. It failed in the separate default-instance scan with `Timed out after 5s waiting for helper response (plugin may have hung)`, before any full source worker was constructed. The 28.4428s test duration is not LoadPlugin duration, and NI's own initialization log interval does not prove the whole host load returned. Post-initialization controller/state synchronization, GUI capability, MIDI/program/parameter enumeration, setup and the Windows `plugin_changed`/reply chain remain to be timed. No existing timeout or success predicate was expanded.
The exact outcome is in `windows-noire-full-source-run-01/source-result.json` and its external terminal receipt. Full input/model/history and executable/helper pins passed; cleanup of the absent source resources is not a source worker Join or scan helper reap claim.
Windows helper diagnostics now have an exact process-start opt-in: `CITRUS_VST3_HELPER_LOAD_DIAGNOSTIC=windows-load-response-timing-v1`. Disabled paths retain the original serializer/write/flush, SDK order, retries and deadlines without diagnostic clock, TLS or stderr work. Enabled records correlate common load-handler entry/return, Windows `plugin_changed`, serialization and actual write/flush outcomes using one monotonic load clock. Fixed stderr fields omit paths, state, parameters and audio. Accepted write bytes and successful flush do not prove receipt by the parent. Ten pure diagnostic regressions passed. Actual run02 succeeded: the scan and source handler-entry-to-flush intervals were 3.404s and 2.367s, both 879-byte writes succeeded, and the actual descriptor/source readiness and state acknowledgement passed. This does not explain run01's timeout or establish repeated stability. The actual `cannot resolve resource: resources_ENG` and `lib_ENG` messages remain recorded; no NI resources were edited.
Its ordinary regressions are covered by source34; the explicit source36 actual Saved13 load-only check passes, preserving original notes/state/routing/automation without creating a native instance.

Private bounded spool and opaque sidecar foundations have executed 48 selected tests. They retain bounded storage, partial-write/cancel/unwind state and actual writer Join obligations.
Source44 registers typed control/App/native codecs, complete identity framing and the reusable spool under the test-only observer. It adds borrowed encoding/decoding, inline PCM bits and a whole-operand header budget without changing the default memory collector. Its 116 actual tests include 44,000 synthetic PCM-sized records reusing one buffer; this is not native filesystem/NI throughput proof. Source46 also registers and tests the supplied-instance dictionary, state sidecar and stream sink. Cancellation preserves original allocations and cannot publish success after the same actual writer joins. Source49 adds all 23 live borrowed taps, terminal replay and the full-song driver; the source50 native attempt did not reach streaming or rendering.
Commit 018cb92 contains 19 source/test/document/attribute files covering the qualified source49/50 integration and independent auditors and is verified on the development branch remote. Five unverified vendor load-phase diagnostic files were preserved outside that commit. Existing ignore rules retain 14 local composition files and target receipts, opaque state, PCM and binaries outside Git; no user work is deleted.
Typed-control replay and streamed observation integration pass their targeted source tests; actual complete-song retirement and audio qualification remain open. The independent state-file audit first ran 23 cases with 11 failures/6 errors; that original result remains. Candidate02 subsequently executed 28 tests successfully, retaining separate Path.stat/fstat baselines and comparing only common identity fields across APIs.
Source49 passes targeted gates for all 23 borrowed taps, whole-owner early Drop/recovery, record-by-record typed replay, raw PCM projection and the original complete-project production driver. Default memory caps and the original immutable prelude are neither increased nor reset. Stream budgets are separate and successful hot-path records avoid owned PCM/RecordKind copies. Source50 actually attempted the full native entry but failed during the separate production scan, before source initialization or streaming. The real complete render, retirement and full-sample audit remain untested.
Complete PCM/WAV auditor candidate02 executed 35 tests successfully. Actual Windows stat/fstat ctime differences are covered without weakening each API's own mutation checks; a short read locks WaveCursor failure and forbids further draining. The independent builder and numerical policy remain unchanged.
These are private file fixtures. They do not certify real SDK identity, source retirement, writer Join or full NOIRE audio.
Old finite observation/reply/state quotas cannot be bypassed by resetting counts or dropping evidence.

The subsequent diagnostic source adds 31 fixed SDK load phases and four helper call boundaries under the existing exact startup opt-in. Each before/returned pair uses the active command thread's original load clock; available HRESULT values remain original signed integers. The disabled gate precedes callback/TLS/clock/IO work. An unavailable TLS scope drops the diagnostic rather than panicking during SDK cleanup. Probe-view Release, error teardown and ExitDll have separate boundaries; neither a Drop-body return nor ExitDll proves FreeLibrary completed. Five local library-gate and four added helper-boundary regressions now pass under source52, along with compilation, quality gates and the complete original native offline entry.
The new full-song receipt distinguishes owned resource cleanup from verified source readiness. Cleanup when no source was created cannot establish a source-worker Join; independent scan-helper reap remains unproven. Existing source50 failure evidence remains unchanged.
Source51 retains the legacy PowerShell policy refusal and two source precheck failures caused by a copied runner still referencing source49; Cargo had not started. Its corrected 277-pin runner then failed on a new temporary-Box test pointer; source52 retains that failure and binds returned Boxes before comparison, passing its gates. Jobs1 CPU checks overlapped the other chat's functional UI checks by notification; this overlap is not performance acceptance. Native run02 started only after the other chat's actual terminal handoff.

## Reproduction and next acceptance

Source59 wires both complete-project preflights to retained checked scan evidence
and adds a single ordinary-build Kontakt scanner-only ignored entry in the existing
Windows QA module. Completed/Stopped and reap/healthy-exit/metadata admission
remain separate; evidence is written before admission, and later failures retain
observed scan reap. Source helper, renderer, writer and audio proof stay independent.
Source60 narrows this test-only module to Windows, matching its actual consumers
and avoiding Linux test-build dead code; production Windows/Linux scanning is
unchanged. Source59 remains an unrun frozen cohort. Source60 actually passed both
default and all-feature root Clippy with all targets and `-D warnings`, six new scan
evidence tests, 34 existing full-source/full-song regressions (three explicit native
fixtures ignored), ordinary-build native selector compilation/listing, root and
private QA formatting, and diff checks. All eight terminal receipts have exit 0 in
`target/backend-reliability-20261014/source60-scan-*`; four existing dependency SDK
warnings remain, so this is not a zero-warning vendor qualification.

Source60 native acceptance remained unrun. The scanner-only entry
compares actual binary bytes before/after; its supplied SHA fields require
independent outer runner verification and do not establish Kontakt version or
NOIRE audio. Synthetic evidence tests and listing an ignored selector are not an
actual Kontakt scan, NOIRE state restoration or sound qualification.

Source61 bound pushed commit `1d374ce` to the same 282 tested source/provenance
bytes plus two current document pins. Default helper build and actual Cargo test
artifact binding passed (`default,vst2,vst3`, no experimental feature). The one
ordinary Kontakt scan actually ran: test PID22476 exited101, body 6.55s; helper
PID25428 returned metadata but did not retire naturally within the unchanged
cleanup budget. The same typed receipt records `reaped=true`, `kill_requested=true`,
exit1 and no load/shutdown/kill/wait/pipe error fields. Production correctly stopped
with `UncleanRetirement` and `metadata returned but helper did not retire cleanly`.
This did not exercise the actual App index UI. Static installed PE FileVersionInfo
is 8.4.1; it is not an SDK loaded-version reply. No NOIRE state, editor, MIDI/DSP,
audio device, save/reopen, conversion or export was requested.

The original outer receipt additionally reported a false field-consistency error:
PowerShell unwrapped the one-element receipt array into a Hashtable whose Count
was 11. Independent explicit object[] validation of the frozen JSON agrees with
all three reap/healthy-exit booleans, with zero validation errors. This fixes the
interpretation, not the failed native scan. Original result, terminal and log bytes
are unchanged. Evidence is in `windows-kontakt-default-scan-run-01/` and
`source61-default-scan-independent-validation-01.json`.

Reproduction used the exact ignored selector
`windows_kontakt_qa::runtime_qa::windows_kontakt_default_class_scan_and_reap`
with `--ignored --exact --test-threads=1 --nocapture`, a new private adjacent helper,
and the explicit scanner opt-in. Preserve this failed cohort; any diagnostic run
needs a new source/binary binding and new directories. Keep the five-second load
and 250/2250ms cleanup deadlines. Shutdown consumption, dispatcher destruction,
whole-plugin destruction and OLE teardown need observation before attributing the
stall to terminate or FreeLibrary; successful local Shutdown write is not command
consumption. Existing SDK teardown markers currently lose their active trace after
the LoadInfo response and need a separate opt-in shutdown trace.

Source62 implements that explicit exit trace in the existing helper logger; it
reuses the exact `windows-load-response-timing-v1` opt-in and preserves the load
trace. Reader and main causes remain separate, the first cause is retained, and
exit records share a 128-record per-process cap before clock/sink access. Disabled
paths avoid clock/TLS/allocation/I/O. Fixed markers cover input consumption,
dispatcher field destruction, whole plugin slot clear, OLE teardown and the actual
return from Windows run; six existing SDK teardown stages use the exit prefix.
SDK Drop/ExitDll body return is not labelled FreeLibrary or process termination.
Original commands and deadlines are unchanged; there is no vendor patch. Six new
pure regressions plus the original 39 helper regressions actually passed, along
with default/all-feature strict Clippy, root/private formatting and diff checks
(six gates, exit0). The next actual diagnostic scan remains unrun; this
instrumentation does not fix or reclassify Source61's failure.

Source62 read-only review found a separate unwind-order regression despite those
passing gates: extracting dispatcher fields into locals reverses the remaining
receiver/protocol destruction order if Editor drops with a Rust unwind, including
the disabled path. It must not be described as fully preserving original cleanup.
Source63 restores aggregate dispatcher destruction, brackets only the original
explicit editor.close, and adds a diagnostic on/off by Editor normal/unwind
four-case regression. Read-only review confirms the original order is restored.
Actual corrected acceptance passed all 46 helper regressions, default/all-feature
strict Clippy, root/private formatting and diff checks (six gates, exit0, all 284
pins unchanged). Evidence is in `source63-exit-*`. Original Source62 manifest and
successful gate receipts remain intact alongside the review finding. That gate
cohort did not run actual Kontakt; the native retirement problem remained
unresolved, and the works/protected files were unchanged.

Source64 ran the same exact ignored selector against committed `9795f16`, with
the existing `CITRUS_VST3_HELPER_LOAD_DIAGNOSTIC=windows-load-response-timing-v1`
opt-in and a new private adjacent helper. Default helper and actual Cargo test
artifact builds passed; all 284 input pins remained unchanged. Test PID18440
actually exited101 (outer 5.565s, zero validation errors). Helper PID25912 returned
metadata, required forced termination and was actually reaped: `kill_requested=true`,
exit1, `reaped=true`, with no load/shutdown/kill/wait/pipe error fields. Production
still stopped with `Plugin scan stopped: metadata returned but helper did not
retire cleanly; previous index retained`. The App index UI was not exercised.

The same raw log records Shutdown consumption, editor.close and aggregate
dispatcher destruction returning, component terminate returning original SDK
result0, and the plugin Drop body returning. It then records
`windows_exit_dll edge=Before` without Returned before the unchanged cleanup
deadline. Whole-slot clear, OLE and Windows run have no returned markers. This
localizes the observed stall inside ExitDll; it is not evidence of a FreeLibrary
stall or a fixed retirement path. Read-only module lifetime review found no
release-order defect explaining this attempt. Preserve
`windows-kontakt-default-scan-run-02/` and its sibling `-stderr.log`/`-terminal.json`.
No NOIRE state, MIDI/DSP, editor open, audio device, save/reopen or export was
requested. Keep the original five-second load and 250/2250ms cleanup budgets.

Source65 adds only fixed numeric PID and Windows native TID to the existing exit
prefix, obtained after the original opt-in and 128-record reservation permit
emission. SDK calls, destruction order, deadlines and privileges are unchanged.
The added pure formatting regression and all 46 existing helper tests actually
passed (47 total); default/all-feature root strict all-target Clippy, root/private
formatting and diff checks all exited0, with all 284 pins unchanged. Four existing
dependency unused-code warnings remain. The first local wrapper retained the
Source63 manifest path and failed preflight with `Source65 manifest drift` before
Cargo started; new wrapper 02 binds the correct manifest and passed. Preserve the
first failure and `source65-exit-*-02` receipts. Actual Kontakt execution of the
new identity fields and the target-only wait-chain experiment remain unrun;
the latter is neither compiled nor part of this commit. Only the logger source
and two verification documents are committed, with generated evidence and works
retained outside Git.

Source66 actually exercised the numeric identity fields on pushed `07e08de`.
Default helper/test artifact builds passed; all 284 source pins were unchanged.
Test PID15736 exited101 (outer 5.499s, zero validation errors), and the production
receipt records helper PID8828 forcibly terminated and actually reaped with exit1.
Shutdown's main trace records native TID18888, component terminate result0 and
the Drop body returning, then ExitDll Before at 563264800ns with no Returned.
The ordinary scan still stopped with UncleanRetirement. No NOIRE state, MIDI/DSP,
editor, device, project save/reopen or export was requested.

The target-only collector passed a compile-only installed SDK x64 layout check.
Its initial synthetic wrong-PID/TID callback regression actually failed; preserved
wrapper failures first exposed array-expression and reflection PSObject fixture
bugs. Revised collector03 then passed Add-Type, managed ABI and 264 checks across
20 fixtures, including null/error/partial callbacks and retained process/thread
liveness. Strict original-emitter incarnation remains unproven. These managed
results do not establish actual WCT collection.

The native collector refused with `Direct child enumeration exceeded cap 128.`;
no WCT session opened and Chains is empty. Owner completion and raw EOF were
confirmed before ending the same NoExit host, followed by a fresh zero owned
Cargo/helper/test inventory. Read-only review found that continue in the do-while
skips Process32NextW and can count the same candidate repeatedly. This is not
evidence of 128 distinct children or a deadlock. The actual candidate image path
was not recorded; an adjacent helper file precheck does not prove the child image.
A new target-only enumeration correction and regressions remain uncompiled/unrun.
Preserve `windows-kontakt-default-scan-run-03*`, source-check cohorts01..05 and
`waitchain-sdk-layout-build-01/`. Another scan needs a new bound cohort and an
explicit resource handoff; budgets, privileges and healthy-exit requirements stay
unchanged.

Source67 acquired a new explicit resource handoff and tested collector04's shared
bounded enumeration core. All 279 managed checks passed, including skip/error/
duplicate/cap regressions; default helper/test artifact builds passed and all 284
source pins were unchanged. Test PID25468 exited101 (outer 5.547s, zero validation
errors). Helper PID15912 returned metadata but never recorded ExitDll Returned
after Before at 486396700ns; production forcibly terminated and actually reaped
it with exit1. Successful collection does not reclassify the failed scan.

The actual child image path/creation and retained process/thread interval checks
matched. The 342 process records contained two direct children: PID14120 was
excluded for PathMismatch, leaving the unique bound helper PID15912. Of 4889
thread records, 19 helper threads were queried once through asynchronous WCT;
false/997 is the expected pending submission. All 19 callbacks had status0 and
returned only a singleton Thread node. The node numerically correlated with
logged TID12940 was Blocked, with no waiting object or owner supplied. Its interval
binding passed, but the marker lacks thread birth identity, so strict original
emitter incarnation remains unproven. IsCycle=false is not proof of no deadlock;
raw WaitTime is not treated as this attempt's wait duration. Actual Close return,
owner completion and raw EOF preceded ending the same NoExit host, followed by
a fresh zero owned-process inventory and explicit CPU/native handback. Preserve
`windows-kontakt-default-scan-run-04*`, `waitchain-observer-source-checks-07/` and
`source67-default-*`. No privileges, budgets or works changed, and no NOIRE sound
or full project flow was requested. Stack evidence is still needed to identify
the wait that WCT did not expose.

Source68 passed 45 offline metadata fixtures, the installed SDK x64 layout and
function-signature compile assertions, Add-Type with 25 managed capture checks,
C++ `/W4 /WX` compilation, and default helper/test artifact builds. All 284 source
pins were unchanged. An initial native preflight refused the still-open VK3D
PID8068 through the existing occupancy guard; no private bins or NI test started.
The requested inner exit2 was initially misrecorded as observed status; preserve
the original refusal and independent validation recording actual outer exit1.
The unsaved GUI was retained. Only after the peer saved, normally closed and
explicitly handed off did the single actual NI attempt run.

Test PID26516 actually exited101 (outer7.693s, zero validation errors). Helper
PID21868, logged main TID20900, returned component terminate result0 and the Drop
body, then recorded ExitDll Before at451214800ns with no Returned. It was forcibly
terminated and actually reaped with exit1; ordinary scanning still failed with
UncleanRetirement. The 20 WCT callbacks and actual Close/owner/raw EOF completed.

The external Normal|ThreadInfo MiniDumpWriteDump call actually returned true:
126110 bytes, SHA256 `f31a3fa4f8b721af5a4059494cabbab89f6748fb0880116292d11a1b3795f483`.
The original native-output handle hash matched the independently reopened file.
Fresh PID/path/creation and current numeric TID/creation checks passed; actual
collector DLL paths matched the pinned SDK pair. Read handles and collector DLLs
were released before ending the same NoExit host (actual outer exit101). Original
cleanup budgets and privileges were unchanged; capture may perturb timing.

Reader01 actually rejected the same file with exit4, `duplicate_stream`: its
14-entry directory includes four all-zero UnusedStream slots, which the parser
treated as duplicate payload streams. Preserve the original dump, failed metadata
and 45 passing old fixtures. New reader02 admits only type0/size0/RVA0 unused slots;
nonzero entries and duplicate meaningful streams remain guarded. Its regressions,
actual reparse and C++ frame reading are still unrun. No new NI capture is needed
to debug this offline format issue.

Read-only lifecycle review establishes that final Shutdown stops the host message
pump and removes its TLS dispatcher before plugin-slot clearing; OLE and the
message-only command HWND remain alive during ExitDll. Metadata loading probes
and releases an unattached createView. Host source cannot establish NI's hidden
HWNDs or wait ownership, so a message dependency remains an unproved hypothesis.
Preserve `source68-*`, the four `minidump-*-01/` gate directories and
`windows-kontakt-default-scan-run-05*` outside Git. This round requested no NOIRE
state, MIDI/DSP, editor, device, save/reopen or export and proves none of them.

The subsequent explicitly handed-off offline window passed all 52 reader02
fixtures with unchanged inputs. Re-reading the original dump returned exit2,
explicit `partial` for unknown streams and misc-info extensions/flags. PID,
thread birth, second-resolution process creation, bytes/SHA and the target AMD64
context matched. Only all-zero UnusedStream slots gained compatibility; invalid
unused slots and duplicate meaningful streams remain rejected.

The existing offline DbgEng reader PID24668 actually exited0 (0.328s), with all
20 thread selections/stack calls S_OK, no cap reached, completed pipes and no input
drift. Main TID20900 produced 32 engine frames through Kontakt, msvcp140,
KERNELBASE and ntdll. The returned symbol strings contain only module names, so
they do not establish exact functions, complete unwind, wait ownership or cause.
The same NoExit host exited0 before a fresh zero-owned check and explicit
handback. There was no recompilation, new NI capture, editor or device request.
Preserve `minidump-reader-pure-02/`, `source68-minidump-metadata-02.json` and
`source68-offline-frames-01/` alongside the original rejected reader01 result.

The subsequent source/evidence-only PE export review matched AMD64, timestamp,
image size and checksum for four current PE files against the dump.
The main-thread RVAs fall just after public exports NtWaitForAlertByThreadId,
RtlSleepConditionVariableSRW, SleepConditionVariableSRW and _Cnd_wait; the Kontakt
return frame is ExitDll+9. This supports a condition-variable wait interpretation,
not exact function membership, loaded-byte identity, wait ownership or a cause.
NI private frames remain unnamed. Preserve `source68-export-map-01.ps1` and its
JSON outside Git; no new NI, compiler, device or desktop operation was used.

An independent, default-disabled single native-message pass before final editor
retirement is prepared as a diagnostic hypothesis. Only the exact independent
`CITRUS_VST3_HELPER_SHUTDOWN_MESSAGE_PASS=windows-bounded-retirement-pass-v1`
opt-in enables it. The TLS borrow is released before callbacks; no IPC resumes,
and intact dispatcher destruction and existing cleanup deadlines are retained.
It stops on empty queue, WM_QUIT or 32 messages; callback time remains unbounded.
Source69 passed all 51 helper tests, including the four new pure regressions,
default helper/QA artifact builds, and helper-only all-feature Clippy. The remote
Windows formatting check passed; complete root all-target CI was still running
at admission. The narrow native experiment was admitted by the actual local
same-profile gates, not by an invented completed-CI result.

The sole actual test PID56664 exited101 (outer5.711s, validation errors0). Helper
PID61080/main log TID64468 executed the opt-in pass: Returned, messages0, no quit
and no count cap. Component terminate returned0, the Drop body returned, and
ExitDll Before at600477900ns again had no Returned. Original cleanup forcibly
terminated and actually reaped exit1; metadata remained UncleanRetirement and
the previous index was retained. An empty queue in this pass does not exclude a
later asynchronous message dependency. The pass is not promoted as a fix.

The external capture actually returned true:127646B, SHA256
`5948b48cd7946411ce8effdb6d4e57f0588d5fd61dc36ec613b2ee35c4affbca`.
WCT Close/owner/raw EOF and dump owner/resource closure completed before the same
NoExit session18149 explicitly exited101; fresh owned-heavy count0 preceded
explicit handback. Preserve `source69-*` and the actual/future runners outside
Git. This capture has not yet been decoded or compared with the prior stack.

The source-only implementation now provides a distinct factory-only metadata
request through the existing helper and retained transport. Its host path calls
no component/controller/view instantiation; declaration-only metadata preserves
unknown fields instead of inventing bus/MIDI/editor capabilities. It does not
activate a new production scanner or change default-class/verified semantics.
Module drop must return before metadata is sent, and checked healthy retirement
remains separate. The ignored actual QA caller retains the same cleanup owner
through Pending and requires an independently byte-bound helper/test runner.

Both remote `b37cc39` Windows jobs subsequently failed, each reporting
2247 passed/4 failed/10 ignored. Quality run38633329267 failed the Saved13 source
body byte guard, both already-faulted source-intent cases, and the run-local
observer witness-drop assertion. Preview run38633329186 instead failed the same
first three cases plus a whole-export fixture admission timeout. Later full-root
Clippy/build, final preview verification and native fixture acceptance were
skipped, not passed. Preserve `ci-b37cc39-windows-{quality,preview}-01.log` locally.
The source-body failure was reproduced by Git checkout line endings: the existing
8154-byte LF section has the expected digest; its8352-byte CRLF form matches the
remote failure. A targeted repository LF rule now preserves the frozen section
and original expected digest. The fault-injection fixture now waits for actual
source settlement Ready before injecting its exact error; this ordering repair
still needs a rerun because the old log omitted the actual outcome. Retired
test-observer snapshots now release witnesses despite a held Arc and reject late
taps. Shared App fixtures no longer apply a work deadline while queued before
owning resources; sticky failed-owner rejection and actual work/cleanup deadlines
remain. Concurrent retirement and failed-admission regressions were added.

Source70 (`2e0fcdd`) passed rustfmt, complete vendor patch replay of all74 source
files byte-for-byte and all170 packaging tests. The Windows quality workflow now
runs dependency factory/mock-child tests explicitly; root cargo test alone does
not execute dependency unit tests.

The actual [quality run38638030631](https://github.com/wrench1997/DAW/actions/runs/38638030631)
passed31 vendor factory regressions. Both that run and
[preview run38638030629](https://github.com/wrench1997/DAW/actions/runs/38638030629)
passed the complete root suite:2254 app+51 helper+13 protocol tests,2318 passed
and11 ignored. All four prior failing cases passed in each relevant suite.
Both workflows subsequently failed strict Clippy: the newer toolchain deprecates
`Atomic::fetch_update` in `plugin_runtime.rs` and `load_response_timing/mod.rs`.
Later application/helper build and smoke/native fixture steps were skipped;
the final preview archive was not verified. Actual logs and terminal run JSON
remain in `ci-2e0fcdd-windows-{quality,preview}-01.log` and
`source70-{quality,preview}-ci-actual-terminal-01.json` outside Git.

The Source71 repair uses short compare_exchange_weak loops with the same original
Acquire/AcqRel or Relaxed orderings, overflow rejection,128-record limit and lease
release behavior. It retains the pinned Rust1.97.1 release toolchain rather than
silencing warnings or adopting a newer-only API. Three boundary/concurrency
regressions were added. The actual
[quality run38641052438](https://github.com/wrench1997/DAW/actions/runs/38641052438)
passed31 dependency factory tests,2256 app+52 helper+13 protocol tests (2321 passed,
11 ignored), strict all-target Clippy, both bins, helper protocol/harness smoke,
the no-default-feature check and source-built editor fixtures. Existing vendor
replay/package inputs are unchanged by this root-only repair.

That quality run still concluded failure: native paint before/after edit returned
`PRINTWINDOW_ACCEPTANCE_FAILURE`. Target and same-process button probes both
returned false; native attach/resize/DPI, modal IPC, control/dirty feedback,
stopped-edit state, session completion and lifecycle/Shutdown/EOF/crash cleanup
passed separately. Repaint comparison was skipped because pixels were unavailable.
This cloud fixture result does not establish a Kontakt UI defect or UI acceptance.

[Preview run38641052404](https://github.com/wrench1997/DAW/actions/runs/38641052404)
instead stopped in the app suite:2255 passed/1 failed/11 ignored. The exact failure
was `startup_state_history_audio_restart_retains_nonempty_request_zero_verification`
at its `!ui.app.dirty` assertion; Clippy and final packaging were not reached.
Source review found that the fixture modified notes/plugin state after establishing
its piano baseline, then synchronized only history. A due750ms dirty check can
correctly detect the stale saved fingerprint. The old failure log did not capture
those baseline values, so this is a source-supported explanation awaiting rerun.

The test-only repair now builds the complete baseline through existing
`mark_saved()` in the device/file-free harness. It forces the real dirty-check
branch to be due, adds deterministic stale/complete baseline negative/positive
controls, and prints history/saved/current fingerprints on the original failure.
All existing restart, state-byte, dirty and Undo assertions remain. Source72
actually passed both the original restart case and the new baseline controls.
Preserve
`ci-33830ed-windows-{quality,preview}-01.log` and the corresponding
`source71-*-ci-{actual-terminal,jobs-actual-terminal}-01.json` outside Git.

The actual [Source72 quality run38643946938](https://github.com/wrench1997/DAW/actions/runs/38643946938)
passed31 dependency tests and the complete root suite:2257 app+52 helper+13
protocol,2322 passed/11 ignored. Strict Clippy, both bins, protocol/harness smoke,
no-default-feature and fixture builds passed again. Only the final native paint
before/after capture failed; the whole workflow remains failure and GUI remains
unqualified.

[Preview run38643946888](https://github.com/wrench1997/DAW/actions/runs/38643946888)
instead reported2255 passed/2 failed/11 ignored. The exact failures were
`state_save_ui_dispatched_capture_timeout_still_finishes_with_unacked_transport`
at its outstanding-admin-work assertion, and
`actual_whole_frozen_destination_review_refuses_a_real_parameter_receipt_before_any_stop_or_capture`
at reset count2 versus1. Their earlier rejection-reason/transport invariance
assertions passed; this does not identify which operation completed that reset.

The new test-only repairs address two source timing gaps. Startup's intentionally
held admin/reset calls previously returned their own fixture error after5s,
independently of the UI's capture timeout. Explicit Condvar gates now release
only by test action or fixture Drop, which releases both before retained join.
The timeout case records actual call entry/return, retains the outstanding-work
and recording fences, then proves exactly one return after explicit release
without another UI/callback pump. Observation/cleanup deadlines are unchanged.

Whole-song Review binds current callback/Project/native identity but does not
attest completion of the worker's earlier epoch reset. The fixture now confirms
the same binding's actual settlement Ready/reset-completed epoch before sampling
the rejection counters. A new held-reset regression separates callback ACK and
Review availability from worker Waiting, checks that rejecting the old Review
changes no transport/file/model/capture count, then releases the original gate
and verifies Ready. These are source-supported explanations; the old logs did
not capture gate expiry or the competing reset's completion. Source73 actually
compiled and passed both repaired modules and their new controlled regressions
in quality and preview. Product deadlines and original safety assertions are
unchanged.
Preserve `ci-87e3141-windows-{quality,preview}-01.log` and corresponding
`source72-*-ci-{actual-terminal,jobs-actual-terminal}-01.json` outside Git.

The next no-GUI factory diagnostic is admitted by actual successful source/backend
CI steps and new default helper/QA builds, with exact source/binary bindings and
an explicit resource handoff. Record the whole workflow's actual conclusion;
independent cloud editor-paint failure does not replace those step results and
cannot be relabeled as a complete quality or GUI pass.

The actual [Source73 quality run38646579485](https://github.com/wrench1997/DAW/actions/runs/38646579485)
passed31 dependency factory tests and2258 app+52 helper+13 protocol tests:
2323 passed/11 ignored. Formatting, strict Clippy, both bins, protocol/harness
smoke, no-default-feature and source-built editor fixtures passed. The workflow
still failed its final native paint capture; control/state/lifecycle/cleanup
passed separately, without establishing a Kontakt GUI pass.

[Preview run38646579522](https://github.com/wrench1997/DAW/actions/runs/38646579522)
also passed2323 root tests/11 ignored, strict Clippy and the static-CRT release
build. Final packaging then failed with `Broken package document link:
DEV_STATE.md -> docs/WINDOWS_MELODY_WORKFLOW.md`. Final ZIP smoke/upload were
skipped. Source74 adds the workflow to the required source-document list; all11
local direct guides were already included. New regressions check the actual
release document set's link closure, exact ZIP copy/extraction and missing-required-
document refusal. All175 packaging tests actually passed after refreshed vendor
pins (19.439s). A rebuilt real preview archive is still unverified.
Preserve `ci-329fe23-windows-{quality,preview}-01.log` and the corresponding
`source73-*-ci-{actual-terminal,jobs-actual-terminal}-01.json`. The earlier
preview observation remains a distinct in-progress snapshot.

In Source73's explicitly handed-off local window, the new default helper and QA
Cargo artifacts actually built exit0 with source/binary bindings and no pin
errors. The first native runner failed its DateTime ParseExact preflight before
copying binaries or launching a native test; the unchanged original is retained.
Runner02 accepts actual DateTime/DateTimeOffset values as well as ISO strings.
It started exactly one factory-only Kontakt test, PID84964, which actually
exited101 (outer diagnostic3.792s, zero validation errors). The request failed
with `ProbeFactory: Failed to load plugin: factory-only metadata:
setHostContext failed: 0x80004001`; metadata is null and the expected class is
not yet verified. This is a new setter-success prerequisite, not a failed
declaration getter or evidence about NOIRE.

Helper PID73524 naturally exited0 and was actually Reaped, without kill or
shutdown/wait/pipe errors. EOF, editor.close, aggregate dispatcher destruction,
slot clear, OLE uninitialization and Windows run completion were observed. This
proves retirement of this failed factory-only attempt, not healthy ordinary
component scanning or a source-worker Join. The persistent NoExit host received
`exit 101`, while its outer exec actually reported exit1; retain both facts.
All native/pipe owners completed and fresh heavy count0 preceded explicit
handback. No GUI, GPU, audio device, component/controller/view creation, state,
MIDI/Process or project access was requested. Preserve `source73-factory-native-run-01/`,
its sibling logs/terminal, both actual runner revisions and the default-build
receipts. Do not relabel those bindings as a later source cohort's qualification.

The next narrow repair retains the Factory3 context offer and its lifetime
through module unload, accepting only the setter's SDK `kNotImplemented` as an
optional unsupported offer. Other setter failures and required getter failures
remain errors; the generic success predicate is unchanged. The
[official Steinberg hosting implementation](https://github.com/steinbergmedia/vst3_public_sdk/blob/master/source/vst/hosting/module.cpp)
offers the context without propagating the setter result into declaration
queries. Regression coverage must distinguish optional context from required
factory/class getters, retain zero createInstance calls and verify reference/
module release order. Source74 refreshed vendor provenance and actually replayed
all74 source files byte-for-byte from the original archive.

The actual [Source74 quality run38650940387](https://github.com/wrench1997/DAW/actions/runs/38650940387)
passed32 retained factory regressions,2258 app+52 helper+13 protocol tests
(2323 passed/11 ignored), strict Clippy, both bins, protocol/harness smoke,
no-default-feature and source-built fixtures. Only final native paint failed
with the same target/same-process `PrintWindow=false`; the whole run remains
failure, with control/state/lifecycle/cleanup passing separately. Preview
run38650940397 was still running at the native diagnostic's admission.

After a new explicit resource handoff, Source74's default helper/QA artifacts
actually built exit0 (5.092s/15.074s) with source/profile/artifact bindings.
Exactly one factory-only NI test PID2400 actually exited0, wrapper0, zero
validation errors, diagnostic outer3.940s. The actual response declared Kontakt8
version8.4.1, SDK VST3.7.12, Audio Module Class CID
`5653544E694B386B6F6E74616B742038` with Instrument subcategory, plus its compatibility
class. It did not choose or instantiate a runtime default class.

Helper PID20596 naturally exited0, actual Reaped/no kill, with null shutdown,
wait and pipe errors. Actual Shutdown consumption, dispatcher/slot clear/OLE
completion and Windows run return were logged. Test and both pipe owners
completed before ending the same NoExit session46347 with actual exit0; fresh
heavy count0 at23:17:35Z preceded explicit handback. No CU/GPU/audio device,
project read, component/controller/view, preset state, MIDI or Process request
was made. This passes the factory-only declaration/retirement path, not ordinary
component retirement, NOIRE restore/audio/export or performance acceptance.
Preserve `source74-factory-native-run-01/`, its sibling raw logs/terminal,
default-build receipts, `ci-f79fd3f-windows-quality-01.log` and actual run/jobs
JSON outside Git; the frozen preparation and actual runner remain distinct.

Source74 [preview run38650940397](https://github.com/wrench1997/DAW/actions/runs/38650940397)
was later cancelled by the existing workflow concurrency on the next source push.
Before cancellation, these tests had already reported FAILED:

- `actual_whole_forced_app_drop_keeps_the_cancelled_capture_and_exact_source_until_worker_join`
- `actual_whole_review_permit_tail_zero_and_three_seconds_keep_metadata_warning_and_tight_noteoff`
- `qa_pcm_limit_is_explicit_bounded_and_does_not_change_other_observer_caps`

Four `occurrence_media` tests reported running for over60s and never completed:
`verifier_admission_drop_cancels_but_live_receipt_can_drain_same_worker`,
`verifier_assertion_unwind_releases_gate_drains_same_handle_and_preserves_panic`,
`verifier_busy_refusal_returns_the_exact_unstarted_publication` and
`verifier_cancel_capability_cannot_release_admission_or_erase_media`.
The unfinished libtest run did not print final assertion/panic details. Preserve
`ci-f79fd3f-windows-preview-01.log` and both actual terminal run/jobs JSON.
Root tests, Clippy, release build, archive verification and extracted-helper smoke
did not complete; Source73's release build cannot be attributed to this run.

Source75 implements the module catalog contract described above. Its
[quality run38654912869](https://github.com/wrench1997/DAW/actions/runs/38654912869)
passed32 factory and2277 app+52 helper+13 protocol tests (2342 passed/12 ignored),
formatting, strict Clippy, both bins, protocol/harness smoke, no-default-feature
and fixture builds. Only native paint before/after edit failed, including
same-process Button PrintWindow=false; control/gesture/dirty feedback, stopped
state round-trip, lifecycle and Shutdown/EOF/crash cleanup passed separately.
The workflow remains failure. Its preview job later completed the same2342 root
tests/12 ignored, strict Clippy and no-default-feature check before the Source76
push cancelled it during static-CRT release compilation. No completed release
build, final archive or extracted-helper smoke is recorded for that run.
Preserve `ci-46359c2-windows-{quality,preview}-01.log` and their actual terminal
run/jobs JSON. Independent source review found no acceptance-blocking catalog
defect. The new production route's actual NI result remains unrun. Source74's
success did not activate that connection or fix the older component ExitDll
stall. Strict conversion/full-owner qualification still needs actual class/bus/
MIDI evidence. The later Source77 bridge is a separate unexecuted candidate.

Source76 addresses definite test-isolation gaps found while investigating the
unfinished Source74 run. The two regular worker-identity tests and the cap test
now share App admission for their full observer lifetime. Identity cleanup
failure retains the original chain/observer and marks the same sticky failure
before releasing admission. A verifier waiter observes both local and parent
sticky failure instead of blocking forever behind a correctly retained guard.
Healthy queued owners retain their original admission and work deadlines.
Both new handoff/refusal regressions observe actual WouldBlock before proceeding.

The two whole-export failures now write bounded control-only diagnostics
directly to stderr before resuming the original panic. Their assertions,
five-second Join and ten-second work-observation limits are unchanged. The final
candidate actually compiled and passed45 targeted cases: queued16, worker
identity2, observer cap1, whole export14 and verifier12. Targeted rustfmt and diff
checks passed. Logs are `source76-fixture-targeted-02.log` and
`source76-fixture-targeted-terminal-02.json`; candidate01 predates the review
repairs and is retained separately. The [Source76 quality run38656787235](https://github.com/wrench1997/DAW/actions/runs/38656787235)
actually completed32 factory and2279 app+52 helper+13 protocol tests (2344 passed/
12 ignored), strict Clippy, both bins, protocol/harness smoke, no-default-feature
and fixture builds. Its final paint before/after capture again failed, including
same-process PrintWindow=false; attach/resize/DPI, modal IPC, control/state,
lifecycle and Shutdown/EOF/crash cleanup passed independently. The workflow
remains failure. Its full raw log is `ci-389f9af-windows-quality-01.log`, with
actual terminal run/jobs JSON. [Source76 preview38656787312](https://github.com/wrench1997/DAW/actions/runs/38656787312)
completed success: the same2344 root passes/12 ignored, strict Clippy,
no-default-feature check, static-CRT release build, final ZIP integrity and
sanitized-PATH extracted-helper smoke all passed. The archive is bound to
389f9af, not Source77. Upload was skipped by the existing configuration, so no
downloadable build attachment was published. Preserve
`ci-389f9af-windows-preview-01.log` and actual terminal run/jobs JSON.
These results do not establish the missing original CI assertion or qualify
NI restore/audio/export.

The existing GitHub Windows quality/preview workflows can run source checks away
from the local desktop. A cloud coding environment still needs the repository and
tool setup; this local chat has not moved. Keep licensed Kontakt/NOIRE, private
projects/state/audio and iCON monitoring on this machine. The next local acceptance
is the actual production catalog path, followed by one intact Pure save/cold-reopen/export
flow. Conversion state consistency/Undo and effects/general PDC remain open.

Use private new directories and explicit opt-in ignored QA only after the coordinated desktop/build window is available. Preserve the original project and identify the exact test/helper/source inputs.
Normal source checks can be reproduced independently; real NI tests are intentionally excluded:

```powershell
cargo test --locked --jobs 1 --all-features --all-targets -- --test-threads=1
cargo test --locked --jobs 1 --manifest-path vendor/vst3-host-0.9.0/Cargo.toml --all-features --all-targets -- --test-threads=1 --skip internal::module_info::tests::reads_sdk_generated_bundle_metadata
cargo clippy --locked --jobs 1 --all-features --all-targets -- -D warnings
cargo clippy --locked --jobs 1 --all-targets -- -D warnings
cargo fmt --all -- --check
```

Source33's two failures were the stale `LatencySamples:1` command assertion and
`OSError: [Errno 22] Invalid argument` while the late mock reply was written.
Source34 fixes only the fixture: bundled getters retain their exact order and late write failure
records its actual type/errno/message without claiming delivery or a closed-pipe diagnosis.
Source35's Clippy failure remains historical; source36 helper tests and both strict checks now pass.
Source34 root/filtered vendor and source35 fixture/Python retain their original execution attribution.
The source50 native attempt failed in scanning and produced no complete-song audio. Historical hardware listening and short-export results remain separately attributed; actual project load is not native restore.
Then complete one intact original-song playback/save/cold-reopen/offline flow, checking preset,
notes, parameters, routing, automation, finite PCM, duration, silence, clipping and tail.
Repeat editor open/resize/close during playback locally and verify Icon monitoring explicitly.
Keep failing conversion-state restoration separate from successful short whole export.
Qualify effects/general PDC and Felt only after the supported core flow is dependable.
Related established contracts: [offline workflow](OFFLINE_EXPORT_WORKFLOW.md),
[WAV options](WAV_EXPORT_OPTIONS.md), [plugin restore limits](PLUGIN_STATE_RESTORE_LIMITS.md),
[native editor](NATIVE_VST3_EDITORS.md) and [build/release](BUILD_AND_RELEASE.md).
