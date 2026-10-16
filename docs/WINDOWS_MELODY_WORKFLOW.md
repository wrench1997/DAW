# Windows melody workflow — support and verification

Updated: 2026-10-16 (Asia/Shanghai). Development branch:
`ci/windows-reliability-20261009`; this effort started from `b0d8d5c0`.
Citrus Studio remains a development DAW. The complete melody workflow is not yet qualified.
This document separates implemented behavior, actual local acceptance, failures and untested work.

## Independent local test copy (2026-10-15)

Current separate preview: `D:\DAW-Preview\Citrus-Studio-20261015-03`.
Desktop **Citrus Studio 新版测试03** runs `Start-Citrus.cmd`, not the EXE directly.
The launcher sets the existing `CITRUS_STUDIO_DATA_DIR` to this copy's `UserData`;
window/audio preferences, catalog, autosave and recordings are separate. Save
test projects under `Projects`. New saves are Project13, unreadable by01, so use
Save As on old works. Select the previously heard iCON output
**扬声器 (2- CB4ND HW 1/2)** in this fresh profile.

Actual Source114/81b2572 main/helper were copied as independent ordinary files;
Cargo cannot overwrite them. Copied helper protocol smoke exited0/reaped with
no plugin. Root did not start main GUI/audio/NI. This is same-machine MSVC debug
with this PC's installed VC/UCRT, not the static-CRT release; no plugin, library,
user work or system DLL is included. Physical devices/Kontakt settings still
share the machine. Native03 use/NOIRE acceptance remains pending. Root continues
only code/Git/cloud work while the user tests; desktop/GPU operations coordinate
with the peer.

Historical01/Source102 remains at `D:\DAW-Preview\Citrus-Studio-20261015-01`, with
its shortcut/profile/projects unchanged.02/Source111 also remains unchanged.
The user used01 and reported Playlist/PAT/SONG issues;02 contains those locally
verified repairs.03 adds Playlist/Piano/Rack linking without a new format change.
Build-Info/README/Helper-Smoke records accompany each copy.

