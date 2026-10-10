//! Disabled-feature library session. Owning COM references and prepared storage remain
//! in the loading-thread owner. Only the fixed processing dispatcher crosses threads.

use super::*;
use crate::scoped_worker::{
    AttachedUiBridge, AttachedUiReply, AttachedUiRequest, ScopedWorkerCallback, ScopedWorkerError,
    ScopedWorkerFailure, ScopedWorkerReport, ScopedWorkerResult, WorkerCommand, WorkerIo,
    WorkerOutcome, WorkerReply, WorkerStatus,
};
use std::{
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    ptr::NonNull,
    sync::mpsc,
};
use vst3::ComRef;

struct ProcessAddress<'owner> {
    pointer: NonNull<IAudioProcessor>,
    _owners: PhantomData<(&'owner mut ComPtr<IAudioProcessor>, &'owner dyn VstModule)>,
}

impl<'owner> ProcessAddress<'owner> {
    fn new(processor: &'owner mut ComPtr<IAudioProcessor>, _module: &'owner dyn VstModule) -> Self {
        Self {
            pointer: NonNull::new(processor.as_ptr()).expect("owning processor is non-null"),
            _owners: PhantomData,
        }
    }
}

impl ProcessorProcessCall for ProcessAddress<'_> {
    unsafe fn process(&mut self, data: &mut ProcessData) -> tresult {
        // SAFETY: the sole caller is the unchanged full kernel, which prepares and keeps
        // every nested buffer/queue pointer live and exclusive for this synchronous call.
        // The owner/module loan pins this vtable. ComRef never AddRefs or Releases it.
        unsafe {
            ComRef::from_raw(self.pointer.as_ptr())
                .expect("borrowed processor")
                .process(data)
        }
    }
}

struct RuntimeLoan<'owner> {
    core: PreparedRuntimeOps<'owner, ProcessAddress<'owner>>,
    levels: &'owner Mutex<crate::audio::AudioLevels>,
}

// SAFETY: this private loan is constructed exactly once from disjoint fields of the
// exclusively borrowed loading-thread owner. All owning processor/controller/factory/module
// references and current prepared COM queue owners stay in that owner until scoped join.
// The fixed dispatcher can only call the existing checked processing kernel, validated
// ordinary MIDI admission, and owned output drain. It cannot rebuild/drop/replace runtime
// storage, invoke controller/state/metadata/lifecycle, or expose a COM address. The borrowed
// processor grants only SDK process, a processing-thread method; the kernel owns the ONE
// existing data-exchange gate. RuntimeLoan is not Sync/Clone and dispatch requires &mut.
// Main simultaneously receives only disjoint ControlOps and its view scale scalar. Shared
// native/COM queues retain their existing synchronization. Meter access uses its existing
// Mutex; user callbacks are rejected before entry and never run here. A hung foreign call
// pins the scoped join, never permits detached unload. A plugin-retained retired host queue
// may still receive final Release/free in a later Process after owner-side rebuild: this is
// NOT a whole-Process no-allocation/no-free or universal host-container drop-affinity claim.
unsafe impl Send for RuntimeLoan<'_> {}

