# Offline WAV export workflow contract

FILE → Export WAV uses the ordinary built-in/WAV renderer or, for eligible
projects with `experimental-fresh-helper`, the experimental single-VST whole-song
path. This document describes the ordinary renderer's fidelity and background-job
contract. The shared settings and level choices are in
[WAV export options](WAV_EXPORT_OPTIONS.md).

## Experimental VST Whole export

The Linux/Windows `experimental-fresh-helper` path requires exactly one active
VST3 Generator at the current sample rate and no other music/audio source. It
supports optional fully wet VST3 effects in one static, ordered PostFader path to
Master, plus sample-exact Master Volume/Pan/Mute automation. Stopped source state,
bases, identities and DSP metadata are captured before separate strict clones
render each stage. Cumulative native latency is removed once at the final output;
this serial compensation does not implement general graph PDC.

The current source also implements automation for the source Generator's physical
slot 0. Driven `PluginParameter` targets need stored finite normalized Project
bases, actual captured bases and an actual writable/automatable source catalog;
the clone must match that catalog and verify restored values. The compiled
Timeline supplies each raw Q128 block-start value before MIDI and DSP. This is
block-rate, not sample-accurate within 128 frames. The fixed tail holds the last
actually sent musical-block values without a tail parameter write. Separate SDK
cases now distinguish a lane authored after cold Open from one persisted by
ordinary Manual Save and consumed unchanged by a fresh App.

**Current local validation:** the rebuilt test artifact and all 64 directed cases,
including 19 new cases, passed. The genuine Q128 SDK run matched all 96,768 stereo
frames against an independent Generator FIFO32/two-FX/single-96-frame-crop oracle;
the retained genuine and audited SDK cases also passed. Fixture validation passed
9 feature unit tests, the default-factory metadata negative control, strict fixture
Clippy and the feature-bundle rebuild. At `4cffe`, Root strict Clippy/no-default/fmt
also passed.
The changed Review's real two-size WGPU check passed1/5.54s and both frames were
inspected. At `4cffe`, quality38893797179 and preview38893797217 both passed,
including the after-Open Q128 case, real eligible-session PrintWindow and preview
release/ZIP/extracted helper. Those runs do not include the added saved-lane case below.

**Saved-lane acceptance:** the separate
`actual_sdk_generator_parameter_q128_manual_save_cold_open_genuine_whole_matches_full_pcm`
passed locally in 11.81s. It authors the lane before ordinary Manual Save, proves
the complete disk/fresh-App payload, three initialization ACKs, actual catalog
Gain 0.625 and accepted opaque-state probes, then captures driven ID 0/catalog/base
and renders through the genuine default Whole factory. All 96,768 stereo frames
match the independent FIFO32/two-FX/single-96-frame-crop oracle; the original and
cold App each join all three source workers. A beat-zero value of 0.625 preserves
the acknowledged base during stopped legacy dispatch. No lane is added after cold
Open. Its legal 0.0625-beat placement ends at frame 1500: raw Q128 values are 0.625
at frame 0, 0.75 at 128, 0.5 at 256..1408 and restored base 0.625 from 1536. The
retained after-Open case remains distinct (initial 0.25, end 385, restore 512) and passed again
in 11.51s. The saved Project/WAV were retained. This is fresh-App SDK acceptance,
not an OS restart or NOIRE/physical-output qualification.

The saved-case first compile failed E0689 when the oracle lost its f32 type;
its first disk-payload comparison also failed because normal Project loading
expanded the 385-frame placement to the model's 0.0625-beat minimum. Only the new
test was corrected to explicit f32 and a legal window; payload assertions and the
model were not weakened. Production/UI are unchanged, so their 64-case/two-size
WGPU evidence is reused. All four shared-flow SDK cases and this revision's
strict Clippy/no-default/fmt passed. Saved-case code `12ae7f3` also passed
quality38897277400/preview38897277409, including SDK7 (Saved16.60s/full96,768
frames), real PrintWindow and static-CRT/ZIP/extracted-helper packaging.

