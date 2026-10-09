# Generator MIDI-port routing

This first implementation connects one MIDI-producing channel device to one or more
instrument channel devices. It targets the Harmony Blueprint-style workflow, but
Harmony Blueprint itself has **not** been installed or tested here. Its Windows/macOS
binary and native editor still require platform-specific acceptance.

## Workflow

1. Load a VST3 instrument or a scanned VST3 device with a real MIDI output bus into a
   Channel Rack device slot. The scanner uses the actual default VST3 class and bus
   metadata; filenames are not evidence of MIDI capability.
2. Stop playback. Open the channel Inspector and find **PLUGIN MIDI PORTS**.
3. Set the producer's **Output** port, then set the instrument's **Input** to the
   same number. Port **0 is valid**. **Off** is a distinct value.
4. Optionally select **Mute device audio monitor** on the producer. This silences its
   own audio while continuing to process and emit MIDI. Bypass and Channel Mute/Solo
   have different semantics: a muted/bypassed/failed producer stops its routed notes
   safely.
5. Wait for callback-confirmed activation and start transport playback. Stopped live
   MIDI chaining is not implemented in this slice. A numbered input is **exclusive**: that
   instrument's Piano/step notes, direct preview notes and hardware MIDI input are
   inactive while the plugin port is selected. Play the producer's channel instead.

A device must expose the required bus in its **loaded backend**, not merely in an old
scan cache. Missing or changed endpoints cannot acquire another instance's route.
Native-editor state capture, recording, project replacement and pending device
changes block port edits; valid changes are one Undo transaction. Undo/Redo cannot
change the port configuration while playback is running.

## Timing and realtime boundaries

- VST calls, helper IPC, process reset and shutdown remain off the device callback.
  Callback routing uses preallocated, bounded data only.
- Processing quantum is 128 frames. In projects with active MIDI port edges, source
  and destination Generators use a fixed 16-quantum worker lookahead plus one quantum
  of accumulation: **2,176 frames per endpoint**. Existing Mixer insert and Master
  workers use this conservative lookahead in the same project so downstream FX do
  not depend on completing IPC within the current callback.
- Minimum source-to-instrument audio latency is therefore **4,352 frames**, about
  **90.7 ms at 48 kHz**, before reported plugin latency and downstream inserts. Each
  worker-backed Mixer stage adds its bridge and plugin latency. This is not a
  low-latency performance mode. The Inspector shows final graph output latency.
- Projects without plugin MIDI routes retain their previous one-quantum worker
  bridge timing. Non-routed Generators also retain that timing. **This legacy path
  is not realtime-qualified by routed-mode results:** the paced Linux Off baseline
  observed deadline misses even with 128-frame callbacks. General bridge timing
  remains a separate follow-up.
- The supported maximum *whole device callback* is 2,048 frames. Larger callbacks
  fail routed MIDI closed even if the engine splits them internally. A lookahead
  prevents deterministic same-callback misses; it does not guarantee an overloaded
  CPU or helper will meet its deadline.
- Output events travel inside their originating audio block and are accepted only
  for the exact epoch, sequence and latency attestation. The adapter schedules each
  event at the corresponding audio-output stream position, then converts it to the
  destination callback offset. It does not forward whichever event arrived most
  recently and does not promise zero-latency or universal sample-accurate VST behavior.
- Sink transport describes the incoming content position, delayed by the producer's
  2,176-frame bridge. Negative preroll positions are supported. Per-block BPM,
  sample/quarter-note position, play state and existing 4/4 signature travel in the
  same helper audio request; the helper must acknowledge applying them. Serial FX
  slots additionally subtract the preceding active plugin latency from their context.
- Seek and loop boundaries start a new epoch and refill every bridge. Pending audio
  is discarded; playback has a new preroll. Native NoteOff releases plugin voices,
  but a plugin's own release tail can remain briefly after a restart. Seamless looping is not supported,
  and loops shorter than the combined bridge latency can remain silent.

## Explicit first-slice limits

- One producer per numbered input; fan-out is supported. Multiple producers on one
  destination, self-loops, cycles and multi-hop MIDI processor chains are rejected.
  This avoids ambiguous ownership of same-channel/same-pitch note-offs.
