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
behavior and host data-exchange callbacks. ParameterChanges/EventList storage and public plugin
metering still require bounded, allocation/lock-safe replacements before enabling a processor
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
native values does not invent native delivery loss. Existing COM parameter/event storage can still
allocate and lock, and data-exchange/metering remain separate future work.

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
