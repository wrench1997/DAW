//! Scoped, same-thread capabilities. The aggregate remains exclusively borrowed and owns
//! every COM reference, module and runtime field throughout the visit. No handback state,
//! movable owner, user callback or worker authority is introduced.

use super::*;
use std::{marker::PhantomData, rc::Rc};

pub(crate) type DomainSessionCallback<'callback> = dyn for<'owner> FnMut(&mut ControlOps<'owner>, &mut ProcessorOps<'owner>) -> Result<()>
    + 'callback;

/// Only controller/editor/UI fields. No component, processor, module, factory or runtime.
pub(crate) struct ControlOps<'owner> {
    controller: &'owner Option<ComPtr<IEditController>>,
    view: &'owner Option<ComPtr<IPlugView>>,
    resize: &'owner Arc<Mutex<Option<(i32, i32)>>>,
    deferred_controller_sync: &'owner ArrayQueue<(u32, f64)>,
    control_thread: ThreadId,
    #[cfg(target_os = "linux")]
    host_ui: &'owner HostApplication,
    #[cfg(target_os = "linux")]
    run_loop: &'owner Arc<Mutex<RunLoopRegistry>>,
    _same_thread: PhantomData<Rc<()>>,
}

/// Only exclusive runtime state, a non-owning process capability, and an active-call marker.
/// The copied active flag cannot become stale: no facade can activate/deactivate the owner.
pub(crate) struct ProcessorOps<'owner> {
    runtime: &'owner mut ProcessorRuntime,
    processor: ProcessorLease<'owner>,
    gate: DataExchangeProcessGate<'owner>,
    is_active: bool,
    _same_thread: PhantomData<Rc<()>>,
}

pub(super) fn visit(
    owner: &mut PluginImpl,
    callback: &mut DomainSessionCallback<'_>,
) -> Result<()> {
    let ControlDomain {
        processor,
        controller,
        plugin_view,
        editor_resize,
        deferred_controller_sync,
        control_thread,
        _host_app,
        #[cfg(target_os = "linux")]
        run_loop,
        _module,
        is_active,
        ..
    } = &mut owner.control;
    let mut control = ControlOps {
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
    let mut processing = ProcessorOps {
        runtime: &mut owner.runtime,
        processor: ProcessorLease::new(processor, _module.as_ref()),
        gate: _host_app.data_exchange_process_gate(),
        is_active: *is_active,
        _same_thread: PhantomData,
    };
    callback(&mut control, &mut processing)
}

// The established legacy processor call remains legal on its caller thread. Only an explicit
// two-domain session requires the loading thread; otherwise existing playback would change.
pub(super) fn process_buffer_view(
    owner: &mut PluginImpl,
    buffers: &mut CallerAudioBuffers<'_>,
) -> Result<()> {
    let mut processing = ProcessorOps {
        runtime: &mut owner.runtime,
        processor: ProcessorLease::new(
            &mut owner.control.processor,
            owner.control._module.as_ref(),
        ),
        gate: owner.control._host_app.data_exchange_process_gate(),
        is_active: owner.control.is_active,
        _same_thread: PhantomData,
    };
    processing.process_buffer_view(buffers)
}

// This private seam is intentionally not yet used by helper dispatch. Its methods are tested
// against real COM mocks; exposing a future dispatcher is a separate reviewed checkpoint.
#[allow(dead_code)]
impl ControlOps<'_> {
    fn drain_controller_sync(&self) {
        debug_assert_eq!(thread::current().id(), self.control_thread);
        while let Some((id, value)) = self.deferred_controller_sync.pop() {
            let Some(controller) = self.controller.as_ref() else {
                continue;
            };
            let result = unsafe { controller.setParamNormalized(id, value) };
            if result != kResultOk && result != kResultTrue {
                log::debug!(
                    "setParamNormalized({id}) returned {result:#x}; applying anyway \
                    (many plugins report kResultFalse on success)"
                );
            }
        }
    }

    pub(crate) fn get_parameter(&self, id: u32) -> Result<f64> {
        self.drain_controller_sync();
        let controller = self
            .controller
            .as_ref()
            .ok_or_else(|| Error::InterfaceError("No controller available".to_string()))?;
        unsafe { Ok(controller.getParamNormalized(id)) }
    }

    pub(crate) fn format_parameter(&self, id: u32, normalized: f64) -> Result<String> {
        self.drain_controller_sync();
        let controller = self
            .controller
            .as_ref()
            .ok_or_else(|| Error::InterfaceError("No controller available".to_string()))?;
        unsafe {
            let mut text: String128 = std::mem::zeroed();
            if controller.getParamStringByValue(id, normalized, &mut text) == kResultOk {
                return Ok(crate::internal::utils::vst_string_to_string(&text));
            }
        }
        Err(Error::InvalidParameter(format!(
            "Plugin could not format parameter {id}"
        )))
    }

    /// Resize an already attached editor. Attachment/detachment still rejoins the owner.
    pub(crate) fn resize_editor(&mut self, width: i32, height: i32) -> Result<(i32, i32)> {
        resize_view(self.view, width, height)
    }

    pub(crate) fn take_editor_resize_request(&self) -> Option<(i32, i32)> {
        self.resize
            .lock()
            .ok()
            .and_then(|mut request| request.take())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn service_run_loop(&mut self) {
        self.host_ui.service_run_loop();
        super::super::com_implementations::service_linux_run_loop(self.run_loop);
    }
}

#[allow(dead_code)]
impl ProcessorOps<'_> {
    fn process_buffer_view(&mut self, buffers: &mut CallerAudioBuffers<'_>) -> Result<()> {
        self.runtime.process_buffer_view(
            &mut self.processor,
            &mut self.gate,
            self.is_active,
            buffers,
        )
    }

    pub(crate) fn process(&mut self, buffers: &mut AudioBuffers) -> Result<()> {
        self.process_buffer_view(&mut CallerAudioBuffers::Flat(buffers))
    }

    pub(crate) fn process_buses(&mut self, buffers: &mut BusAudioBuffers) -> Result<()> {
        self.runtime.validate_bus_buffers(buffers)?;
        self.process_buffer_view(&mut CallerAudioBuffers::Buses(buffers))
    }

    /// DSP queue admission only. Does not mirror, query or acknowledge the edit controller.
    /// Mixed legacy setters keep their existing capacity-preflight → mirror → queue sequence.
    pub(crate) fn queue_parameter_at(&mut self, id: u32, value: f64, offset: i32) -> Result<()> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameter(format!(
                "Value {value} is out of range [0.0, 1.0]"
            )));
        }
        self.runtime.preflight_parameter_admission()?;
        self.runtime.pending_param_changes.push(ParameterChange {
            id,
            value,
            sample_offset: offset,
        });
        Ok(())
    }

    pub(crate) fn send_plugin_event(&mut self, event: PluginEvent) -> Result<()> {
        crate::plugin::validate_plugin_event(&event)?;
        self.runtime.send_plugin_event(event)
    }

    pub(crate) fn note_on(
        &mut self,
        channel: MidiChannel,
        note: u8,
        velocity: u8,
        offset: i32,
    ) -> Result<crate::midi::NoteId> {
        crate::plugin::validate_note(note)?;
        crate::plugin::validate_velocity(velocity)?;
        self.runtime.note_on(channel, note, velocity, offset)
    }

    pub(crate) fn note_off(&mut self, id: crate::midi::NoteId, offset: i32) -> Result<()> {
        self.runtime.note_off(id, offset)
    }

    pub(crate) fn send_note_expression(
        &mut self,
        id: crate::midi::NoteId,
        kind: crate::midi::NoteExpressionType,
        value: f64,
        offset: i32,
    ) -> Result<()> {
        if !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameter(format!(
                "note-expression value {value} out of range [0.0, 1.0]"
            )));
        }
        self.runtime.send_note_expression(id, kind, value, offset)
    }

    pub(crate) fn set_process_transport(
        &mut self,
        transport: crate::plugin::ProcessTransport,
    ) -> Result<()> {
        self.runtime.set_process_transport(transport)
    }

    pub(crate) fn take_output_events_with_loss(&self) -> (Vec<PluginEvent>, bool) {
        self.runtime.take_output_events_with_loss()
    }
}

