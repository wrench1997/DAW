# Strict offline client: Stop A

Status: **UNVALIDATED SOURCE CHECKPOINT**. The preceding combined baseline is
source/mock tested at0363923. This new Linux-only client is behind the existing
`experimental-fresh-helper` feature, disabled by default. It has no App, bounce
RenderBackend, editor, audio-device or real-plugin caller. Source, codec, syscall
and fixture review plus durable archival precede its first mock execution.

## Closed authority and exact wire subset

`strict_offline::Client` owns the private FreshHelperSession and remains
!Send/!Sync/!Clone. The caller constructs it on its eventual owner thread. There
is no raw process/pipe/COM extraction, arbitrary HostCommand exchange, automatic
load, cold-load retry, transparent recovery or generic callback. Only the closed
CancelHandle is cloneable and transferable. Spawn always supplies the required
stdin/stdout pipes and inherited stderr. A setup failure after spawn returns the
same still-owning cleanup client; dropping it retains the child in the reserved
quarantine slot rather than proving reap.

Named methods load an exact32-hex selected class with maximum2048, select Offline,
read ResetOriginSupport, restore project state, start/stop, read native revision,
read the complete bounded parameter catalog, set a captured f32 base, read a raw
parameter, query checked audio/event layouts and latency/tail, admit ordinary
NoteOn/NoteOff at offsets0..127, and process one no-input mono/stereo Q128 block.
No new wire variants are introduced. Legacy helper/process-isolation paths stay
unchanged. Existing internal SaveState/ProcessBuses support remains private.
No reset, panic, editor, controller-mapped MIDI or warmup operation is exposed.

LoadPlugin reports bounded actual information without receipt-time clamping;
checked layouts remain separate owner-only queries. Reset support is configured
maximum/status evidence, not by itself proof of prepared buffers. Native dirty
revision is read evidence only, with no new edit-generation fence. No catalog
title immutability is assumed across first Process. Catalog numeric fields retain
their existing wire types, without invented min/max/default coercion.

## Borrowed requests and bounded replies

A private synchronous RequestView supplies one validation/count/emission path.
State, text and audio are borrowed for the call and never enter a queue or error.
The absolute deadline/cancellation check starts before traversal or encoded
allocation. There is no eager64MiB state clone. Caller storage is excluded from
the original256MiB charged in-flight exchange working-set limit; encoded and
materialized payload capacities remain charged. Returned replies retained by the
caller, fixed error envelopes, stack scratch, allocator metadata and total RSS
are outside that bound. No whole-process allocation or hard-time guarantee is
made. The existing96MiB line, base64, depth and cooperative checkpoint limits stay.

Catalogs contain at most4096 entries. Every exposed Parameter field is required
and preserved: id, name, value, min, max, default, unit, step_count, can_automate,
is_read_only, is_bypass and flags. Name/unit are each at most1024 UTF-8 bytes,
with at most8MiB combined strings. A fixed4096-ID stack array rejects duplicates
in the full allocation-free preflight before materialization; checks bracket its
bounded sort. Ordinary catalog JSON numeric fields must be finite. Scalar
ParameterValue separately preserves finite bits and nonfinite value classes;
base64 audio preserves exact IEEE bits.

Checked audio and event layouts allow at most32 buses per direction. Raw channel
counts, main/aux and Option<bool> activation remain intact. The client does not
project away unknown activation or apply a guessed stereo fallback. The later
renderer must validate these raw fields before projecting BouncePluginMetadata.
A bounded MetadataQueryRejected reason is an explicit terminal candidate failure;
its exact Unsupported/PendingTopology/InconsistentTopology category is preserved.
A complete known refusal or helper failure is not replaced by cancellation that
arrives afterward. No error implies rollback or authorizes replay.

Output audio shape is exact and never padded. Nonempty output-event arrays fail
before event payload allocation; empty-array loss and transport-applied evidence
survive. Nonfinite audio is transported, not accepted for publication here.
The future renderer must validate every sample, transport acknowledgment and loss.

`captured_base_matches` explicitly requires finite raw f64 and captured f32 values
in[0,1], then exact numeric equality after f32 projection; signed zeros compare
equal. There is no epsilon or clamp. Readback does not prove SDK Process applied
a queued parameter. The actual all-base ordering/admission policy is Stop B.

## Two deadlines, retained ownership and continuation

Each normal method receives the original absolute job deadline. Healthy disposal
uses min(protocol deadline, cleanup deadline) for its single reply-free Shutdown
write, closes pipes, and observes/forces/reaps under the separate cleanup deadline.
An expired or cancelled protocol budget cannot grant a new request merely because
cleanup time remains. Terminal/partial exchanges send no further protocol. A
healthy Stop is an explicit separate method whose future caller must also use
min(job deadline, cleanup grace).

The first100ms pipe-close grace never renews. `continue_cleanup` is valid only
after disposal began and pipes closed. It performs at most one try_wait and one
eligible kill attempt, with no Shutdown, sleep, poll or fresh grace. It may retry
a prior failed kill after the original cleanup reporting deadline. A successful
kill remains Pending until a later concrete reap. Child identity/reservation and
first Shutdown/kill/wait evidence survive Pending, OS errors, Rust unwind and
quarantine. Completed sessions cache the proven ExitStatus and evidence, so
repeated cleanup cannot downgrade Reaped to Empty. No global lock spans I/O/reap.

One global strict-child reservation is acquired before spawn. Pending or abandoned
ownership blocks another strict clone. No eviction, background reaper, detached
I/O thread or PID-only cleanup receipt is added. Existing private disposal mocks
use a wrapper over the same kernel. Drop remains outside the checked-success
contract. Spawn, allocation, scheduling, syscalls and reap are not universal hard
deadlines. The supported host must retain ignored SIGPIPE; this API checks the
actual disposition and never changes it.

## Scope of the one-shot cleanup revision

The reviewed future adapter is a disposable, never-reused offline clone with no
external MIDI output. Root explicitly approved healthy checked Stop/Shutdown and
proven reap in place of its earlier bounce panic/reset128-or-Surge256 cleanup
oracle. That future lifecycle oracle must be updated with actual adapter tests;
this transport/client checkpoint does not claim the old reset evidence satisfies
it. Reusable realtime plugin reset remains unchanged. Faulted or partial exchange
paths send no music, reset, Stop or Shutdown. Pending cleanup forbids successful
render/WAV publication and replacement admission.

Stop B's RenderBackend/cancellation-registration/typed-cleanup wiring, selected
occurrence origin/history/media integration, and genuine plugin qualification
are separately gated. Tagged Step2 and observed-age metadata activation remain
deferred. No user-facing rendering control is enabled here.

## Planned source/mock evidence

The new corpus covers exact/+1/malformed catalog and layout limits, duplicate IDs,
unknown fields and raw main/activation differences; typed refusal/cancel ordering;
ordinary-note restriction; copied-state-free early rejection; exact audio shape;
public spawn-failure/abandonment ownership; split deadlines; partial Shutdown;
cancellation before/during Stop and before Shutdown; repeated kill/wait errors,
nonwaiting cleanup, cached reap and Rust unwind. Cooperative child ledgers must
exit successfully without force kill; force-cleanup cases are labeled separately.
The external compiler runner uses actual dependency APIs, positive controls and
marked-expression diagnostics, including feature absence and ownership boundaries.
Existing three source-linked runners change only their expected module count20.
No execution result is claimed until the separately frozen test receipt exists.