impl RuntimeLoan<'_> {
    fn dispatch(&mut self, command: WorkerCommand) -> (WorkerReply, Option<Arc<Error>>) {
        match command {
            WorkerCommand::Process {
                mut buffers,
                transport,
            } => {
                let result = transport
                    .map(|t| self.core.set_process_transport(t))
                    .transpose()
                    .and_then(|_| self.core.process(&mut buffers))
                    .map_err(Arc::new);
                if result.is_ok() {
                    if let Ok(mut levels) = self.levels.lock() {
                        levels.update_from_buffers(&buffers.outputs);
                    }
                }
                let failure = result.as_ref().err().cloned();
                (WorkerReply::Processed { buffers, result }, failure)
            }
            WorkerCommand::ProcessBuses {
                mut buffers,
                transport,
            } => {
                let result = transport
                    .map(|t| self.core.set_process_transport(t))
                    .transpose()
                    .and_then(|_| self.core.process_buses(&mut buffers))
                    .map_err(Arc::new);
                if result.is_ok() {
                    if let Ok(mut levels) = self.levels.lock() {
                        levels.update_from_bus_buffers(&buffers.outputs);
                    }
                }
                let failure = result.as_ref().err().cloned();
                (WorkerReply::ProcessedBuses { buffers, result }, failure)
            }
            WorkerCommand::SendMidi {
                event,
                sample_offset,
            } => {
                let event = event.into_midi_event();
                let result = crate::plugin::validate_midi_event(&event)
                    .and_then(|_| {
                        self.core
                            .runtime
                            .send_ordinary_midi_event_at(event, sample_offset)
                    })
                    .map_err(Arc::new);
                let failure = result.as_ref().err().cloned();
                (WorkerReply::MidiAdmission { result }, failure)
            }
            WorkerCommand::TakeOutputEvents => {
                let (events, lost) = self.core.take_output_events_with_loss();
                (WorkerReply::OutputEvents { events, lost }, None)
            }
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::internal::plugin_impl) enum WorkerTestPoint {
    BeforeDispatch,
    AfterDispatchBeforeReply,
    AfterReply,
}

#[derive(Default)]
struct DriverHooks {
    #[cfg(test)]
    hook: Option<Box<dyn FnMut(WorkerTestPoint) + Send>>,
    #[cfg(test)]
    fail_spawn: bool,
}

impl DriverHooks {
    fn before_dispatch(&mut self) {
        #[cfg(test)]
        if let Some(hook) = self.hook.as_mut() {
            hook(WorkerTestPoint::BeforeDispatch);
        }
    }
    fn after_dispatch(&mut self) {
        #[cfg(test)]
        if let Some(hook) = self.hook.as_mut() {
            // Deliberate test panic is Rust-only, after SDK/gate return, never in COM.
            hook(WorkerTestPoint::AfterDispatchBeforeReply);
        }
    }
    fn after_reply(&mut self) {
        #[cfg(test)]
        if let Some(hook) = self.hook.as_mut() {
            hook(WorkerTestPoint::AfterReply);
        }
    }
}

struct PanicStatusGuard(Arc<WorkerStatus>);
impl Drop for PanicStatusGuard {
    fn drop(&mut self) {
        if thread::panicking() {
            // Observable before join, even if the caller only polls session status.
            self.0.mark_terminal();
        }
    }
}

struct WorkerExit {
    completed: usize,
    failure: Option<WorkerOutcome>,
}

fn worker_loop(
    mut runtime: RuntimeLoan<'_>,
    commands: mpsc::Receiver<WorkerCommand>,
    replies: mpsc::SyncSender<WorkerReply>,
    status: Arc<WorkerStatus>,
    mut hooks: DriverHooks,
    first_error: Arc<Mutex<Option<Arc<Error>>>>,
) -> WorkerExit {
    let mut exit = WorkerExit {
        completed: 0,
        failure: None,
    };
    // Closing the unique sender wakes recv. An already accepted command is still drained;
    // a CLOSED flag is not permission to silently discard accepted work.
    while let Ok(command) = commands.recv() {
        hooks.before_dispatch();
        let (reply, failure) = runtime.dispatch(command);
        exit.completed = exit.completed.saturating_add(1);
        if let Some(error) = failure {
            // Only failures take this latch. Release it before hooks, publication or join.
            // Retain the exact Arc even if a later Rust panic prevents returning WorkerExit.
            *first_error
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::clone(&error));
            status.mark_terminal();
            exit.failure = Some(WorkerOutcome::Failed(error));
        }
        hooks.after_dispatch();
        if replies.try_send(reply).is_err() {
            status.mark_terminal();
            if exit.failure.is_none() {
                exit.failure = Some(WorkerOutcome::ReplyUndelivered);
            }
            break;
        }
        hooks.after_reply();
        if exit.failure.is_some() {
            break;
        }
    }
    exit
}