Follow source `b2e56dc` writes format14: new Draw/Paint follows source growth,
a real trim fixes the window, and Inspector can restore Follow. Legacy placements
and exact MIDI Take windows stay fixed. Local12 focused input cases, explicit
Saved13 private-copy/payload preservation, strict Clippy/no-default/build and8
real WGPU cases passed;9 frames inspected. The original cancel/Clippy failures
and valid reused model/WAV evidence are in [Playlist transport](PLAYLIST_TRANSPORT.md).
Cloud [quality38804721761](https://github.com/wrench1997/DAW/actions/runs/38804721761)
and [preview38804721840](https://github.com/wrench1997/DAW/actions/runs/38804721840)
actually passed at `f6678ae`:2481 root tests/14ignored and all25 Follow cases,
real eligible-session PrintWindow, static-CRT release/ZIP/extracted helper.
Python355PASS/12 conditional skips reflect4 Unix and8 absent private Saved13 inputs;
no GUI skip was used in this run. Upload stayed disabled.03 does not contain Follow, and03/02 cannot read new14
saves: use Save As. The user is still testing03 and requested code-only work;
Root has not operated main GUI, soundcard or Kontakt. Full NI acceptance waits.

The first generic-base restore candidate compiled but failed existing history checks.
Ordinary Open formerly sent saved bases once through a 64-entry untagged admin
queue; rejected enqueues were not retained, yet SlotReady could publish Loaded.
The candidate retains exact project/session/endpoint/slot/control ownership,
uses existing tagged parameter acknowledgments, and withholds readiness for
nonempty maps until replay completes. Backpressure retries the retained base;
rejection or the existing 30-second inactivity budget latches an explicit retry.
Transport, save and export gates cover the Installed-before-Ready interval.
Empty maps keep the old Ready/request-zero State contract. Source89 (`48ea977`)
actually passed its eight new worker/queue cases in both cloud jobs, but each
finished with 2325 App passes, four failures and 12 ignored. Quality passed32
factory cases and preview passed344 harness cases; later helper/protocol,
Clippy, release and ZIP gates did not run.

All four failures occur on a second Undo/Redo: history restores a session-only
Unloaded display status, which the new pre-Ready gate mistakes for current-worker
initialization despite the same owner having completed ACK replay. The repair
retains exact owner Ready/Completed evidence, including empty-map SlotReady,
without admitting truly pending/failed or replacement owners. The four original
startup assertions are unchanged. Source91 (`2853db2`) actually passed all eight
generic restore cases, all11 startup history cases and all five loaded-identity
cases locally. Strict all-feature/all-target Clippy and a matching main/helper
build each exited0; formatting passed. Native07 has now used these new bins for
a successful ordinary cold-open/Whole export; same-source onset repeatability
remains untested. Preserve the Source91 per-phase stdout/stderr logs and
`source91-local-gates-01-terminal-recovery.json`.
Preserve `ci-48ea977-windows-{quality,preview}-01.log` and Source89 actual terminal
JSON. Native05/06 use earlier frozen bins, not this candidate.

Source92 (`2bb9f80`) quality has actually completed. It passed 32 factory cases,
all 4 dependency offline-MIDI cases and 2329 App + 52 helper + 13 protocol = 2394
root cases/12 ignored. All 11 startup history cases passed. Strict Clippy, both
bins, protocol/harness, fixture and no-default-feature checks also passed. The
workflow remains failure only at independent native paint before/after edit:
PrintWindow and the same-process button probes returned false. Control/gesture/
dirty feedback, stopped state round-trip, lifecycle and Shutdown/EOF/crash cleanup
passed separately. Preview also completed success: 344 harness cases, the 4 SDK
cases, the same 2394 root passes/12 ignored, strict Clippy, no-default-feature
checks, static-CRT release, final ZIP integrity and extracted sanitized-PATH
helper smoke all passed. The archive is bound to `2bb9f80`; upload was skipped.
Preserve `ci-2bb9f80-windows-{quality,preview}-01.log` and Source92 actual terminal
run/jobs JSON. These results predate SDK-B.

Earlier full-cloud backend qualification: Source87 (`769525d`) quality actually passed 32
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
| Whole-song offline WAV | Built-in sources; experimental single-VST arrangement, ordered fully-wet FX on one PostFader path, and Master Volume/Pan/Mute automation; current qualification below | Native05/06/07 ordinary GUI cold-open→10.5s PCM24 WAV completed; Native07 uses new event semantics and still has a first-note offset. Warm-path cleanup, listening and full state/automation remain open; earlier short/405-note evidence is separate, and serial fixtures are not NOIRE listening |

These paths must retain their own receipts and must not be substituted for each other.
The ordinary build explicitly refuses unsupported active-VST offline export.
Experimental VST whole export requires `experimental-fresh-helper` on Linux or Windows.
The supported VST path is one active source, optionally followed by one static,
ordered serial FX chain. Master Volume/Pan/Mute automation also works on this path;
additional sources, Tempo, non-Master and plug-in parameter automation, unsupported
graphs and active partial-wet Inserts are rejected rather than omitted.
A fixed tail window requires explicit review; unknown/infinite tail retains its
warning. General FX routing, graph PDC and full natural release remain open.

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

## Stereo Effect helper foundation (2026-10-16)

The existing strict Client now accepts borrowed, genuine stereo Q128 PCM through
`load_stereo_effect` → Offline/state/bases/stereo selection → `admit_stereo_effect`
→ Start → `process_stereo_effect`. Its private role binding comes from the actual
Load reply: exact class/vendor/name/version/category, active main stereo input
and output, and no event buses. State/parameter/configuration mutations invalidate
admission. Actual latency, tail, async-start and native-edit revisions are checked
at admission and across DSP. Empty input fails; explicit zero input drains tails;
finite internal samples above full scale are preserved. Existing generator APIs
retain their isolation and original entry contracts. Every stage uses the same
bounded child owner, with the process-wide reservation retained until actual reap.

Before the role repair, 169 local helper/codec tests passed, including 26 Effect/encoding
cases; the expanded mutation test also passed separately. Four source-built SDK
tests and the real production-helper target passed. The independent no-GUI Effect
implements `y[n] = gain × (x[n−32] + 0.5×x[n−160])`: all 1280 stereo samples and 35
transport/activation/output-clear observations matched, latency was 32, the final
response was frame 415 and frame 416 onward was silent. Restore/readback and natural
Shutdown/exit/reap passed. This uses no installed plugin or audio device. Reproduce
with the repository's `tests/fixtures/vst3-effect/README.md` instructions.

Independent review then found that generic Load could clear an Effect binding.
The repair makes that role sticky until disposal/reap: both reload entry points
refuse before a second Load is sent, while legacy unbound clients retain reload.
One new real-child regression covers both entry points before/after Start,
post-refusal operations, Pending/Busy/reap ownership and the legacy reload control.
This final case was not rerun locally; both Windows and Linux actually passed the
complete 27-case `effect_` gate at `0c4872c`, including the new reload regression.

The fixture's strict all-target Clippy passed. A separate vendor-wide test-target
Clippy run failed with 75 existing SDK-glue lints; its actual exit 101/log is retained in
`target/backend-reliability-20261014/strict-effect-input-source-candidate-01/`.
Initial fixture compile/type and loop-style errors were corrected and rerun.
At exact `0c4872c42516a1d6cf7be7f421b1dbb20d7a5589`,
[quality38813369205](https://github.com/wrench1997/DAW/actions/runs/38813369205)
actually completed SUCCESS. Each platform passed 27 Effect tests, 4 real SDK tests
and 1 production-helper host test, including the full response/transport/state and
natural-reap checks above. Windows also passed 2481 root tests/14 ignored, factory 33,
offline MIDI 4, async readiness 15, established strict Clippy, bins and no-default
checks. Its interactive session 2 was eligible: real PrintWindow paints/repaint and
native lifecycle passed, rather than being skipped. Summary HTML was not separately
read; the actual harness/log receipts are retained.

[Preview38813369197](https://github.com/wrench1997/DAW/actions/runs/38813369197)
also actually completed SUCCESS: 368 Python cases = 356 passed/12 conditional skips
(4 Unix-only and 8 private Saved13 inputs absent from Git), 27 Effect tests and the
same 2481 root/14 ignored; strict Clippy/no-default, optimized static-CRT release
(8m 18s), exact ZIP/PE/import/provenance checks and extracted sanitized-PATH helper
smoke passed. Upload remained false. The real SDK target runs in quality with a
10-minute outer limit; it is not a preview step. Full metadata/logs are saved under
`target/backend-reliability-20261014/source118-{quality,preview}-*`.

### Stopped Effect source capture and exact bases — published b73

At `b73b57abb20bb0299bfb9526fa62cd834c8e4f24`, private capture obtains an entire
ordered Effect chain for one stopped mixer endpoint through its actual source
backend. The receipt binds Project/session/endpoint, each slot's placement and
enabled/bypass/wet, actual Fx identity, full audio/event layout, opaque state,
bounded catalog, writable f64 bases and DSP latency/tail. A refused slot refuses
the chain; clone admission never substitutes for source capture.

Actual Stop/Timeline and worker settlement, callback consumed/terminal/submitted
evidence and admin-intent/reset history fence the source binding. A sampled
playback token alone is not a lease. The non-clearable `epoch_resets` ticket
rejects E→E+1→E reset ABA. Effect-only sticky counter exhaustion preserves legacy
Save/Instrument and epoch entry points. `Client::set_parameter_f64` preserves
transport precision while the old f32 entry retains `f64::from(f32)` semantics.
Restore opaque state, apply every captured base, verify exact readback, then
select/admit/start; either setter invalidates the prior Effect receipt.

[Quality38824285643](https://github.com/wrench1997/DAW/actions/runs/38824285643)
actually completed **SUCCESS** at 2026-10-15T23:54:32Z. 2497 root tests/15 ignored, the
real-worker near-MAX regression, strict Clippy, bins and no-default checks passed.
Windows and Linux each passed 29 Effect cases, 4 source-built SDK tests and 1 real
production-helper test. The helper controller base 0.75000004 was read back with
exact f64 bits; its deliberately f32 DSP gain was modeled independently. All 1280
stereo samples, 35 probes, latency32/tap160, silence from frame416 and natural reap matched.
Windows also explicitly ran the ordinarily ignored App source SDK target: actual
source Gain 0.12345678912345678 (bits `3fbf9add37c1215e`), source positive-frame DSP count0,
original guard joined, opaque40 bytes, exact restored bases and stereo3×Q128, latency32/
tap160 and natural strict child exit0/reap passed. Eligible session 2 performed real
PrintWindow before/after/repaint and native lifecycle checks, all PASS, not SKIPPED.
This accepts the f64 delta; the older Gain 0.375 local SDK snapshot is only earlier
evidence. The separate vendor-wide 75-glue-lint failure above remains unresolved.

The preceding `689e0af` quality38821625393/preview38821625349 failures remain:
Windows 29 Effect and 2496 root/15 ignored passed, but new `fetch_update` deprecation
failed Root Clippy before Windows source SDK/PrintWindow or preview release/ZIP.
Preview had 369 Python = 357 passed/12 conditional skips. The b73 checked weak-CAS
repair retains old toolchains, AcqRel/Acquire and legacy admin behavior; its new
near-MAX case passed in quality without lint suppression.
[b73 preview38824285597](https://github.com/wrench1997/DAW/actions/runs/38824285597)
(job 116727965422) actually completed **SUCCESS**, updated2026-10-16T00:04:46Z.
The static-CRT source/archive step and ZIP/extracted-helper sanitized-PATH step
both actually succeeded. Upload was requested false, so its upload step was SKIPPED.
Actual receipts are in `target/backend-reliability-20261014/source120-quality-acceptance.txt`
and the source120 integration record. The b73 checkout excludes the following
Whole integration candidate and new Instrument fixture.

### SDK melody: Play, disk Save, cold App Open and Whole WAV (2026-10-16)

Two opt-in Windows regressions now connect previously separate checks through
ordinary App catalog/default-class load, the actual no-device audio callback,
Play/Stop, manual Save to a new disk project, original App destruction and source
worker Join, then a fresh App's `Project::load`/`finish_open_project` and Whole
export. Cold Open does not copy the old runtime registry, prepared chain or an
explicit Instrument class override. It checks notes, placement, routing, Master
automation, actual opaque restoration and acknowledged generic parameter replay.
The joint exact first passed locally on attempt 05 in 7.94 s, actual exit 0;
code `bdf6901418f89fe5d0f4ac763a498c432156dacb` subsequently passed both cloud
workflows, including the explicit joint SDK run in 12.64 s. Earlier failed
executions remain recorded below.

The source-built fixture's optional `instrument-first` feature makes its existing
Instrument the default class in a separate `CitrusLookaheadMIDIFixture.vst3`
bundle. The old default Effect bundle, two class UIDs, CE01 state and DSP remain.
Feature-only read-only probes 1007/1008 record accepted SDK `setState` calls and
the last accepted gain. A warm baseline, generic Gain change to 0.625, and cold
accepted state Gain 0.625 distinguish opaque disk restoration from successful
generic replay alone. Generic setters cannot write these probes; they are not
saved as writable bases. Project f32 bases and private exact-f64 capture retain
their different precision contracts.

The authored project is one ordinary 4-beat bar at 120 BPM/48 kHz: 96,000 music
frames, 768 explicitly selected tail frames, and 96 cumulative native latency
frames. The final WAV expectation is 96,768 stereo frames; Q128 render input is 96,896
frames before the single latency crop. The independent complete PCM expectation
includes relative Master ramps/mute, first onset, silent regions and finite tail.
Audible actual callback output and still-playing transport are required before
manual Stop. This is a source-built SDK/new-App-instance test, not an OS program
restart, physical audio listening or NOIRE tone recovery.

The two exacts share the full authored workflow but select different render paths:

- `actual_sdk_catalog_play_stop_manual_save_cold_open_genuine_whole_factory_matches_pcm`
  installs neither renderer starter nor a backend factory override. Before every
  Whole tick it asserts both starters are None, covering their actual selection
  before `take()`, and observes real CapturingEffects with the retained serial
  Generator plan. Normal production dispatch selects `start_master_song_serial`
  with real `StrictBackend::new` / `StrictEffectBackend::new(None)`. Typed Complete
  follows renderer termination ACK and admission drain. Its marker labels
  `render_factory=default_production`,
  `genuine_whole_production_factory_selected=true`, `strict_child_pid_audit=false`.
- The retained `actual_sdk_catalog_play_stop_manual_save_cold_open_whole_master_matches_pcm`
  uses the closed read-only audit wrappers and the same
  `RenderAdmission.start_serial_with_backends` owner. Its marker remains
  `render_factory=closed_read_only_audit_wrapper` and
  `genuine_whole_production_factory_selected=false`. This case directly observes
  all three strict clones naturally exit0/reap in order with maximum live owner1
  and both actual Effect Offline probes1005=1. Those detailed child observations
  are not independently collected by the genuine case.

Both cases require the same complete independent96,768-frame PCM, unchanged
project/saved-file bytes, original three source joins before cold Open and three
healthy cold joins at final cleanup. In the new slice, first local executions
passed: genuine1/8.69s and retained audit1/8.47s. Actual Cargo artifact, strict
all-feature/all-target Clippy, no-default all-target check and fmt all exited0;
all six phases actually terminated/reaped without a resource guard. No production
code, protocol, fixture or deadline changed. The existing CI step explicitly runs
both staged exacts with immediate failure propagation and its original10-minute
limit. Existing CI-contract7 tests and the actual PowerShell step parse passed.
New-candidate cloud acceptance is pending. Full logs/terminals and both actual
WAV/project copies are under
`target/backend-reliability-20261014/melody-genuine-whole-source-candidate-01/`;
Root record: `source130-genuine-factory-direction-01.json`.

**Failures and necessary test-fixture repairs:**

- Exact 01 exited 101 at normal load: the old test callback queue of 64 could not
  hold ordinary 32-track synchronization's 130 commands. Only the new workflow
  selects the production's existing 1024 capacity through a test-only constructor;
  old tests keep 64 and production capacity is unchanged.
- Exact 02 exited 101 after 31.80 s: the Effect fixture rejected legitimate Realtime
  source processing with `Plugin process() failed: 0x1`. Its former Offline-only
  guard now admits both known modes only when setup/data agree. A real SDK unit
  checks cross-Q128 FIR PCM and that both mismatch directions/unknown mode reject
  without advancing history. Production processing is unchanged.
- Exact 03 exited 101 after 1.13 s: transport stopped before the required audible
  frames, without a plug-in processing fault. Source inspection shows that normal
  SONG enables looping and a worker-backed loop wrap deliberately pauses playback
  pending a prepared new epoch. The former 6000-frame music was also shorter than
  the serial source path's adapter accumulation/lookahead plus native latency
  (`3 × (18 + 1) × 128 + 96 = 7392`). The bare failed assertion does not distinguish
  a transient restart wait from a cancelled restart. The fixture now authors a
  normal one-bar melody. [Existing timing limits](PLUGIN_TIMING.md#retained-limits)
  already allow short loops to remain silent during preroll; this is not evidence
  of a new nonloop end/drain regression. Seamless short-loop playback remains
  independent work. Intermediate half-beat artifact 04 was only compiled, never
  executed.
- Exact 04 exited 101 after 1.59 s: audible Play/still-playing, manual Stop/Save,
  original App's three source joins, fresh disk Open, three matched generic ACKs
  and actual accepted opaque Gain 0.625 passed before Whole refused
  `Effect source catalog contains an invalid normalized value`. The failed
  literal ID/value was not recorded. Source inspection establishes that valid
  stopped Insert PDC can produce negative transport and the fixture previously
  exposed unbounded ratios as normalized parameters. Scaled read-only probes
  1000/1001/1002/1003/1007 now saturate to [0,1], retaining raw context/counters,
  DSP/state and all old in-range positive values. The new actual-SDK boundary unit
  passed in both fixture builds; it cannot reconstruct the earlier failure
  literal. Production catalog validation remains strict. The one-bar artifact 05
  also had a preserved E0308 usize/u64 compile failure; explicit report conversion
  compiled successfully as artifact 06.

All failed processes and held source helpers actually terminated/reaped; exact 02's
secondary cleanup fault assertion is retained. The current source uses the same
original 30-second source wait and the job's original 300-second render deadline,
not an extended budget. The final normalized-probe fixture passed 8 default/9 feature tests,
both strict Clippy modes and both rebuilt real bundles. `melody-exact-05` actually
passed all 96,768 stereo PCM frames, first nonzero frame 1, mute/silence, 768-frame
fixed tail and one 96-frame latency crop. All three original workers joined before
cold Open; all three cold workers joined at final cleanup. All three strict children
naturally exited 0/reaped, maximum live strict owner 1, and both Effect mode probes
returned 1005=1. The actual saved project and WAV were copied from their owned
temporary output into the evidence directory's `accepted-app-exact05-output/`.
Both retained Whole SDK cases and private Effect SDK passed with the new actual
Offline probe. Eight ordinary initialization cases, one manual Save case and one
catalog Save/Open case also passed: 31 Rust test executions/23 distinct tests,
plus 11 fixture-builder
mock cases. Root all-feature/all-target strict Clippy and no-default all-target
check passed. The first Root Clippy exit 101 on the new oracle's
`needless_range_loop` is retained; only indexed iteration became
`iter_mut().enumerate()`, preserving sizes, index order, arithmetic and assertions.
The valid PCM run was reused, not rerun for reassurance. Production matching
bins/helper reuse their actual successful build because later edits affect only
test code or the independent fixture. All owned native phases actually
terminated/reaped without a resource guard firing. Exact cloud qualification is
recorded below.

The existing 10-minute no-GUI quality step retains every old target/failure gate,
adds both feature units and 11 fixture-builder cases on Windows/Linux, and
explicitly executes both joint ignored App exacts on Windows. It stages the actual
Cargo JSON-reported test artifact and matching helper as private siblings;
ordinary helper lookup is unchanged. Missing inputs and real test failures fail
CI. See the [source-only fixture commands](https://github.com/wrench1997/DAW/blob/3b79f14250ef4900e7d50599d998003f00f91942/tests/fixtures/vst3-effect/README.md#disk-save-and-cold-app-open-with-the-default-instrument).
Local full logs, frozen source and actual terminals are under
`target/backend-reliability-20261014/melody-save-cold-open-source-candidate-01/`;
Root integration record is `source127-first-flow-direction-01.json` in its parent.
User test03 remains in use and unchanged; no main GUI, NI or audio device was
operated for this slice. Native04's NOIRE warm Save cleanup failure, full NOIRE
state recovery and hardware listening still require their own acceptance.

The first cloud preview at `3b79f14` failed its document-closure check:
`Broken package document link` pointed from this shipped guide to the unshipped
fixture README. Python ran 372 cases: 359 passed, one error, 12 conditional skips;
Rust/release/ZIP steps did not run. The guide now links the exact GitHub source
instead of expanding the release whitelist. The existing document-closure
regression passed locally: one test in 4.959 s. Linux's same-SHA fixture
8/9, builder11 and real helper2 passed. Windows quality also failed, updated
06:34:05Z: App 2485 passed/one failed/18 ignored in 526.68 s. The retained
`effect_admin_ticket_exhaustion_is_sticky_without_blocking_legacy_admin_or_instrument_capture`
failed its receipt-current assertion at `plugin_effect_capture_tests.rs:1043`.
The Windows joint SDK, Clippy/build and PrintWindow steps did not run.
Source inspection established a test-contract gap: capture result/termination ACK
can precede release of the enclosing admin lease, and unchanged periodic metadata
can temporarily make settlement Waiting. The captured cloud run did not log its
actual settlement operands. The test-only repair waits only Waiting→Ready under
the original capture's absolute five-second deadline; Invalid or expiry fails
with actual operand diagnostics. A new real-worker held-metadata case requires
ACK/current=false/Waiting, then Ready after actual release. Failed-path editor
diagnostics use nonblocking `try_lock`. Production, Ready rules, ownership and
all sticky/MAX, legacy Save and Instrument assertions are unchanged. The affected
module actually passed all 11 cases with eight test threads in 0.74 s, including
the original near-MAX case and the new held-metadata counterexample. Root strict
Clippy, no-default and formatting also passed. All owned processes actually
terminated/reaped. The existing local SDK PCM evidence remains valid because this
correction changes only test code. Both complete
original logs and terminal metadata are retained as
`source127-*-actual-terminal-01.*`; the narrow repair is in
`effect-admin-receipt-settlement-repair-01/` under the same evidence parent.

**Cloud acceptance passed for `bdf6901418f89fe5d0f4ac763a498c432156dacb`.**
[Quality38863519248](https://github.com/wrench1997/DAW/actions/runs/38863519248)
completed SUCCESS on attempt 1, updated 2026-10-16T07:12:11Z. Windows
job116851010093 passed 2552 root cases/18 ignored (App2487 + helper52 +6+5+2),
including the original near-MAX assertion and new deterministic ACK≠Ready case.
Strict Clippy, production bins and no-default passed. Fixture8/9, builder11,
Effect/Instrument helper2 and all four explicit App SDK targets actually ran:
private Effect source, static Whole, Master-automated Whole and the new joint
Save/cold-Open/Whole. The joint target passed all 96,768 stereo frames in 12.64 s,
with actual saved opaque acceptance/generic ACKs, original/cold source joins,
three natural strict reaps and both actual Offline probes. The two retained
Whole targets also preserved exact-f64 bases and their full 1280-frame oracles.

The Windows runner had a usable visible Default/session2/WTSActive desktop.
Real PrintWindow paint before/after, repaint, native interaction/state/lifecycle
and Shutdown/EOF/crash cleanup all passed; this was not an environment SKIP.
Linux job116851010395 newly passed async15/Effect29/Unix1, fixture8/9,
builder11 and both real helper targets at this SHA.

[Preview38863519233](https://github.com/wrench1997/DAW/actions/runs/38863519233),
job116851010207, completed SUCCESS on attempt 1, updated 07:18:13Z. Document
closure passed; Python ran 372 cases in 21.941 s: 360 passed and 12 skipped
(four Unix-only and eight unavailable private Saved13 input cases).
Root2552/18 ignored, strict Clippy/no-default and the MIDI/async/Effect
gates passed. Static-CRT release took 7m53s; exact ZIP/PE/import/provenance and
the extracted sanitized-PATH helper passed, including three JSON replies,
invalid-command recovery, Shutdown with stdin open and natural exit0/reap.
Upload was requested false, so only artifact upload was SKIPPED; user03 was not
replaced. Full raw logs and actual API terminals are retained under
`source128-*-actual-terminal-01.*` in the evidence parent. The original `3b79f14`
failures remain separate; no assertion, timeout or release whitelist was relaxed.

### Master automation and source-checked publication (2026-10-16)

The current serial Whole source reuses the existing `OfflineMasterControls`
evaluator for Master Volume, Pan and Mute, including canonical aliases, placement
and slip, chase at frame0 and overlapping automation. After the single cumulative
DSP-latency crop, retained output frame0 is the automation clock. The reviewed
tail holds the controls at the last music frame, N−1; an event at N does not
start a new tail value. Driven controls replace their saved base, rather than
multiplying it twice. Master mute automation can unmute a saved-muted Master;
Channel/non-Master mute and solo/path gates remain effective. Undriven volume,
driven controls and undriven pan preserve the existing sample×volume×pan order,
followed by one Master tanh and the selected export level policy.

The full original Project remains capture/publication authority. This does not
enable plug-in parameter or non-Master automation, Tempo changes, parallel routes,
partial wet or arbitrary sources. The original 300s deadline, 24M raw-frame cap,
600s music/30s selected-tail limits, one strict child at a time and natural-reap
requirements remain.

A new regression exposed a real false-stale condition: normal empty admin polling
and read-only metadata work can temporarily make an unchanged source `Waiting`
and its Prepared token absent. The first affected App run passed10/failed1 with
`Captured Insert source work or graph witness changed during Whole render`.
The exact diagnostic rerun passed1 before the repair; that rerun is not a fix,
and the original failure did not identify its precise settlement enum.

Generator and Insert checks now distinguish transient Waiting from actual
invalidation. At the existing checked writer's final commit boundary, only App
serial Whole requests one fresh source check. App must see the complete current
Project/session/native/identity/ticket/endpoint/placement authority and the exact
Prepared token Ready, then immediately perform the original RUNNING→COMMITTING
CAS. Waiting grants nothing; changed authority fails. Writer timeout/unwind retires
the one-use request under the same lock as the host CAS. Cancel/AppDrop wakes it
without another App tick. If CAS already won, its canonical state survives a lost
notification or host unwind and the original rename finishes. The default ungated
writer, direct SDK, Instrument and Occurrence entries retain their old contracts.

**Local qualification passed:** plan12/render8, the new boundary7 and the
complete affected App19 have actually passed. Both real Whole SDK targets passed:
the retained static oracle and the new Master-automation oracle. The latter checks
all1280 retained stereo frames against an independent Generator/two-Effect
convolution, total latency96, initial muted/zero Master→unmute, aliases, mute
interval and tail hold at music frame255. All three originals have source DSP0,
state40 and actually joined guards; all three strict stages naturally exit0/reap,
with max live owner1 and exact non-f32 Effect bases. Existing Master14/no-FX18
compatibility cases passed. Root inspected the actual Whole Review at logical
1440×900 and1080×680: support/limits, fixed-tail warning, consent and all action
buttons are readable and unclipped. This uses the actual no-window Vulkan App
renderer, not a native PrintWindow substitute. Strict all-feature/all-target
Clippy, clean no-default all-target check and production all-feature dual-bin
build actually exited0. No-default01 had passed with a new unused installer
warning; the installer cfg was narrowed to test or the existing strict
platform/feature caller, preserving identical effective all-feature code.
No-default02 then passed without that warning. Every phase actually exited and
its held processes were reaped, with no resource guard triggered. Root consumed
the actual handback and a fresh empty heavy-process inventory at03:41:35Z, then
returned resources to the other project.

**Cloud acceptance passed for `89ef2cb32a7b722c16c3672ef6f53a67305cf68f`.**
[Quality38846965192](https://github.com/wrench1997/DAW/actions/runs/38846965192)
completed SUCCESS on its first attempt, updated2026-10-16T03:56:10Z. Windows
job116799572267 passed2551 root cases/17 ignored (App2486 + helper52 +6+5+2),
including all19 serial App cases and all7 commit-boundary cases. Strict Clippy,
production bins, no-default and fixture6/real host2 passed. The existing private
App source SDK1 and both old-static/new-Master Whole SDK targets actually ran
explicitly and passed, separate from their ordinary-suite ignored entries. The
new Master target checked the full1280-frame independent PCM oracle described
above. Original source Join, exact f64 bases, strict natural exit/reap and one
crop/tanh all retained their actual receipts.

The usable Windows environment was Default/session2/WTSActive with a visible
station. Real PrintWindow paint before/after, repaint, interaction, state and
lifecycle/Shutdown/EOF/crash cleanup passed; this was not an environment SKIP.
Linux job116799572417 newly executed async15/Effect29/Unix1, fixture6 and both
real Effect/Instrument host targets successfully at this SHA; no earlier run was
substituted.

[Preview38846965091](https://github.com/wrench1997/DAW/actions/runs/38846965091),
job116799572116, completed SUCCESS on its first attempt, updated04:13:15Z. Python
371 =359 passed/12 conditional skips in22.560s; root2551/17 ignored (App2486 in
514.33s), MIDI4/async15/Effect29, strict Clippy and no-default passed. The
static-CRT release completed in8m41s; exact ZIP/PE/import/provenance/whitelist and
the extracted sanitized-PATH helper passed, including three JSON replies,
invalid-command recovery, Shutdown with stdin open, natural exit0 and reap.
Upload was requested false, so only artifact upload was SKIPPED. No new user
binary copy was delivered.

Independent implementation review found no reachable P1/P2. A nonblocking
diagnostic race remains: host deadline refusal followed by App cancellation can
report writer `kind=Deadline` or `Cancelled`; App keeps the original deadline text
and Failed terminal state, with unchanged abort/destination/Join semantics.
Receipts, full original failure and later executions are retained under
`target/backend-reliability-20261014/whole-serial-master-automation-source-candidate-01/`
and `source125-master-integration-progress-01.json`. Cloud full raw logs and
actual terminal/API records use the same parent's `source125-quality-*` and
`source125-preview-*` prefixes. No ordinary GUI, soundcard or
NI was operated; user test03 and its projects were not replaced.

### Serial Whole FX: static single-source path (2026-10-16)

Accepted code `b9f27091fbf5dd91e70ee318dcd897434fb5f6cf` admits one Generator
with a unique PostFader main serial path
through Master and its ordered FX. Active FX must be fully wet; inactive/bypassed
slots skip DSP. Branches, sends, sidechains, off-path FX, additional sources,
active automation on the FX path and partial wet are rejected early. The original
no-FX Master automation, Instrument and Occurrence contracts remain. This implements a
narrow renderer path; general routing, FX automation and graph PDC remain open.

The production renderer reuses two bounded Vecs (24M raw-frame cap), preserving
Generator lead-in, exact NoteOff, tail and Q128 padding through every PCM stage.
Strict raw PCM retains finite over-full-scale values. Whole follows existing
realtime endpoint semantics: it clamps PCM samples to [-8,8] at Channel/Effect
endpoints, including endpoints whose slots are all inactive/bypassed, then applies
the exact sequential controls. It sums actual DSP latency, crops only once at the
end, applies one final Master tanh and uses the original single final-CAS publication.
No production spool or intermediate music WAVs are added. A strict stage must
naturally exit and actually reap/destruct before the next stage acquires the
process-wide reservation. Source Generator/endpoint captures and all earlier
receipts stay revalidated while later captures are pending.

Healthy user Cancel drains the same typed Effect acknowledgment and clone cleanup,
retaining the same live sources and Project/history/MIDI. Engine loss/AppDrop or
already-retired cancellation instead retains participating original guards until
actual Join. Independent review found one P2: a pending Effect could renew the
source budget. The repair uses the earlier of the original 300s deadline and capture
fence, forbids subsequent dispatch after expiry, and drains pending cancel through
the original receiver. An actual held 100ms App boundary case passed as Failed,
not user Cancelled.

The pre-lint `source-native-01` snapshot passed 31 new ordinary cases plus the
explicit Whole SDK1, and 150 adjacent compatibility cases (Effect worker10/actor6,
Instrument29, stopped10, runtime settlement19, renderer22, Master18, old Whole36).
The initial three compile errors and one CLI configuration error remain recorded. Serial
render initially passed5/failed1 because Channel1.5 was correctly refused by the
existing Timeline. The fixture was corrected to legal Channel1 with a real 12→8
endpoint-clamp assertion; only that exact case reran and passed. The unchanged 5
results remain valid; the full 6 were not rerun.

The real Whole SDK uses the source-built 32-sample FIFO Instrument plus two DelayFX.
All three original sources had positive-frame DSP0, state40 and actual joined guards.
Three production strict stages naturally exited0 and reaped/destructed before the
next, with max live owner1. Non-f32 FX bases `3fe9febc5d7f36d1` and `3fed8cf54091bdc5`,
total latency96, reported tail352, raw1408 and retained1280 (music256 + reviewed
fixed tail1024) matched. Independent full-WAV convolution checked cross-Q128 taps,
over-full-scale samples, exact endpoint controls and one final crop/tanh. This is
actual serial-latency evidence, not NOIRE listening or a complete natural-tail claim.

Root's new Instrument class `43545253454658440000000000000002` uses no audio input,
stereo output, 16-channel main events and a physical 32-sample FIFO; default Effect
class/DSP stays unchanged. Local fixture 6 unit tests, strict fixture Clippy, release,
matching helper and 2 real host targets passed. Both host targets naturally reaped;
the Instrument target checked 768 samples/state40/latency32/tail32. These asset checks
are distinct from the Whole SDK oracle. Linux fixture/host execution at `8d8e94d` is recorded
below; Windows App Whole SDK was not reached. The same 10-minute no-GUI quality step source-wires the Windows exact
Whole SDK target and both-platform Instrument host target; 6 CI contracts passed.
No test DLL enters the release payload and no installed NI/plugin/device is used.

Root Clippy01 actually exited101 with 13 local style findings; equivalent repairs
are preserved separately from the tested `source-native-01` snapshot. The final
21-source candidate then passed strict all-feature/all-target Clippy02, no-default
all-targets02 and production all-feature bins build01 (all actual exit0). No-default01
had exited0 with an unused new test-hook warning; after that phase, its type/field/init
cfg was narrowed to the implementation's supported target+feature. All-feature
compilation stayed unchanged; no-default02 had no App warning.

Exact Whole SDK02 actually passed 1 case in 0.71s, rechecking the Q128 iterator rewrite:
three original source guards actually joined and all three strict stages naturally
exited0/reaped, with the same latency/tail/PCM oracle above. The 31 ordinary and 150
compatibility results reuse unchanged `source-native-01` inputs; those groups were
not rerun. All held handles reached actual terminal/reap, guard=false. SDK02 peaked
at 4.409GB private/26.380GB physical used; Root's fresh heavy-process check was empty
at 2026-10-16T00:22:36Z and resources were actually handed back to the peer.

Published candidate `8d8e94d1abb253e324fb92ac067987910b039a6f` has **FAILED** cloud
acceptance. [Quality38829430252](https://github.com/wrench1997/DAW/actions/runs/38829430252)
completed failure, updated 2026-10-16T00:40:35Z. Windows job 116744506979 reported
2462 App tests passed/1 failed/16 ignored in 450.01s, not an all-target aggregate. Its sole
failure was `actual_whole_serial_healthy_cancel_drains_effect_ack_without_destroying_any_source`
at `src/app/clip_render_job_tests.rs:726:17`: `Actual join observation was incomplete`.
Subsequent Windows Clippy, builds, no-default, real SDK and explicit App source/Whole
SDK targets, harness/native checks and PrintWindow were not reached. PrintWindow
was neither an environment SKIP nor PASS; ordinary-suite SDK ignores are not executions.

Linux job 116744507455 completed SUCCESS: async 15, Effect 29, Unix isolation 1, fixture 6,
real Effect host 1 and lookahead Instrument host 1 passed. The latter actually checked
768 stereo samples, physical 32-sample FIFO, state40, latency32/tail32 and natural
cleanup. This accepts the Linux fixture/host slice, not the Windows App Whole oracle.

[Preview38829430476](https://github.com/wrench1997/DAW/actions/runs/38829430476),
job 116744507983, completed failure, updated 2026-10-16T00:43:17Z. Python 371 = 359 passed/
12 conditional skips in 23.113s and the existing Effect 29 passed. App then reported
2445 passed/18 failed/16 ignored in 498.29s. The first new AppDrop case,
`actual_whole_serial_app_drop_keeps_all_source_guards_until_effect_ack_and_actual_join`,
failed at the same hook with incomplete join observation. The following 17 failures
reported `prior App fixture cleanup is unproven`; retained sticky ownership refused
new fixtures. Release/archive, ZIP and extracted-helper checks did not execute.

Source review established a test-fixture defect: new Serial fixtures used the
process-global `ACTIVE` collector, with no source-owner filter, four lifetime
worker-identity slots and sticky failure from foreign false hooks. Both 8d raw logs
omitted the actual `snapshot.failure` literal, so their precise first failure enum
cannot be reconstructed. The first repair was source-only and attempted to retain
three original controls with `.clone()`. That plan was not compilable:
`PluginChainControl` owns exclusive Consumers and does not implement Clone.

At `eae18b72512ffbfdd98141a940b8ef2e236b416c`,
[quality38832663475](https://github.com/wrench1997/DAW/actions/runs/38832663475)
actually failed (updated 2026-10-16T01:08:05Z), Windows job 116754709863.
[Preview38832663364](https://github.com/wrench1997/DAW/actions/runs/38832663364),
job 116754708957, also failed (updated 2026-10-16T01:08:15Z). Both reported E0599 at
`src/app/clip_render_job_tests.rs:791:42` and `:796:50`: no `clone` method on
`PluginChainControl`. None of the eight new App cases or old observer regressions
ran; later Clippy/build/SDK/no-default/native gates were not reached. PrintWindow
was not reached, not an environment SKIP. Preview had passed 371 Python = 359 passed/
12 conditional skips in 24.295s, offline MIDI4, async15 and Effect29; release/ZIP/
extracted-helper checks did not execute. Linux job 116754709658 actually succeeded:
async15/Effect29/Unix1, fixture6 and both real Effect/Instrument host targets passed,
including the Instrument 768-sample/32-FIFO/state40/natural-cleanup oracle.

The corrected implementation changes `src/plugin_runtime.rs` and the clip fixture.
It retains the existing `PluginControlIdentity(Arc<BridgeMetrics>)` returned by each
original control's `owner_identity()`, checking both `matches(control)` and the
exact original worker ID. Only cfg(test), supported-feature/target read-only
identity ID/stats/join methods are added. Identity and control getters share the
existing join decoder; actual Join write sites/order and the old weak-only
`WorkerIdentityWitness` contract stay unchanged. No production control Clone,
command/Consumer ownership, global observer/caps/sticky, vendor or CI change is added.

The fixture pins three distinct original identities, accepting only
`HealthyJoinReturned` or `HealthyJoinObserverCallReturned` after the original
`JoinHandle.join()` actually returns (and, for the latter, its observer returns).
Panic, detach and timeout fail; NotPolled/unfinished/finished are not joins.
Healthy Cancel retains the before/after ACK-drain NotPolled assertions; AppDrop
still requires all three actual original joins. Budgets, sticky refusal and
emergency retained ownership remain. The eighth ordinary case first gives a
scoped foreign thread its own `ObservationRun`, then asserts its false-hook
failure, finishes that run and actually joins before checking local source
identities. No false hook targets unknown `ACTIVE`; no existing failure is cleared.

The corrected two files were then actually compiled/tested locally: serial App8
passed 8 in 2.01s with test-threads8, including healthy Cancel, AppDrop and the foreign
collector counterexample; unchanged old identity2 passed in 0.26s. Root strict
all-feature/all-target Clippy and no-default all-target checks both exited0; the
latter had no App warning. All four phases reached actual exit0/held-handle reap,
guard=false. Root's 01:18:07Z fresh heavy-process inventory was empty and resources
were actually handed back to the peer. Those four local phases did not execute
the full suite, SDK, production bins or GUI/device/NI checks; cloud coverage follows.

**Cloud acceptance is now complete for code `b9f27091fbf5dd91e70ee318dcd897434fb5f6cf`.**
[Quality38834734658](https://github.com/wrench1997/DAW/actions/runs/38834734658)
attempt 2 actually completed SUCCESS, updated 2026-10-16T01:55:07Z, Windows
job 116764894163. It passed 2529 root tests/16 ignored = App2464 + helper52 +6+5+2.
All eight Serial cases passed, including healthy Cancel, AppDrop and the foreign
collector regression; old identity2 and `genuine_app_observer` 258 also passed.
Root strict all-feature/all-target Clippy, production bins and no-default checks
passed, as did fixture6 and both real host targets.

Windows explicitly executed private App source1 and Whole SDK1 after their ordinary
suite ignores, with both `REAL_PRIVATE_EFFECT_CAPTURE_PASS` and
`REAL_SERIAL_WHOLE_SDK_PASS` receipts. The source case confirmed exact non-f32 gain,
source DSP0, original guard joined and natural strict reap; the Whole oracle confirmed
three original guards joined, three natural strict exits/reaps, latency96/tail352,
raw1408→retained1280 and one crop/tanh. These are real selected SDK executions,
not inferred from an ignored test listing.

PrintWindow/native checks actually passed, not SKIPPED: Default input/thread
desktops, visible station and usable session2; paint before/after, repaint,
interaction, state round-trip and cleanup passed with `EDITOR ACCEPTANCE OK`.
Linux's fixture6/real host2 plus async15/Effect29/Unix1 were carried from the original
same-SHA 01:22–01:24 successful execution (current API job 116764895810). Linux was
not rerun when the failed Windows job was retried.

The first quality attempt remains a real failure: Windows job 116761199271 reported
factory 32 passed/1 failed at `factory_and_default_load_share_one_attempt_seal_in_both_directions`.
ProbeFactory exceeded its original 3s budget, exchange3.0117158s with empty phase
trace, Deadline/MayHaveApplied; kill/reap completed. Root changed no source and
made exactly one same-SHA failed-Windows-job rerun. The original timeout/full raw
log remains; the later success establishes neither a scheduling cause nor that an
intermittent defect is fixed. No deadline, assertion or CI failure rule was weakened.

[Preview38834734476](https://github.com/wrench1997/DAW/actions/runs/38834734476),
job 116761198689, actually completed SUCCESS, updated 2026-10-16T01:50:57Z. Python 371
= 359 passed/12 conditional skips in 23.380s; root 2529/16 ignored (App2464 in 509.70s,
including Serial8), offline MIDI4/async15/Effect29, strict Clippy and no-default
passed. Static-CRT release completed in 8m53s; ZIP/PE/import/provenance and the
extracted sanitized-PATH helper passed: three JSON replies, invalid-command
recovery, Shutdown with stdin open, exit0 and natural reap. Upload requested false
made the upload step SKIPPED. No new user binary copy was delivered.

Earlier 8d/eae failures, 8d's missing original failure literal, the local 10-pass
identity repair and earlier 31/150/SDK evidence retain their exact inputs and limits.
Records under `target/backend-reliability-20261014/`: source121/122 failure reports
and full raw logs; `source123-preview-actual-terminal-01.json`/full raw,
`source123-quality-attempt2-*` API/raw records and
`source123-factory-single-rerun-decision-01.json` preserve the final acceptance and
first-attempt failure. The original proposal and local repair evidence remain in
`whole-serial-effects-source-candidate-01/cloud8d8e94d-repair-01/repair.json` and
`cloudeae18b7-compile-repair-01/cpu-handback.json` beneath that candidate directory;
older local App/SDK/CPU receipts and the Instrument fixture handoff are retained.
General routing/FX automation/PDC, complete natural tails and real NOIRE cold
restore/export/listening remain required. The user is still testing03; this work
has not operated the main GUI, soundcard or NI and has not replaced that copy.

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
off. Native04's cleanup-only primary places the failure after render checks but
does not show whether Shutdown was consumed or which teardown boundary stalled;
its AppDrop marker follows export failure. Native08 below has now reused the
Native05 bins and validation02 with a Play/Escape prelude, no Ctrl+S or editor,
exact timing on and message-pass off. Its successful publication and actual exit
boundaries narrow the next comparison to Save-related or intermittent behavior;
the preference-seed difference prevents strict single-variable attribution.
Do not extend deadlines or make forced termination count as successful export.

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
The parent contains `native06-resource-handback-01.json`. These native sessions
predate the parameter-restore and offline-MIDI candidates; neither entered this
comparison.

### Native07: a successful export does not yet qualify onset repeatability

Native07 froze the actual Source91 Cargo main/helper artifacts into a new
`bin-native07` directory. It uses SDK-A and the history-ready repair, not the
next IProgress candidate. With exact timing still disabled and ordinary
diagnostics1 retained, normal Ctrl+O opened the same validation02 project.
The GUI showed 4 bars/16 beats, one eight-note clip, Kontakt Loaded, Channel1→FX01,
2304 samples of latency and the iCON output at 48 kHz/XRUN0. There was no playback, new project Save,
native-editor operation or instrument reload in this session.

The ordinary GUI export used stereo 48 kHz/24-bit PCM, legacy peak attenuation and
an explicitly accepted 3000 ms fixed tail. The native Save dialog kept `Untitled`
and navigated through its actual address control into the new
`native07-offline-midi` directory; the target was absent before Alt+S. Whole
published 3,024,044 B/504000 frames/10.500 s, with 7.500 s music and 3.000 s tail. All
eight onset windows contain signal, samples are finite, clipping count is 0 and
the final 20 ms are digital zero. Listening and complete unknown release remain
unqualified.

| Actual export | First sample above −100 dBFS | Time | First 400 ms overlap relative to Native05 |
| --- | ---: | ---: | --- |
| Native05 | 138 frames | 2.875 ms | Reference |
| Native06 | 522 frames | 10.875 ms | Exactly 384 frames later after alignment |
| Native07 | 266 frames | 5.542 ms | Exactly 128 frames later after alignment |

For Native07, the corresponding shift gives zero RMS sample difference in the
overlapping portions of both earlier first 400 ms windows. Differences end at
frame 33151; later audio and the tail are identical. The differences are
128-frame multiples. Native07 is only one run of the new SDK-A candidate, so its
same-source repeatability is still untested. Keep these frozen bins for that
comparison before attributing a change to SDK-B. These outputs do not support a
fixed warmup, sleep or compensating trim.

Normal Alt+F4 and held OS handles confirmed App3772, source helper1872 and clone2640
all exited 0; observer1340 also actually exited 0. Fresh windows/processes were 0
before peer handback. Resource peaks were 2.309 GiB owned private and 23.977 GiB global
physical used. Preserve the workflow's `bin-native07/freeze-receipt.json`,
`launch-07.json`, `cold-open-and-export-native07-01.json`,
`normal-close-process-terminal-07.json`, and the output directory's
`ordinary-whole-native07-pcm-audit-01.json` and
`native05-native06-native07-pcm-comparison-01.json`; the parent contains
`native07-resource-handback-01.json`.

### Native08: playback before export succeeds once; Save remains open

This run reused Native05's frozen Source87 main/helper, validation02, exact
timing on and diagnostics 1, with message-pass absent. Normal Open restored Kontakt
Loaded, Channel 1→FX1, 2304 samples and the iCON output at 48 kHz/XRUN0. Play was sampled
at 002:02:02; Escape stopped and rewound to 001:01:00 while the project remained
Saved. No Ctrl+S, parameter/note edit, editor opening or NKI reload followed.
The pre-Native06 preference seed is not a Native05 launch preimage, so this is
not a strict single-variable comparison with Native05.

Ordinary Whole Review used 48 kHz/stereo/24-bit PCM, attenuation above 0.95 and explicit
acceptance of a 3000 ms fixed tail. Native Save navigated into the new
`native08-play-only` directory, where Untitled.wav was absent before submission.
No Stop/Escape/Cancel followed Save. Actual publication is 3,024,044 B/504000 frames/
10.500 s. Independent decoding found signal in all eight onset windows, finite
samples, 0 clipped samples, peak −26.738 dBFS, and exact digital zero in the final 20 ms
and last frame. This WAV has not been listened to; complete unknown release
remains unqualified.

At −100 dBFS its first signal is frame 394/8.208 ms, 256 frames later than Native05. The first
400 ms overlap has exactly zero RMS difference at that offset. Differences are
limited to frames 129..33151; all later PCM and the tail match Native05. This adds another
old-Source87 Q128 onset difference, not SDK-A/B acceptance or a causal explanation.

Clone 107928 actually consumed Shutdown; component terminate returned SDK 0 and
ExitDll returned after 839.011 ms. Its Main-consume→Windows-run-return span was
1098.569 ms. Reader and Main clocks have different origins. Held OS handles after
normal Alt+F4 confirmed App 83328, source 76024, clone 107928 and observer exec 5501
all exited 0, with no binding failures or budget violation. Fresh process/window
counts were 0 before handback; resource peaks were 2.222 GiB owned private and
27.917 GiB global physical used. The Play/Escape prelude is usable in this one
run; Native04's Save factor and intermittent cleanup failure remain unresolved.

Preserve `launch-08.json`, `cold-open-play-only-and-export-native08-01.json`,
`stderr-08.log`, `normal-close-process-terminal-08.json`, the new output's
`ordinary-whole-native08-pcm-audit-01.json`,
`native05-native08-pcm-comparison-01.json` and `native08-shutdown-trace-audit-01.json`.
The parent contains `native08-resource-handback-01.json`.

### Offline event metadata candidate and async-state gap

The ordinary MIDI convenience path previously marked every event `kIsLive` and
set event PPQ to zero, including Whole offline rendering. The SDK defines the
flag as directly played keyboard input and PPQ as the event's project position.
[Official event fields](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/structSteinberg_1_1Vst_1_1Event.html).

The candidate tags only internally generated ordinary MIDI and resolves its
Offline headers at the actual DSP chunk boundary, after current transport and
final offsets are known. It clears live and computes event PPQ independently
of optional ProcessContext musical-field requests. Realtime/Prefetch and explicit
PluginEvent headers remain unchanged. Four real COM process-boundary cases cover
chunking, the next block, changed tempo/nonzero PPQ, stopped/zero-frame handling
and existing entry points. Both CI consumers explicitly run `--lib offline_midi`;
root Cargo tests alone do not execute dependency unit tests. Formatting, complete
75-file patch replay, explicit vendor pins and two packaging/consumer contracts
passed. Local Source90 (`8fc027d`) subsequently passed all four new
SDK cases and15 original HostEventList regressions. Root generic8 also passed,
but startup history reproduced7 passes/four failures; identity, strict Clippy
and matching-bin build did not run. The subsequent Source91 App repair passed
8+11+5 targeted cases, strict Clippy and matching bins; unchanged SDK4+15 evidence
was reused rather than rerun. Existing vendor dependency warnings remain. This
corrects a demonstrated event contract, not a demonstrated cause of the384-frame
difference. Native07 subsequently exercised SDK-A as recorded above; the next
IProgress candidate has not entered NI acceptance.

After all Cargo processes terminated, the diagnostic wrapper incorrectly waited
on its own conhost infrastructure. Only that wrapper was interrupted; exec77452
actually exited1 and fresh heavy/root/console absence preceded peer handback.
Keep the original script/raw logs plus the independent
`source90-local-gates-01-terminal-recovery.json`; it does not invent a successful
aggregate run. The next observer must not wait for a console whose lifetime
depends on its own wrapper. No NI or user process was terminated.

Source91 excluded console infrastructure but then waited on the observed MSVC
VCTIP data uploader after all five Cargo phases had actually exited0. Local
executable metadata identified that infrastructure, and a fresh process check
showed no Cargo/rustc/test/link/App/helper work. Only the owned diagnostic wrapper
was interrupted; exec10380 actually exited1. Fresh heavy/root/console counts were
zero before handback. The recovery receipt records these different outcomes,
the two actual Cargo artifacts and50 resource samples (peak4.076GiB owned private,
23.665GiB global physical used). Do not call it an aggregate runner exit0 or
terminate unrelated processes/change system settings to avoid an observer wait.

SDK-B now implements a second confirmed gap in source. The actual ComponentHandler
formerly did not expose IProgress, and the separate HostApplication registry
retained untyped active IDs with only drained incremental notifications. The SDK
expects the controller to query IProgress from its component handler.
[Official IProgress interface and controller example](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IProgress.html).

The candidate shares one bounded typed registry across both COM interfaces and
the processor. A non-consuming snapshot distinguishes async restoration from UI
work. Only Strict's disposable clone opts into the readiness guard: it waits
after LoadState and StartProcessing using its original deadline/cancellation,
then arms the exact async-start revision under the registry lock after generic
bases, readback and metadata. Actual DSP checks the revision before and after
each foreign call. Even an async start+finish within one call rejects output;
a later block's change rejects the entire staged render. No lock crosses a
plug-in call. UI-only work and existing Realtime/unarmed Offline entry points
keep their prior processing prerequisites.

Snapshot, clone-owned progress-notification discard and arm use three additive
closed helper commands. A drain never clears active tasks or restores a lost
readiness proof. Static review found two rejected-start gaps: an unknown kind
could bypass the fault latch when the notification FIFO was full, and async/
unknown kinds could bypass it on null out-ID or an unterminated 1024-unit
description. Both now latch a non-UI fault while preserving the original error
return and legal UI-only behavior. The existing test functions add deterministic
COM-interface and UI-only counterexamples.

Static checks passed for 19 source files, two Python provenance/consumer contracts
and the four-file vendor bundle. The complete patch replays all 76 vendor files
from the original 41-file archive; notes and both pins are synchronized. Windows
quality/preview are wired to run the dependency's 15 new readiness cases. A narrow
Linux SDK job runs those cases and the changed Unix-only ordinary no-respawn
case without running the DAW, a desktop, a plug-in or an audio device.

Source93 (`63bf538`) has now reached actual cloud terminal results. Linux passed
all15 SDK readiness cases and the one exact Unix ordinary no-respawn case.
Windows quality passed32 factory,4 SDK offline-MIDI and15 SDK readiness cases,
then App2330 passed/3 failed/12 ignored. Preview passed344 harness, the same4+15
SDK cases, then App2329 passed/4 failed/12 ignored. All four new Root readiness
cases passed. Three shared failures are stale exact fixture suffixes: the
cancel-before-dispose and metadata-drift boundaries now include a snapshot;
pending disposal now follows arm. Preview additionally failed a held-prior-reset
case when it re-read an instantaneous settlement observation after a successful
Ready wait and received Waiting. The held-reset production contract remains
intact; the fixture repair below removes that second-sample race.

Locally, legacy progress2 passed and Strict32 passed/3 failed with the same
three boundary mismatches (Cargo101). Clippy and matching-bin build were not
reached, and SDK-B has not entered NI acceptance. The diagnostic runner actually
exited1 after all observed work terminated; its aggregate result is not a pass.
Preserve `ci-63bf538-{windows-quality,windows-preview,linux-helper-readiness}-01.log`,
Source93 cloud actual-terminal JSON and `source93-local-gates-01-terminal.json`.
Source92 and Native07 results do not qualify the newer candidate.

Source94 aligns the three exact Strict fixtures with the complete getter,
snapshot/arm and cleanup boundaries. The Whole test helper returns the actual
Ready observation that completed its wait; both direct callers consume that
sample instead of requiring another instantaneous read to remain Ready. Original
held-reset, epoch, zero-capture, Project-byte, no-publication, retained-owner and
concrete-reap assertions remain. Production readiness, cancellation and cleanup
behavior is unchanged. The legacy constructor receives only a function-scoped
compatibility dead-code annotation. Formatting, diff checks, complete76-file
vendor replay, four-file bundle validation and two Python contracts passed.
Source94 (`e50f92f`) locally passed four affected exact cases: cancellation,
pending owner and both Whole observations. Strict all-feature/all-target Clippy
and a matching main/helper build each exited0. The dry-topology case still
failed: its initial prepare rejects capture drift after the complete getters,
before a post-snapshot or arm. The prior repair incorrectly applied the armed
metadata suffix to that first occurrence. Runner exec90217 actually exited1
(Cargo101); the separate remaining-check runner exec46212 actually exited0.
Keep both `source94-local-gates-{01,02}-terminal.json` records rather than calling
the aggregate a pass. Both cloud runs now actually completed failure: Linux
passed SDK15/Unix1; quality passed32 factory,4 SDK offline-MIDI and15 SDK readiness;
preview passed344 harness and the same4+15 SDK cases. Each App run finished2332
passed/1 failed/12 ignored, only the initial-prepare boundary mismatch. Neither
Whole second-sample case failed. Later cloud Clippy, bins, release and ZIP were
not reached; local passes do not substitute for them. Keep
`ci-e50f92f-{windows-quality,windows-preview,linux-helper-readiness}-01.log` and
Source94 actual-terminal run/jobs JSON. SDK-B has not entered NI acceptance. Preserve
`strict-cancel-readiness-candidate-01/applied-handoff-01.json`,
`strict-readiness-fixture-provenance-01/` and
`whole-review-settlement-success-sample-fix-01.json`.

Source95 corrects only that exact dry-topology fixture: occurrence1 retains the
complete getters then cleanup without a post-snapshot/arm; occurrences2..5 retain
Snapshot(2*n) before cleanup. Raw-policy early rejection is unchanged. Production,
vendor sources and pins do not change. Scratch formatting/apply-check and the
applied diff passed. Source95 (`92e9d3b`) actually passed the complete affected
matrix case and changed-input strict all-feature/all-target Clippy; both Cargo
phases and runner exec76910 exited0. Source94's unchanged four passing cases,
matching bins and SDK evidence remain applicable, so all five affected cases
now have actual passing evidence. The Source94 SDK-B bins are separately frozen
with actual Cargo artifact sizes/times at
`normal-noire-workflow-20261015-01/bin-source94-sdk-b/freeze-receipt.json`; NI is
still untested. All observed work terminated and fresh heavy/owned-root counts
were zero before peer handback. Peak owned private/global physical used were
3.826/23.161GiB. Full-cloud qualification of the new source remains pending;
Source94's actual terminal evidence was retained before the next publication.
Keep `source95-local-gates-01-*`,
`source95-local-resource-handback-01.json` and
`strict-dry-topology-prepare-boundary-candidate-01/`.

Source96 (`1687ec5`) quality has now actually completed: 32 factory, 4 SDK
offline-MIDI and 15 SDK readiness cases passed; App2333 + helper52 + protocol13 =
2398 root passes/12 ignored. All previously failing fixtures passed, including
the complete dry-topology matrix. Strict Clippy, matching bins, all backend
harness/fixture and no-default-feature gates passed. Linux SDK15/Unix1 also
passed. Overall quality still failed only at native paint before/after edit:
PrintWindow and same-process button probes returned false. Attach/resize/DPI,
modal IPC, control/gesture/dirty, stopped-edit state and Shutdown/EOF/crash
cleanup passed independently. SDK-B NI remains untested. Preview subsequently
completed success: 344 harness cases, the same SDK4+15 and root2398/12 ignored,
strict Clippy, no-default-feature checks, static-CRT release, final ZIP integrity
and extracted sanitized-PATH helper smoke all passed. Release took 8m23s; upload
was skipped. Preserve
`ci-1687ec5-{windows-quality,windows-preview,linux-helper-readiness}-01.log`
and Source96 actual-terminal run/jobs JSON.

### SDK-B short source qualification candidate (2026-10-15)

A separately named backend-short-source-load entry is prepared. It reads pinned
Saved13 state/generic bases into a private two-note/two-placement score, requests
the saved class, and requires this attempt's actual source worker metadata,
opaque ACK, callback and stopped-source capture before the production Whole/
Strict controller. The original full Saved13 entry retains its checked loaded-
default scan and healthy-retirement contract; no catalog data supplies live
metadata. Two fixed run-owned clone summaries copy only existing PrepareWait and
ArmedVerify snapshot returns. Missing observations and observed zeros stay
separate; no new SDK query, warmup, sleep or timeline/PCM stream record is added.

Static rustfmt/diff checks passed for11 source files. Fifteen new nonignored
regressions and one ignored NI selector are prepared. Frozen `dd204d9` actually
compiled locally: observer8 and FullOwner16 passed; short-model finished3 passed/
1 failed because the synthetic valid-metadata fixture failed the actual validator.
The validator was not relaxed. One unused-mut warning also needs correction.
Independent review found missing preservation of initial source errors and receipt-
write errors; the original owner is repairing bounded diagnostics/fallback and
adding regressions before NI. Runner130088/exec42591 actually exited101; every
held child terminated and fresh heavy count was zero before peer handback. Peak
owned-private/global-physical used were5.002/23.165GiB, below the existing budgets.
Strict36, Clippy, cohort construction and NI rounds were not reached. Preserve
`source98-local-gates-01-*` and `source98-local-resource-handback-01.json`.
The candidate and exact recipe are retained in
`target/backend-reliability-20261014/noire-sdk-b-short-source-candidate-01/`.
It preserves original-frame PCM/onset and exact MIDI Q/offset for two independent
same-source rounds. Source worker thread join is distinct from helper OS exit;
unavailable PID/exit fields remain null and need Root's actual process handles.
This does not qualify ordinary GUI Save/cold Open, hardware listening, complete
internal parameters or Felt.

The subsequent frozen repair corrects only the synthetic fixture's declared
input main/aux flag, retaining the real metadata policy and adding an aux-input
refusal control. Initial diagnostics retain head64 plus a separate latest8 actual
failure tail with counts/truncation and original order. The regression puts75
successes before Load/Prepare/Metadata/SlotFault failures, then checks overflow
with13 failures. Receipt Err/panic/write failure gets an independent createNew
fallback, with original causes sent to stderr if that write also fails. The
unused-mut warning is removed. Five-file rustfmt, complete patch replay and diff
checks passed. Frozen `a4850d6` actually compiled and passed short7; Strict36
finished35 passed/1 failed. The existing cancellation/original-deadline blocked-
stage matrix expired waiting for the DiscardProgressNotifications:1 marker.
Retained mock audit contains forced-reap exit1/disposal/drop but no planned
blocked marker; the cause is not established. The original owner is examining
the retained fixture instead of increasing the deadline or retrying without new
evidence. Clippy, cohort construction and NI were not reached. Runner113380 /
exec64377 actually exited101; every held child terminated and fresh heavy count
was zero at09:51:15Z before peer handback. Preserve `source99-local-gates-01-*`.
Reuse unchanged observer8 /
FullOwner16 passing evidence. Keep `noire-sdk-b-short-source-repair-02/`; the first
repair and failed Source98 execution are preserved.

A subsequent one-file Strict diagnostic keeps the original2s/20s deadline and
15s reporting budgets. Its local wait preserves the same receipt's actual typed
terminal outcome if it finishes before the planned marker, then still fails;
normal marker paths record their real outcome before the original assertions.
Primary request/stage/delivery/limit fields remain separate from cleanup. A
new deterministic LoadPlugin refusal before the planned Discard stage requires
concrete same-child reap, released admission and no later I/O. Only this new
exact and the original matrix use the local wait; the other35 passing Strict
cases keep their existing wait. Frozen `bbd4c3a` actually compiled in Source100:
the new diagnostic and original matrix each passed1 (0.35s/24.42s). Strict
Clippy then exited101: short QA's constant `chunks_exact(2)` triggers1.98's
`chunks_exact_to_as_chunks`. A local repair uses the project's existing
`as_chunks::<2>().0.iter()` style without raising MSRV or suppressing the lint.
Complete-stereo/finite admission stays intact; the two existing onset tests add
left/right frame0 and complete-frame-plus-odd-tail refusal controls. Those two
changed tests and strict Clippy are not yet run; keep
`short-onset-as-chunks-lint-repair-01/`.
Artifacts/build and NI were not reached. Runner114200/exec42942 actually exited1;
all19 held children terminated, with fresh heavy count0 at10:13:21Z before peer
handback. Peak private/global physical used were4.017/23.549GiB, below budget.
Keep `source100-local-gates-01-*`, `source100-local-resource-handback-01.json` and
`strict-blocked-stage-terminal-evidence-01/`. The original fixture15 has no
startup ledger witness, so neither slow startup nor SDK-B causality is proven.

Source101 (`16f7ff5`) actually passed the changed onset2, strict all-feature/
all-target Clippy, actual test artifact and matching main/helper build. Runner
70624/exec75399 exited0 and all15 held children terminated. Reuse Source100's
two exact passes and unchanged observer8/FullOwner16/other short5/Strict35;
this was not a rerun of the complete suite. The actual compiler-artifact test
executable and helper were frozen in `bin-source101-sdk-b-short-01`.

The first actual NI attempt then failed, test101/56.70s, with
`complete source initialization deadline: complete actual callback has not reached its exact paused Timeline epoch`.
There was no capture, clone or WAV. Original records49/50 contain one128-frame
paused OrdinaryProcess Attempt and its actual Ok return, without a capture/epoch
binding. The aggregate excludes that unbound pair; its zero cannot establish
zero DSP, and the successful API return does not prove non-silent audio.
Worker Join completed, while the outer observer
independently retained helper97680 through natural OS exit0. Test47092,
runner110816/exec21772 and all observed descendants actually terminated without
forced kill or budget violation. No second round began after this failure.
Fresh heavy count was0 at10:19:36Z before peer handback. The source owner is
investigating the actual callback/Timeline epoch dependency without enlarging
the40s initialization budget or removing its exact gate. SDK-B onset and
two-phase async reports remain unqualified. Preserve `source101-local-gates-01-*`,
`source99-noire-short-native-round01/`, its stdout/stderr/terminal files and
`source101-native-short-resource-handback-01.json`; source99 is the old runner
label, not the actual source revision.

The current attribution checkpoint adds a fixed read-only readiness object before
cleanup: actual Timeline phase/desired/compiled/confirmed identities and errors,
callback/command-ring state, and at most4 worker epoch/settlement rows. Null is
not replaced with zero/false. It adds no pump, SDK, DSP or command; independent
diagnostic failure cannot skip cleanup. Two new real plugin-free controls cover
compile-to-exact-paused callback plus a late command with the same legacy barrier,
and missing identity/original error retention without pumping. Original FullOwner16
inputs, source-ready/pump predicates and40s/120s/35s budgets stay unchanged.
Frozen `79a32f6` actually passed both new exacts, strict Clippy, test artifact and
matching bins in Source102 (runner20432/exec93994 exit0; all18 held children
terminated). The single native attribution attempt then failed101/56.99s before
any capture, clone or WAV. Its actual phase was Faulted; desired/compiled/source
generation2 and fingerprints matched, with no compile error. The retained sync
error was `Compiled generator route is not callback-ready: Generator channel 1 instance 1 has no callback-confirmed Running endpoint`.
Finally running1/pending0 and worker settlement were observed, but callback
generation, exact graph and confirmed Timeline identity were still absent.
FullParent's initial compile/mixer pumping before source-pending admission fits
this latched early missing-route failure; the owner is repairing that real
Pending-to-callback-Running order, not clearing all faults or raising budgets.
Helper143552 naturally exited0; test105916, runner133104/exec95516 and all observed
work terminated without forced kill or budget violation. Fresh heavy count0 at
10:41:39Z preceded peer handback. Preserve `source102-local-gates-01-*`,
`bin-source102-sdk-b-short-01`, `source102-noire-short-native-round01/`,
`source102-native-short-resource-handback-01.json` and
`source-callback-epoch-attribution-01/`.

Pushed `7ab716a` quality run38719646323 actually completed success:2352 App +52
helper +13 protocol =2417 root passes/13 ignored,32 factory, SDK4+15, strict
Clippy/both bins,14 helper and80 editor harness, fixture/no-default-feature and
actual native PrintWindow all passed. Linux SDK15/Unix1 passed. The ignored NI
entry was not run and is not qualified by cloud green. Preserve
`ci-7ab716a-{windows-quality,linux-helper-readiness}-01.log` and
`source101-quality-actual-terminal-01.json`. Preview run38719646340 also actually
completed success:363 harness cases=351 PASS/12 SKIPPED (four Unix conditions and
eight private Saved13-input exclusions, not a GUI skip), SDK4+15, the same2417
root/13 ignored, strict Clippy/no-default-feature, static-CRT release8m10s, final
ZIP validation and extracted sanitized-PATH helper smoke passed. Upload was not
enabled. Preserve `ci-7ab716a-windows-preview-01.log` and
`source101-preview-actual-terminal-01.json`. These checks predate79a attribution
and the pending ordering repair.

The ordering repair is now source-frozen in
`source-admission-mixer-order-fix-01/`: keep mixer initialization before native
load, but drain its130 actual commands through the unchanged callback-work
service without polling Timeline controls before a real Pending source exists.
Then perform the original one-shot source admission and normal full ticks.
Successful ordinary GUI Open already registers sources synchronously before
periodic polling; its strict production route gate is unchanged. Real compiler
regressions reproduce the old sticky missing-owner fault and exercise a real
held-prepare worker through Pending, actual callback Installed, exact graph and
paused acceptance. FullOwner now has20 tests. Replay/format/diff passed; Rust,
Clippy and corrected NI execution are unrun. No-source mixer pumps cannot warm
NOIRE, and no extra pump, placeholder owner, fault clearing or larger deadline
was added. NI/audio verification is deferred while the user tests the independent
copy; software gates may continue without replacing or stopping that copy.

The user has authorized the separate catalog-retirement repair. Its current
diagnostic candidate keeps all five original scenario and health/failure/kill/
reap assertions, adding actual scenario/elapsed/primary/stopped/full-lifecycle
output. A new real child returns Error, waits for EOF, then delays exit; it must
be killed and reaped as UncleanRetirement, never healthy. The catalog module has5
tests. Terminal's100ms natural-exit preference differs from healthy Shutdown's
two seconds, and the existing Python fixture still performs interpreter teardown;
the missing historical receipt prevents choosing that as the old failure cause.
Production/vendor/budgets are unchanged. Formatting/diff passed; one actual
catalog5 gate is next, followed by repair on its evidence rather than retrying to
green. Keep `catalog-retirement-diagnostics-01/`.

Source103 (`3e711cc`) actually passed FullOwner20 (3.07s), catalog5 (4.06s),
strict all-feature/all-target Clippy, test artifact and matching bins. Runner
27256/exec80781 exited0 and all22 held children terminated; private/global memory
peaked at3.936/25.004GiB. Catalog cases0/1/4 naturally exited0 at61/67/61ms with
no kill/pipe/wait errors; the added delayed Error case was killed/reaped as
UncleanRetirement at156ms with the original rejection retained. The old failure
did not reproduce. Its owner is removing irrelevant interpreter-teardown timing
from the normal negative fixtures while retaining the100ms production boundary
and delayed-kill rejection controls. Corrected NI execution remains unrun.
The user actually started preview PID14900 at11:04:46Z; it remains an intentional
independent process, not Root work to adopt or stop. Only Root's software CPU
window was returned; user desktop/audio testing stays protected. Keep
`source103-local-gates-01-*` and `source103-local-resource-handback-01.json`.

The catalog fixture repair now ends only matrix cases0/1/4 with `os._exit(0)`
after all actual request/reply/EOF assertions and stdout/stderr flushes. Normal
healthy Shutdown, exit9, sleep30, all original health predicates and production/
vendor deadlines remain. The controlled real-child pair registers the same30s
atexit finalizer: direct protocol-complete exit must retire healthy; ordinary
interpreter exit must retain the original rejection and actually kill/reap as
unclean, with an EOF-finalizer stderr phase marker. Catalog still has5 tests.
Formatting/diff passed; the changed catalog and Clippy are unrun. Keep
`catalog-retirement-fixture-exit-repair-01/`. This removes the mock's unnecessary
interpreter-finalization dependency; the historical CI receipt remains missing.

Source104 (`c7290a5`) actually passed changed catalog5 (3.96s), strict all-feature/
all-target Clippy, test artifact and matching bins. The direct-exit child retired
healthy with OS exit0 at57ms; ordinary exit emitted its real EOF-finalizer marker
then was killed/reaped at160ms with exit1, UncleanRetirement and the original
OperationRejected. All five original scenario assertions passed. Runner6612/
exec93717 exited0 and all18 held children terminated; private/global memory
peaked at3.809/25.517GiB. The user's independent preview was never adopted or
stopped. NI/CU/audio were not run. Keep `source104-local-gates-01-*` and
`source104-local-resource-handback-01.json`.

### Playlist transport and AUTO end — local acceptance (2026-10-15)

Source `885306f` completes local scoped acceptance: green PAT/orange SONG, current
Pattern and actual scope in the chooser/caption/title/ruler; continuous held ruler
seek isolated from Draw; new UI songs use content-derived AUTO. Playback,
Timeline and supported export share the endpoint. View1/4/8 bars/FIT does not
limit duration. Existing Clip trims, Manual ranges and old Draw/import/record
extension contracts remain;4096 beats is an editing/resource quota. Project13
prevents older builds silently ignoring AUTO. See [usage](PLAYLIST_TRANSPORT.md).

**Actual passes:**24 new nonignored cases and one explicit Saved13 read-only/
private-copy case;214 unaffected headless passes plus the repaired short-Clip
exact; Model53, TempoMap12, AudioClip6, ClipFade8, native automation6, all6 Audio
split cases and unchanged prior export coverage; FullOwner20. Source111 strict
all-feature/all-target Clippy, actual test artifact and matching main/helper
build each exited0. Twelve actual WGPU cases and final mode/layout exacts passed.
Root inspected all four final frames: no stale Pattern toast, foreground Playlist/
AUTO visible at logical1280/1920 widths and unclipped toolbar. No new native
CU/NI/soundcard verification occurred.

The actual Saved13 source remains1,298,729B/591772 opaque bytes, five clips/one
automation, format12/Manual160. Both private-copy complete persisted music
payloads match; Auto derives128 and source bytes are unchanged. Generic maps are
empty, so this is data round-trip evidence, not full NOIRE sound/parameter restore.
Growing source notes still does not expand every existing trimmed placement.

Preserved failures and repairs: Source105's missing Storage trait; Source106's
runner-only empty capture env (207 failures, independently reproduced) and the
real Inspector0.125-to0.25 mutation; short-Manual source geometry outside the
playback map; v10's independent oracle crop; inherited fade silence in the moved
v11 tail, now exact-zero PCM/WAV plus a same-source fade-reset audible control;
and Source109/110 initializer lints. Production geometry follows later actual
tempo automation under262144-checkpoint admission without extending playback.
Full original errors/selectors and phase/resource records remain in
`playlist-transport-progress-history-01.md`
and the Source105–111 original logs.

The separate user preview02 is delivered at
`D:\DAW-Preview\Citrus-Studio-20261015-02`, desktop **Citrus Studio 新版测试02**.
Actual Source111 matching main68,053,504B/helper5,054,464B were ordinary independent
copies; copied helper no-plugin smoke exited0/reaped. Root did not launch main.
UserData/Projects are isolated from01 and user works. This is same-machine MSVC
debug, not the static-CRT release. New saves are13 and cannot open in01; use
Save As on old works. Native02 use/NOIRE listening remains untested. This feature's
quality result is recorded below; repaired preview acceptance remains pending.

The earlier backend233f6f0 preview succeeded:363 harness=351 PASS/12 conditional
SKIP, SDK4+15,2422 root/13 ignored, strict Clippy/no-default-feature, static-CRT
release8m45s, final ZIP/extracted helper. No upload. Its quality actually failed
at factory31PASS/1FAIL, original3s Deadline; the historical cause stays unknown.
The later fixture-only isolated startup/bounded-phase repair passed factory33
locally without changing production/seal/cleanup/deadline rules, then passed
the cloud quality run below. Source104's changed catalog5 passed its direct-exit versus
real interpreter-finalizer kill/reap controls.

Evidence under `target/backend-reliability-20261014`:
`source111-local-gates-01-*`, `source111-local-resource-handback-01.json`,
`source111-playlist-{mode,layout}-render-01-*`,
`playlist-transport-render-02-{mode,layout}/`,
`user-offline-preview-20261015-02.json`, the four Playlist candidate/repair folders
and `factory-attempt-seal-fixture-01/`. Earlier cloud raw logs/terminal JSON remain.

Cloud preview `431af5d`, run38737463544, actually failed its first Python gate:
363 cases,350 passed/one error/12 conditional skips. The package document list
omitted `PLAYLIST_TRANSPORT.md`, leaving published guide links unresolved. Rust,
release build and ZIP smoke did not run. The failure was reproduced locally;
the narrow two-script repair includes the guide in the reviewed package list,
checks its exact ZIP/extracted bytes and provenance, and still rejects a linked
missing guide before creating output. All179 directly related package tests
passed locally; no link/closure assertion or workflow failure rule was removed.
Keep `ci-431af5d-windows-preview-01.log`,
`source112-preview-actual-terminal-01.json` and
`playlist-package-document-closure-repair-01/`. New cloud acceptance is pending.

The same `431af5d` [quality run38737463353](https://github.com/wrench1997/DAW/actions/runs/38737463353)
actually succeeded at13:08:24Z: Windows and Linux jobs passed. Factory33, SDK
offline MIDI4/readiness15,2381 App+52 helper+13 protocol=2446 root cases passed
with14 explicitly ignored. Strict Clippy, matching bins, helper14/editor80,
no-default-feature and native fixture gates passed. The cloud Windows session2
had an eligible interactive Default desktop; actual PrintWindow before/after
paint and repaint-change checks passed, rather than SKIPPED. This result does
not replace the separate preview's document-closure failure. Raw run/jobs and
both complete job logs are retained as `source112-quality-*-01.*`.

After the package repair, `4102953` preview run38740984829 passed all367 Python
cases except12 explicit conditional skips, plus SDK4/15. Its Rust App gate
actually failed:2380 passed/one failed/14 ignored,497.59s. The ruler-drag test's
line126 `!ui.app.dirty` assertion failed; release/ZIP did not run. The same SHA's
quality run38740984624 succeeded with2381 App/2446 total, strict Clippy/build and
real session2 PrintWindow. That pass does not override preview's failure.
The UI owner traced a deterministic fixture issue: replacing the startup demo
updated the history fingerprint but not the saved Project baseline; crossing
the750ms dirty check correctly detected that setup change. A fixture-only repair
establishes the saved baseline, forces checks before/after navigation and retains
a real music-edit dirty control. Source114 execution below passed. Preserve
`ci-4102953-windows-preview-01.log`, `source113-preview-actual-terminal-01.json`
and `source113-quality-*-01.*`.

The user's next issue is Playlist/Piano linking. Shared Pattern data/previews
already existed; missing double-click entry, global instead of source-local
Piano cursor and callback mode's inactive Rack cursor are the actual gaps. A
single editor-linking candidate now implements the [documented interaction](PLAYLIST_TRANSPORT.md#open-and-edit-the-pattern-used-by-a-playlist-clip),
keeping existing trims and callback scheduling. Source/input/render acceptance
for that new candidate is recorded separately from the passed transport build.

### Linked editors — local acceptance and separate preview03

Source `81b2572` passed all225 actual headless UI/callback cases in403.84s,
including nine new linking cases, the retained ruler case and its new deterministic
dirty control. The latter emitted the original startup Night Orchard/one Pattern/
eight clips saved fingerprint11943620587148262240 versus the Untitled/two Pattern/
two clip fixture13432136539258871030. Restoring that old mismatch dirtied on idle;
the corrected baseline stayed clean across forced checks before/after seeking;
a real Rack step edit still dirtied the project without replacing its saved baseline.

Strict all-feature/all-target Clippy, actual compiler-artifact gate and matching
main/helper build each exited0. The same frozen binary passed all9 genuine WGPU
cases in14.19s on RTX3060/Vulkan. Root inspected four actual1498x936 frames: correct
Piano target/clicked source bar, both shared Playlist references updated, offset-
mapped SONG cursor and callback-driven Rack active-step borders. No native window,
physical audio or NI was used. The source clocks preserve Piano/Rack's independent
compiler periods and f64 Swing pair-start boundaries; callback scheduling is unchanged.

Separate preview03 contains the actual68,087,808B main and5,054,464B helper as
ordinary independent copies. Its five-second no-plugin helper smoke exited0/reaped;
Root did not open main. Format13 remains compatible with02;01 cannot read new13
saves, so preserve original works with Save As. Current shortcut/profile location
is at the top of this document. Per-placement follow-length remains an explicit
gap; this change does not expand existing trimmed clips. Cloud acceptance is
recorded below; native03/NOIRE listening remains unverified for this build.

Evidence under `target/backend-reliability-20261014`:
`playlist-pattern-editor-link-source-candidate-01/`, `source114-local-gates-01-*`,
`source114-local-resource-handback-01.json`, `source114-linked-render-01-*`,
`playlist-pattern-editor-link-render-01/` (raw PPM/PNG and Root visual review),
and `user-offline-preview-20261015-03.json`.

### Linked-editor cloud acceptance — both workflows succeeded

Exact source `63d2f99652171b9d4c1e839dd7a3e8ef7a43b19f` completed
[quality38746810086](https://github.com/wrench1997/DAW/actions/runs/38746810086)
at14:08:02Z and [preview38746809594](https://github.com/wrench1997/DAW/actions/runs/38746809594)
at14:23:44Z, both SUCCESS. Each passed2391 App+52 helper+13 protocol=2456 root
cases/14 explicitly ignored. The nine linking cases, original ruler and new
saved-baseline/real-edit dirty control passed. Quality also passed factory33,
SDK4/15, strict Clippy/build, helper14/editor80 and Linux readiness15+Unix1.
Its Windows session2/Default/WTSActive/visible desktop was eligible: genuine
PrintWindow before/after paint and repaint-change checks passed, not SKIPPED.

Preview passed367 Python cases=355 PASS/12 SKIPPED: four Unix descriptor cases
and eight private Saved13 input cases not distributed in Git. These are separate
from GUI eligibility. SDK4/15, strict Clippy, no-default-feature, static-CRT
release8m50s, exact ZIP integrity/PE/import allowlist and extracted sanitized-PATH
helper smoke all passed. Artifact upload was disabled, so no downloadable
published package is claimed. The local independent03 remains the delivered copy.

Full raw preview log: `ci-63d2f99-windows-preview-01.log`; actual run/jobs/results:
`source115-preview-actual-terminal-01.json`. Quality's complete Windows/Linux
logs and run/jobs/index are `source115-quality-*-01.*`. Earlier failures remain
recorded above. Subsequent acceptance-record edits change documentation only;
code/build scripts remain identical to this exact validated source.

### PrintWindow environment gate and actual outcomes (2026-10-15)

The native harness now probes the OS, current process session, WTS connection
state, visible window station and input/thread desktop identity independently
of capture. Linux/macOS, session0 and explicitly unavailable interactive desktop
facts return77 with `SKIPPED` and the reason. Active RDP is eligible; a physical
console with the RDS service stopped still needs the independent visible/input
desktop proof. Unexpected API failures, malformed facts, probe timeouts and any
real capture failure remain nonzero. Losing desktop support after preflight also
fails the real run. Only the workflow's exact exit77 branch becomes success;
formatting, compilation, tests, Clippy, Windows builds and preview packaging keep
their existing failure contracts.

Both preflight and each real PrintWindow run have separate five-second disposable
child deadlines (kill/reap); the native CI step also has a six-minute limit.
`--summary PATH` appends actual PASS/FAIL/SKIPPED rows and reasons. Dependent stages
that cannot run after a real failure are explicitly SKIPPED, while the overall
result remains FAIL. The ordinary local entry remains:

```powershell
python -B scripts/smoke_vst3_editor.py --helper target/debug/vst3-host-helper.exe --timeout 60 --summary target/native-editor-summary.md
```

Local real preflight found session1/WTSActive, visible station and matching
Default desktops. The first attempt correctly rejected a stale source-built
fixture receipt. Rebuilding both checked-in fixture variants offline exited0;
the next full run exited1 because direct PrintWindow on the standard child
Button returned false before/after edit. Those actual failures remain in
`printwindow-local-{native,summary,terminal}-02.*`.

An owned five-second diagnostic then isolated the target choice: the same
process's STATIC top-level returned success for flags0/1 with614/14 real colors,
while its standard child Button returned false and a uniform bitmap for both.
Both classes reported CS_PARENTDC, so class style itself is not an environment
skip rule. Microsoft documents [parent DC use by standard push buttons](https://learn.microsoft.com/en-us/windows/win32/gdi/parent-display-device-contexts)
and [PrintWindow's interaction with class DC styles](https://devblogs.microsoft.com/oldnewthing/20120604-00/?p=7463).
No class styles or system settings are changed.

The corrected harness calls the same PrintWindow API on the actual helper's
verified top-level container, then crops the original Button's actual mapped
client rectangle. PID, class, visibility, control ID, parent chain, GA_ROOT and
fixed geometry are checked before and after capture; container/panel identities
and mapped crop must stay identical. Only the cropped RGB pixels can satisfy
nonuniform drawing and before/after edit change. Other container pixels, an owner
or diagnostic window, or a different screenshot API cannot substitute for them.

The final complete Python suite passed359 with4 existing Unix-condition skips
(363 total, including80 editor cases); AST, PowerShell parsing and diff checks
passed. The final full local native harness actually exited0 in6.14s: both paint
stages, repaint change, interaction, state and Shutdown/EOF/crash cleanup all
passed. Fresh heavy processes were zero before handback. Keep
`printwindow-local-{native,summary,terminal}-03.*`,
`printwindow-owned-target-probe-01.*`, `printwindow-local-fixture-build-01.log`,
`printwindow-container-crop-handoff-01.json` and
`printwindow-ci-environment-handoff-01.json` under
`target/backend-reliability-20261014`.

Final code `9bb3110` cloud outcomes actually completed:

- [Preview](https://github.com/wrench1997/DAW/actions/runs/38703209158): SUCCESS.
  363 Python cases=351 PASS/12 SKIPPED (four Unix conditions and eight private
  Saved13-input exclusions), SDK4+15, App2333+helper52+
  protocol13 =2398 root passes/12 ignored, strict Clippy, no-default-feature,
  static-CRT release (8m11s), final ZIP integrity and extracted sanitized-PATH
  helper smoke passed. Upload was skipped; no artifact was published.
- [Quality](https://github.com/wrench1997/DAW/actions/runs/38703209376): FAIL.
  Linux SDK15/Unix1 passed. Windows App finished2332 passed/1 failed/12 ignored:
  `plugin_catalog_transport_tests.rs:107` rejected `receipt.healthy_retirement()`.
  The child was reaped, but the failing receipt field/scenario was not logged.
  The terminal transport's100ms natural-exit window is a possible mechanism,
  not an established cause. The same test passed in preview; that does not erase
  this failure. Later build/native screenshot stages were not reached, so cloud
  PrintWindow PASS/SKIPPED acceptance was not reached in this run. No failure
  rule was relaxed. This independent intermittent test remains undiagnosed.

Keep `ci-9bb3110-{windows-quality,windows-preview,linux-helper-readiness}-01.log`
and `printwindow-final-*-actual-terminal-01.json`. The environment-only `8f4f35c`
runs actually ended cancelled when the final capture correction superseded them;
neither is counted as passing evidence.


The unchanged code then actually passed [quality on documentation commit
`666ce96`](https://github.com/wrench1997/DAW/actions/runs/38706859200): factory32,
SDK4+15, root2398/12 ignored, strict Clippy, matching Windows bins, helper
harness14, editor harness80, fixture and no-default-feature gates all passed;
Linux SDK15/Unix1 also passed. The hosted runner was actually session2/WTSActive,
visible station, matching Default desktops. It therefore ran real PrintWindow:
paint before/after edit, the original Button crop's repaint change, interaction,
state and all three cleanup modes passed. This is real supported-desktop PASS,
not an environment-forced skip. Unsupported-OS/desktop and genuine failure/
timeout paths remain covered by the regression suite.

Together with the `9bb3110` preview success, local and cloud acceptance of this
capture fix is complete. The earlier independent retirement test failure remains
recorded and was not modified or weakened. Keep `ci-666ce96-windows-quality-01.log`
and `printwindow-docs-quality-{run,jobs}-actual-terminal-01.json`. This final
record-only update reuses the same verified code candidate.

The classification follows Microsoft's [process session API](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-processidtosessionid),
[WTS connection states](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ne-wtsapi32-wts_connectstate_class)
and [visible window station flags](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-userobjectflags).
[PrintWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-printwindow)
is synchronous and blocking, so its result is bounded by a child process rather
than treated as a desktop-availability signal.

Preserve `async-progress-readiness-candidate-01/applied-handoff-02.json` and
`async-progress-readiness-provenance-02/`. This guard tracks work reported through
IProgress; whether NOIRE reports that work and whether its onset becomes
repeatable require actual matching-bin acceptance. Native07 and generic parameter
ACK evidence do not qualify this candidate's opaque asynchronous restoration.

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
