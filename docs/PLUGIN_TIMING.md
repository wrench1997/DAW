# Callback-safe plug-in worker timing

This slice replaces the route-dependent one-turn/16-turn production worker timing
with one prepared operating plan for all physical Generator, Mixer insert and
Master workers, including the existing stopped direct-MIDI monitor. Serial slots
inside one worker add their reported plug-in latencies, **not extra worker bridges**.

## Contract

- Fixed processing quantum Q = 128 frames.
- Explicit whole-backend-callback budget B = 128, 256, 512 or 2048 frames.
- Guard G = ceil(sample_rate × 4 ms / Q), at least one quantum.
- Worker lookahead K = ceil(B / Q) + G.
- Accumulation plus worker latency L = (K + 1) × Q frames per physical worker.
- Supported plan rates are finite whole-number rates from 1 through 384000 Hz.
  Unsupported rates are rejected, never silently converted to another device setting.

At 48000 Hz the profiles add 512 / 640 / 896 / 2432 frames per worker respectively,
about 10.67 / 13.33 / 18.67 / 50.67 ms. A one-hop MIDI producer → instrument path
adds 2L, before reported plug-in latency and subsequent physical FX workers. The
Settings estimates use the actual sample rate. A tiny sample rate may therefore
have very large real latency despite the same frame counts.

These are **provisional operating limits**, not hardware guarantees or passed
low-latency certifications. CPAL/driver buffer requests, including ALSA Fixed,
do not guarantee a maximum future callback size. The unknown/unobserved default
is B=2048. Selecting a smaller profile requires observed callback evidence that
fits it; a past maximum still cannot promise the next callback will fit. Changing
this profile does not change the requested device, device buffer or sample rate.

Every complete raw backend callback is admitted before internal chunking or any
plug-in submission. B+1 fails closed and latches a visible replan/retry fault.
Callbacks above 2048 are unsupported; splitting them cannot make them admissible.

## Preparation and activation

The control side prepares the plan with its monotonic request revision and the
stopped timeline chase. The callback validates its exact canonical B/G/K/L values,
sample rate and still-current requested revision. Existing candidate preflight
binds the plan to exact physical endpoint manifests and coherent latency snapshots.
Only the existing fresh-epoch transaction switches timing, endpoint lookahead,
PDC, content-time transport, route bindings and Q128 control histories together.
A callback-acknowledged topology revision also binds each prepared chase. Accepted endpoint
install/remove/replace/route commands suspend plug-in processing while a matching candidate
is prepared. During this gap, paused MIDI and parameter service cannot submit against cleared
latency bindings. Only a newly prepared chase with exact Generator and all Mixer slot manifests
clears the gap; ordinary edits need no explicit Retry. An accepted Timeline clear or non-plug-in
resync also suspends old bindings without mislabeling that gap as a physical endpoint fault. Unsolicited identity drift still faults.
A full retirement queue leaves the lifecycle command and its ownership queued, without entering
this gap or destroying any resource on the callback. An obsolete candidate cannot silently apply
a prior profile or authorize a replacement accepted after that candidate was prepared. Seek/loop epochs
reset partial quanta and queued output; a fault needs an explicit newer timing
revision, rather than being cleared by a loop or ordinary same-plan seek.

Bypass/enabled/wet changes use an asynchronous stopped state-capture transaction instead of
mutating an admitted worker's latency identity. New generic edits and conflicting lifecycle
operations are fenced; existing edits drain before the exact stopped epoch. Every slot in the
replaced physical chain contributes a tagged live-state receipt. A fresh worker receives those
captured blobs and reconciled parameter bases. All slots must finish load/prepare, report coherent
latency, and acknowledge parameter replay before the endpoint is transferred. Preparation retains
the authoritative captured blobs; it does not re-save a not-yet-processed replacement or claim
arbitrary plug-in state equality. No hidden Process block is inserted as a generic state-settling
operation. Old-chain capture verifies worker epoch both before and after the backend call.
An epoch-tagged snapshot error does not mark the host processor slot faulted. Failure, cancellation, timeout or stale identity
preserves the previous configuration and chain and leaves playback paused. A connected MIDI-port
source/sink cannot be bypassed or disabled in this slice; disconnect its route first. FX along
that path remain configurable. These transactions are separate from explicit fault Retry and
cannot silently restart a faulted session.

