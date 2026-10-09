# Linux native-editor functional preview validation

Historical ownership-preparation regression is **partial**: fixture and fresh-instance
vendor paths pass, while reused Surge first-note/getter consistency and native
content/container sizing fail on old and new helpers. See the [current state limits](PLUGIN_STATE_RESTORE_LIMITS.md).
The later exact Surge1.3.4 guard refuses used/editor-opened loads before helper
closure/mutation; fresh/rejected-load checks below are separate from successful
reused restoration. The measurements below retain their original source/helper identity; they are not
silently reattributed to the newer ownership implementation.

2026-10-09 UTC. This records the integrated production helper on the frozen MIDI-routing
runtime (`e54a6e4`), not the earlier staging helper. The tested standalone desktop was
Xfce/X11, `DISPLAY=:0`. No DAW renderer was launched or changed; no display server,
plugin package, or system dependency was installed by this test.

## Identified executable and inputs

- Production `vst3-host-helper` SHA-256:
  `9cc00c81f07dc88c7514c8fb2b016cd1f080803fefbb281e76cc43b0a16a3ffd`.
- Source-built MIT fixture: current checked-in source, lockfile and ELF bytes verified
  against `source-build.json` before the interactive test; both editor/no-editor variants.
- Genuine Surge XT 1.3.4 and Stochas 1.3.13: existing task-local official release bundles.
  These binaries and their assets are not included in Citrus source or release packages.
- Production source hashes, fixture receipts, raw commands/replies, state blobs, build
  logs and native desktop screenshots are retained in the task's Linux-editor QA records.
  Earlier staging evidence and unsuccessful QA commands are retained separately.

## Source and protocol gates

- 1,131 all-feature application tests and 21 production helper tests passed.
- 1,127 no-default application tests passed; both strict Clippy modes passed.
- Windows all-feature source check and Windows fixture source check passed. These are
  source checks, not Windows GUI or binary acceptance.
- Seven exact-production vendor run-loop regressions passed: factory/frame separation,
  load-failure cleanup, fd readiness/bounds, release outside lock, stale registration
  tokens, closed-registry resurrection and token exhaustion.
- Unix closed-stdout fail-closed unit test and four live-helper fd-isolation tests passed.
- 64 Windows/Linux Python smoke-harness regressions and 92 packaging tests passed.
- Pristine registry archive plus `CITRUS.patch` exactly reconstructs every upstream file;
  original MIT license/manifests are unchanged and provenance hashes are explicitly pinned.
- Sanitizers/Valgrind were not run. Cross-platform source checks do not replace them.

## Actual native fixture

The interactive gate passed real painted button input, native parameter value 0.25,
begin/value/end callbacks and dirty revision; focus away/back followed by both key press
and key release while the pointer was outside the plugin; factory and frame fd/timer
callbacks on the main thread; frame timers stopping on close while factory timers
continued; fresh-instance state restore; repeated Open/Close, actual titlebar close,
no-editor and Windows-owner rejection, EOF, Shutdown and forced helper termination.
Forced termination checks process/window cleanup, not graceful detach during a crash.
Screenshots show the genuine fixture before and after the click; no mock paint is used.

## Genuine vendor windows and state

Surge painted its own UI while receiving repeated 256-frame, 48 kHz generated-PCM process
requests. A real Global Volume drag changed parameter `1336600346` from 1.0 to
`0.8276583552360535`, with begin/value/end callbacks and native dirty revision 84.
The native window resized from 1178×735 to 1005×627 through a real WM border drag.
SaveState detached it and returned 51,929 bytes. A fresh plugin reset volume to 1.0;
restoring that state recovered the exact edited value, reopened generation 2 and resumed
finite nonzero audio output. Actual titlebar close reported `open=false`, zero dimensions.

The first processing interval returned 10,242 blocks / 5,243,904 channel samples, all finite,
peak 0.4151516 and aggregate RMS 0.0814000. After fresh-instance restore/retrigger, 13,372
blocks / 6,846,464 samples were finite, peak 0.1658413 and RMS 0.0390929. These runs do not
assert sample-exact waveform equality across a synthesizer reload. The real Output meter
was active; Surge's prior “Audio Output Unavailable” label meant an inactive plugin process
heartbeat, not a failed hardware-device probe.