- Constant project tempo only for routed projects. Enabled Tempo automation and
  parameter automation targeting a routed instrument are rejected, including empty
  enabled lanes that could later acquire points. Source parameter
  automation retains its existing timeline contract. There is no new time-signature
  editor; the project uses 4/4. Variable-tempo plugin transport has not been fixed
  globally for ordinary projects by this change.
- MIDI1 channel-voice events on VST3 event bus 0: notes, CC, pressure, program change
  and pitch bend. Original MIDI channel and in-block offsets are preserved; velocity
  is narrowed to MIDI1 resolution. Controller delivery to VST3 instruments depends
  on the instrument exposing the corresponding IMidiMapping assignment. No MIDI2,
  lossless MPE/note IDs/tuning, SysEx,
  additional event buses, filtering or channel remapping is claimed.
- Producers reporting nonzero audio latency are rejected until a separate event-
  versus-audio latency contract is verified.
- Mixer plugins keep their existing audio FX order. They cannot be selected as
  plugin-MIDI producers or destinations in this slice.

## Failure handling

A missed/invalid/old block cannot emit its stale MIDI. Event loss, unsupported output,
queue overflow, source failure, capability loss, replacement, mute and route changes
trigger destination cleanup. An independent atomic worker latch cannot be displaced
by ordinary MIDI events. It resets the backend even without a subsequent audio block,
blocks queued MIDI, and silences the affected sink until a new transport epoch. VST3
cleanup queues native NoteOff messages through `midi_panic`, consumes them in a
stopped worker process flush, discards flush output, and performs the existing reset.
Sustain/all-notes/all-sound-off controllers supplement native NoteOff cleanup.
The Inspector reports a routing fault and directs the user to fix the cause and
stop/restart transport. A plugin may change its declared latency only after its
first DSP block. The old graph fails closed; a stopped fresh-epoch activation
rebuilds PDC from the coherent new latency. The genuine Surge Effects 0-to-32-frame
transition exercised this recovery. Suppression and fault ownership from an old
epoch clear only after that endpoint has successfully reset; same-epoch or failed
resets keep the safety state.

The bridge output batch is bounded at 128 events; the exclusive sink reuses the
reserved Timeline lane (96 events per 128-frame quantum) without consuming the
independent System safety lane. Overflow is a safety failure, never silent note-off
loss. Same-offset note-offs precede note-ons; equal-priority events retain producer
order without allocating sort scratch.

## Project compatibility

Project format is **v12**. v10/v11 projects load with all plugin ports Off and audio
monitor enabled. New saves require a v12-capable build, preventing older v11 software
from silently ignoring routing fields. Keep a backup before evaluating this build.
Ports and independent audio-monitor mute round-trip with stable plugin identities.
If a saved project has an unsupported/inconsistent port graph, loading disables its
input ports safely and reports the exact reason; unrelated musical content remains.
Runtime capability checks still run again after reopening and loading the devices.

## Verified source checkpoint

Implementation source: `e54a6e49fe02b78ff299ce56dddd781925f7fe3c` (Linux).
The following results belong to this exact code, not to a future combined build:

- **1,131 application + 15 helper + 5 editor-protocol + 2 transport-protocol tests**
  passed with all features; **1,127 application tests** passed without default
  plugin features. The 13 production-worker routing cases cover fan-out, reverse
  endpoint order, monitor mute, exclusive input, overflow/fault cleanup, bypassed
  FX, unused output MIDI, oversized callbacks and fresh-epoch recovery.
- Deterministic generated-event/audio alignment at offsets 0/1/127 across callbacks
  1/31/64/127/128/255/256/512/2048, with worker progress allowed only between callbacks;
  partial-quantum transport, same-epoch safety retention, Off versus 0, save/load and
  incompatible-automation rejection are covered.
- Both strict Clippy configurations, formatting, app/helper build, helper protocol
  smoke, Windows MSVC **source check**, and 167 Python checks passed. This is not a
  Windows execution or package-release result.
