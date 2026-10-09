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