Stochas painted its real grid. A native click created layer 0, row 115, step 0 with
probability 20, velocity 127 and length/offset 0. Native dirty revision advanced; SaveState
closed the view. Fresh-instance restore and re-export preserved that exact cell, and the
reopened vendor UI showed it. This GUI test did not repeat the separate MIDI-routing matrix.

## Material limits and preserved failures

**Realtime continuity did not pass.** The first Surge run measured a maximum process-request
wait of **346.9 ms during native resize**, against a nominal 5.33 ms block duration. Before
resize the observed maximum was 29.8 ms; the restored run reached 145.1 ms. PCM remained
finite and processing recovered, but main-thread vendor GUI/DSP serialization can stall
requests. The Python/JSON test harness is not an audio-device deadline measurement, and
these stalls must not be hidden by a dropout-free claim or by increasing buffers.

One hand-written QA SaveState request incorrectly used a map instead of the protocol's
unit command; the helper rejected it normally. The corrected command succeeded. Earlier
staging Surge state capture also exceeded the small fixture harness's 64 KiB line limit;
the separate real-plugin harness was corrected to a bounded 96 MiB response reader.
Neither was a vendor/helper crash. The initial packaging pin regression caught the changed
patch hash and passed after its reviewed expected hash was updated.

Still unverified: an actual Wayland DAW plus XWayland plugin session, compositor ownership/
activation behavior, per-output/fractional DPI transitions, physical audio devices and
sustained realtime playback under native GUI stress. This is standalone native UI/state
functional acceptance on X11, with a concrete processing-stall limitation.


## Integration identity and fresh source gates

The combined app at `fcf57b0e417cc18d7649577c68ef031d026bc6d5` adds the separately
reviewed metronome to this native-editor source. Cargo, plugin runtime, helper,
vendor and fixture bytes still match `88c498c58794c8c7407149dac3a9629cd78bacae`.
The fresh helper build is **byte-identical** to the above `9cc00c81…` GUI-tested
executable. This preserves its exact component evidence without claiming a second
native GUI run or validating the changed app's desktop/device behavior.

Fresh integration passes 1,137 app +21 helper +5 editor protocol +2 transport
protocol all-feature tests, 1,133 no-default tests, fmt, both strict Clippy modes,
app/helper build, ordinary helper smoke and Windows MSVC source cross-check.
All 195 Python tests pass, including four real descriptor-isolation cases against
that fresh helper. Seven vendor run-loop and one closed-stdout regression were
rerun from actual vendored source using an external test manifest, leaving original
upstream manifests unchanged. The verified registry archive plus pinned patch
reconstructs all 41 upstream files; original license/manifests remain byte-identical.
Windows native execution and optimized packaging remain new-candidate CI gates.


## Follow-on guarded state rejection

The historical source identities/results above remain unchanged. Reviewed guard
`c056dc5`, integrated as `60134a5`, was tested with exact helper SHA-256
`9dd18d74783e2ed13e947008d4dc4d5b2bdc0032948680c1cac678daed9aae60`.
Independent X11 QA passes fresh Surge1.3.4 restoration and used/editor-opened
rejection with the same XID/generation/geometry/native value, closed-used rejection
and normal reopen. Stochas reused native cell restore/re-export/repaint and the
trusted fixture also pass. The [state report](PLUGIN_STATE_RESTORE_LIMITS.md) records
exact scope and historical defects. This is a refusal before helper detachment or
state mutation, not successful used-instance restoration or preservation through
the legacy full-chain Admin caller. The 346.9ms resize stall remains; no realtime
continuity or latency improvement is claimed. Fresh combined gates are pending.


## Bounded native-edit transport follow-on

Reviewed48d3f97, integrated as87ceb06, has helper SHA256
`dca08353e3f23308d535a791c9fa2c89635ee683db29625fa8d2a3d0a988cbe8`.
Independent native QA verifies41 source hashes and the exact copied helper before
and after testing. A real stopped Surge edit changes volume from1.0 to
0.8691863417625427 (native−6.28dB), followed by20 polling rounds/80 dirty/value/
change/gesture requests and SaveState with no positive Process. The helper's
existing zero-sample flush applies the pending edit; newly captured state SHA256
`011ded9e14952b8a631e9e9b443fad6f0997daf641919862e95f5e860f414272`
contains component−6.27905654907227dB. Separate exact-blob fresh-instance tests at
17/47/128/256 frames retain matching getter/component and immediate first-note PCM,
with no positive warmup. The native session itself did not run continuous audio.