- Actual headless Inspector input selected 1, scrolled to 255 and back to 0, selected
  Off, cleared an unavailable Input to Off and changed independent audio-monitor
  mute. The 1498×1248 offscreen Vulkan image was inspected; PNG RGB matched readback
  exactly. No native plugin window, OS audio device or physical input was involved.

### Genuine Linux VST3 acceptance

The exact source also passed **5/5 independent real-plugin tests** using official
Stochas 1.3.13, Surge XT 1.3.4 and Surge XT Effects 1.3.4. These use the production
DspState/Timeline/PluginChain/helper path and a paced offline callback driver, with
no wait inside a callback. They do not establish physical audio-device performance.

- Active Stochas-to-Surge routing passed callbacks 128/256/512/2048 at 120 BPM and
  512 at 60 BPM. Each two-second endpoint run submitted 750 Q128 blocks and consumed
  734 exact plugin blocks plus 16 startup blocks, with zero additional fallback,
  missed deadlines, queue gaps or latency drift in the final quiet run. MIDI spacing
  followed actual tempo and the first source output arrived at frame 2176.
- Surge Effects initially changed latency from 0 to 32 frames. The old graph stopped
  safely. Stopped fresh-epoch reactivation rebuilt PDC and restored the real FX path
  with 750 submissions/734 exact outputs/16 startup blocks per endpoint and no new
  drift or misses. The source-to-synth-to-FX bridge minimum is 6528 frames plus that
  32-frame plugin latency; this is intentionally high latency.
- Stop/seek and a genuine blank-source restart produced no new routed notes. The
  isolated Surge bus became exactly silent after its short release. Single held-note
  and overlapping C4/C/E/G retrigger/chord cleanup also became exactly silent.
- Deliberately unpaced 2048-frame callbacks caused a real sink deadline miss after
  three callbacks. The published fault count became 1 and subsequent output was
  exactly silent. Stop/new epoch cleared the fault and restored 375 submitted blocks,
  359 exact outputs plus 16 startup blocks per endpoint, with no new missed deadline.

### Retained limitations and negative observations

**K16 routed-mode acceptance does not qualify the old Off-mode bridge.** The final
paced Off baseline had 15 sink and 10 source deadline misses at callback 128. Its
instrument output remained silent as expected, and every fallback was recorded;
that case is functional routing-Off/accounting evidence, not a realtime pass.

A prior concurrent-build run lost a source output batch at callback 2048. Its exact
cause was not captured before the assertion; the negative receipt is retained.
The same copied binary passed the quiet comparison. The separately instrumented
unpaced overload above verifies deadline cleanup/recovery, but cannot retroactively
prove the original loss's cause. There is no unlimited CPU-overload guarantee.

Native no-note checks at the reviewed `e54a6e4` routing checkpoint isolated the
instrument bus because that checkpoint's Master metronome clicked automatically.
Its peak must not be misreported as an unreleased plugin voice. The subsequent
[metronome control](METRONOME.md) defaults Off; this does not change the historical
acceptance source or qualify the old Off-mode bridge. A callback-safe general
bridge remains separate work.

Harmony Blueprint, Windows/macOS execution, physical speakers/devices, native
plugin UI, seamless loops and stopped live multi-plugin audition remain unverified
or outside this slice. Full raw receipts, hashes and the source-only native harness
are preserved with the independent real-VST3 validation deliverable.


### Integrated source and portable evidence

Reviewed implementation `e54a6e49fe02b78ff299ce56dddd781925f7fe3c` plus acceptance
docs `9f4a64b3ea10d4eec4eb451d51a67eea15a36c51` integrate as `5e8ff0f`. All
production source/Cargo/vendor/test bytes are preserved. Fresh Linux integration
gates pass 1,131 app +15 helper +5 editor protocol +2 transport protocol tests,
1,127 no-default app tests, fmt, both strict Clippy profiles, build, ordinary helper
smoke, Windows source cross-check and 187 Python tests. Exact Windows execution
remains a separate candidate gate. The [portable real-plugin receipt guide](PLUGIN_MIDI_ROUTE_VALIDATION.md)
contains five-test results, all negative observations, import provenance and a
plugin-free integrity/source-byte verifier. Linux native editors and a metronome
toggle are separate changes and are not part of this checkpoint.
