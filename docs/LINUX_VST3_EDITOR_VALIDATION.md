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