Routed instrument transport is delayed by the producer's actual prepared L.
Mixer/Master transport uses graph join latency; serial slots subtract preceding
active plug-in latency within the same worker. No hardcoded 2176-frame route delay
or route-presence lookahead selection remains in production audio code.

## Bounded resources and health

At the largest supported rate and budget, K=28 and the largest whole-callback
submission burst is 16 quanta, including every initial partial-quantum phase.
K + ceil(B/Q) + 2 = 46 fits each preallocated 48-block input, output and future
queue. Compile-time size ceilings keep all three queues plus endpoint scratch
under 4 MiB per physical endpoint, and each adapter under 512 KiB. With the existing
96 active physical-endpoint maximum this storage is bounded below 432 MiB, independent
of how many slots share each endpoint. This is an active-graph bound, not a whole-process
memory cap: off-thread prepared, queued and retired endpoints require additional storage,
as do device/PDC buffers and plug-in-internal memory. Preparation/allocation and retirement stay
off the device callback; there is no queue growth, plug-in call, IPC, mutex, join or
resource destruction in the timing/health callback path.

Device XRUN reports and plug-in processing health are separate. Priming, running,
faulted and recovered state identify the current epoch/revision; cumulative
miss/input-loss/output-loss/fault/recovery counters retain earlier evidence.
The first fault is latched independently of event queues, with endpoint, epoch,
expected sequence, raw callback frames, active B/K, timing revision and reason.
A latency publication observed after the initial callback refresh is classified before
Timeline cleanup clears its binding, including the second endpoint-batch precommit read and
normal/paused render refresh. Same-callback paused safety and parameter service stop immediately
once a fault or suspended binding is established; an unrelated Timeline error without observed
plug-in drift does not manufacture a plug-in fault.
Only exact epoch/sequence/latency output may become audible; missed or invalid
output cannot silently bypass an effect with dry audio. A worker safety latch
requests off-thread note cleanup even when no subsequent block is submitted.

“Retry audio processing” prepares a stopped fresh epoch and revised timing plan.
It never automatically restarts the music or its loop. If the selected budget is
smaller than an observed callback, select a larger budget before retrying. A truly
failed plug-in backend may still require removing/reloading that plug-in.

## Retained limits

MIDI routing remains playback-only and single-hop, with fan-out; routed tempo
and sink parameter automation, stopped live multi-plugin chaining and seamless
looping remain outside this slice. Short loops can remain silent during preroll.

Buffering does not fix the helper's current GUI/DSP main-thread serialization.
The separate genuine Surge editor result measured a **346.9 ms process wait during
resize** (29.8 ms baseline). Native-editor realtime qualification remains blocked;
this timing work must not be presented as hiding or solving that stall. Physical
audio devices, speakers, Windows/macOS execution and Harmony Blueprint are not
certified by Linux headless/paced tests.

## Verification

The new tests explicitly cover whole-callback-only worker progress, every Q128
initial phase and profiles at 48000/384000 Hz, sizes 1/31/64/127/128/129/255/256/512/2048,
changing partitions, MIDI offsets 0/1/127 aligned with audio and declared graph PDC,
ordinary Generators, routed fan-out, serial FX slots, Master, stopped direct MIDI,
B+1 before submission, allocation/deallocation guards around command processing, retirement
backpressure, refresh, activation/rejection and render, stale candidates, partial
quanta, endpoint/latency identity, cleanup and explicit fresh-epoch retry. Settings
are exercised by real egui pointer/AccessKit tests, with separate image review.

Exact source-bound final gates and genuine plug-in pacing results are recorded
with the implementation delivery. The historical e54a6e4 routing receipt is
unchanged and cannot certify this new timing implementation.

### Preserved genuine-plug-in evidence and Experimental profiles

The source-bound `f68b9a0` unoptimized Linux harness, using official Stochas, Surge XT
and Surge XT Effects binaries, passed **13/16 quiet** one-second profile cases and
**10/16 under four-thread controlled CPU load** with the same executed binary.
B2048 passed all four ordinary/routed × fixed/changing cases under each condition.
It remains the default operating ceiling, not a realtime or hardware certification.
The smaller B128/B256/B512 choices are explicitly **Experimental** in Settings,
with a visible warning that callbacks fitting B do not guarantee worker deadlines.
No guard, hardware-buffer request or default was retuned to erase failures.