pub(super) fn resize_view(
    plugin_view: &Option<ComPtr<IPlugView>>,
    width: i32,
    height: i32,
) -> Result<(i32, i32)> {
    if width <= 0 || height <= 0 {
        return Err(Error::Other(
            "editor dimensions must be greater than zero".to_string(),
        ));
    }
    let view = plugin_view
        .as_ref()
        .ok_or_else(|| Error::Other("Plugin editor is not open".to_string()))?;

    unsafe {
        if view.canResize() != kResultTrue {
            let mut current = ViewRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if view.getSize(&mut current) != kResultOk {
                return Err(Error::Other(
                    "Plugin editor is fixed-size and its size could not be queried".to_string(),
                ));
            }
            return view_rect_size(&current);
        }

        let mut requested = ViewRect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        // The view constrains `requested` in place. `kResultFalse` means it left the rect
        // alone (nothing to constrain, or it wants a different size than asked for) — a
        // refusal to adapt, not a broken call — so take whatever rect it ended up with and
        // only reject result codes that mean the call itself failed.
        let constraint_result = view.checkSizeConstraint(&mut requested);
        let constrained = constraint_result == kResultOk
            || constraint_result == kResultTrue
            || constraint_result == kResultFalse
            || constraint_result == kNotImplemented;
        if !constrained {
            return Err(Error::Other(format!(
                "Plugin failed to check the editor size constraint: {constraint_result:#x}"
            )));
        }
        let accepted = view_rect_size(&requested)?;

        // The SDK calls onSize only when the size actually changes; re-sending the current
        // one makes VSTGUI-based editors rebuild their frame for nothing.
        let mut current = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if view.getSize(&mut current) == kResultOk
            && view_rect_size(&current).ok() == Some(accepted)
        {
            return Ok(accepted);
        }

        let resize_result = view.onSize(&mut requested);
        if resize_result != kResultOk && resize_result != kResultTrue {
            return Err(Error::Other(format!(
                "Plugin rejected editor resize: {resize_result:#x}"
            )));
        }
        Ok(accepted)
    }
}