fn ui_request(
    control: &mut ControlOps<'_>,
    scale: &mut f32,
    request: AttachedUiRequest,
) -> Result<AttachedUiReply> {
    if thread::current().id() != control.control_thread {
        return Err(Error::Other(
            "attached UI request must run on loading thread".into(),
        ));
    }
    // Run-loop registrations can predate/outlive a view. This is existing UI host delivery,
    // not ServiceHostRequests, processor metadata, or a component lifecycle capability.
    if request == AttachedUiRequest::ServiceRunLoop {
        control.service_run_loop();
        return Ok(AttachedUiReply::RunLoopServiced);
    }
    let view = control
        .view
        .as_ref()
        .ok_or_else(|| Error::Other("Plugin editor is not attached".into()))?;
    match request {
        AttachedUiRequest::AttachedSize => {
            let mut rect = ViewRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { view.getSize(&mut rect) } != kResultOk {
                return Err(Error::Other("attached editor size query failed".into()));
            }
            Ok(AttachedUiReply::AttachedSize(view_rect_size(&rect)?))
        }
        AttachedUiRequest::CanResize => Ok(AttachedUiReply::CanResize(
            unsafe { view.canResize() } == kResultTrue,
        )),
        AttachedUiRequest::Resize { width, height } => control
            .resize_editor(width, height)
            .map(AttachedUiReply::Resized),
        AttachedUiRequest::SetScale { factor } => {
            // Temporary QI owning reference is created, used and released on main only.
            let accepted = unsafe { set_view_scale_factor(view, factor)? };
            *scale = factor;
            Ok(AttachedUiReply::ScaleAccepted(accepted))
        }
        AttachedUiRequest::TakeResizeRequest => Ok(AttachedUiReply::ResizeRequest(
            control.take_editor_resize_request(),
        )),
        AttachedUiRequest::ServiceRunLoop => unreachable!("handled before attached-view lookup"),
    }
}

fn rejected(error: Error) -> ScopedWorkerError {
    ScopedWorkerError {
        primary: ScopedWorkerFailure::Rejected(error),
        secondary_worker: None,
        unconsumed_reply: None,
    }
}

pub(in crate::internal::plugin_impl) fn run(
    owner: &mut PluginImpl,
    levels: &Mutex<crate::audio::AudioLevels>,
    body: &mut ScopedWorkerCallback<'_>,
) -> ScopedWorkerResult {
    run_inner(owner, levels, body, DriverHooks::default())
}

/// Test hooks are absent from production and execute only in Rust outside the SDK call.
#[cfg(test)]
pub(in crate::internal::plugin_impl) fn run_for_test(
    owner: &mut PluginImpl,
    levels: &Mutex<crate::audio::AudioLevels>,
    body: &mut ScopedWorkerCallback<'_>,
    hook: Option<Box<dyn FnMut(WorkerTestPoint) + Send>>,
    fail_spawn: bool,
) -> ScopedWorkerResult {
    run_inner(owner, levels, body, DriverHooks { hook, fail_spawn })
}