Quiet failures were B128 ordinary/fixed (sink sequence 336), B128 routed/fixed
(source sequence 1), and B256 ordinary/fixed (sink sequence 70). Under load all
four B128 cases missed the FX sequence-1 deadline, B256 routed/changing missed FX
sequence 1, and B512 routed/fixed missed sink sequence 2. One-second B512 quiet
passes did not establish longer-run reliability: separate four-second cleanup
and further Retry runs also encountered deadlines. All are preserved as negative
product evidence, alongside earlier attempts, rather than replaced by later passes.

Every B+1 callback was rejected before submission; explicit restart after that test
passed B256/B512/B2048 but encountered a B128 sink deadline. Blank-source cleanup
emitted no source MIDI, and a retained run's natural post-FX tail fell below 1e-6
by 1.75 seconds, before a later deadline. Forced silence after a fault is not counted
as tail-decay proof, and the longer cleanup run is not labeled a full pass.

These captures use an unoptimized, source-linked test wrapper with allocation-based
recording outside the callback allocation proof. Measured wrapper/render time and
callback-start lateness are reported separately from plug-in DSP time. Optimized,
bounded-capture measurements are still pending. Fresh-instance Surge state restore
preserved the immediate first note and captured volume after actual processing;
this does not validate live restore into an already-used instance. Native editor
serialization, physical-device performance and optimized-release behavior remain
unqualified. The full raw receipts retain source/build and helper/harness hashes.


The [historical debug receipt tree](../qa/plugin_timing_debug/RESULT.md) preserves
all six attempts, exact per-case measurements, build/helper/source identities and
path-only normalization custody. Its pinned publication inventory is validated
during package creation and extraction. These f164/f68 results used the earlier
helper and are not silently reattributed to the later ownership/Surge-guard helper.
Combined-source and optimized measurements must be recorded separately.


### Combined ownership/guard helper check

Exact combined runtime **4fdfbc2ea5c828fd9c329f31e9be203810fa62c7** is bound to123
production source files. Its matched preallocated debug harness SHA256 is
`d38e6a2b1d2f323a7fda9d1e36addbc9849b0e119b3f424d1391f8a80695608f` and helper is
`9dd18d74783e2ed13e947008d4dc4d5b2bdc0032948680c1cac678daed9aae60`.
The [separate combined summary](../qa/plugin_timing_combined/README.md) preserves
the exact original generated JSON and raw-record hashes; it is not the old matrix
or a complete new reproduction-harness distribution.

All four B2048 ordinary/routed × fixed/changing delivery cases passed actual native
FX0→32 latency fencing, stopped Retry/new revision/epoch, exact PDC/events and375
completed blocks per endpoint (357 exact +18 startup). PDC is4896 ordinary and7328
routed frames with one downstream FX worker. Fresh restored Surge immediately
played its first note with exact controller/component volume (sample60, peak
0.2113886625 over1024frames). Matched B128 had two PASS and two FX DeadlineMiss
failures; both fixed cases failed (sequence151 ordinary,1 routed). No capture
overflow occurred.

These are **delivery/state results**, not interval qualification. The two passing
B2048 changing cases still recorded14 callback-core interval overruns each; B128
changing cases recorded18/24. Preallocated capture separates capture cost and
outer scheduling, but the wrapper and builds are still unoptimized. Release
qualification is a separate ongoing experiment. No smaller-profile reliability,
dropout-free operation, physical device or native-GUI continuity is established.


### Later helper changes do not renew these measurements

The native-edit transport follow-on48d3f97/87ceb06 changes vendored helper behavior
without changing App/timing source or wire messages. All earlier debug, combined
and optimized matrices retain their exact original source/helper identities. The
optimized12/16 quiet and12/16 loaded result was added as a separately attributed
post-freeze appendix to the244f622 source delivery; it is not a current-helper
performance result. The new helper requires only a separately recorded bounded
default2048/fresh-state regression for this correctness checkpoint, not a relabeled
performance pass. The2048 default and Experimental128/256/512 labels are unchanged.

The separately source-bound [new-helper correctness receipt](../qa/native_edit_regression/RESULT.md) retains all four default2048 delivery passes, fresh-state immediate-note proof, and the11/14 changing-callback debug interval overruns for87ceb06/helperdca08353. The historical optimized4fdfbc2/244f622 results are not reattributed to this helper.
