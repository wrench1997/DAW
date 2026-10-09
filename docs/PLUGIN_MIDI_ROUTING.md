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

## Verification status

Current focused implementation checks include deterministic generated-event/audio
alignment at offsets 0/1/127 across callbacks 1/31/64/127/128/255/256/512/2048, with
worker progress allowed **only between callbacks**; partial-quantum transport context
and epoch reset; Off versus port 0; stable fan-out/reordering; graph/automation rejection;
and persisted defaults/round-trip. These are not physical device or native GUI proof.

Final production graph, headless UI, real Stochas/Surge and aggregate gate results are
recorded with the reviewed source checkpoint; do not infer their completion from this
implementation description. Controlled or paced Linux helper tests are distinct from
physical audio-device realtime acceptance and from Windows/macOS Harmony acceptance.
A concurrent-build paced run also observed a 2,048-frame source deadline loss;
its failure receipt is retained. A large lookahead is not an unlimited CPU-overload
guarantee. Active-route acceptance checks exact output counts and safety telemetry,
not just a nonzero audio peak.

Native no-note checks isolate the instrument bus: this build's existing Master
metronome produces playback clicks even when the instrument is silent. Its peak
must not be misreported as an unreleased plugin voice. A metronome control is a
separate follow-up, not part of MIDI routing.