Same-XID/generation2/1178×735/menu preservation on rejected different-state Surge
LoadState passes, as do closed-used rejection and normal generation3 reopen.
Stochas row115/step3 native-created cell survives used/open reset/restore/re-export/
repaint. Trusted fixture mouse/focus/key release, callbacks/retirement, state,
close/reopen, owner/no-editor rejection and Shutdown/EOF/crash cleanup pass.
There are no submitted/applied wire counters; fence semantics are proved by source
fixtures and this independently observed outcome, not invented native telemetry.

This fixes polling versus pending-edit ownership, not thread serialization. The
350ms transport fixture is not a real native resize benchmark. Earlier346.9ms
processing stall and hardware/mixed-Wayland/sanitizer limitations remain. Legacy
full-chain Admin closes before backend LoadState and faults after rejection.
The old optimized matrix is not reattributed to the changed helper. Fresh combined source gates pass1,170 app +22 helper +5+2 protocol,1,166 core,
215 Python after receipt packaging,304 available vendor cases with one named fixture exclusion,26 doctests
and strict root/cross checks. New-helper default2048/fresh-state passes; subsequent exacte487a50 Windows source/preview passes while known native paint remains failed.

The separately source-bound [new-helper correctness receipt](../qa/native_edit_regression/RESULT.md) retains all four default2048 delivery passes, fresh-state immediate-note proof, and the11/14 changing-callback debug interval overruns for87ceb06/helperdca08353. The historical optimized4fdfbc2/244f622 results are not reattributed to this helper.


## Bounded parameter-container native follow-on

Reviewedbf573a4 integrates atfb7b91a. Exact41 production hashes and helper
`a29e4942b5fe027e9891a28e5d7f7611d2c5b2f12ef5f283599d15c4278aee64` bind the final
sparse-storage native receipt SHA256
`450ee6a6153866195d4d6297f6cb3b7da521bad6c006f8f5d0e8d5c7ada7f8fc`.
The earlier dense-index helper/native run is superseded and not counted as final.

Actual stopped Surge volume edit,20 polling rounds/80 requests, zero-positive-call
SaveState/internal zero-sample flush and fresh numeric/native/component agreement
pass. Newly authored state has deterministic SHA011ded9e14952b8a631e9e9b443fad6f0997daf641919862e95f5e860f414272;
its new creation is recorded separately, not inferred from that hash. Exact-blob
first-note checks at17/47/128/256 pass with no positive warmup; first nonzero samples
are60/18/73/55. Twelve fresh/used-active/used-stopped Surge compatibility cases and
separate Stochas deterministic-pattern state/MIDI checks also pass.

Same-window Surge rejection retains XID31457280/generation2/1178x735 and volume;
closed-used rejection and reopen pass. Native Stochas layer0,row115,step4 cell
(probability20,velocity127,length/offset0) survives empty reset and used-instance
restore/reexport/repaint. This single native cell is distinct from the deterministic
MIDI-pattern proof. Trusted fixture input/focus/key release/state/retirement/owner/
EOF/Shutdown/crash checks pass; all windows close, Shutdown0.

GUI session executes no positive Process or hardware audio; first-note PCM belongs
to the separate source-owner test. No new wire submitted/applied counters exist.
The helper remains single-threaded, with historical346.9ms resize stall and mixed
Wayland/DPI/sanitizer/device limitations unchanged. Parameter microbenchmarks and
allocation fences are separate source tests, not GUI continuity qualification.

The [exact parameter-helper regression](../qa/parameter_storage_regression/RESULT.md) binds124 production files tofb7b91a and new helpera29e4942. Its separate fresh-instance case uses historical captured input state104bc8ab… (not the newly authored011ded9e… native blob), with onset24/peak0.22329643368721008 and exact controller/component values. Both inputs have independent functional proof; their test identities and13/14 debug core interval overruns remain distinct.