The first compile failed with E0046 for missing test-backend latency/tail methods;
those methods were added before the successful rebuild. The first cursor group
had 5 passes/2 failures: f32 loop/slip beats hit the existing
`AutomationRangeCollapsed` guard, and the Tension oracle used the wrong span.
Only new tests changed to exact binary beats and a 4096-frame piecewise oracle;
all 7 cursor cases then passed. Production behavior was unchanged by these fixes.

Branches, sends, sidechains, off-path Inserts (including disabled/bypassed ones),
active partial-wet Inserts, additional sources, Effect/other-instance parameter
automation, native Channel/non-Master Mixer controls, Tempo and Swing are refused,
not omitted. The review requires explicit acceptance of a fixed tail window; it
does not detect silence or guarantee a plugin's full natural release. Stems and
complete live/offline parity remain open.

**Verified baseline:** at `820cddb`, genuine default-factory and separately audited
SDK melody workflows passed complete 96,768-frame stereo PCM comparisons through
Play, disk Save, fresh App Open and serial Whole export. Both quality and preview
passed. The later documentation revision `84ca9f6` passed quality but failed preview
in two existing panic-wait tests; its release/package steps did not run. These are
not new Generator-automation or NOIRE acceptance. See
[Windows melody workflow](WINDOWS_MELODY_WORKFLOW.md) for exact runs and remaining
local NOIRE state, GUI and device checks.

Selected MIDI-to-audio conversion retains its own source-validation and history
contract. Realtime Master Capture records the live Master result. Neither operation
substitutes for Whole offline export.

## Fidelity preflight

For the ordinary renderer, both the File menu and the rendering entry point
support Tempo and Master Volume/Pan/Mute automation and reject other enabled,
nonempty, active targets. The error identifies the lane name, stable ID, and
target, and remains visible until dismissed. Realtime Master Capture is offered
for recording the live result, not as an offline-export substitute.

Activity follows the existing automation placement convention:

- A legacy lane without any corresponding Automation Clip is global, including endpoint holding before/after its points.
- A lane with placements is active only in its unmuted half-open clip windows that overlap the exported song.
- Disabled/empty lanes and placements that are entirely outside the song do not block export. Unrelated clips cannot disable a global lane.

Tempo automation uses TempoMap. Active plug-ins and unsupported sidechains fail
the ordinary renderer's preflight; admission to the separate experimental VST
path does not remove those guards.

## Background control and destination safety

The app captures a project snapshot and owns one background export at a time. Progress is an atomic, monotonic snapshot of completed preparation/render/mix/check/encode work, rather than an elapsed-time estimate. A single-slot result channel carries the terminal outcome. Repeated clicks and a Cancel request cannot start a second renderer while the old worker is still active.

Cancellation is checked between preparations and pattern cycles, every 4096 rendering/mixing/checking/encoding frames, between final output operations, and immediately before publication. Individual media reads/resampling, allocation, tempo-map construction, sorting, and OS flush/sync calls are not interruptible; the UI remains responsive and shows the pending cancellation until the worker reaches a checkpoint.

The output is staged beside the destination. Accepted cancellation closes and removes that staging file, leaving an existing destination unchanged. An atomic state transition arbitrates cancellation against the final rename: if Cancel wins, publication is impossible; once commit wins, Cancel is disabled and the worker completes or reports a commit failure. Progress does not show 100% until publication succeeds. There is no success notification for a cancelled export.

Installing another project requests cancellation and suppresses any old-session completion/error. The old slot is retained until its worker retires. The new project cannot accumulate an additional renderer during that interval. Closing the app requests cancellation; an OS/process termination cannot guarantee staging cleanup.

## Regression coverage

Production-source tests cover unsupported automation targets, placement/legacy gating, early and ten in-pipeline cancellation checkpoints, destination preservation and staging cleanup, the final cancellation/commit boundary, monotonic progress, repeated starts, stale project-session results, worker disconnects, normal export output, and a headless egui pass through progress/cancelling/error surfaces.

The focused source-only harness can compile exporter, job state, and egui surfaces without an audio device. It is not a complete application build or a Windows GUI/audio-device acceptance test. The normal Windows all-feature test/Clippy/build gates still apply to the integrated commit.
