# VST3 control / processor ownership preparation

This is the first staged preparation for removing native editor stalls from the helper's
processing path. It deliberately keeps the Linux/Windows helper single-threaded. It does not
claim lower process latency, dropout prevention, allocation-free DSP, or completed thread
isolation. The measured real-editor resize stall remains a reason for the subsequent work.

## Ownership and entry points

- `Vst3Host::load_main_thread_plugin` / `load_main_thread_plugin_class` are additive in-process
  loaders. They reject an isolation-enabled host before attempting a load. They instantiate the
  existing concrete implementation and return a `MainThreadPlugin` with private `Plugin` storage.
- The facade is deliberately `!Send` / `!Sync`, using an `Rc` marker. It has explicit method
  forwarding and no `Deref`, `DerefMut`, mutable legacy-owner access, or owner extraction. Loading
  must happen on the native UI/main thread; safe code cannot later move the facade or its destructor
  elsewhere. Linux/Windows helpers now use this entry point and a main-thread `Rc` container.
- Existing `Plugin`, `PluginInternal: Send`, playback interfaces, and the macOS sample-helper
  threading model retain their compatibility behavior. The additive facade does not retroactively
  prove the legacy paths safe to use with editors on arbitrary threads.
- `PluginImpl` contains `ProcessorRuntime` followed by `ControlDomain`. The latter retains owning
  component/processor/controller references, editor and frame, lifecycle administration, module,
  factory and host context. Normal teardown still detaches, stops, deactivates, disconnects,
  terminates and releases in order. Single-component aliases are initialized/terminated once.
  The factory is retained on every platform. The initialization guard is consumed and releases
  its extra COM references immediately at ownership transfer, before subsequent load failures can
  unload the module. Host context is created before module loading on all platforms so early
  error-unwinds also retain it through module teardown.
- `ProcessorRuntime` owns prepared process data and audio/event storage, transport, note tracking,
  pending processor parameters and controller-derived MIDI/program caches. Its processing methods
  cannot query the component/controller. Runtime storage drops before the control/module owner.
- `ProcessorLease` borrows the owning `ComPtr<IAudioProcessor>` exclusively. Its only operations
  are `process` and `set_processing`; it cannot clone, cast, addRef/release, query metadata or own
  COM references. Its borrow cannot outlive the owner, and it is also `!Send` / `!Sync` at this stage.
  No new unsafe `Send` implementation is introduced.

## Lifecycle and cache behavior

Control preparation performs `setupProcessing` and `setActive(true)` before the narrow processor
start notification. Stop completes the processor notification and ends the exclusive lease before
control deactivation. Failed stop/deactivation retains the last confirmed state and reports error.
The lease is a synchronous lifetime fence only; it is not a future worker acknowledgement.

The flat Process helper now obtains channel counts from the prepared runtime buffers. It no longer
makes per-block component bus metadata calls after successful setup. Metadata fallback remains for
inspection when the initial best-effort setup has not succeeded.

This work also fixes a discovered behavior bug rather than hiding it as a pure refactor:
`LoadState` previously resumed with the old prepared bus buffers. State restore now invalidates and
rebuilds storage while inactive before resume, preserving activation choices for surviving bus
slots and applying defaults for new slots. Reconfiguration and serviced I/O restarts invalidate
and rebuild too. A declined `setBusArrangements` can legally select a different fallback layout,
so its refusal path refreshes storage before reuse. Failed rebuilds cannot reuse old process
pointers or pretend to resume successfully. State and bus-change errors remain observable. The in-process facade synchronizes its processing
flag after state/restart failures, so a retry cannot falsely return success from an obsolete flag.
The legacy outer isolated client still has separate cached-state recovery limitations after such
errors; no new protocol response fields are added to solve them in this stage.

## Preserved protocol and safety boundaries

No HostCommand/HostResponse variant, field, ordering, protocol version or request-ID behavior is
changed. Claiming isolated stdout still precedes loading any plugin. Native dirty revisions,
loss/capture rejection, editor detach, zero-sample flushes, checked drains, data-exchange gates and
Linux factory/frame IRunLoop ownership remain in the existing synchronous paths. There is no new
worker to detach and no module-unload-on-hung-worker policy implied by this commit.

At the ownership checkpoint the temporary `LegacyProcessBridge` retained GUI-parameter locks, feedback
behavior and host data-exchange callbacks. The follow-on parameter storage below supplies
bounded prepared parameter containers, but their mutexes remain; EventList storage, data exchange
and public metering still need separate allocation/lock-safe work before enabling a processor
thread. An `Arc<Mutex<Plugin>>` would not solve native editor stalls.

## Subsequent work, separately reviewed

A real split needs bounded lanes with distinct producers: main-to-DSP native edits, broker-to-DSP
jobs, DSP-to-main controller feedback and DSP-to-broker completions. The broker must be the sole
protocol writer, keep v1 ordered one-in-flight semantics, and dispatch Process independently of
GUI pumping. Administrative/state requests remain head-of-line blocking in v1.

Worker activation also requires verified quiescence, edit watermarks and generation rejection,
state-capture loss checks, main-thread COM release and run-loop cleanup, and fail-stop handling
when a worker cannot join. No owner/module may be unloaded beneath a live processing lease.
These requirements are not implemented by the preparation commit.

## Contract sources

- [Steinberg IAudioProcessor](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IAudioProcessor.html): setup and metadata are UI-domain operations; process and the lightweight processing notification have separate roles.
- [Steinberg IComponent](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IComponent.html): activation, state, buses and component lifecycle.

## Verification

Validation receipts and exact executed commands are recorded in `WORK_LOG.md`. Headless mocks,
compile-fail checks and protocol tests establish their own limited properties. They do not replace
native Windows/Linux editor regressions, real-plugin resize-under-audio measurements, or physical
hardware acceptance of a future worker implementation.


## Historical ownership-preparation genuine regression outcome

After the source freeze, the exact helper passed native fixture lifecycle/input and
fresh-instance Surge/Stochas state checks. Unpaced functional PCM/MIDI/reconfiguration
checks are also scoped separately. **Full restoration acceptance is not passed:**
reused Surge XT 1.3.4 state loses the first immediate note and leaves the host getter
stale even after the component volume has applied. Old/new helper comparisons prove
this defect predates the ownership preparation. Native content/container sizing can
also disagree until detach/reopen. The [state-restore limit report](PLUGIN_STATE_RESTORE_LIMITS.md)
records positive results, corrected interpretation and failures together. The old
resize processing stall remains; no new worker or latency improvement is implied.


## Ownership-preparation combined-source verification

Integrated runtime `3fb549a059916dca7b3af3eff00ff27fba0bddd4` exactly preserves
reviewed helper/vendor/Cargo/fixture bytes. App/audio/UI and metronome are unchanged
from the preceding source. Fresh root gates pass 1,137 app +21 helper +5 editor
protocol +2 transport protocol all-feature tests, 1,133 core tests, fmt, both strict
Clippy modes, app/helper build, ordinary protocol smoke and both Windows MSVC source
profiles. All 195 Python tests pass, including four live Unix descriptor checks.

