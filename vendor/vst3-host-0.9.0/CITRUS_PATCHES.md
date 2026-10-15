# Citrus native-editor and transport extensions

## Exact upstream source and license

- Package: `vst3-host 0.9.0`, MIT; original `LICENSE` retained unchanged.
- Official registry archive: <https://static.crates.io/crates/vst3-host/vst3-host-0.9.0.crate>
- SHA-256: `6ec579d54bd13b83c60c1fd8bb756cf234e36ccbfb4833ff756b417e64db7fea`.
  This was checked against the original Citrus `Cargo.lock` registry checksum before extraction.
- Upstream repository: <https://github.com/HelgeSverre/rust-vst3-host>.
- Package `.cargo_vcs_info.json` identifies commit
  `ed054908cfe057694d8cf037d0c39dfb5eb4c2ca`, package directory `vst3-host`.
- The original package sources and manifests are retained. No binary plugin or vendor SDK
  is included. The application manifest explicitly selects this directory with `[patch.crates-io]`.
  The path override is recorded in the root lockfile; upstream version stays 0.9.0.

## Bounded upstream code changes

### Windows Kontakt editor compatibility (local verification, 2026-10-11)

Windows `PluginImpl::open_editor` defers the authoritative `IPlugView::getSize`
until after attachment: Kontakt 8.4.1 rejects that query before `attached`.
The production helper still validates actual geometry/resize requests before
publishing a visible window, and detaches on failure. Non-Windows paths retain
their pre-attachment geometry validation. The native fixture reproduces the
pre-attachment failure through its real COM view.

On explicit isolated Open/Focus, the Windows host grants foreground permission
only to its existing child helper PID. Denial preserves visible-window behavior;
no OS policy, input queue attachment or global foreground permission is changed.
The production helper's real visibility checks, normal taskbar entry, deferred
background recovery save and main-thread modal IPC dispatcher live in the root
application, not the upstream sample helper. Native lifecycle/modal DSP fixtures
passed locally; genuine Kontakt Library display passed, while NOIRE loading and
sound/state acceptance remain pending. PrintWindow capture is still a failed
independent acceptance check. Full receipts: `docs/WINDOWS_MELODY_WORKFLOW.md`.

### Dormant instrument-bounce event-bus query (unvalidated source checkpoint)

Adds `Plugin::event_bus_layout`, its main-thread facade and an unsupported-default
internal method, plus additive `EventBusLayout` command/response variants. The
direct implementation enumerates current component event buses on the control
thread, bounds counts/channels and checks every BusInfo result. Activation is
reported only from a matching successful host activation record; unknown is
explicit and `kDefaultActive` is never used as proof. The isolated query is
fallible and never retries/recovers or substitutes cached PluginInfo MIDI bits.
Production and vendored helper dispatch run the query on their existing owner.
Existing wire commands, processing and state methods are unchanged. Plugin-free
component/protocol tests are added, but no execution or vendor compatibility is
claimed by this source checkpoint. The application keeps bounce dormant.

### Strict audio-bus attestation (unvalidated source checkpoint)

Additive `checked_audio_bus_layout`/`CheckedAudioBusLayout` compares actual component
count/index/channels/main-or-aux descriptors with prepared buffers on the owner
thread. Prepared bus types are retained from the same SDK reads that prepare
channel counts, preserving legacy fallback behavior but refusing unknown proof.
Existing `audio_bus_layout` behavior is unchanged. Host activation is attested
only where known; default flags never prove live state. A non-draining restart
flag read before/after enumeration rejects pending I/O/reload without servicing
or reconfiguring the source. Both current event and checked audio queries use it.
A bounded `MetadataQueryRejection` enum and additive typed helper response carry
only Unsupported/PendingTopology/InconsistentTopology admission refusal. Untyped
Error, wrong/malformed reply, SDK/IPC failures remain faults; no recovery/default
layout is permitted. Added component/protocol fixtures cover Start/parameter/type
changes, pending/reentrant flags and SDK/protocol errors; none has run at freeze.

### Previously integrated extensions

1. `src/plugin.rs`: additive `IsolatedEditorOwner`, `IsolatedEditorCommand`, and
   `IsolatedEditorState` types; `Plugin::isolated_editor` and an unsupported default internal
   implementation; checked `try_take_parameter_edits`/`try_take_host_notifications` drains so
   transport failure cannot masquerade as no native changes; checked, non-draining
   `Plugin::native_dirty_revision`; a failed in-process stop retains the confirmed processing
   flag. No forged `WindowHandle` is needed for a helper-owned editor.
2. `src/process_isolation.rs`: additive `HostCommand::Editor` and
   `HostResponse::EditorState` wire variants, plus `NativeDirtyRevision` command/response.
   Existing variant names remain compatible; additive process fields are detailed below.
   Windows protocol output is retained through a private,
   non-inheritable handle; Win32 stdout and CRT descriptor 1 are redirected before plugin
   load, failing closed if isolation fails. Registry dependency manifests remain unchanged.
3. `src/internal/isolated_plugin_impl.rs`: typed forwarding and snapshot validation,
   updating legacy cached open/size state only after a valid reply; fallible edit/notification
   drains preserve transport and protocol errors. Checked drains, revision, editor lifecycle,
   restart servicing and state capture never transparently recover a dead helper: a newly
   loaded instance cannot stand in for lost native edits. Existing infallible APIs remain.
4. `src/lib.rs`: re-export the three public types.
5. `src/bin/vst3-host-helper.rs`: the preserved upstream sample helper explicitly rejects the
   new command. The production Citrus helper lives at `src/bin/vst3-host-helper.rs` in the
   repository root and implements the Windows lifecycle; building the dependency's sample
   helper is not a substitute for building or shipping the production helper. Both forward
   the checked native revision query.
6. `src/internal/com_implementations.rs`: per-instance saturating `AtomicU64` native revision
   increments independently of bounded queues for `performEdit`, `setDirty(true)`, and
   parameter-value/reload restart requests. Draining feedback never clears it. Exhaustion is
   an error rather than wraparound. Native values lost before DSP delivery latch a separate
   capture failure, including queue overflow/poison, and cannot become a clean snapshot.
7. `src/internal/plugin_impl.rs`: in-process revision forwarding; state capture rejects
   undelivered/lost native values and native revision changes during capture. Failed DSP
   delivery and a stopped legacy display drain cannot hide lost processor updates. Successful
   state restore discards stale native values, while an unreconciled loss latch stays set until
   plugin reload. Stop/deactivate return codes are checked before changing lifecycle flags,
   so a temporary zero-sample flush cannot silently succeed after failed lifecycle restoration.

Existing APIs and `CreateGui`/`CloseGui` wire values remain compatible. A new client talking
with an old helper gets a normal protocol error rather than silently assuming GUI support.
The new types reject unknown fields. Native owner identity is data only, validated in the
helper against a live HWND and PID before use. It is logical association, not unsafe pointer
transport or cross-process embedding.

