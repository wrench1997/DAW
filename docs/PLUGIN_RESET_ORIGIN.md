# Stopped plug-in reset-origin processing

## Reason and compatibility boundary

The MIDI-routing implementation (`d812b6c`, subsequently integrated in the routing checkpoints)
added a positive one-frame stopped VST3 flush so queued native NoteOff messages are consumed even
when no more callback blocks arrive. The source documents this intent, but does not separately
justify the numeric choice of one frame. Historical cold-start receipts remain unchanged.

Official [Surge FX 1.3.4 source](https://github.com/surge-synthesizer/surge/blob/release_xt_1.3.4/src/surge-fx/SurgeFXProcessor.cpp#L251-L261)
switches its non-latent mode off when a positive block is not a multiple of its internal 32-frame
block. Thus the host's exceptional one-frame cleanup can request a native latency change from
0 to 32 despite all ordinary timeline DSP using Q128. The recorded cold traces establish the
0-to-32 change at an epoch reset; they do not timestamp the individual SDK call/restart callback.

Replacing one frame with 128 without changing reset input timing is unsafe: a previously queued
NoteOn at offset64 can execute after an appended panic NoteOff at offset0. Mapped sustain/other
parameter points have the same ordering hazard. Such pending input is possible on bypassed or
disabled slots, which admit controls but skip ordinary processing. A zero-sample replacement is
also insufficient: the pinned JUCE wrapper skips `processAudio/processBlock` when the host hides
all audio buses for a zero-sample call. Parameter-flush success does not establish note cleanup.

The DAW VST3 backend now requires a prepared maximum in 128..=2048, checked before loading or
prepare-time stop/reconfigure/state restoration. This restriction belongs only to the DAW's
fixed-Q128 adapter. Standalone vendor/helper processing at 17, 47, or other supported positive
sizes is unchanged. An explicit reset-origin request must fit that instance's prepared maximum;
it is never silently clipped, split into smaller calls, or enlarged past the maximum.

## Explicit owner-only contract

`ResetOriginSupport` is a pure, versioned capability query. It reports the configured maximum
and current processing readiness, and refuses already-known processor-output parameter poison.
An old helper's rejection is explicit; no ordinary Process, one-frame, zero-frame, restart, or
state-restore fallback exists. Loading may already have initialized a new plug-in, but negotiation
precedes reset-specific mutation and prepare-time state/lifecycle changes.

The worker calls backend reset preflight before the first of its 48 safety CC messages. VST3
checks the fixed quantum, stopped finite transport, ready processing, and supported contract.
It repeats preflight before native panic. The additive `process_reset_origin(frames, transport)`
operation then revalidates before staging or changing transport. Ordinary `MidiPanic` admission
and partial-prefix tracking are unchanged. A rejected panic does not run the reset Process or
stop/start and does not forge cleanup success. A safety-CC failure remains a slot fault even if
best-effort backend cleanup proceeds. Faulted workers do not resume themselves.

The operation is administrative and owner-only, after any scoped-domain session has rejoined.
Neither scoped control nor processor facade exposes it. It processes one positive silent block:
- Pending event and ordinary/mapped host-parameter offsets become zero for this call only,
  retaining FIFO/equal-offset order, matching the former one-frame input behavior.
- Native edits use their existing bounded staging and actual SDK-success acknowledgment.
- Project sample/PPQ position is stationary with `playing=false`. Requested continuous/system
  clocks and free-running internal DSP can advance by the frame count. DAW Q128 is 2.667ms at48k,
  127 internal samples more than the former flush. This is not hidden warmup or state settling.
- Audio is discarded inside the owner. Prior undrained processor MIDI/parameter feedback is
  invalidated as old-epoch data with separate counts. Reset-generated processor output goes to
  a private discard sink. Native UI/display feedback is not drained by this operation.
- Reports contain counts and errors, never output audio/MIDI/parameter values. Existing and new
  output MIDI loss stays sticky and is reported; output-parameter poison remains fatal. SDK
  failure still runs output cleanup, and only actual SDK success acknowledges native input.
- Uncertain post-dispatch IPC results invalidate local old-epoch MIDI, latch loss, and return an
  explicit failed report with unknown remote counts. They never retry or claim successful cleanup.
  Pure preflight rejection leaves pending state intact.

Only a checked successful report allows the DAW's existing stop/start lifecycle to proceed.
Existing worker latency/tail queries and strict callback identity/revision fences remain in place.
A fresh aligned Surge FX instance is expected to avoid the host-induced mode transition; an
already-32-frame instance is never relabeled as zero. Delayed or genuine latency changes still
require the existing visible fault/replan workflow. Stop/start is not a universal tail-clearing
contract for arbitrary plug-ins.

## Regression and qualification boundaries

Synthetic COM, worker, protocol, and fake-helper tests cover origin ordering, mapped sustain,
positive stopped transport and next-block positioning, output discard and sticky loss, native
acknowledgment/feedback, partial panic rejection, bypassed/disabled worker preflight, small
standalone maxima, and unsupported/failed helpers. Ordinary Process and zero-sample SaveState
keep their existing behavior and tests.

Genuine acceptance must use a new source/helper-bound receipt, independently checking fresh
Surge FX latency across epochs, an already-32 instance, held-note/tail/retrigger cleanup, source
MIDI discard, and ordinary/routed playback. Historical timing failures and one-frame traces must
not be overwritten or reclassified as passes. This change does not qualify native editor DSP
thread isolation, hardware callback bounds, or universal real-time/plugin compatibility.