The external source-linked vendor test manifest preserves production dependencies
and enables exactly `cpal-backend,process-isolation`, with default features off.
All **270 available unit cases** pass serially; the named
`internal::module_info::tests::reads_sdk_generated_bundle_metadata` is filtered
because the registry package omits its upstream Dexed bundle. That is not a full
upstream-fixture pass. All **26 doctests** pass, including the five new facade cases
(one positive and four compile-fail contracts). The original parallel fake-helper
ETXTBUSY observation is retained; the serial pass does not explain its cause.

Pristine archive plus the pinned patch reproduces all **43 original/added files**;
original license/manifests remain unchanged. Fresh helper bytes match the genuine
regression helper `cb1899fd…` exactly. The exact published5befb51 preview passed;
quality still failed native paint, while independent state/interaction/lifecycle
checks passed. New guard verification remains separate. No app UI source changed
or new desktop/performance run was performed during integration.

## Follow-on Surge state-restore compatibility guard

The ownership-preparation checkpoint did not fix Surge XT 1.3.4 reused-instance restoration.
Fresh-instance restoration before the first positive Process succeeds, but after prior processing
Surge defers component state, the first note can be erased by `loadRaw`/`stopSound`, and JUCE's
immediate controller synchronization can cache old values. A later component SaveState and native
UI show the correct volume while the controller getter remains stale. This is not evidence of
permanent processor-volume loss.

A proposed internal 32-frame settlement transaction was rejected before commit: Surge's native
preset loader can leave `halt_engine` true while a plugin-owned background task waits for its
mutex. Process then reports success without reaching the queued state application. Fixed blocks,
sleeps and arbitrary retries cannot establish completion, and no hidden settlement is enabled.

The separate compatibility guard matches only instantiated factory UID
`ABCDEF019182FAEB566D624153675854` and exact version `1.3.4`. Display names and bundle paths are not
used. This metadata selects policy; it does not authenticate a binary. The characterization is
from the installed official Linux x86-64 Surge 1.3.4 package and its official source tag, not a
cross-platform/build completeness claim.

- Any attempted positive Process, including one returning an error, makes this instance ineligible
  for later state restore. Zero-sample parameter flushes do not mark it processed.
- Entering an explicit native editor opening/attachment attempt also makes the history ineligible,
  even if opening later fails. Existing temporary createView probes used during loading or metadata
  inspection are unchanged; this flag is not proof that no plugin UI initialization ever occurred. Closing the editor, stopping, reconfiguring or changing process mode never resets
  either history flag. Only a newly loaded instance is fresh.
- Ineligible LoadState returns an actionable error before component/controller calls, lifecycle
  changes, queue drains, state mutation or helper-native editor detachment. Linux and Windows
  dispatchers share the local preflight; the in-process implementation repeats it authoritatively.
- A caller must prepare a fresh candidate, restore its authoritative blob before playback/native
  interaction, and retain the old instance until candidate success. This is a requirement for
  safe transactional replacement, not a handshake added by this helper guard. The later
  [timing configuration path](PLUGIN_TIMING.md) adds tagged capture and prevalidated replacement
  retention at the App/worker level; combined verification is separate. There is no new
  helper-side ownership swap or wire command.
- The current App's normal configuration/reopen/restore path already uses fresh candidates. The
  legacy public `PluginChainControl::load_state` path still closes its native editor and faults its
  slot when the backend rejects a load; it is not called by the current App and is not changed here.
  Direct helper/in-process preservation must not be presented as preservation through that legacy
  full-chain API. Remote preflight remains the helper's responsibility; the additive local API does
  not introduce an extra isolated-client protocol roundtrip.

An adjacent existing alias-state bug is fixed separately in this slice: a nonempty controller
payload in an envelope must not call `IEditController::setState` when that controller aliases the
component. Both controller state calls now obey the same single-component guard.

