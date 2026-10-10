//! TEST-ONLY concurrent use of the complete existing processing kernel.
//!
//! Unlike the earlier raw SDK token, this loan preserves checked input admission,
//! native acknowledgments, positive-call history, transport, cleanup and output faults.
//! Every owning COM reference and prepared allocation remains physically in PluginImpl.
//! Neither this loan nor ControlOps can rebuild, replace, activate, save or destroy it.
//! The caller uses a scoped join before returning to the aggregate owner. No helper or
//! production worker entry is introduced; production ProcessorOps remains !Send/!Sync.

use super::*;
use std::ptr::NonNull;
use vst3::ComRef;

/// Exclusive prepared runtime access. Its only field is private; there is no Deref or
/// extraction route. Moving/dropping the loan itself releases no COM reference.
pub(crate) struct RuntimeLoan<'owner> {
    inner: PreparedRuntimeOps<'owner, WorkerProcessCall<'owner>>,
}

// SAFETY (cfg(test) only): the private constructor loans disjoint fields while the loading
// thread retains PluginImpl, its module and every owning COM wrapper. Only the exclusive
// runtime borrow crosses threads; its callable methods cannot replace/drop ProcessData or
// owning event/parameter wrappers, rebuild buffers, or call controller/lifecycle/metadata.
// Their shared implementation is the existing prepared process kernel. Host queue access
// uses its existing synchronization; no mutex/COM auto-trait is relaxed. The borrowed SDK
// address exposes only IAudioProcessor::process, a documented processing-thread method:
// https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IAudioProcessor.html
// PreparedRuntimeOps retains its !Sync marker and each operation needs exclusive access (except
// the sole-consumer owned-event drain); no second loan or aggregate access can overlap.
// The process gate reaches only an AtomicBool and is entered exactly once, by the kernel.
// The enclosing scoped lifetime pins both runtime storage and main-owned processor/module;
// a hung worker prevents scope completion, never permits detached unload. This is NOT a
// blanket Send implementation on ProcessorRuntime, ProcessorLease, a COM owner, or ControlOps.
// Current-generation host queue owners survive calls. Plugin-retained retired queues may
// still have a final Release/free on a later worker call after a main-thread rebuild; this
// is memory-safe ownership, not a whole-Process no-free or universal final-release claim.
unsafe impl Send for RuntimeLoan<'_> {}

/// A borrowed SDK address only. It does not enter the gate: process_chunk owns that scope.
struct WorkerProcessCall<'owner> {
    processor: NonNull<IAudioProcessor>,
    _processor_and_module:
        PhantomData<(&'owner mut ComPtr<IAudioProcessor>, &'owner dyn VstModule)>,
}

impl<'owner> WorkerProcessCall<'owner> {
    fn new(processor: &'owner mut ComPtr<IAudioProcessor>, _module: &'owner dyn VstModule) -> Self {
        Self {
            processor: NonNull::new(processor.as_ptr()).expect("owning processor is non-null"),
            _processor_and_module: PhantomData,
        }
    }
}

impl ProcessorProcessCall for WorkerProcessCall<'_> {
    unsafe fn process(&mut self, data: &mut ProcessData) -> tresult {
        // SAFETY: the full runtime kernel supplies prepared, live, exclusive SDK buffers
        // and queues. The constructor's processor/module loan preserves this exact vtable.
        // ComRef is non-owning; no AddRef/Release occurs on construction or destruction.
        unsafe {
            ComRef::from_raw(self.processor.as_ptr())
                .expect("borrowed processor is non-null")
                .process(data)
        }
    }
}

/// The higher-ranked callback cannot return either loan. Only UI fields are available on
/// the loading thread until every scoped worker has joined and this callback returns.
pub(crate) fn with_runtime_loan_for_test<R>(
    owner: &mut PluginImpl,
    callback: impl for<'owner> FnOnce(ControlOps<'owner>, RuntimeLoan<'owner>) -> Result<R>,
) -> Result<R> {
    owner.ensure_control_thread("test-only prepared runtime loan")?;
    if !owner.control.is_active
        || !owner.runtime.is_processing
        || owner.control.applied_setup.is_none()
        || owner.runtime.process_data.is_none()
        || owner.runtime.observe_parameter_output_fault()
        || owner
            .control
            ._host_app
            .data_exchange_process_gate()
            .flag_for_scoped_worker_test()
            .load(Ordering::Acquire)
    {
        return Err(Error::Other(
            "test-only runtime owner is not prepared/active".into(),
        ));
    }
    let ControlDomain {
        processor,
        controller,
        plugin_view,
        editor_resize,
        deferred_controller_sync,
        control_thread,
        _host_app,
        _module,
        is_active,
        #[cfg(target_os = "linux")]
        run_loop,
        ..
    } = &mut owner.control;
    let control = ControlOps {
        controller,
        view: plugin_view,
        resize: editor_resize,
        deferred_controller_sync,
        control_thread: *control_thread,
        #[cfg(target_os = "linux")]
        host_ui: _host_app,
        #[cfg(target_os = "linux")]
        run_loop,
        _same_thread: PhantomData,
    };
    let processing = RuntimeLoan {
        inner: PreparedRuntimeOps {
            runtime: &mut owner.runtime,
            processor: WorkerProcessCall::new(processor, _module.as_ref()),
            gate: _host_app.data_exchange_process_gate(),
            is_active: *is_active,
            _same_thread: PhantomData,
        },
    };
    callback(control, processing)
}

impl RuntimeLoan<'_> {
    pub(crate) fn process(&mut self, buffers: &mut AudioBuffers) -> Result<()> {
        self.inner.process(buffers)
    }
    pub(crate) fn process_buses(&mut self, buffers: &mut BusAudioBuffers) -> Result<()> {
        self.inner.process_buses(buffers)
    }
    /// Queue admission only; never mirrors or confirms controller state.
    pub(crate) fn queue_parameter_at(&mut self, id: u32, value: f64, offset: i32) -> Result<()> {
        self.inner.queue_parameter_at(id, value, offset)
    }
    pub(crate) fn send_plugin_event(&mut self, event: PluginEvent) -> Result<()> {
        self.inner.send_plugin_event(event)
    }
    pub(crate) fn note_on(
        &mut self,
        channel: MidiChannel,
        note: u8,
        velocity: u8,
        offset: i32,
    ) -> Result<crate::midi::NoteId> {
        self.inner.note_on(channel, note, velocity, offset)
    }
    pub(crate) fn note_off(&mut self, id: crate::midi::NoteId, offset: i32) -> Result<()> {
        self.inner.note_off(id, offset)
    }
    pub(crate) fn send_note_expression(
        &mut self,
        id: crate::midi::NoteId,
        kind: crate::midi::NoteExpressionType,
        value: f64,
        offset: i32,
    ) -> Result<()> {
        self.inner.send_note_expression(id, kind, value, offset)
    }
    pub(crate) fn set_process_transport(
        &mut self,
        transport: crate::plugin::ProcessTransport,
    ) -> Result<()> {
        self.inner.set_process_transport(transport)
    }
    pub(crate) fn take_output_events_with_loss(&self) -> (Vec<PluginEvent>, bool) {
        self.inner.take_output_events_with_loss()
    }
}
