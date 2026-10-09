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

The DAW VST3 backend requires a prepared maximum in 128..=2048 generally, and 256..=2048
for the exact Surge XT instrument compatibility case below. The generic floor is checked before
loading; after loading supplies actual factory identity, the selected floor is checked before
pending state, prepare-time stop/reconfigure/state restoration, and every reset's first safety CC.
A rejected fresh load may already have initialized a new instance. Insufficient capacity never
raises the requested maximum or changes device preferences. This restriction belongs only to
the DAW backend; ordinary worker processing remains fixed-Q128. Standalone vendor/helper processing at 17, 47, or other supported positive
sizes is unchanged. An explicit reset-origin request must fit that instance's prepared maximum;
it is never silently clipped, split into smaller calls, or enlarged past the maximum.

## Explicit owner-only contract

`ResetOriginSupport` is a pure, versioned capability query. It reports the configured maximum
and current processing readiness, and refuses already-known processor-output parameter poison.
An old helper's rejection is explicit; no ordinary Process, one-frame, zero-frame, restart, or
state-restore fallback exists. Loading may already have initialized a new plug-in, but negotiation
precedes reset-specific mutation and prepare-time state/lifecycle changes.

The worker calls backend reset preflight before the first of its 48 safety CC messages. VST3
checks the selected reset size, stopped finite transport, ready processing, and supported contract.
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
  clocks and free-running internal DSP can advance by the frame count. The default128 is
  2.667ms at48k; the exact instrument256 exception is 5.333ms at48k (255 samples beyond the
  legacy one-frame flush). These durations scale with sample rate; timeline position and
  bridge/PDC budgeting do not advance or change. This is explicit reset cleanup,
  not a general state-settling contract.
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

## Exact Surge XT 1.3.4 instrument cleanup bound

Only actual loaded factory UID `ABCDEF019182FAEB566D624153675854` with version exactly `1.3.4`
selects **one** explicit reset-origin256 operation. This UID is the genuine instrument class;
Surge XT Effects has UID `ABCDEF019182FAEB566D624153465854` and retains128. Other versions,
classes, display names, requested class metadata, and bundle paths do not select this policy.
Factory identity selects a compatibility rule; it is not binary-authenticity attestation.

[Surge 1.3.4's CC120 handler and engine](https://github.com/surge-synthesizer/surge/blob/release_xt_1.3.4/src/common/SurgeSynthesizer.cpp)
set a pending all-sound-off fade, decrement it by0.125 per internal block, then release both
scenes at zero. Its [default internal block is32](https://github.com/surge-synthesizer/surge/blob/release_xt_1.3.4/src/CMakeLists.txt#L9-L11):
eight blocks require256 samples. The existing128 cleanup can leave this release pending and
truncate a newly submitted immediate note. [Surge's reset callback](https://github.com/surge-synthesizer/surge/blob/release_xt_1.3.4/src/surge-xt/SurgeSynthProcessor.cpp)
resets block position without cancelling that fade, so merely moving stop/start did not fix it.

Preserved reset-origin diagnostic runs011–016 compare legacy1, reset128, lifecycle reordering,
a matched CC120-only contrast, and explicit256. Both legacy1 and128 truncated the immediate new
note; removing only CC120 in the diagnostic sustained it;256 sustained it with **all** safety
messages retained and unchanged state/native latency. Production retains every safety message,
input-origin ordering, stopped project position, output discard/loss reporting, and stop/start
order. One operation avoids a native-edit interleaving gap between two reset calls. There is no
extra ordinary Process, bridge/PDC change, generic warmup, or relaxed latency/tail query.

Historical `fd6f5b1` first/replay scheduled-graph successes remain valid within their measured
scope; they did not establish direct immediate-note sustain. Its original128 traces and all
failed diagnostic hypotheses remain immutable. The bounded256 policy requires its own exact
source-bound immediate-note offsets0/1/127, state, latency, held/future-input cleanup, and graph
qualification. It makes no guarantee for unknown versions or arbitrary internal tail behavior.

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

## Retained pre-correction real-plugin counterexamples

The reset-only validation retains initial failed assertions that treated every
replay as a fresh silent onset or assumed stop/start clears all plugin tails.
An actually advancing blank-source control distinguishes retained audio from new
source delivery. A further direct comparison finds immediate held NoteOn after
stop/start produces only a transient with BOTH legacy one-frame cleanup and the
then-new128-frame reset; a fresh no-reset instance sustains. Component state hash and
controller/component volume remain unchanged. Normal graph first/replay can pass
without proving this separate immediate retrigger contract. This historical limit
is not silently reclassified as passing by the later exact-identity256 correction. In the reset-only four-arm after-decay control,
a distinct G4 sustains after four seconds of genuinely advancing silent DSP;
this does not establish immediate-note success or a general required wait time.
Combined-source acceptance is separately bound.

## Corrected-source genuine acceptance

Reviewed correction `236f7846ff869f38815ec66ca138d7387adc4c1b` and integrated
`f08617092f74903862a9e7bf04698c2db49b523c` match all 136 frozen owner-source hashes;
127 runtime snapshot files and the two external harness appendices are separately
bound. The real tests use the ordinary production factory and automatic identity
selection, with helper `421a8d7d…73c41`, rather than manually injecting256.

All five invocations pass. Immediate distinct G4 notes at offsets0/1/127 sustain
for one second after acknowledged automatic reset, with no additional submitted
Process between the reset acknowledgment and NoteOn. State/controller values and
native latency0 remain correct. Held C4 plus future offset64 G4/sustain cleanup
leaves all192000 genuinely processed blank samples zero, with unchanged serialized
state and stopped host sample12345/PPQ9.25. Actual preparedmax128 loading is rejected
before ready for this exact instrument.

Four ordinary/routed fixed/changing configurations pass both first play and replay
at fresh FX native0 without initial Retry, with PDC4864/7296. Replay first PCM2432
is retained FX tail, not a new-note onset. A genuinely advancing blank-source
control checks1500 native quanta per endpoint and no source MIDI. Deliberate ordinary
Process1 still changes FX to32, triggers strict LatencyDrift, and requires actual
stopped Retry/new revision/epoch; honest post-replan PDC is7328. No metadata or fault
counter is fabricated. A separate fresh-state-before-first-Process case passes.

Changing debug callbacks retain12/14 ordinary and15/17 routed raw-core interval
overruns. Correct delivery is not wall-deadline, hardware or optimized-performance
certification. The source-only corrected archive has SHA256
`fe46a506eb1a936fcf0fc13f8f3ed571f31ff5c1e3339e5563c29a77578b70f5`
(339092 bytes,129 source/text files), including one preserved harness compilation
failure. Its original source/helper attribution remains separate from the335-file
historical diagnosis archive SHA256
`3ff69eaba1a11b8640e6b83534768d13f395430be92be6ec9085818b97e4320d`
(814776 bytes). Both are supplied as separate source-bundle validation appendices;
none substitutes for native UI/state or Windows acceptance.