The production Windows helper initializes a balanced main-thread OLE apartment before plugin
loading, failing explicitly if initialization fails. It closes its editor before SaveState/LoadState as well as
replacement/unload. SaveState flushes queued values with a genuine zero-sample process call
(one explicitly empty channel avoids upstream's channel-less full-block fallback), then
captures the component/controller state. Failed flush/capture stays an error; no phantom audio
samples are rendered. A revision is scoped to one loaded plugin instance, and is dirty-state
evidence rather than a replayable gesture log. These changes do not implement automation
recording, touch/latch modes, or reliable gesture grouping.

## Maintenance and test boundary

Do not replace this directory with a newer upstream package without porting/reviewing these
changes and rerunning Citrus helper/editor tests. Compare the directory to the exact registry
archive above; only the listed changed/added code files, this manifest, and the reviewable `CITRUS.patch` should differ.

Focused regressions cover revision persistence after drains/overflow, notification backpressure,
exhaustion/poison, unloaded-plugin checks, lossless JSON revisions above JavaScript's exact-integer
range, checked IPC failure without recovery, and helper state-detachment/zero-frame policy.

The standalone source-only fixture under `tests/fixtures/vst3-editor` has separate provenance.
It is a test input, not a runtime dependency or release payload. Protocol tests do not prove a
real editor rendered. Windows lifecycle and interactive smoke must report their own results;
Linux typechecks and unsupported-desktop outcomes do not establish Windows GUI acceptance.

## Authoritative MIDI-routing transport extension

The same seven reviewed source files also carry the additive per-block transport and output
loss contract used by Citrus MIDI routing:

- `ProcessTransport` exposes independent project sample/quarter-note positions, current tempo,
  playing state, and time signature. `Plugin::set_process_transport` validates it and the direct
  implementation applies it to the next process context. Sample/PPQ positions freeze while
  stopped; playing PPQ advances by each block's duration at its tempo, never by multiplying an
  absolute sample position by the latest tempo. Continuous processing time remains independent.
- The isolated implementation caches context without sending an additional IPC command.
  `Process` and `ProcessBuses` carry optional, serde-defaulted `transport` in the same audio
  request; both helpers apply it immediately before processing. Each audio response explicitly
  acknowledges applied transport with a serde-defaulted `transport_applied` flag; an explicit
  context request fails closed if a mismatched/older helper omits or rejects that acknowledgement.
  Legacy tempo/time-signature/
  playing setters remain supported, and requests without context retain their legacy setup.
  Existing callers should set context for every quantum, especially after seeks or loop wraps.
- `Plugin::take_output_events_with_loss` returns events plus a loss latch. SDK event capture
  rejection, bounded event-list/output-queue overflow, and isolated backlog loss are observable.
  Legacy event drains remain available but do not clear the latch. Each audio response includes
  a serde-defaulted `output_events_lost` boolean so a routing host can panic the destination
  rather than silently lose note-offs. There should be one draining consumer per plugin.
- The event wire codec rejects more than 4096 events or more than 8 MiB of aggregate event
  payload, and isolated accumulated output uses the same limits. Helpers reject oversized
  flat process frame counts before allocating output buffers. Unsupported MIDI event/bus
  policy remains a responsibility of the application, which must treat dropped note data as
  a destination-safety concern.

Headless regressions exercise transport validation, independent PPQ/seek origins, tempo
changes, stopped and zero-sample contexts, single-request context forwarding, output-loss
propagation/clearing, bounded overflow, and backward-compatible JSON defaults. These tests
make no native GUI or real-plugin interoperability claim.

### Regenerating the reviewable patch

Verify the original registry archive against the SHA-256 above and extract a fresh copy.
For each changed upstream source file, produce a unified diff with paths `a/<relative path>`
and `b/<relative path>` against that pristine copy. Concatenate those diffs in sorted path
order into `CITRUS.patch`; exclude `CITRUS.patch` and this manifest themselves. Apply it with
`patch -p1` to another pristine extraction, then byte-compare every upstream file with this
vendor directory. The source additions listed below are included in that patch. Only this manifest and the patch are additional provenance files. Keep the
original LICENSE, `.cargo_vcs_info.json`, package version, and registry checksum unchanged.
Finally review the changes and update the explicitly pinned manifest/patch SHA-256 values in
`scripts/package_windows_preview.py` and its packaging regression test. Those pins deliberately
require a fresh source review when this extension changes.

## Linux standalone native-editor extension

The production helper now also implements standalone X11 plugin windows usable through the
session's system XWayland server. Linux uses `Open { owner: None }`; Windows HWND/PID validation
is unchanged. No compositor is spawned and no Wayland parent handle is reinterpreted as X11.
See `docs/LINUX_VST3_EDITORS.md` for the window, focus, DPI and validation boundaries.

- `com_implementations.rs` exposes Linux `IRunLoop` from both the factory host context and
  each attachment's plug frame. Bounded registries use unique registration tokens to reject
  stale readiness/timer snapshots after callback reentry. COM references are released outside
  registry locks. Closing a registry permanently rejects reentrant registration; reopening an
  editor creates a fresh frame registry. Factory callbacks continue while its editor is closed.
- `plugin_impl.rs` retains the factory through plugin teardown, while keeping the host context
  alive through module unload. Failure guards close registrations before unloading a partially
  loaded module. Normal teardown closes factory registrations before module unload; retired frame
  callbacks cannot run after detach. Factory and frame callbacks are serviced on the helper main
  thread, including idle periods with no stdin traffic.
- `process_isolation.rs` now fails closed on Unix protocol descriptor isolation errors. A private
  close-on-exec descriptor is required, stdout redirection is checked, and absent or protocol-
  aliasing stderr is replaced with a valid `/dev/null` diagnostic sink before plugin loading.

Focused Linux regressions exercise callback reentry, registration identity, closed-registry
resurrection, descriptor/timer bounds, factory/frame separation and failed protocol claims.
They do not establish mixed Wayland/XWayland runtime acceptance or sanitizer coverage.

## Synchronous control / processor ownership preparation

This stage preserves protocol v1 and does not enable a DSP worker or claim a latency/real-time
improvement. The production Linux/Windows helper uses an additive thread-bound entry point:

- `src/host.rs`, `src/plugin.rs`, `src/lib.rs`: `load_main_thread_plugin[_class]` rejects an
  isolation-enabled configuration and returns a `MainThreadPlugin` facade owning the existing
  concrete in-process implementation. Its private `Rc` marker is !Send/!Sync. Explicit
  forwarded methods expose no Deref, mutable legacy-owner access or extraction. Existing
  Plugin/PluginInternal Send compatibility is unchanged. Compile-fail examples accompany a
  positive same-thread compilation example and policy-validation tests.
- `src/internal/plugin_impl.rs`: `ControlDomain` retains owning COM/module/editor/host and
  lifecycle administration, while `ProcessorRuntime` owns prepared process data, transport,
  event/parameter storage, note tracking and cached routing. Runtime processing receives a
  narrow lease and an explicit legacy callback bridge. Existing queue locks/growable containers
  remain; this is not an allocation-free or lock-free worker implementation. Teardown order,
  single-component aliases, data-exchange and run-loop behavior are retained. Loader transfer
  consumes/disarms the initialization guard and immediately releases its extra COM references;
  factory ownership and pre-module host-context declaration now apply on all platforms, closing
  error-unwind windows that could release plugin vtables or context after module teardown.
- Added `src/internal/processor_lease.rs`, registered by `src/internal/mod.rs`: non-owning,
  exclusive lifetime-bound IAudioProcessor access limited to process/setProcessing, with no
  clone, cast, addRef/release or metadata interface. The lease is deliberately !Send/!Sync in
  this preparation. There is no new unsafe Send implementation.
- Prepared output channel counts avoid per-block component metadata calls after successful
  setup. This uncovered and fixes old-state bus-buffer reuse: LoadState, bus reconfiguration
  and serviced I/O restarts invalidate/rebuild while inactive. Declined arrangements may
  choose different fallback buses, so refusal refreshes too. Failed rebuild cannot process
  old buffers; surviving bus activation choices are retained during state restore.
- Added `src/internal/plugin_impl/domain_tests.rs`: source-only instrumented COM fixtures
  cover single/separate controller initialization/termination, non-owning lease refcounts,
  loading-thread teardown and module order, state/fallback topology changes, exact prepared
  pointer ownership, processing after refresh and failed-rebuild rejection/recovery.

No upstream archive, license, package version, dependency manifest or registry checksum is
changed. See repository `docs/PLUGIN_PROCESSOR_DOMAINS.md` for the remaining worker/broker,
bounded lane and state-fence requirements, and `docs/WORK_LOG.md` for executed validation.

## Narrow Surge XT 1.3.4 restore eligibility

`src/internal/plugin_impl.rs` tracks positive Process invocation and explicit native-editor-opening-attempt history
without resetting either on stop/reconfigure. The exact instantiated UID
`ABCDEF019182FAEB566D624153675854` plus version `1.3.4` rejects state restore after either history,
before lifecycle/queue/controller/component mutation. `src/plugin.rs` exposes additive local
preflight through Plugin and MainThreadPlugin; remote helpers still enforce the authoritative
check. Root helper native dispatchers preflight before detachment. No protocol change, internal
settlement Process, wait loop or replacement-instance swap is added. Metadata is a compatibility
selector, not authentication; official Linux x86-64 1.3.4 is the characterized binary/platform.

The proposed settlement approach was rejected because a Surge background patch loader can leave
halt_engine set while Process still returns success. Callers must restore a fresh candidate before
first playback/native interaction and retain the old instance until candidate success. Current
App fresh-candidate paths are distinct from the unchanged legacy public live-admin LoadState path,
which still closes/faults its slot on backend rejection. See the ownership guide for that limit.

An adjacent existing alias bug is corrected: an optional controller payload must not call
IEditController::setState when controller and component are the same object. Both controller-state
calls now share the alias guard. Added controlled fixtures cover exact policy selection,
zero/positive/failed Process history, editor attempts, rejection without COM or queue mutation,
continued processing, and a nonempty legacy controller payload on a single-component instance.

## Bounded native-edit delivery and acknowledgment

`src/internal/native_edit_transport.rs` adds an existing-rtrb, fixed-capacity native value lane
with generation/sequence tags, preallocated runtime staging, checked admission and per-SDK-call
acknowledgment. Only the producer uses a nonblocking try_lock; the exclusive consumer has no GUI
producer/display mutex access. No unsafe Send/Sync, worker, wire change or settlement processing
is added. `ComponentHandler` retains independent display/gesture logs and durable dirty/loss
status. Stopped display polling no longer steals processor edits needed by a later explicit
zero-sample flush and SaveState. Display overflow is distinct from native delivery loss.

At the transport checkpoint, `ParameterChanges::try_enqueue` reports admission failures while
its COM queue locks and dynamically sized storage remain; the following parameter-storage section
supersedes the dynamic-storage part of that boundary. Successful Process acknowledges only that call's admitted
native batch; failure/abandonment cannot be hidden by later empty queues or successful calls.
Capture checks submitted/applied, in-flight publication/process, sticky loss/exhaustion and dirty
revision before/after state serialization. Ordinary empty failed Process does not invent loss.

Successful state application supersedes old native intent without claiming it was processed, at
the existing queue-clear boundary even if subsequent setup fails. Rejected Surge restore still
preflights before any mutation. A post-application producer/fence contention permanently closes
native input and refuses capture with an explicit partial-restore error rather than replaying
stale values. Existing positive-call/editor-attempt history, aliases and stopped flush semantics
are preserved. Administrative capture/restore still relies on main-thread serialization.

Transport-only allocation tests and a blocked-producer/display independence fixture are narrowly
scoped. They do not prove whole-plugin allocation freedom, enable independent helper processing,
or qualify native resize latency. See `docs/PLUGIN_PROCESSOR_DOMAINS.md` and executed receipts in
`docs/WORK_LOG.md`; upstream archive/license/version/dependencies remain unchanged.

## Prepared fixed-capacity parameter queues and total-point arenas

The parameter portions of `src/internal/com_implementations.rs` now prepare stable COM queue
wrappers and one shared total-point arena per container, with 8192 input queues/points and 4096
output queues/points. Safe mutex synchronization and legal COM retention remain. Borrowed queue
lookups do not addRef; a separately retained queue keeps its arena alive without a container
backpointer or ownership cycle. Logical reset, checked insertion and COM point reads use prepared
storage only. First-seen IDs and equal-offset repeated values retain their previous ordering.
A shared ordinal-index cache provides direct indexed reads after bounded materialization.
Binary upper-bound search selects stable insertion rank in a valid queue slice. Insertion shifts
its affected ordinal suffix and adjusts later populated-queue offsets. A preallocated ordered
populated-slot index skips empty queue offsets without changing public empty-queue order/count.
First-point activation is checked before list mutation; rejected new host IDs restore recycled
metadata/counts. Bounded costs and large-suffix layouts are measured rather than treated as
real-time qualification. Dirty cache fallback remains bounded and cannot falsely become valid.

`src/internal/plugin_impl.rs` selects explicit budgets, checks every host/native admission before
SDK Process, and observes output write/storage failure even when the plugin ignores it. Successful
SDK calls still acknowledge their admitted native batch before a separate output failure is
reported. A runtime-owned output fault survives clear/rebuild/restore and blocks Process/SaveState
until a fresh instance. Observation before/after getState also catches reentrant retained-queue
failure. Pending-host overflow now rejects before controller mirroring instead of logging a drop
and returning success. `src/error.rs` adds small allocation-free structured rejection variants.

Storage tests cover ordering, bounds, cache invalidation, no-allocation first use/high-water/reuse,
COM reference lifetime and poison/overflow. Extended domain tests cover checked admission, native
acknowledgment, sticky output faults and state/lifecycle recovery limits. Only the cfg(test)
allocator helper in `native_edit_transport.rs` changes; its production transport remains byte-
identical. No event, helper, wire, application, timing or processor-lease behavior is changed.
See the parameter-storage section of `docs/PLUGIN_PROCESSOR_DOMAINS.md` and executed receipts in
`docs/WORK_LOG.md` for constructor memory, comparative cost and qualification limits.

## Checked event admission and transactional note bookkeeping

The event portions of `src/internal/com_implementations.rs` replace silent owned/raw enqueue
with checked results, reused by `IEventList::addEvent`. Existing header, aggregate payload and
per-event limits are checked under one mutex before raw payload copying. Malformed metadata
and unsupported variants fail without publication; admitted payload ownership/UTF-16
representation remains unchanged. Checked admission failures use the existing loss latch
rather than logging. No payload arena or lifetime-sticky output-event fault is introduced.

`src/internal/plugin_impl.rs` propagates `src/error.rs`'s unit `EventInputRejected`. Ordinary
counters and tracked IDs/voices commit only after enqueue. Full tracked storage, a full ordinary
counter and a still-active wrapped candidate ID reject before enqueue without eviction or
silent saturation. Rejected releases retain tracking. Panic commits only its admitted prefix,
up to 4096 combined releases, and retains each unadmitted obligation for retry. A later parameter
failure cannot undo that prefix. These are disclosed correctness changes, not only refactoring.

Focused COM/domain fixtures check limits, pre-copy rejection, malformed pointers/lengths,
FIFO/deep-copy ownership, poison/no-allocation scalar paths, bookkeeping rollback, wrapped IDs,
partial panic/retry and output-event loss versus successful native acknowledgment. Existing
state, split-block, MIDI, native delivery and Surge guards are retained. Source scope excludes
helper/protocol/application/timing/lease and payload representation. See the event section in
`docs/PLUGIN_PROCESSOR_DOMAINS.md` for remaining legacy ignored-result and void-staging limits.
The original upstream archive, MIT license, version and dependency manifests are unchanged.

## Scoped private domain-session checkpoint

`src/plugin.rs` adds a crate-private checked MainThreadPlugin callback entry and an object-safe
PluginInternal default that rejects unsupported backends. The new
`src/internal/plugin_impl/domain_session.rs` constructs disjoint borrowed !Send/!Sync UI/control
and processor capabilities while the aggregate remains exclusively borrowed. Constructors and
fields are private; no downcast, owning extraction, raw COM accessor or movable runtime slot is
introduced. Existing owner fields, initialization/alias/drop order and public compatibility API
remain unchanged. Mixed controller/runtime operations still require owner rejoin.

`src/internal/plugin_impl.rs` shares mechanically extracted runtime-only note/transport/output
bodies, preserves legacy parameter mirror ordering and routes ordinary processing through the
same restricted processor capability. Validation helpers are crate-visible only so the private
facade reuses existing validation. No helper/broker/worker/metadata-policy change is made.
`src/internal/data_exchange.rs` and the HostApplication accessor replace rich process-bridge
access with an AtomicBool-only borrowed RAII process gate. Existing data-exchange receiver,
delivery, dispatch, background thread and shutdown semantics are unchanged. No unsafe Send/Sync
is added. Guard Drop clears the active-call marker on return/Rust unwind without allocation or
COM retention; no foreign COM unwind recovery guarantee is made.

Direct gate and real domain fixtures cover borrowed COM ledgers, loading-thread behavior,
success/error/unwind rejoin, native acknowledgment, output faults, alias teardown and unchanged
Surge/state/zero-sample behavior. Private compile-only fixtures use the real crate source and a
positive same-import baseline before checking owner-borrow/capability exclusions. See the scoped
session guide in `docs/PLUGIN_PROCESSOR_DOMAINS.md` for scope and reproduction. This checkpoint
retains all previous finite-storage, lifetime, legacy-caller and native-resize limitations.

## Explicit stopped reset-origin processing

Additive owner-only `ResetOriginSupport`/`process_reset_origin` and typed isolation commands
consume pending events and host/mapped parameter points at origin in one positive silent block.
The private reset input/output policy leaves ordinary processing and zero-sample state capture
unchanged. Processor output is discarded with separate old-epoch/current counts; loss/poison
remains visible, native UI feedback is retained, and native delivery acknowledgment follows the
actual SDK result. Unknown helpers fail without fallback; uncertain post-send replies quarantine
local stale MIDI. Standalone small blocks remain supported. The DAW alone requests Q128 and
requires a prepared maximum of at least128. See `docs/PLUGIN_RESET_ORIGIN.md` in the parent repo.

The combined scoped-session/reset integration retains the restricted AtomicBool
RAII gate in ordinary and reset SDK calls. Owner reset obtains disjoint borrowed
processor/module and gate capabilities only after session rejoin. Neither facade
exposes reset. A combined COM fixture observes the marker inside SDK entry and
its cleared state after success, SDK error and pre-SDK staging failure.


## Checked processing metadata (2026-10-10)

Additive `try_latency_samples` and `try_tail_samples` methods preserve isolated transport and
protocol failures rather than reporting zero. Checked isolated queries never recover/reload;
local defaults preserve direct SDK values, and absent implementations fail explicitly.
Legacy infallible polling remains unchanged. Both helper binaries forward checked metadata
through existing response/Error variants. The production worker admits/publishes a complete
fallible pair and retains its existing sticky-fault and candidate-rollback boundaries.

The first source recovery archive was explicitly unvalidated. Focused tests cover legitimate
zero/infinite tail, wrong/Error/malformed replies, helper death with recovery opt-in, partial
latency/tail reads and old-chain retention. Executed validation is recorded separately.
No wire addition, metadata cache, automatic reload, COM threading or SHM policy change.


## Test-only borrowed processor capability (2026-10-10)

Add a wholly cfg(test), private non-owning process capability and scoped-thread
COM fixtures. The sole new unsafe Send applies only to that test token; no
production owner, lease, runtime or host interface gains cross-thread authority.
Owner/module lifetime, main-thread reference destruction, serial unsafe Process,
atomic data-exchange interval and scoped join are audited independently before
execution. The fixtures intentionally omit production runtime orchestration and
do not enable an audio worker, metadata cache or user mode. Source recovery
checkpoint is unvalidated until separate receipts record executed tests.

## Scoped full-runtime process dependency (2026-10-10)

Separate process-only SDK authority from lifecycle setProcessing notification.
The existing kernel bodies, caller-side public envelopes and non-Send production
session remain intact. A wholly cfg(test) disjoint runtime loan exercises that
same complete kernel while loading-thread controller/UI access remains possible;
only the test wrapper gains narrowly audited Send, without moving owning COM
references, replacing prepared buffers or adding a helper worker. Retired host
queue final Release after rebuild remains a disclosed processing-thread free
possibility, not a no-free guarantee. Source checkpoint is unvalidated until the
separate executed receipt, and unsafe fixture invocations require source review.

## Disabled scoped worker library feature (2026-10-10, unvalidated)

Add opt-in `experimental-scoped-worker`, absent from defaults, with a Linux-only
scoped full-kernel worker and closed borrowed message/UI bridges. Main retains
all owning COM/module/prepared references; only a private fixed dispatcher uses
a narrow audited Send loan. Callback registration rejects entry, while existing
public callback affinity and the worker's whole-success meter behavior are kept.
Capacity-one transport and one outstanding credit provide bounded command count;
caller audio/payload bytes and processing allocations are not claimed bounded or
allocation-free. Accepted work joins before owner state/setup/reset/metadata or
teardown. No helper/wire/App activation or metadata policy changes are included.
Ordinary MIDI admission is mechanically shared with the owner, with the same
public scalar validation before worker bookkeeping; mapped MIDI remains main-side.
New source/COM/compiler fixtures require review and archival before execution.

## Disabled strict fresh-helper foundation (source checkpoint; unvalidated)

`experimental-fresh-helper` compiles a Linux-only crate-private `fresh_helper`
module for low-level codec/pump/cleanup mock validation. It has no production
caller. The legacy process-isolation implementation, wire variants, retries and
serde behavior are unchanged. New files implement a closed eight-operation
subset, strict bounded payload handling and exact audio shape, cooperative
whole-exchange deadlines, owner-thread nonblocking I/O and cancellation, and
retained Child ownership through a single reserved process-wide quarantine.
No real plugin/COM authority is added. Full limits and the deliberate restrictions
on nonempty event replies, legacy state arrays, SIGPIPE and concurrent strict
sessions are documented in the repository's strict-foundation guide.

## Closed strict offline client (Stop A source checkpoint; unvalidated)

Extend the disabled Linux fresh-helper feature with a doc-hidden, typed, non-Send
client and synchronous borrowed request view. Ten existing metadata/lifecycle/
ordinary-note operations receive bounded closed codecs; full parameter fields,
raw checked layouts and typed metadata refusals survive unchanged. No generic
protocol/owner extraction or caller activation is exported. Split protocol/reap
deadlines and a nonwaiting retained-owner cleanup continuation share the original
teardown kernel, preserve first error evidence and cache proven exit status.
The repository's STRICT_OFFLINE_CLIENT.md records limits and the separately
gated future one-shot adapter; no real plugin or App integration is included.

## Dry-generator acknowledgment and silent-input prerequisite (unvalidated)

Owner activation records now distinguish successful false/true SDK acknowledgments
from prepared participation, retaining a complete observed bus descriptor binding.
Default inactive buses remain unknown; no default activation calls are added.
Known state/topology changes and failures invalidate records; owner setup rechecks
audio and event bindings. Pure rejected used-Surge restore leaves evidence intact.
Checked getters expose the recorded known state, never infer false from prepared
nonparticipation. No SDK processing kernel or owning COM thread authority changes.

The disabled closed strict client adds count-only Q128 silent generator inputs
(0..2); its fixed borrowed positive-zero view uses the existing bounded codec,
without arbitrary external input authority or a per-call input Vec. New codec,
client, activation and actual f32/f64 COM fixtures are unexecuted at this freeze.
See docs/DRY_GENERATOR_TOPOLOGY.md for source/capture admission and remaining gates.

## Windows strict fresh-helper transport (2026-10-11)

The opt-in strict client now has a Windows transport implementation. Private
byte-mode named pipes have overlapped host ends and synchronous inherited child
ends; manual-reset events provide I/O and cancellation wakeups. There is no
blocking pipe worker or transfer of owning COM objects. Fixed operation buffers
and OVERLAPPED storage remain pinned in the owner until completion, including
completed cancellation. Child exit, pipe completion and first cleanup failures
are separate observations; pending pipe I/O retains the global reservation and
quarantine even after a concrete process exit. Linux and Windows use the same
retained-child lifecycle kernel. The existing bounded codec and closed API stay
unchanged apart from additive pipe-cleanup diagnostics.

Native MSVC mock/codec regression passed 106 tests against the archived candidate
in target/melody-stability-20261011/windows-strict-source-02. This qualifies the
transport tests only, not any real instrument, Windows App bounce or whole-song
export. Linux-only signal/descriptor fixtures remain explicitly Linux-only.
Two SysEx test assignments use inferred integer casts for the Windows SDK field
binding; the represented enum value and runtime event processing are unchanged.

## Explicit output selection across I/O rebuilds (2026-10-11)

Host bus preferences are separate from SDK activation acknowledgments. Successful
explicit selection retains the complete observed bus descriptor and media-side
count. An I/O restart still invalidates acknowledgments; while stopped, the owner
checks every retained binding before replaying actual SDK activation calls and
rebuilding storage. A drift, SDK refusal or storage failure does not resume DSP
or silently fall back to default buses. A failed valid explicit request blocks
default fallback until a complete new explicit request succeeds. Invalid indices
rejected before the SDK do not destroy an existing preference.

Plugins without explicit preferences retain their default activation behavior.
Generic LoadState retains its established by-index participation behavior; its
new acknowledgments do not replace a previously bound explicit preference. A
later I/O rebuild rejects a changed descriptor or side count. The full declared
audio/event bus list and roles remain intact, including inactive output buses.

Ten new actual COM-owner regressions cover 32 declared main stereo outputs
with only the first output active, event participation, restart acknowledgments,
count/descriptor drift, failed selection and non-resumption. All ten passed on
native Windows MSVC (2.97 s; vendor-kontakt-explicit-output-final.log). The added
lifecycle fault matrix first failed with `case 0: stale processing storage`
(vendor-kontakt-io-failure-before.log). I/O suspend/deactivate failures now clear
all ACKs and prepared storage without automatic resume; reactivation/resume
failures do the same, with one component deactivation cleanup after rejected
resume. Rejected cleanup preserves the SDK-confirmed active flag and both error
causes. The latency-only legacy resume policy is unchanged and has a regression.
An actual rejected stop still leaves the processing flag true: storage removal
prevents another SDK process, but does not pretend the component stopped or that
facade Start can recover it. This vendor change alone does not
qualify the root playback integration, NOIRE sound/state, physical monitoring,
strict instrument bounce or whole-song offline export.

An older topology regression expected successful activateBus to make the whole
explicit setter succeed even when that SDK call changed the bus role. It still
checks unknown/non-resurrected ACKs; the setter now also rejects that incomplete
transaction with the full descriptor/count error. The first broader domain run
retained its one expectation failure (116 passed, 1 failed). The corrected test
asserts the actual rejection and original non-resurrection checks rather than
relaxing bus verification.
The corrected complete owner-domain subset passed all 117 tests in 4.96 s
(vendor-kontakt-domain-regressions-corrected.log). The full vendor suite and
platforms other than this native Windows MSVC run were not rerun.

### 2026-10-14 strict failed-request context (source12, validation pending)

The disposable strict client attaches a fixed `RequestContext` only to a real
failed exchange. It contains the closed operation label and, for `SetBusActive`,
media type, direction and bus index. Helper Error/Crashed text, typed failure,
delivery and diagnostic-copy evidence are preserved separately. Local rejection,
spawn and cleanup do not inherit a preceding successful request's context.
No plugin path, state, title or parameter value is retained. Protocol caps,
codec checks, catalog admission, cancellation, terminality and reap are unchanged.
This diagnostic addition follows actual NOIRE run03's unlocalized Prepare Limit;
it does not identify the limit or establish sound/export success. Root protocol
and regression checks are pending; Windows run04 has not started.
### 2026-10-14 source13 numeric limit evidence and source15 natural exit policy

Source12's root strict backend and scoped vendor regressions passed; actual run04
located the failed request at GetAllParameters. Source13 preserves typed, fixed
numeric evidence for the existing catalog-entry, field-text and aggregate-text
guards. Actual run05 observed the first excess catalog entry (at least 4097,
maximum 4096), not a complete parameter count. Caps and codec admission remain
unchanged. Source14 avoids this unused query only for a validated exact empty
requested/captured generic set; nonempty sets still require a complete catalog,
all IDs validated before any setter, then all replay and readback.

Run06 reached 1661 actual Process replies but refused publication after a forced
clone exit. The session previously allowed only 100ms after Shutdown even when
the renderer had a longer independent cleanup budget. Source15 keeps that
ordinary foundation policy and adds a private strict-only healthy Shutdown
natural-exit window: at most 3s, limited by the original remaining cleanup budget
minus 1s for kill/reap. A sticky cancellation or failed/terminal Shutdown retains
100ms. No protocol deadline or first stored grace can be renewed; successful
kill is still not a concrete reap or admission release. Actual-child regressions
are written; their Cargo execution and a new NOIRE run remain pending. This
policy does not itself prove why Kontakt teardown exceeded 100ms, plugin sound,
complete release or whole-song export.

### 2026-10-14 actual natural-exit regression and NOIRE receipts

The strict root backend group passes all 28 tests, including a real child delayed
350ms after complete Shutdown, natural exit0, concrete reap and subsequent WAV
publication. Four focused vendored child-policy tests also pass. Their first run
failed only because Python translated fixture marker LF to CRLF on Windows;
three marker writers now use explicit `newline=''`. Original failure logs remain
preserved; no production exit deadline or process predicate was changed by that
test-only repair. Source15 and source19 actual NOIRE runs07/08 each reached1661
PCM replies and natural clone exit0/reap before producer WAV publication. Both
still fail the application's independent source recapture byte protection;
conversion/Undo, complete NI settings and whole-song sound are not qualified.
Source22 strict all-target ordinary and all-feature Clippy passed; the complete
root cohort is executing. The four preexisting scoped-worker dependency warnings
remain visible and are not root lint suppressions. Evidence is retained under
`target/backend-reliability-20261014`; the reviewable patch and packaging pins
must be regenerated against the verified original archive before distribution.


### 2026-10-14 source32 selected Kontakt metadata bundle (applied; tests UNRUN)

The selected Kontakt 8.4.1 strict clone now uses one closed
StrictMetadataSnapshot exchange for its outer four metadata getters. For the
reviewed zero-input/32-stereo-output and 1-in/1-out event declaration shape this
changes the counted outer requests from 11 IPC to 5 IPC per positive Q128 block,
plus actual MIDI requests. It retains the same 149 counted host SDK methods,
the checked audio/event/latency/tail getter order, fresh full bus declarations
and activation evidence. Selected Process still performs its existing actual
audio-layout checks before and after processing. Generic backend dispatch keeps
its original four separate metadata exchanges. There is no cached snapshot,
reduced SDK getter frequency, topology fallback, or native speed claim.

The actual helper independently admits only loaded Instrument factory UID
5653544E694B386B6F6E74616B742038 at version 8.4.1; StrictBackend also binds its
selected dispatch to captured identity. The low-level Client method requires
completed explicit audio selection but does not itself prove factory identity.
The existing main known-active event-input gate precedes latency and tail calls.
The legacy vendored sample helper explicitly returns finite Unsupported without
fabricating metadata. The production helper owns the real getter implementation;
its directory module follows the existing editor module layout so it is not
offered as an independent top-level Cargo binary.

State bytes, parameter bases, MIDI, transport, render sample treatment and the
existing latency/tail policies remain intact. Original protocol/payload/catalog/
observer caps, absolute render deadlines and stored cleanup grace are unchanged.
Only a fully validated complete failure reply carries a finite actual inner
getter; malformed, duplicate, missing, wrong-type, cancelled or expired replies
cannot inherit a partially read getter or a preceding successful request.
The real failed wire context remains the bundle operation. Actual first getter
failure and bounded diagnostic-copy evidence stay separate. Pure admission
refusal carries no qualified partial snapshot and permits no retry/default.

An already delivered held getter may finish after cancellation or the original
deadline, including subsequent getters in that bundle. The terminal host must
refuse its complete late result and send no later Process/MIDI or publication.
Actual child completion/reap remains required; successful kill, EOF, a cancelled
pipe or a cleanup attempt is not a fabricated reap or renewed deadline.

Twenty-two Rust regression functions are authored: seven production-helper owner/
closed-request tests, one legacy rejection test, six codec tests, five real mock-
pipe tests and three strict-backend renderer/worker tests. Late getter cases
include cancellation and original deadline; codec and actual pipe fixtures cover
a 59-case malformed failure-field matrix. All 22 are UNRUN for this applied
source32. Isolated formatting of the 14 applied paths passed, but current
compilation, Clippy, Rust/package tests and actual NI/native/UI performance were
not run. Earlier NOIRE receipts do not qualify this new bundle implementation.

The full reviewable vendor patch was regenerated from the verified original
archive to every one of the 69 actual source files outside vendor/target,
excluding only these two provenance files as patch inputs. The archive has
41 original files; 41 paths differ and
28 are additions. LICENSE and .cargo_vcs_info.json
remain byte-identical upstream; package/version/license and original archive
remain unchanged. Patch SHA-256 is 5699f33c0817fa58b0e729437293dfcf2e3c6e388bd7561452bcd79e52f385e0.
A real private git applycheck and apply reproduced all 69 files exactly.
This is byte replay/source provenance evidence, not runtime or package acceptance.
The complete source ledger and root dirty preimage backups are retained in
target/backend-reliability-20261014/source32-vendor-provenance-01.


### 2026-10-14 source51 opt-in SDK load diagnostics (runtime tests UNRUN)

Five vendored SDK source files add a process-wide diagnostic gate and 31 closed,
static Windows load/teardown stage names. The public callback is installed once.
The disabled gate reads AtomicBool before accessing the callback and introduces
no environment lookup, clock, file I/O, allocation or thread-local state.
The production helper owns timing and active load scope through its existing
explicit startup option; that separate root implementation is not vendored here.
Callbacks carry only the fixed stage, before/returned marker and the original
SDK HRESULT when actually available. None does not invent a result or success.
Paths, plugin state, PCM, parameters and arbitrary error text are excluded.

Observed stages cover Windows library construction/InitDll/factory/ExitDll,
component/controller creation and initialization, controller synchronization,
view creation and immediate probe Release, the whole MIDI/program caches,
bus arrangements, setup/activation and existing error/normal teardown.
The SDK order, return handling, original budgets and release ownership are
preserved. A wrapper return including Err/None is an actual return marker, not
a success claim. ExitDll is not evidence of FreeLibrary, process exit or reap.
This source change diagnoses the retained scan/load timeout; it neither fixes
nor identifies that timeout before an actual new native run.

The five frozen vendor paths passed isolated formatting/check and diff checking
as reported by their owner. Five pure local-gate tests are authored but UNRUN.
Compilation, Clippy, Rust tests, actual Kontakt/NOIRE/UI behavior and performance
have not been run for these new SDK diagnostic sources at this regeneration.
Earlier Source49 Rust gates and Source50's timed-out scan do not qualify this
new implementation. Synthetic Python packaging tests are recorded separately;
they execute no Windows binaries and do not establish native/runtime acceptance.

The full reviewable patch was regenerated from the verified original archive
against all 70 actual source files outside vendor/target, excluding only
CITRUS.patch and this provenance document from patch inputs. The archive contains
41 original files; 43 paths differ, 29 are additions and
0 are deletions. LICENSE and .cargo_vcs_info.json remain byte-identical
upstream; package/version/license, archive checksum and upstream commit are
unchanged. Patch SHA-256 is b1e42cd73c98c5a87668a0d2b2463d6c8f2fca64b32c53e7db46e088c5311e1a.
Actual private git apply --check and git apply succeeded with core.autocrlf=false
and reproduced the full 70-file set and every raw byte, including line
endings and missing final newlines. This is source provenance evidence.
Existing four-file packaging pins and hard regression assertions retain the
same enforcement, with only the patch/document hashes refreshed. Raw preimages,
the five SDK pins, original archive, complete source ledger and actual replay
receipts are retained in
target/backend-reliability-20261014/source51-provenance-01.


### 2026-10-14 source52 diagnostic test lifetime repair (quality/runtime pending)

The actual Source51 root all-feature Clippy gate exited 101 on a new helper
test's pointer borrowed from a temporary Box. Its original terminal/log remain
retained; no Rust test or new native NI run followed that failed gate.
Source52 binds returned Boxes for the lifetime of the root helper assertion
and two equivalent vendor assertions. Within the vendor, only
src/internal/load_phase_diagnostics.rs changed: its production prefix is
byte-identical to Source51 and all other 69 source files remain byte-identical.
The five local-gate test functions and original assertions remain intact.
These test-only repairs passed their owners' isolated formatting/check; current
root Clippy/Rust/native gates are pending at this regeneration. They do not
resolve the actual prior scan timeout or establish Kontakt/NOIRE behavior.

Source51's original 70-file byte replay and actual 170-test synthetic package
pass remain historical evidence. Its first receipt-parser wrapper failed to
recognize Windows CRLF summary lines despite the test process's exit0; the raw
logs, first receipt and corrected binding remain unchanged in the Source51
directory. No test rerun or product repair was needed for that parser correction.

The Source52 patch was independently regenerated against the verified original
41-file archive and all 70 actual vendor source files; only CITRUS.patch
and this document are excluded as patch inputs. There are 43 changed
paths, 29 additions and 0 deletions. Patch SHA-256 is
daff9a294e6e987adabe548aeed2be1a1808e9852ef2f440b2ad9e85f7eb9ea1. Actual private git apply --check and apply succeeded and all
70 file paths/raw bytes matched, with original line endings retained.
Upstream archive/commit/license pins and all packaging hard assertions remain
unchanged; only patch/document hash literals are refreshed. New synthetic Python
package execution is recorded separately and executes no Windows/SDK/NI binary.
Complete fresh replay, four-file preimages, test-only delta and source bindings
are retained in target/backend-reliability-20261014/source52-provenance-01.


### 2026-10-14 source53 ordinary checked default-class probe (Rust/native UNRUN)

The existing retained fresh-child transport is now also compiled for ordinary
process-isolation on Linux and Windows. A closed DefaultClass Probe API performs
one LoadPlugin with the scanner's original class_id None, 48000 Hz/Q128, tempo120
and meter4/4. It exposes no editor, state, Process, arbitrary command or request
replay interface. Spawn failure retains a real post-spawn cleanup owner; sticky
cancellation authority wakes the same in-flight transport. Metadata acceptance
and actual child/pipe teardown remain separate facts. Dispose sends at most one
healthy Shutdown, retains the original absolute deadlines and clamps the ordinary
two-second natural-exit preference with 250 ms reserved for kill/reap. Failed or
cancelled loads retain the existing short policy. Pending/Busy, kill requested,
Drop, process absence and pipe completion alone cannot establish concrete reap.

Delivery, Failure, LoadedPlugin, CancelHandle and CleanupObservation now have one
shared internal definition used by the ordinary probe and re-exported by the
strict API. There is one transport/quarantine/reservation rather than a second
child pool. The selected strict client and renderer remain explicitly behind
experimental-fresh-helper, with original class selection, block2048, state/audio
policies and bounded cleanup. No Cargo feature default, dependency, lock entry,
package/version/license or original archive checksum is changed. Preview still
requires exact root vst2,vst3 features with no experimental feature activation.
The root scanner/App's distinct Completed/Stopped handling and prior-index
preservation are separate root source changes, not vendored in this patch.

Twelve new real Python-child protocol/cleanup regressions are authored for the
ordinary API: original load configuration/metadata and actual Shutdown/reap,
no repeated load/respawn, helper crash, timeout, pre/in-flight cancellation,
post-spawn setup failure, retained wait error/Pending, Drop quarantine, failed
healthy disposal, expired exchange and wrong reply. Shared existing test hooks
are reused under cfg(test). The seven vendor paths passed their owner's isolated
format/check and diff checking; current Rust/Clippy/build/native gates are UNRUN
at regeneration, and package tests await the coordinated CPU handoff.
Earlier Source52 whole-song evidence does not qualify this changed ordinary
scan/shared-transport implementation or establish new NI stability/performance.

The verified original archive still has 41 files. This full patch now
reproduces all 73 actual source files outside vendor/target; only
CITRUS.patch and this document are excluded as inputs. There are 46
changed paths, 32 additions and 0 deletions. Patch SHA-256 is
82316984b16d5e904fdd8fe5e9e700dc50178d07fba975c9ccd0cfae96f04a1d. Actual private git apply --check and apply succeeded and every
file path/raw byte matched, including original line endings and final-newline
state. LICENSE/.cargo_vcs_info.json remain byte-identical upstream, and the
archive/commit/license hard pins and package assertions are unchanged. Only the
patch/document literal hashes are refreshed. The Source52 evidence remains
intact; fresh preimages, source delta, complete ledger and actual replay receipts
are retained in target/backend-reliability-20261014/source53-provenance-01.


### 2026-10-15 source58 ordinary probe test helper result cleanup

Source57 completed 16 actual Rust/build/format/metadata gates, with 306 Rust tests
passed (187+5 root scan/App, 12 Probe, 45 shared transport, 26 strict client and
31 root strict-backend cases). Its last Python module entry failed to import
package_windows_preview before executing the 170-test package suite; the original
failure log is retained. Source58 uses the standard scripts-directory unittest
discovery entry for that separate actual package gate. The Probe test helper
returned a must_use CleanupObservation
that seven test callers did not consume, producing new unused-result warnings.
Source58 makes only that test helper return unit and removes its final result
expression. All actual disposal/reap/failure assertions and the same seven
calls remain intact, with all 12 test functions preserved. The other 72 vendor
source files are byte-identical to the Source53/57 production snapshot.
No production API, feature/dependency, transport, protocol, cleanup predicate,
deadline or state/audio behavior is changed by this repair. Current Source58
test/package gates remain pending at regeneration; prior Source57 production
gates are separate exact-source evidence, and no new NI/GUI/performance claim is
made. Historical failures, warnings and Source52 native receipts are retained.

The complete patch is freshly regenerated from the same verified 41-file
original archive against all 73 source files, excluding only this document
and CITRUS.patch as patch inputs. There are 46 changed paths,
32 additions and 0 deletions. Patch SHA-256 is
352fbe0c34167f0a4babacae4a0a8b096241194ff081a3eb33f6328de5e19303. Actual private git apply --check and apply succeeded, and the
complete 73-file set/raw bytes matched exactly, including line endings.
Original archive/commit/LICENSE/VCS pins and all package hard assertions are
preserved; only the two provenance hashes in each Python file are refreshed.
Fresh preimages, the exact test-only delta, complete ledger and replay receipts
are retained in target/backend-reliability-20261014/source58-provenance-01.


### Factory-only declarations and retained isolated probe (source70)

Adds discovery::probe_factory_metadata and separate FactoryMetadata/class types.
The existing platform module loader and factory getters are reused; no component,
controller or view is created. Factory3 Unicode metadata takes precedence over
Factory2/Factory1. Optional fields remain unknown when unavailable. Successful
extended getters must agree with base class identity/category/cardinality;
counts and fixed text are bounded. All interface references are released before
normal module unload, and host context remains alive until after unload.

ProbeFactory/FactoryMetadata are additive protocol variants. Both helper entries
require an unloaded helper and return only after normal module retirement.
The retained Probe adds one closed factory request, sharing its one-attempt seal,
absolute exchange budget, cancellation, bounded two-pass codec, path binding and
existing cleanup. A declared class is not the runtime default or evidence of bus,
MIDI, editor, latency, tail, parameter, state or audio support. Existing LoadPlugin,
scan/cache/verified/default-class behavior and strict clone budgets are unchanged.

New mock-COM, wire, codec and real mock-child regressions are authored. Formatting
passed; compilation, test execution and actual NI behavior remain UNRUN for this
source at provenance regeneration. Prior Source69 pass/failed-NI results do not
qualify this new capability. Production activation awaits distinct evidence.

The complete patch was regenerated from the same original archive against all
74 current source files, excluding only this document/CITRUS.patch.
Private git apply --check/apply succeeded; complete file inventory and raw bytes
matched. Patch SHA256: ed3a7915361126d278b7085f0e57edaaf2eaf64a785c6196d9991115783acd94. Original archive/commit/LICENSE/VCS pins are
unchanged; only the two provenance digest literals in each packaging Python file
are refreshed. Evidence remains in target/backend-reliability-20261014/source70-provenance-01.


### Optional factory host-context offer (source74)

Source73's actual factory-only Kontakt 8.4.1 attempt returned E_NOTIMPL from
IPluginFactory3::setHostContext, before declaration getters. That attempt failed,
while its helper naturally exited0 and was actually reaped without kill/pipe
errors. Ordinary component scan retirement and NOIRE state/audio are separate.

Only this new declaration-only API now accepts the SDK kNotImplemented result
from the context setter. It still offers context, retaining it through module
unload; other setter failures and all required getter failures remain errors.
The shared getter success predicate, runtime loading, scanner/cache/verified,
class selection, protocol, cleanup and existing budgets are unchanged. The
[official SDK hosting helper](https://github.com/steinbergmedia/vst3_public_sdk/blob/master/source/vst/hosting/module.cpp)
offers context without propagating the setter result into class declarations.

New mock regressions distinguish unsupported context from mandatory factory
and base-class metadata, retain zero createInstance calls, and preserve factory
release before module unload. Formatting passed; new Rust compilation/tests and
actual NI behavior remain UNRUN at this provenance regeneration. Source73's
passed31 factory regressions do not qualify this new branch or production scan.

The complete patch was regenerated from the same original archive against all
74 current source files, excluding only this document/CITRUS.patch.
Private git apply --check/apply succeeded; complete inventory and raw bytes
matched. Patch SHA256: 256a299ccafb7450ed89aaa8ac8d8780982e6539b90000561ab59bc1ecbd53f9. Original archive/commit/LICENSE/VCS pins are
unchanged; only the two provenance digest literals in each packaging Python file
are refreshed. Evidence remains in target/backend-reliability-20261014/source74-provenance-01.


### Factory attempt-seal failure diagnostics (2026-10-15; Rust execution pending)

Source86's Windows factory gate failed its first real mock-child exchange in
factory_and_default_load_share_one_attempt_seal_in_both_directions: ProbeFactory
returned Deadline with MayHaveApplied delivery. The other 31 factory cases
passed. Spawn establishes the owned child and pipes, not Python script readiness;
the retained log cannot locate the delay within startup, request/reply or decode.

Only src/default_class_probe_tests.rs changes. Its original two child paths now
write bounded script-entered, JSON-ready, request-read/validated and reply-flushed
phase markers with child elapsed time to inherited stderr, never protocol stdout.
A failed exchange still fails the test and reports its original Failure, owned
PID, spawn/exchange elapsed time, absolute exchange budget and actual disposal
receipt. The same requests, one-attempt assertions, Shutdown/EOF checks and
three-second exchange deadlines remain; there is no readiness wait or retry.
Production transport, seal, cancellation, cleanup policy and budgets are unchanged.
Formatting and diff checks passed. Rust compilation/tests, Clippy and new native
NI behavior have not been run for this diagnostic change.

The full patch was rebuilt against the verified original 41-file
archive and all 74 current vendor source files, excluding only this
document/CITRUS.patch and generated vendor/target output. There are 48
changed paths, 33 additions and 0 deletions. Private git
apply --check/apply succeeded; the complete file inventory and raw bytes matched.
Patch SHA256: e4daaf77db4dcc90ead5487b44d51dddb7d027ea021649e6cb66542026f7ee95. Original archive/commit/LICENSE/VCS identities are
unchanged. The two provenance digest literals in each packaging Python file are
refreshed. Replay evidence is retained in
target/backend-reliability-20261014/factory-attempt-seal-provenance-01.


### Offline ordinary MIDI flags and event PPQ (2026-10-15; Rust execution pending)

The ordinary MIDI convenience API previously published every note-on/off and
poly-pressure event with kIsLive and PPQ zero, including offline Whole/Strict
SendMidiAt. Steinberg defines kIsLive for direct keyboard events and event PPQ as
the project position:
https://steinbergmedia.github.io/vst3_doc/vstinterfaces/structSteinberg_1_1Vst_1_1Event.html

HostEventList now retains private per-entry ordinary-MIDI provenance under its
existing admission lock and budgets. It survives the existing chunk transfer
without any public PluginEvent or wire-format addition. At the actual process
chunk, Offline convenience events clear only kIsLive and receive the block's
authoritative quarter-note origin plus their final, rebased sample offset at
the current tempo/sample rate. A stopped project retains its musical position.
Transport may arrive after MIDI admission, as it does on the helper Process
command. Event musical position advances independently of optional projected
ProcessContext music fields; an explicit transport replaces the origin without
reconstructing it from sample time. Legacy calls without explicit transport
retain their existing zero-origin, tempo-integrated processing path.

Realtime/Prefetch metadata and caller-supplied PluginEvent flags/PPQ are unchanged.
Note payloads, event FIFO, offsets/clamping, mapped parameters, process mode,
lifecycle and the public protocol remain unchanged. The internal queue wrapper
adds bounded header metadata, not another queue, lock, payload allocation or
cross-thread owner. Existing input/output loss and budget handling is retained.
No sample compensation, delay, warmup, retry or Kontakt-specific behavior is added.

Four regressions inspect headers returned by the actual COM IEventList inside
the fixture's IAudioProcessor::process. They cover two SDK chunks, nonzero note/
poly-pressure/release offsets and PPQ, admission before transport, a second block
with an independent sample/PPQ origin and changed tempo, automatic advancement,
omitted ProcessContext music requirements, stopped and zero-sample processing,
the no-explicit-transport entry point, preserved parameter offsets, unchanged
Realtime/Prefetch live events and explicit event headers. Rustfmt and diff checks
passed. Compilation, these Rust tests, Clippy, native Kontakt behavior and any
relationship to the observed 384-frame audio drift remain untested at this
source checkpoint. Root cargo test does not run dependency unit tests; run:

    cargo +stable-x86_64-pc-windows-msvc test --locked --manifest-path vendor/vst3-host-0.9.0/Cargo.toml --lib offline_midi --target-dir target -- --test-threads=1
    cargo +stable-x86_64-pc-windows-msvc test --locked --manifest-path vendor/vst3-host-0.9.0/Cargo.toml --lib host_event_list_tests --target-dir target -- --test-threads=1

The complete patch was regenerated from the verified original 41-file
archive and all 75 current vendor source files, excluding only this
document/CITRUS.patch and generated vendor/target output. There are 49
changed paths, 34 additions and 0 deletions. Private git
apply --check/apply succeeded; the complete file inventory and raw bytes matched.
Patch SHA256: 3b66657084dca6bc2d4428aaa740debbdfa584d80624455c057d1843f4d164d8. Original archive/commit/LICENSE/VCS identities remain.
The two provenance digest literals in each packaging Python file are refreshed.
Replay evidence:
target/backend-reliability-20261014/offline-midi-timing-provenance-01.


### Component-handler progress and guarded asynchronous state readiness (2026-10-15; Rust execution pending)

Steinberg specifies IProgress as an interface queried from the component handler,
with separate AsyncStateRestoration and UIBackgroundTask kinds:
https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IProgress.html
The actual ComponentHandler now exposes that COM interface and shares a bounded
typed ProgressRegistry with HostApplication and the actual processor runtime.
The old ComponentHandler constructor remains usable with an independent registry.

Active IDs/types and the asynchronous-start revision are separate from the
bounded legacy notification FIFO. Notification drains, including the explicit
progress-only discard used by a disposable Strict clone, never complete tasks.
A non-consuming AsyncStateSnapshot reports version, revision, active async/UI/
unknown counts and a readiness-integrity fault. Accepted unknown kinds and
untrackable non-UI starts fail closed. In particular, a notification-capacity
refusal for an unknown kind latches the fault before returning kResultFalse;
draining that FIFO cannot admit an untracked unknown task. Capacity-refused
recognized UI work alone does not block readiness. Existing bounded backpressure,
failed-finish active state, old drains, ID ownership and thread checks remain.

Three additive helper operations carry the snapshot, clone-owned progress drain
and ArmOfflineStateReadiness through both helper dispatchers, ordinary isolation,
the bounded closed codec, current owner and original total deadline/cancellation.
Ordinary isolation sends each operation once without respawn/recovery. The arm
checks the revision, active state and integrity and installs its token under one
registry lock. No registry lock crosses foreign plug-in code. Only an unarmed
Offline clone opts in; realtime and unarmed processing keep their prerequisites
and do not acquire this registry lock on their DSP path.

Strict observes readiness after LoadState and StartProcessing, before generic
bases, using its original owner/deadline/cancellation and only its own clone's
notification queue. It arms after generic bases, readback and metadata. Every
actual SDK process validates the token before and after the foreign call, and
metadata reads validate on both sides. Async start+finish within one call still
invalidates the candidate; output from that call is not copied, and the existing
terminal/retained-child cleanup prevents publishing a partial WAV. UI-only tasks
do not block DSP. No fixed sleep, new deadline, LoadState/MIDI/Process retry,
automatic re-arm, sample compensation or Kontakt-specific behavior is introduced.
SDK-reported readiness does not infer unreported instrument background state;
the observed first-note frame differences have not been causally tied to this
change, and native NOIRE acceptance remains pending.

Nineteen new regression functions cover 6 registry/actual COM QI cases, 5 actual
processor-boundary cases, 3 closed-codec cases, 1 closed-client owner/reap case,
and 4 application Strict cases. The existing backpressure case includes a legal
UI task plus 1023 updates, then a rejected raw kind 42 start, drain and failed
snapshot/arm, paired with a non-faulting legal UI-only rejection. Existing helper
no-instance and Unix ordinary-isolation no-respawn cases include all three new
operations. Strict fault/protocol/death/limit and blocked cancellation/deadline
matrices and exact command orders include their real readiness exchanges.

Actual static checks passed: vendor/root rustfmt with their respective editions,
embedded mock Python syntax, private complete 19-source reviewed patch replay,
and diff whitespace checks. Every new/affected Rust test, compilation, Clippy,
native Kontakt and audio check is UNRUN at this source checkpoint. The direct
SDK selector is expected to select 15 tests; Root all-feature tests select the
4 application tests. The exact Unix no-respawn selector selects 1 on Linux and
0 on Windows. CI/preview consumers and package source assertions retain the
previous offline MIDI gate and add direct SDK readiness gates, including a narrow
Linux readiness/no-respawn lane. Commands frozen for execution:

    cargo +stable-x86_64-pc-windows-msvc test --locked --manifest-path vendor/vst3-host-0.9.0/Cargo.toml --features experimental-fresh-helper --lib async_state_readiness --target-dir target -- --test-threads=1
    cargo +stable test --locked --manifest-path vendor/vst3-host-0.9.0/Cargo.toml --features experimental-fresh-helper --lib async_state_readiness --target-dir target -- --test-threads=1
    cargo +stable test --locked --manifest-path vendor/vst3-host-0.9.0/Cargo.toml --features experimental-fresh-helper --lib internal::isolated_plugin_impl::tests::checked_native_operations_never_respawn_a_dead_instance --target-dir target -- --exact --test-threads=1

The complete patch was regenerated from the verified original 41-file
archive and all 76 current vendor source files, excluding only this
document/CITRUS.patch and generated vendor/target output. There are 50
changed paths, 35 additions and 0 deletions. Private git
apply --check/apply succeeded; the complete file inventory and raw bytes matched.
Patch SHA256: 447b42ad6c89dd54a12390827ba6f5072169dad1cc72e672a5ecda315ac74be9. Original archive/commit/LICENSE/VCS identities remain.
The two provenance digest literals in each packaging Python file are refreshed.
Replay evidence:
target/backend-reliability-20261014/async-progress-readiness-provenance-01.
Reviewed source and original application evidence:
target/backend-reliability-20261014/async-progress-readiness-candidate-01.


### Rejected progress arguments also invalidate non-UI readiness (2026-10-15; Rust execution pending)

The readiness admission review found two additional untracked-start paths:
start returned kInvalidArgument for a null out-ID or a valid 1024-unit description
without a terminator before decoding the task kind. Neither refusal guaranteed
that a plug-in stopped its work, so an empty active set could incorrectly arm.
ProgressRegistry now decodes the kind first and uses a short registry-only lock
to latch readiness_faulted for these rejected async/unknown starts. The original
kInvalidArgument is retained, null out-ID is never dereferenced, and a rejected
recognized UIBackgroundTask does not fault readiness. This adds no lock to the
non-opt-in DSP path and retains the original bounded admission/COM contracts.

The existing unknown-kind/exhaustion regression now also uses both actual COM
entry points, AsyncStateRestoration/raw kind 42/UIBackgroundTask, and both invalid
argument forms. The description is a valid allocated 1024-unit UTF-16 array, not
an invalid pointer. Assertions check the exact result, untouched valid out-ID,
empty notification drain, shared non-consuming snapshot, zero active counts and
revision, and coherent fail-closed arm versus a ready UI-only control. All prior
capacity, unknown-kind and exhaustion assertions remain; the new test-function
count remains 19 (15 vendor with experimental-fresh-helper, 4 application).

Actual checks passed: edition-correct rustfmt on all 19 applied source paths,
embedded Python mock syntax, complete reviewed source patch replay and diff
whitespace checks. All Rust compilation/tests, Clippy, Linux execution, native
Kontakt and audio remain UNRUN. Existing Windows quality, preview, Linux and
offline-MIDI source consumer assertions are retained, including Root's newly
added exact Windows readiness command. No native first-note timing causality is
claimed. Source evidence:
target/backend-reliability-20261014/async-progress-readiness-candidate-01/reviewed-applied-static-freeze-02.json.

The complete patch was regenerated from the verified original 41-file
archive and all 76 current vendor source files, excluding only this
document/CITRUS.patch and generated vendor/target output. There are 50
changed paths, 35 additions and 0 deletions. Private git
apply --check/apply succeeded; complete file inventory and raw bytes matched.
Patch SHA256: 3349d66946abdf97244ac72e9d8703093b7c6b3f5d5d367561688d8e5657943a. Original archive/commit/LICENSE/VCS identities remain.
Both packaging Python files' two provenance digest literals are refreshed.
Replay evidence:
target/backend-reliability-20261014/async-progress-readiness-provenance-02.