fn run_inner(
    owner: &mut PluginImpl,
    levels: &Mutex<crate::audio::AudioLevels>,
    body: &mut ScopedWorkerCallback<'_>,
    hooks: DriverHooks,
) -> ScopedWorkerResult {
    owner
        .ensure_control_thread("scoped worker")
        .map_err(rejected)?;
    if !owner.control.is_active
        || !owner.runtime.is_processing
        || owner.control.applied_setup.is_none()
        || owner.runtime.process_data.is_none()
        || owner.runtime.observe_parameter_output_fault()
        || owner
            .control
            ._host_app
            .data_exchange_process_gate()
            .is_entered()
    {
        return Err(rejected(Error::Other(
            "scoped worker owner is not prepared/active".into(),
        )));
    }
    // Positive processing history is intentionally preserved across re-entry. Fresh-helper
    // policy belongs to later negotiation, not an artificial has_processed_audio reset.
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
        run_loop,
        editor_scale_factor,
        ..
    } = &mut owner.control;
    let mut control = ControlOps {
        controller,
        view: plugin_view,
        resize: editor_resize,
        deferred_controller_sync,
        control_thread: *control_thread,
        host_ui: _host_app,
        run_loop,
        _same_thread: PhantomData,
    };
    let runtime = RuntimeLoan {
        core: PreparedRuntimeOps {
            runtime: &mut owner.runtime,
            processor: ProcessAddress::new(processor, _module.as_ref()),
            gate: _host_app.data_exchange_process_gate(),
            is_active: *is_active,
            _same_thread: PhantomData,
        },
        levels,
    };
    let status = Arc::new(WorkerStatus::new());
    let first_error = Arc::new(Mutex::new(None));
    thread::scope(|scope| {
        // The UI dispatch anchor outlives the branded port's Drop. Neither its
        // control borrow nor the port can escape the session callback.
        let mut dispatch = |request| ui_request(&mut control, editor_scale_factor, request);
        // Declared INSIDE scope: Drop closes the unique sender before scope's implicit join
        // on every unwind path. No owned sender or join handle reaches caller code.
        let (mut io, commands, replies) = WorkerIo::channel(Arc::clone(&status));
        let worker_status = Arc::clone(&status);
        let worker_error = Arc::clone(&first_error);
        #[cfg(test)]
        if hooks.fail_spawn {
            return Err(ScopedWorkerError {
                primary: ScopedWorkerFailure::Spawn(Error::IoError(std::io::Error::other(
                    "injected spawn refusal",
                ))),
                secondary_worker: None,
                unconsumed_reply: None,
            });
        }
        let worker = thread::Builder::new()
            .name("vst3-scoped-process".into())
            .spawn_scoped(scope, move || {
                let _panic_status = PanicStatusGuard(Arc::clone(&worker_status));
                worker_loop(
                    runtime,
                    commands,
                    replies,
                    worker_status,
                    hooks,
                    worker_error,
                )
            })
            .map_err(|error| ScopedWorkerError {
                primary: ScopedWorkerFailure::Spawn(Error::IoError(error)),
                secondary_worker: None,
                unconsumed_reply: None,
            })?;
        let mut ui = AttachedUiBridge::new(&mut dispatch, Arc::clone(&status));
        let body_result = catch_unwind(AssertUnwindSafe(|| body(&mut ui, &mut io)));
        io.close();
        let joined = worker.join();
        let pending = io.has_pending();
        let unread = io.take_unconsumed_reply();
        let exit = match joined {
            Ok(exit) => exit,
            Err(_) => WorkerExit {
                completed: 0,
                failure: Some(WorkerOutcome::Panicked {
                    prior_error: first_error
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .clone(),
                }),
            },
        };
        let body_result = match body_result {
            Ok(result) => result,
            Err(payload) => resume_unwind(payload), // exact Rust payload, AFTER join
        };
        if let Err(error) = body_result {
            return Err(ScopedWorkerError {
                primary: ScopedWorkerFailure::Body(error),
                secondary_worker: exit.failure,
                unconsumed_reply: unread,
            });
        }
        if let Some(outcome) = exit.failure {
            let primary = match outcome {
                WorkerOutcome::Failed(ref error) => ScopedWorkerFailure::Worker(Arc::clone(error)),
                WorkerOutcome::Panicked { .. } => ScopedWorkerFailure::WorkerPanicked,
                WorkerOutcome::ReplyUndelivered => ScopedWorkerFailure::BridgeFailed,
            };
            return Err(ScopedWorkerError {
                primary,
                secondary_worker: Some(outcome),
                unconsumed_reply: unread,
            });
        }
        if status.is_terminal() {
            return Err(ScopedWorkerError {
                primary: ScopedWorkerFailure::BridgeFailed,
                secondary_worker: None,
                unconsumed_reply: unread,
            });
        }
        if pending {
            return Err(ScopedWorkerError {
                primary: ScopedWorkerFailure::UnconsumedReply,
                secondary_worker: None,
                unconsumed_reply: unread,
            });
        }
        Ok(ScopedWorkerReport {
            completed_commands: exit.completed,
        })
    })
}