Source references: [Surge processor, release 1.3.4](https://github.com/surge-synthesizer/surge/blob/release_xt_1.3.4/src/surge-xt/SurgeSynthProcessor.cpp),
[Surge synthesis and background loading](https://github.com/surge-synthesizer/surge/blob/release_xt_1.3.4/src/common/SurgeSynthesizer.cpp),
[state queue/application](https://github.com/surge-synthesizer/surge/blob/release_xt_1.3.4/src/common/SurgeSynthesizerIO.cpp),
[the release's pinned JUCE wrapper](https://github.com/surge-synthesizer/JUCE/blob/cf5754b19c87ea63758802e3f4239c05a77f1412/modules/juce_audio_plugin_client/juce_audio_plugin_client_VST3.cpp).


## Bounded native-edit delivery and capture fence

The next bounded slice replaces the native editor-to-processor bridge only. The helper still
executes GUI and Process requests on its existing single thread. It does not solve the measured
resize stall, create a DSP worker, change protocol ordering, or establish whole-process real-time
safety.

`internal/native_edit_transport.rs` allocates a fixed-capacity rtrb ring and equally bounded staging
storage at instance creation. Each native value carries its generation, sequence, parameter ID
and value. `ComponentHandler` owns the producer, serialized by a producer-only `try_lock`; a
contending/reentrant producer refuses delivery and records sticky loss rather than waiting. The
runtime exclusively owns the consumer. No producer/display guard is held around a plugin call,
and the consumer never takes either guard. No new unsafe Send/Sync implementation is introduced.

Each actual SDK Process call, including an explicit zero-sample flush and each split chunk, has
its own admission/acknowledgment unit. `ParameterChanges::try_enqueue` reports failed queue/point
admission. Only complete admission followed by a successful Process advances the applied
watermark. Failed or abandoned staged values make delivery loss sticky for that instance; later
empty or successful calls cannot erase the uncertainty. An unrelated failed call with no staged
native values does not invent native delivery loss. At this transport-only checkpoint, COM
parameter/event storage still allocated and locked; the parameter preparation below narrows that
boundary. Event storage and data-exchange/metering remain separate future work.

Display values and gesture polling are independent of DSP input. This intentionally fixes stopped
polling: polling native values while stopped no longer consumes the pending processor edit or
permanently invalidates an otherwise deliverable save. A later explicit zero-sample flush can
admit and acknowledge the values, after which state capture is allowed. Display-log overflow is
not the same as processor-delivery loss.

Capture requires no in-flight publication/process, submitted and applied watermarks to agree,
no lifetime-sticky loss/exhaustion, and the same durable native dirty revision before and after
component/controller state serialization. Queue emptiness alone is never delivery evidence.
Counter exhaustion refuses capture instead of wrapping.

Successful component/controller state application explicitly supersedes pending edits in a new
generation at the existing queue-clear boundary. Discarding old values never labels them applied.
This also applies if later bus/setup rebuilding fails: subsequent recovery must not replay edits
from before the applied state. Earlier rejected/failed state application leaves generation and
pending native input unchanged. The exact Surge preflight still precedes all of these mutations,
and its positive-call/editor-attempt history remains sticky.

If the post-application supersession fence cannot acquire its producer guard, or overlaps a
publisher already in flight, state has already been applied. The operation returns an explicit
restore-fence error and permanently invalidates native input for that instance; queued/new native
values cannot replay, and state capture remains refused until a fresh instance is loaded. This
partial-restore failure is distinct from the mutation-free Surge eligibility rejection.

The transport-only allocation tests measure first-use, full/empty, wraparound and failure paths
without per-operation allocation or deallocation. A latch-controlled 350 ms producer/display
stall fixture demonstrates that an independent consumer can finish already-admitted transport
work before those guards are released. It does not run a real plugin or demonstrate independent
DSP execution in the still-single-threaded helper. COM integration fixtures intentionally allocate
and lock and cannot be used as whole-process real-time evidence.


### Historical native-edit integration verification

Runtime87ceb06 matches reviewed48d3f97 for all41 bound source files; App/timing,
helper dispatch/wire and Cargo remain unchanged from244f622. Fresh integration
passes1,170 app +22 helper +5+2 protocol tests,1,166 core,215 Python,304 available
vendor cases (one missing upstream fixture explicitly filtered),26 doctests, fmt,
both strict root Clippy modes, all-bin build/helper smoke and both MSVC profiles.
Pristine registry archive plus the exact cumulative patch reproduces44 files.

The copied helper hash is
`dca08353e3f23308d535a791c9fa2c89635ee683db29625fa8d2a3d0a988cbe8`,
matching independent native stopped-edit/poll/save/fresh-state checks. A separate
bounded production-graph regression on this helper passes all four default2048
ordinary/routed × fixed/changing cases plus fresh-state immediate-note behavior.
Expected PDC4896/7328 and routed8on/8off remain exact, with no new worker fault or
capture overflow. Both test invocations exit0. Changing debug callbacks still have
11/14 raw-core interval overruns; this is correctness acceptance, not a new
performance matrix, device or realtime qualification. Older optimized results keep
their original helper/source identity. This native-edit source subsequently passed Windows source/preview at e487a50; known paint failure remains.

The separately source-bound [new-helper correctness receipt](../qa/native_edit_regression/RESULT.md) retains all four default2048 delivery passes, fresh-state immediate-note proof, and the11/14 changing-callback debug interval overruns for87ceb06/helperdca08353. The historical optimized4fdfbc2/244f622 results are not reattributed to this helper.

## Prepared bounded parameter COM storage

The parameter-container slice prepares stable queue objects before processing. Input storage has
8192 queue slots and 8192 **total** points, covering the existing 4096 pending host values plus
4096 native values in one SDK call. Output storage has 4096 queue slots and 4096 total points.
These are Citrus host policies, not VST3 SDK limits. No per-queue multiplication of the point
budget is used.

Each container owns a fixed boxed array of COM wrappers. Each queue holds a slot index and safe
shared ownership of one mutex-protected arena; the arena does not own its queues. Returned queue
pointers are borrowed from the container, as in the SDK hosting implementation. A plugin may
explicitly AddRef/queryInterface/release them, and a retained queue keeps its arena alive even
after the container is dropped. Lookup, insertion and logical reset neither create/drop queue
objects nor allocate/free storage. Stable interface addresses survive block resets. Mutexes remain;
no unsafe Send/Sync or concurrent plugin ownership is introduced.

The first-seen parameter-ID order is retained, with one active queue per ID. Points are sorted by
sample offset, preserving arrival order and every value at equal offsets. This deliberately retains
Citrus's prior duplicate semantics rather than adopting the SDK sample helper's equal-offset
replacement. Capacity checks precede mutation: rejected host insertion cannot leave an empty
phantom queue. Invalid read indexes and null optional read outputs follow the COM failure contract
without inventing delivery loss. Lossy write failures and unusable poisoned storage latch evidence
which ordinary clear/reset cannot erase. FFI methods do not panic on poisoned locks.

ID lookup is bounded linear search, as before. Points use one global linked-node arena with a tail
append fast path, one shared ordinal-index array, and a preallocated ordered list of populated queue
slots. This private list does not change public registered queue order/count, including empty
queues. With a valid cache, binary upper-bound search
finds a non-tail insertion rank, retaining equal-offset arrival order. A dirty cache uses the bounded
linked search fallback. A valid index gives constant-time getPoint;
a dirty index is rebuilt once in O(populated queues + points) on the next valid read. An insertion into a valid
index shifts only the affected ordinal suffix and adjusts later populated queues' offsets, retaining
validity. Binary search finds the target in the populated-slot list. Only later populated queues
have their offsets updated; empty queue offsets are never used. The first accepted point inserts
its slot into that list after all admission checks, deriving its start from the next populated
queue or arena end. Rejected new-host-ID activation restores the prior inactive slot and count.
Normal insertion costs O(log populated queues + rank search + point suffix + later populated
queues); final-populated-queue tail append moves no suffix. Public empty queues therefore do not
add per-point offset-update work. An already dirty cache stays dirty until materialized. Reset only
updates logical metadata; it neither deallocates nor changes stable pointers. Mixed writes/reads and large-suffix layouts
are measured explicitly and are not covered by a low-latency claim.

### Admission, output faults and recovery

Every host and native input insertion is checked before the actual SDK Process call. The host's
4096-value pending limit is checked before controller mirroring, so overflow returns
`ParameterInputRejected` without changing either controller or pending input. Its diagnostic
counter saturates. A failure while preparing a call clears its partially staged parameter/event
contents; the caller block's pending host values are consumed/dropped on return, as in existing
failed-call cleanup. Native input is left pending if host admission failed before native staging;
a partially admitted native batch instead records sticky native delivery loss. No failed admission
invokes the plugin, acknowledges native input, or leaves staged points to replay.

The SDK result alone determines whether that call's admitted native edits were applied. Output
parameter overflow is separate evidence: even when a plugin ignores addPoint/addParameterData
failure, Citrus returns `ParameterOutputRejected`, latches a runtime-owned fault and refuses later
Process and SaveState. The fault is observed before staging, before/after state capture, and before
old process storage is discarded; retained queue activity during getState cannot hide it. If the SDK
also fails, output rejection takes diagnostic precedence while native acknowledgment still uses
the failing SDK result. Ordinary invalid read probes do not cause this permanent fault.

Stop/start, generic state restore and reconfiguration may still complete administratively, but
cannot clear the output fault or restore Process/SaveState success. Recovery requires a **fresh
plugin instance**. The exact Surge compatibility preflight keeps its existing precedence. Native
generation, dirty revision, loss and applied watermarks are not reset to disguise output loss.
Existing output feedback force-push behavior is unchanged.

### Evidence boundary

Counted regions begin before first use of cold prepared storage and include full capacity,
8192 distinct IDs, dense curves, reuse, failure, explicit retained COM references, and a dedicated
allocation-free processor mock admitting 4096 host plus 4096 native edits. The guarantee is about
host parameter storage operations. Plugin work, events/payload ownership, controller mirroring,
GUI feedback, metering, data exchange and lifecycle construction/destruction remain outside it.
The helper remains single-threaded; the earlier measured native resize stall remains unresolved.

The SDK ownership contract is visible in its [hosting implementation](https://raw.githubusercontent.com/steinbergmedia/vst3_public_sdk/master/source/vst/hosting/parameterchanges.cpp)
and [parameter interfaces](https://raw.githubusercontent.com/steinbergmedia/vst3_pluginterfaces/master/vst/ivstparameterchanges.h).

Prepared inner-container constructor accounting on Linux x86-64 (Rust 1.99.0, system allocator,
per-thread counting, no allocator metadata/RSS): input requests 1,048,696 bytes in 8,198 allocations;
output requests 524,408 bytes in 4,102 allocations. The sparse populated-slot arrays add 98,352
bytes total versus the superseded dense-offset version (96 KiB arrays plus 48 bytes arena metadata). Each constructor makes no deallocations. These
figures include the queue wrappers, shared arena and index, but exclude the runtime's two outer
`ComWrapper<ParameterChanges>` allocations. Host construction/destruction is lifecycle work.
A plugin's explicitly retained queue can extend an old arena's lifetime; its final Release may
free that retired queue/arena on whichever thread the plugin uses. Balanced retain/release while
the container owner still exists is measured allocation-free, but final retained-after-owner
destruction is outside the measured region. Future worker ownership must account for retired
objects; whole-Process no-free behavior across reconfiguration is unproven. All measured
post-construction parameter operations, including cold first use and failures, allocate/free zero
bytes. Comparative raw timings include the same counting/assertion instrumentation and must not
be interpreted as audio deadlines.

Matched comparisons use the unchanged `48d3f970` baseline and archived cursor/indexed drafts,
five repetitions, identical value-producing workloads and checksums, cold and reused blocks, and
32/128/512/4096/8192-point budgets. Small synthetic cases set queue/point limits to their case
size; production budgets remain 8192/4096. On this AMD EPYC 9V74 x86-64 runner, Cargo test release
(opt-level 3, debug info 0, default harness codegen; no claim of production release/LTO timing),
selected 8192-point reused medians in milliseconds were:

| Operation | Prior baseline | Final prepared storage |
|---|---:|---:|
| Ordered insertion | 9.699 | 0.180 |
| Random insertion | 8.192 | 2.005 |
| Random point reads | 0.0473 | 0.0666 |
| Distinct-ID insertion | 24.543 | 21.218 |
| Earlier-queue alternating insertion/read | 9.747 | 0.455 |
| Large single later-populated-queue suffix edits | 0.657 | 1.293 |
| Large multi-populated-queue suffix edits | 0.744 | 1.678 |

The shared index trades small constant read/check overhead and global suffix movement for bounded
storage. The last two rows explicitly retain a roughly 2.0–2.3x local slowdown at the stress limit;
this is explained algorithmic cost, not a claim of universal speedup. At 512 points those suffix
medians were 0.00557/0.00557 ms before and 0.00867/0.01192 ms after. Small workloads also
retain constant overhead: 32 random insertions measured 0.000590→0.001392 ms and 128
distinct-ID insertions 0.004196→0.008032 ms. Linear ID lookup still makes
all-distinct-ID construction quadratic over the sequence; repeated suffix movement/populated
metadata shifts also retain quadratic aggregate worst cases. Debug results, min/max ranges, cold
allocation costs and rejected drafts are retained alongside the exact loops/source hashes in the
parameter-comparison receipt. The rejected cursor draft's random reads and first index draft's
alternating rebuild regressions were measured before correction, not removed from evidence.

A separate sparse stress diagnostic precreates 4096/8192 registered queues, leaving all later
queues empty, then measures early-queue edits independently of queue preparation. The rejected
dense-offset candidate needed 1.218 ms for 8192 queues/128 edits in the optimized profile
(23.865 ms debug). The populated-slot correction measures 0.004216 ms versus baseline
0.006560 ms optimized, and 0.0602 ms versus 0.0460 ms debug. With 4096 queues/128 edits,
optimized medians are 0.004186 ms versus 0.006130 ms baseline. Thirty-two-edit optimized
cases retain small overhead: 4096 queues 0.001081→0.001473 ms; 8192 queues
0.001412→0.001802 ms. These figures include real reads/checksums; construction, preparation,
reset and empty-queue verification are separately measured. All superseded results remain
in the comparison receipt.


### Combined parameter-source verification

Integratedfb7b91a is byte-identical to reviewedbf573a4 for production/vendor/Cargo/
root tests; App/timing/wire are unchanged. Fresh Linux1170 app +22 helper +5+2
protocol tests,1166 core,339 available vendor cases (one explicit missing fixture),
26 doctests,231 Python, fmt, both strict root Clippy modes, all-bin build/helper
smoke and both MSVC source checks pass. Pristine archive plus pinned patch rebuilds
44 source files and preserves original license/manifests. Fresh copied helper
`a29e4942b5fe027e9891a28e5d7f7611d2c5b2f12ef5f283599d15c4278aee64`
matches the independent native/first-note evidence. New bounded default2048/fresh-
state production-graph regression passes all four default2048 cases plus fresh state,
both exits0, with exact PDC4896/7328 and routed8on/8off. Changing debug cases retain
13/14 core and15/14 outer interval overruns; no performance upgrade is claimed.
The exact parameter checkpointb92c394 subsequently passed Windows source/preview;
known native paint remained failed.

The [exact parameter-helper regression](../qa/parameter_storage_regression/RESULT.md) binds124 production files tofb7b91a and new helpera29e4942. Its separate fresh-instance case uses historical captured input state104bc8ab… (not the newly authored011ded9e… native blob), with onset24/peak0.22329643368721008 and exact controller/component values. Both inputs have independent functional proof; their test identities and13/14 debug core interval overruns remain distinct.


### Portable comparative evidence

The [concise cost/source record](../qa/parameter_storage_cost/README.md) retains
complete final debug/optimized CSV matrices, constructor accounting, native/source
receipts and the exhaustive105-entry raw-cost index. The separate source-bundle
appendix `parameter-storage-bf573a4-source-evidence.zip` has SHA256
`1bd33df7c1ec9afa58cd5b6a508b7af12c4a84f68ce4ad12cda5dca3f388773c`
and5092126 bytes. Its1677 logical files are losslessly represented by435 unique
text objects with path maps and a reconstruction verifier; all450 archive members,
source identities and28 license/provenance associations are checked. Prior cursor,
indexed, maintained and dense candidates remain explicitly historical, including
original folders named `final`; no failed comparison is sampled away.

Only named private path prefixes are normalized, with original and published hashes.
Raw cost CSV CRLF bytes are preserved exactly. The archive excludes executables,
plugin assets, captured state/audio, caches and unrelated files. It is a validation
appendix, not another Git source tree or a renewed whole-plugin realtime measurement.
The earlier optimized244f622 appendix stays independently historical.

## Checked event admission and note-release obligations

The event-admission checkpoint checks the existing 4096 event-header, 8 MiB aggregate-payload,
1 MiB per-data-payload and 16,384 UTF-16-unit per-text limits before copying raw SDK payloads.
One existing event-list mutex covers capacity/metadata checks, deep copy and publication.
Unknown variants, invalid/oversized declared lengths, nonempty null payloads and misaligned text pointers
are rejected before constructing slices. Raw callers must still provide initialized selected
union fields and readable payload memory for admitted events; these checks cannot validate an
arbitrary foreign pointer. Text representation and declared-length copying are unchanged.

The checked owned/raw paths return an allocation-free internal error, and SDK `addEvent` returns
failure when enqueue fails. Checked admission and SDK addEvent callbacks no longer format or log errors;
the existing loss latch reports failure and retains its existing draining acknowledgment.
Rejected writes leave event order, count and payload budget unchanged. Scalar admission,
full-capacity and poisoned-lock rejection use no allocation/free in focused counted tests.
Payload success still allocates a Vec, and rejecting an already-owned payload may free it.
This is not a prepared payload arena or whole-Process allocation guarantee.

`EventInputRejected` now reaches ordinary MIDI, owned events, note expressions and tracked
voice APIs instead of claiming success after a dropped event. Note counters, candidate IDs and
tracked voices commit only after admission. A rejected note-off retains its release obligation.
At 1024 tracked voices a new tracked note is rejected rather than evicting an existing voice.
A positive ordinary note-on at its per-key u16 maximum is rejected; a zero-velocity note-on still
acts as note-off. The existing wrapping ID policy is retained, with rejection if its candidate
is still active; releasing that ID permits a retry without skipping IDs.

Panic is prefix-commit, not atomic rollback: exact tracked releases are queued first in tracker
order, then one event per ordinary count. Only admitted releases are removed/decremented.
It admits at most 4096 releases per call across both kinds; full queues or remaining obligations
return failure and retain the unsent suffix for retry. Exactly 4096 releases succeed when no
obligation remains. A later mapped-controller parameter failure does not undo earlier accepted
releases. Accepted admission means queued, not confirmation that SDK Process applied it.
Event admission itself does not call SDK Process or acknowledge native parameters. Output-event
overflow retains its existing one-shot loss-aware drain and does not turn a successful SDK call
into the separate permanent output-parameter fault or falsify native acknowledgment.

Production changes in this checkpoint are limited to event admission and note bookkeeping.
Chunk routing, failed-process cleanup, state fences, parameter/native storage and helper/wire
behavior are unchanged. The active application propagates event failures through its existing
slot-fault/note-safety path. The unchanged vendor legacy playback/realtime helpers still ignore
some send results. Existing void `reset_with` / staging-drain poison behavior also remains a
separate follow-up. Existing getEvent/getEventCount and clear/reset diagnostics are also unchanged.
Public invalid-MIDI validation may still allocate formatted errors.
Locks, payload allocation/free and arbitrary plugin callbacks remain; the single-thread helper's
historical 346.9 ms native-resize stall is not resolved by this correctness slice.


### Event-source integration and evidence boundary

Reviewed7d8a416 integrates as7d7294b with all124 production hashes equal to the
independently tested frozen source. The [checked-event correctness receipt](../qa/event_admission_regression/RESULT.md)
retains original basebf573a4 plus reviewed482031cc diff and a separate committed
7d8a416 equivalence record; base alone is not the tested source. Helper59b6bcbd and
testeea1adf8 identities remain exact. Default2048 four delivery cases and separate
fresh-state first note pass, both exits0, with12/14 changing debug core overruns.
Current App/reset source remains unchanged; no reset-policy experiment is included.
Fresh combined gates pass1170app+22helper+5+2protocol,1166core,362availablevendor
(one explicit missing fixture),26doctests,239Python,fmt/two strictrootClippy modes,
build/helper smoke and bothMSVCsource profiles. Fresh helper hash equals the
independently tested59b6bcbd artifact; no post-gate production edit. Exact Windows
CI remains separate and pending.

## Private scoped domain sessions (checkpoint 1)

The additive crate-private `MainThreadPlugin::with_domain_session` enters through an object-safe
`PluginInternal` callback seam. Non-local/unsupported backends reject it by default. An explicit
session checks the loading/control thread and exclusively borrows the outer owner for the entire
callback. The ordinary legacy processing path keeps its existing caller-thread behavior.
No helper dispatch, broker, worker, metadata policy or public owner-extraction API is added.

Opaque `ControlOps` and `ProcessorOps` each contain an explicit Rc marker and remain !Send/!Sync.
Their constructors and fields are private; neither has Deref, into_inner, raw COM extraction,
owner access or a second-session entry. Higher-ranked callback lifetimes prevent a capability
or borrow from escaping. All component/controller/processor references, module/factory and
runtime fields remain physically owned by the original aggregate; no runtime slot is removed,
no replacement sentinel is needed and no owning COM reference is cloned/released for a visit.

- `ControlOps` borrows only disjoint controller/view/resize/deferred-display fields and Linux
  host UI services. It can read/format controller values, resize an already attached editor,
  drain resize requests and service the UI run loop. Attach/detach, state, setup, activation,
  teardown and processor metadata remain aggregate-owner operations after rejoin.
- `ProcessorOps` exclusively borrows runtime state, the existing module-backed ProcessorLease,
  a restricted data-exchange gate and a copied active flag. It provides ordinary flat/bus
  processing, validated processor-only parameter admission, shared owned-event/tracked-note/
  expression admission, explicit transport and output drain. Queue-only parameter admission
  does not mirror or freshly confirm the controller. Mixed legacy setters and mapped MIDI keep
  their existing capacity-preflight → controller mirror → queue order on the aggregate owner.
- The copied active flag cannot be changed through either facade. Metadata/cache refresh and
  every coupled administrative operation require lexical session completion first. Ending the
  borrow is the fence; no timeout or readiness boolean substitutes for it. Existing user audio
  callbacks and metering hooks are not smuggled into the internal capability API.

`LegacyProcessBridge` is removed. Its replacement borrows only the existing data-exchange
in_process AtomicBool. `enter(&mut gate)` returns a !Send/!Sync guard whose Drop clears that flag
on return or Rust unwind. Enter/drop allocate nothing, clone no Arc/COM reference and acquire
no GUI/lifecycle mutex. The guard surrounds only each actual SDK Process call. Existing data-
exchange queues, receiver delivery, main/background dispatch and shutdown remain unchanged.

Existing ordinary processing now uses the same restricted processor capability synchronously.
Runtime note, transport and output bodies are mechanically shared with legacy entry points.
Native acknowledgment still follows actual successful SDK calls; callback errors after success
do not revoke application, and failed SDK/native/output-parameter paths keep their original
loss/fault semantics. Zero-sample save flushing and exact Surge preflight/history are unchanged.
Rust callback/gate unwind tests do not establish recovery from a plugin unwinding across an
extern COM boundary or retroactively promise rollback of arbitrary plugin-side effects.

Real COM mocks verify scope/rejoin, loading-thread UI work, flat/bus samples, host/native/event
admission, reference-count ledgers, single-component termination before module drop, error/unwind,
unsupported backends and state/Surge/fault boundaries. Private compiler fixtures use the actual
source/API with one passing same-import baseline before testing forbidden capabilities and
borrows. An unavailable private import is explicitly rejected as false-positive evidence.
This is reusable ownership factoring only. Event payloads and other callback work can allocate,
locks remain, and the historical 346.9 ms native-resize process stall remains unresolved.
### Private domain-session compile contracts

`scripts/check_vst3_domain_contracts.py` checks the real crate-private scoped API
without adding production exports, test hooks, feature flags, or dependencies.
It generates one shared positive body and 52 negative cases in an external QA
directory. The positive fixture reaches both actual entry points and all allowed
facade methods before any negative can count. Negative diagnostics must match
the stated error codes/text and originate in the fixture; unavailable imports
and inaccessible entry points are explicitly rejected.

Run these commands from the full source checkout; the binary preview guide alone
does not include the compiler runner or full vendored source.

Requires Python 3.11+, the same approved runtime-only source-linked vendor
manifest/lockfile used by the vendor source gates, and a coordinated exclusive
slot for the existing Cargo target. The input manifest must reference the selected
production `src/lib.rs`, preserve its dependencies/features/target dependencies,
and omit bin/test/example/dev-dependency targets. Keep that exact matching lock;
the runner neither substitutes root `Cargo.lock` nor resolves/downloads packages.
The caller supplies the established toolchain, build-profile and platform library
environment (for the shared low-debug target: `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0`).

```
python3 scripts/check_vst3_domain_contracts.py \
  --repo "$REPO" \
  --manifest "$VENDOR_QA/Cargo.toml" \
  --lockfile "$VENDOR_QA/Cargo.lock" \
  --qa-dir "$QA/private-domain-contracts" \
  --target-dir "$CARGO_TARGET_DIR"
```

Do not run this concurrently with another task using that target. `--target-dir`
may be omitted when `CARGO_TARGET_DIR` is set; conflicting values are rejected.
All checks use `cargo check --locked --offline --lib --no-default-features
--features cpal-backend,process-isolation`. Generated artifacts must stay outside
the repository. `report.json` and per-run logs preserve exact fixture sources,
compiler identity, all diagnostics, input-lock/shim/runner hashes, and before/after
production source hashes. Source or manifest/lock drift fails the run. Repeated
`--case NAME` options support diagnosis, always preceded by the positive case;
a selected-case run is not the complete 53-case gate.

Coverage includes both facades' Send/Sync exclusions, scoped-reference escape,
move/borrow restrictions, owner SaveState/LoadState/reconfigure/drop and nested
session exclusion, and missing admin/metadata/raw pointer/into_inner/Deref APIs.
These static guarantees complement the runtime session/COM lifetime tests; they
do not establish real-time safety or authorize an audio worker thread.

The combined reset-origin checkpoint adds explicit negative compiler cases for
both reset APIs on each facade and for owner reset during a live session. The
positive baseline calls owner reset after rejoin. The same restricted RAII gate
surrounds only SDK Process in ordinary and reset calls; combined COM regression
checks entry/exit on success, SDK failure and pre-SDK staging failure.


## 2026-10-10 — Checked replacement metadata, unvalidated source checkpoint

The first resumed patch starts from accepted `cbbbe1f`, independently of the unavailable
unpublished dispatcher checkpoint. Isolated latency/tail getters previously converted failed
IPC polls into zero; production readiness and later metadata publication could accept those
zeroes as genuine answers. New checked getters preserve transport, Error and wrong-response
failures without automatic helper recovery. The worker consumes one fallible metadata pair;
neither value is accepted unless both reads succeed. Legacy infallible APIs remain compatible.
Existing helper response variants and main-thread ownership are retained.

Source tests cover valid zero/nonzero/infinite tail, error/wrong replies, helper death with
legacy recovery opt-in, initial readiness rejection, later sticky faults and old candidate
state/endpoint retention. Timing Retry or a new transport epoch cannot revive a faulted helper;
only a separately prepared fresh candidate with successful checked metadata can become ready.
No automatic reload, state discard, silent warmup, worker separation or SHM workaround is added.

Status at this archival checkpoint: **UNVALIDATED, tests not run**. The replaced environment
has no Rust toolchain; the source archive is persisted before toolchain bootstrap. Formatting,
compilation, test execution and mechanical vendor patch/hash regeneration remain pending.
The adjacent AudioBusLayout fallback is a separate approved follow-up and is unchanged here.


### Executed checkpoint 01 validation (2026-10-10)

On the recovered Linux cloud workspace, the restored official Rust/Cargo 1.99.0
(rustc `b940084d7`, LLVM23.1.1) compiles the patch. All 1,185 all-feature App tests,
1,171 core-only App tests, 25 helper tests and 13 protocol tests pass. The directly
source-linked vendor harness passes 400 available tests and 26 doctests with
`cpal-backend,process-isolation`; its original full run records one failure because
the upstream Dexed moduleinfo fixture is absent. That failure is retained, and the
subsequent available-case run explicitly filters only that named fixture. No Rust
test source or cfg was removed; the external harness omits unused example/dev-only
dependencies and records its exact manifest, lock and source hashes.

Both strict root Clippy profiles, formatting and upstream archive+patch replay
(46 files) pass. Python reports 235 tests with one missing-helper skip, followed
by all four real copied-helper Unix descriptor cases passing with the helper path
set. The initial candidate test invocation accidentally selected zero tests; its
log is retained, and the corrected named invocation passes one test. Metadata
failure tests cover malformed JSON/timeout, helper death, wrong responses, an
actual valid-latency/failed-tail backend pair, saturated fault-event delivery and
old-model/endpoint retention. No missing reply becomes a healthy zero.

This is source/mock/helper-protocol validation, not a new genuine-plugin, native
editor, Windows runtime or performance acceptance. The stable accepted release is
unchanged. Timing Retry remains a timing-plan operation and does not revive a dead
helper; there is no automatic state reload. The initial UNVALIDATED archive entries
above are historical checkpoints, not the current test result.


## 2026-10-10 — Checked bus-layout admission, checkpoint 02 (UNVALIDATED)

The production VST3 backend now propagates `AudioBusLayout` query failure instead
of guessing stereo channels. A valid reply retains the existing sum of active
bus channels, including empty, mono and multibus layouts. This change occurs
after helper LoadPlugin initialization, but before backend construction, pending
state restoration, DAW prepare/reconfigure/start/reset or endpoint promotion.
The existing disabled-auto-recovery policy and wire format are unchanged.

New tests exercise Error/wrong-response/helper-death refusal, valid channel counts,
no candidate readiness on layout failure, and the existing App old-model/endpoint
retention path for load failure. At this source archival boundary these added
tests have not been compiled or run. The preceding metadata checkpoint remains
separately committed and source-tested at `6fdfb8f`.

The first checkpoint02 compile found a missing qualified test-only
`PluginPrepareConfig` path in the App fixture. The diagnostic is retained; the
fixture now uses the same fully qualified type as its production loader. Its App
case proves generic nonexistent-plugin loader-failure retention; the direct VST3
backend/candidate tests separately prove layout-specific refusal. These are not
an end-to-end App VST3-layout integration test. No production correction was
needed for this compile error.


### Executed checkpoint 02 validation (2026-10-10)

After the test-only path correction, all three direct VST3 layout regressions
and the expanded App retention case pass. Full current-source runs pass 1,188
all-feature App tests and 1,171 core tests; both strict root Clippy profiles and
formatting pass. All 239 Python tests pass with the copied helper configured,
including its four actual Unix descriptor checks.

The vendor, helper and protocol inputs are byte-identical to checkpoint01 across
68 recorded files; those 400 available vendor tests, 26 doctests, 25 helper tests
and 13 protocol checks are reused results, not claimed as new executions. The
build reproduces the same helper SHA256
`000195915ef88f8ab2e212b97f87f26bb41eb9a7d54a36540fdb470a61e409cd`.
The initial compile failure, absent upstream fixture, and prior checkpoint
qualifications remain in the evidence. Genuine-plugin/native/Windows runtime
acceptance and release are outside these source-only checkpoints.


## 2026-10-10 — Borrowed processor thread proof (UNVALIDATED test-only checkpoint)

The entire new capability and its constructor are `cfg(test)`. Production
`ProcessorLease`, `ProcessorOps`, `MainThreadPlugin`, helper dispatch and strict
metadata policy are unchanged. A private non-owning token exclusively borrows
the aggregate owner and its module lifetime. It exposes only an unsafe mutable
`process` call; one narrowly documented `Send` implementation moves that call
capability into a scoped worker. It is neither Clone nor Sync and grants no
controller, metadata, lifecycle, cast, AddRef or Release access. All owning COM
references and final destruction remain on the loading thread.

The mock uses worker-local f32 mono buffers and prepared ProcessData. Fixtures
cover a 350 ms simulated main-thread hold, a same-thread negative baseline,
checked readiness, SDK result errors, Rust-only worker/main panic, process-flag
cleanup and a blocked worker that must join before owner/module release. No
foreign COM unwind recovery is claimed. The mock intentionally allocates and
locks. Its output proves only thread ownership/progress, not real-time safety.

This is not a production audio fix: the raw fixture does not perform runtime
native acknowledgments, process history, event/parameter orchestration, or
output-fault handling. It cannot be selected by a user or helper. Production
integration must preserve all of those semantics under a separately reviewed
capability. Independent IPC, native polling, metadata freshness and device/PDC
publication remain unresolved by this fixture. Existing strict mode still waits
for fresh main-thread metadata. No experimental 500 ms mode is implemented.

The new full-source compile-contract runner checks the actual test-only private
types and a positive scoped use before asserting the borrow/visibility/auto-trait
failures. At this recovery boundary compilation and execution remain pending;
the exact unsafe call sites require independent review before execution.


### Executed borrowed-capability proof (2026-10-10)

Independent unsafe-boundary review preceded execution. All 8 new mock-thread
fixtures and all 52 private compiler contracts pass. The latter include positive
scoped transfer/rejoin and production-owner controls, then independent production
exclusion, mutable/unsafe-call, authority, lifetime and borrow-failure checks. The
350 ms fixture establishes completion before release of a main-thread hold lasting
at least 350 ms, with a generous watchdog; it is not a 350 ms execution deadline.
The same-thread case is a sequential-call structural control, not an execution of
the actual helper dispatcher. Detached-thread exclusion is for a nonstatic owner
loan; a deliberately leaked static owner is outside the tested teardown contract.

Fresh source gates pass 408 available vendor tests (one known absent SDK fixture
excluded), 26 doctests, 1,188 App tests, 25 helper tests, 13 protocol tests and 239
Python tests. Root strict all-feature Clippy, formatting and the vendor test
Clippy check pass; the latter retains only the already documented deprecated and
drain_collect exceptions. Package closure checks 552 inputs and the upstream
patch replays 48 files exactly. The separately archived lifecycle branch's two
additional tests are not part of this branch or its 1,188 count.

The first compiler-runner input was rejected before Cargo because it contained
two binary targets. A separate QA library-only manifest removes only those two
sections, preserving dependencies, features and the exact lock. The first ad hoc
package-input check passed paths where bytes were required; its retained failure
and corrected 552-input pass changed no source. All Rust bytes remain identical
to the durably archived, independently reviewed pretest checkpoint.

A forced current-worktree vendor compile and exact rlib link produced the copied
helper SHA256 `ca1751af6d848d35a2b0054713d44bb1614d952eb4d280740123c52155dd6b7d`.
These are source/mock/protocol results only. There is no genuine-plugin, native
GUI, Windows runtime, device-audio or performance acceptance. The production
helper has no new process capability, mode, worker or metadata policy.

## Scoped full-runtime processing preparation (2026-10-10, unvalidated checkpoint)

The production render kernel now depends on `ProcessorProcessCall`, which grants
only the SDK `process` call. `setProcessing` remains in the separate lifecycle
`ProcessorCalls` trait. The three runtime render functions use the same bodies,
queue ordering, zero-sample bus hiding, positive-call history, native staging and
per-SDK-call acknowledgment, transport advancement, error cleanup, output-loss
and sticky parameter-output-fault precedence. The existing production
`ProcessorOps` facade remains non-Send/non-Sync. Public `Plugin` flat/bus process
wrappers are unchanged: only whole-call success updates levels and invokes the
arbitrary caller-side audio callback while its existing meter mutex is held.

A separate, entirely `cfg(test)` session loans the already prepared runtime by
value while the loading thread retains the existing disjoint `ControlOps` view.
`RuntimeLoan` has one private field and one narrowly audited unsafe Send. Its
worker adapter exposes only a borrowed processor address with exclusive
processor/module lifetime; it neither owns/clones a COM reference nor enters the
process flag. The existing full kernel is the sole owner of the SDK-call gate.
The owner physically retains all buffer allocations, parameter/event COM
wrappers, controller, processor, factory and module. No loan operation can
replace or rebuild those owners. State, setup, reset, lifecycle, metadata and
destruction require the scoped worker to join and the owner borrow to end.

The concurrent fixture is an ownership/orchestration proof, not helper worker
activation. Its intended main-thread getter hold lasts at least 350 ms; a generous
watchdog checks worker completion before release, without asserting a 350 ms
processing deadline. It uses real host COM mock interfaces and the full runtime,
but does not qualify arbitrary plug-ins or native windows. Same-thread callback
metering and controller-mirroring entry points retain their original contracts;
the runtime-only parameter method remains queue admission without mirroring.

Host-issued queue lifetime is a distinct boundary. During one prepared lifetime,
container-owned COM references remain live on the main owner. A plug-in can
legally retain a queue, and after an owner-side rebuild its final later Release
may free that retired host queue/arena on the processing thread. Join-before-
rebuild prevents stale-owner access but does not move that final release back to
the main thread. The fixture will preserve this negative evidence. No universal
final-release affinity, whole-Process no-allocation/no-free, mutex removal,
metadata freshness lease, independent IPC, or audible GUI-stall fix is claimed.

This source checkpoint is unvalidated until the separate executed receipt is
appended. Concurrent invocation requires independent review of both the narrow
unsafe wrapper and its concrete COM fixture callsites before execution.

### Executed scoped full-runtime proof (2026-10-10)

The source/COM mock gate now passes. Twenty focused tests exercise the same
production processing kernel through the test-only loan, including an actual
main-thread controller getter held for at least 350 ms while the worker completes
before release. They preserve per-SDK-call native acknowledgment across split
blocks, zero-flush and positive-attempt history, checked input rollback, explicit
transport, output loss/fault precedence, caller callback/meter behavior, scoped
join on Rust panic, and module/reference destruction ordering. The retired-queue
case intentionally observes final Release returning zero inside a later mocked
SDK call after owner-side rebuild; that processing-thread free possibility is
not hidden or treated as a passed no-free guarantee.

- Available vendor suite: 428 passed; the one known absent SDK-generated
  module-info fixture remains explicitly excluded. Vendor doctests: 26 passed.
- App suite: 1,188 passed; helper: 25; protocol: 13; Python: 239, including the
  copied helper's Unix descriptor checks. These use this branch's source, before
  the separately published piano correction, which must be retained on integration.
- Compiler contracts: 55 updated domain cases passed. The new runtime corpus has
  88 unique verified outcomes across an interrupted 85-pass prefix and a six-case
  rerun (three positive controls plus the corrected/unexecuted cases). Fixture,
  production-source and lock hashes are compared in the union receipt. The
  interrupted report remains incomplete; it is not rewritten into a full pass.
- Strict root Clippy, formatting, vendor test Clippy with only the documented
  pre-existing deprecated/drain-collect exceptions, 552 package inputs and exact
  50-file official-crate patch replay passed.

The first compile lacked the test's SDK ownership-trait import and stopped before
execution. The first compiler-runner positive control failed on generated nested
module paths; none of its negatives counted. A later detached-thread negative
was correctly rejected with an unnumbered static-lifetime error rather than the
runner's anticipated E0521; only that exact diagnostic expectation was corrected.
All failed/interrupted records and the earlier unexecuted review corrections are
retained. No production Rust changed during the runner corrections.

The helper is bound to forced current-worktree vendor compilation, exact linked
rlib/dep-info and a copied executable SHA-256:
`d0c37d80e03f032004861081a4dd6d8c9a519775a586d9faf43ecaebfb3a42bb`.
Full structured evidence is delivered with the separate tested source archive.
No genuine plug-in, native GUI, Windows runtime, device, timing or audible
isolation acceptance is claimed. No production worker, asynchronous mirror,
independent IPC/native polling or metadata-policy change is activated. Existing
strict metadata still blocks publication when its UI-thread queries are blocked.
The next integration must preserve the accepted piano fix and obtain its own
combined-source validation.

## Disabled-feature scoped worker Step1 (unvalidated source)

`experimental-scoped-worker` is absent from the root and vendor default feature
sets. It exposes a Linux-only library implementation, not a helper command or App
mode. Other backends/platforms reject the entry. Strict metadata queries, native
polling, existing IPC, reset policy and all public processing callbacks retain
their existing behavior. The accepted piano changes are present in this branch.

`MainThreadPlugin::with_scoped_worker` checks loading-thread affinity, prepared
processing state and that both user callback options are empty. The loading
thread keeps the owner, module, owning COM references and prepared storage. Its
callback borrows a main-only attached-UI bridge and an exclusive, Send message
port; neither exposes a raw channel, processor, aggregate owner or thread handle.
The library's private worker alone invokes the same full checked kernel. Its
narrow Send loan cannot rebuild, replace or destroy buffers/COM owners, or call
controller, metadata, lifecycle, reset or state methods. The kernel still enters
exactly one SDK process gate. Retired host queues can still have their final
Release/free during a later Process after main-side rebuild; no whole-Process
no-allocation/no-free claim is made.

One command may be outstanding at a time. Capacity-one channels bound command
count, not the number of bytes in caller-owned audio buffers. Commands are flat
or bus processing, ordinary NoteOn/NoteOff/PolyAftertouch admission, and explicit
output-event drain. Ordinary scalar validation and transactional bookkeeping are
shared with the existing owner path. Mapped CC, pitch bend, channel aftertouch,
program selection and mirrored parameter setters remain owner operations. A
failed process reply's returned buffers are partial diagnostic data, never a
successful audio completion. Event drain remains explicit, as in the public API.

Meter updates use the existing mutex once after whole-call success, including
zero-frame success, and skip poison/failure exactly as before. Arbitrary audio
or parameter callbacks are rejected at entry instead of being moved to a worker
or replayed later. Existing public wrappers keep their callback affinity.

The attached bridge services existing Linux run-loop work and an already
attached view's size/resize/scale operations. It never creates a view as a query
fallback; scale QI/use/Release stays on main. Native container focus/query remains
in binary window code. Open/Close/native-close, SaveState, LoadState, setup, reset,
all metadata (including future EventBusLayout/CheckedAudioBusLayout), and destruction require returning
from the session before the existing owner handler runs. Fresh-helper eligibility
belongs to future negotiation; positive-call history is preserved across re-entry.

On callback return, error or Rust unwind, the library closes its unique sender,
drains accepted work and joins before rejoining the owner. No stop packet competes
with a full queue. Worker replies are nonblocking; failed publication is an
explicit session fault. Kernel errors remain visible even after reply consumption;
unread successful completion cannot be reported as delivered. Callback errors
retain their original identity, and original Rust panic payloads resume only after
join. Joined unread-reply diagnostics use one main-thread box after the body-panic
path has been ruled out; ordinary successfully consumed replies are not boxed.
A worker Rust panic marks terminal status before exit; if a preceding kernel
error was already observed, the exact shared error remains bounded secondary
evidence after join, including when a callback error is primary. Foreign COM unwind
is unsupported. A hung foreign call pins the scope/module; no timeout detach or
unload is permitted. A caller-created nested broker must itself use an interruptible
exit protocol; the outer library cannot interrupt arbitrary caller blocking code.

The new concrete thread/COM fixtures and compile contracts must receive independent
unsafe-callsite review and durable source archival before execution. This checkpoint
has no native/genuine-plugin, timing, device, audible-isolation or release result.