## Checked-event admission native follow-on

Reviewed7d8a416/integrated7d7294b uses exact helper
`59b6bcbdb7a90b08fe5c8ebb2da2ba3ffe368c1089d70a6c7d4e7ab28b90d086`.
Native receipt original SHA256
`835ccf39085011738adf12cd5587ad4cac8d3968a0d0318023b1741a1b0dfc25`
binds41 source hashes verified before/after. Trusted fixture mouse/focus/key release,
callbacks/retirement/state/owner/no-editor/lifecycle/EOF/Shutdown/crash pass.

A newly created stopped Surge edit survives20 poll rounds and zero-sample SaveState;
normalized0.8691863417625427/native−6.28dB/component−6.27905654907227dB agree after
fresh restore. New state011ded9e… is separately creation-bound despite matching
prior deterministic bytes. Separate exact-blob first-note tests at17/47/128/256
have onsets25/46/24/23 without positive warmup;12 fresh/used-active/used-stopped
compatibility cases also pass. This is functional evidence, not latency certification.

Used/open Surge rejection preserves same XID31457280/generation2/1178x735/value;
closed-used rejection and ordinary reopen pass. Native Stochas layer0,row115,step5,
probability20,velocity127,length/offset0 survives used empty reset then exact restore,
reexport and repaint. Its deterministic-pattern MIDI proof is separate from that
single-cell native state. All windows close and Shutdown exits0.

The [separate four-case default2048/fresh-state regression](../qa/event_admission_regression/RESULT.md)
uses historical input104bc8ab… for its immediate-note case (onset25/peak0.21446438133716583),
not the new native011ded9e… blob. Both runs exit0 and all124 tested production files
match the final event source;12/14 changing debug core overruns remain. GUI session
has no positive Process/device audio, and no wire submitted/applied telemetry was
added. Payload allocation, helper serialization,346.9ms native-resize stall and
hardware/Wayland/DPI/sanitizer limitations remain. Reset policy is unchanged.

## Combined scoped-session/reset native smoke (2026-10-09)

A new run binds reviewed corrected source236f7846 and integratedf086170, all136
frozen source hashes before/after, and copied helper
`421a8d7dc8c793cc5e8ecd81e0c74392b24383eed08ed21a4d64fee8f9473c41`.
It started after the corrected-source freeze; the earlier1f021b1 preparation did
not execute native acceptance.

Surge stopped native volume editing,20 poll rounds,zero-sample SaveState and fresh
native/controller/component agreement pass. The newly created GUI blob has SHA256
`011ded9e14952b8a631e9e9b443fad6f0997daf641919862e95f5e860f414272`;
deterministic bytes matching an older blob do not replace this run's new creation.
Fresh-instance first-note PCM at17/47/128/256 passes without positive warmup, with
first nonzero frames11/44/64/40 and unchanged normalized0.8691863417625427/component
−6.27905654907227dB. These onsets are functional traces, not latency qualification.

Open-window and closed-used Surge guard rejections preserve window identity,
generation and value; normal reopen succeeds. Stochas used-instance native state
restores/reexports/repaints the newly edited layer0,row115,step7 cell. Its blob hash
is `671e314184b48ffa868b339a2d0c364c3092092c4537787760581e09328e7fe6`.
Shutdown exits0, with zero remaining plugin windows. There are113 recorded wire
exchanges; this run did not repeat the trusted fixture, EOF/forced exit, sanitizer
or Stochas MIDI matrix, and does not borrow those older receipts as current passes.

Raw native receipt SHA256:
`bd6365eb84026aa96050a0089663a736e1b2f3534d74f6e225c3fc6b7cb2a138`.
The path-normalized source-bundle receipt has SHA256
`de96e3194ea881bcc0589fb68be3c213aba35efc7d53634c34dfae742fa9456c`;
all measurements and identities remain unchanged. State payloads, screenshots,
executables and private HOME data are excluded from the portable receipt.
Automatic reset256 and immediate post-reset-note evidence belongs to the separate
production-graph receipt in the reset-origin guide. This UI run establishes no
DSP thread, hardware, mixed-Wayland or processing-continuity improvement; the
historical346.9ms resize-processing stall remains unresolved.
