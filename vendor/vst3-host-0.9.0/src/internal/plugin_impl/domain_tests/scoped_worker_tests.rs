//! Disabled-feature full-kernel session evidence using allocating, locking COM mocks.
//! A held attached-view call establishes overlap, not a native-plugin or audio deadline.
//! All intentional Rust panics occur outside COM vtables. No native GUI is created.

use super::*;
use crate::scoped_worker::{
    AttachedUiReply, AttachedUiRequest, CommandRejectionReason, OrdinaryMidiEvent,
    ScopedWorkerFailure, WorkerCommand, WorkerIo, WorkerOutcome, WorkerReply,
};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::mpsc,
    time::{Duration, Instant},
};
use vst3::Steinberg::{char16, int16};

const WATCHDOG: Duration = Duration::from_secs(8);

fn public_plugin(internal: PluginImpl) -> crate::plugin::Plugin {
    crate::plugin::Plugin {
        info: internal.info.clone(),
        compatibility: Vec::new(),
        is_processing: internal.runtime.is_processing,
        sample_rate: internal.runtime.sample_rate,
        block_size: internal.runtime.block_size,
        audio_levels: Arc::new(Mutex::new(crate::audio::AudioLevels::new(1))),
        parameter_change_callback: None,
        audio_callback: None,
        internal: Some(Box::new(internal)),
    }
}

fn process_command(frames: usize) -> WorkerCommand {
    WorkerCommand::Process {
        buffers: AudioBuffers::new(0, 1, frames, 48_000.0),
        transport: None,
    }
}

fn exchange(io: &mut WorkerIo<'_>, command: WorkerCommand) -> WorkerReply {
    io.try_submit(command).unwrap();
    io.recv_reply().unwrap()
}

fn assert_process_ok(reply: WorkerReply, frames: usize) {
    match reply {
        WorkerReply::Processed { buffers, result } => {
            result.unwrap();
            assert_eq!(buffers.outputs[0], vec![1.0; frames]);
        }
        _ => panic!("expected flat process reply"),
    }
}

fn note(velocity: u8) -> OrdinaryMidiEvent {
    OrdinaryMidiEvent::NoteOn {
        channel: MidiChannel::Ch1,
        note: 60,
        velocity,
    }
}

fn midi(event: OrdinaryMidiEvent) -> WorkerCommand {
    WorkerCommand::SendMidi {
        event,
        sample_offset: 0,
    }
}

fn assert_midi_ok(reply: WorkerReply) {
    match reply {
        WorkerReply::MidiAdmission { result } => result.unwrap(),
        _ => panic!("expected MIDI admission"),
    }
}

fn internal_exchange(
    owner: &mut PluginImpl,
    command: WorkerCommand,
) -> (crate::scoped_worker::ScopedWorkerResult, WorkerReply) {
    let levels = Mutex::new(crate::audio::AudioLevels::new(1));
    let mut command = Some(command);
    let mut reply = None;
    let result = owner.with_scoped_worker(&levels, &mut |_, io| {
        reply = Some(exchange(io, command.take().unwrap()));
        Ok(())
    });
    (result, reply.expect("entered prepared session"))
}

fn assert_owner_ledger(trace: &Trace, main: ThreadId, workers: &[ThreadId]) {
    for call in trace.calls() {
        if call.method == "process" {
            assert!(workers.contains(&call.thread), "{call:?}");
            assert_ne!(call.thread, main);
        } else {
            assert_eq!(call.thread, main, "owning/control/lifecycle call: {call:?}");
        }
    }
}

fn assert_teardown_before_module(trace: &Trace) {
    let calls = trace.methods();
    let module = calls
        .iter()
        .position(|call| *call == ("module", "drop"))
        .unwrap();
    for target in [
        ("component", "terminate"),
        ("component", "drop"),
        ("factory", "drop"),
    ] {
        assert_eq!(calls.iter().filter(|call| **call == target).count(), 1);
        assert!(calls.iter().position(|call| *call == target).unwrap() < module);
    }
}

#[derive(Default)]
struct ViewState {
    size: Mutex<(i32, i32)>,
    get_size: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    on_size: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    scale_result: AtomicI32,
}

struct AttachedView {
    probe: Probe,
    state: Arc<ViewState>,
}
impl Class for AttachedView {
    type Interfaces = (IPlugView, IPlugViewContentScaleSupport);
}
impl Recorded for AttachedView {
    fn probe(&self) -> &Probe {
        &self.probe
    }
}
impl Drop for AttachedView {
    fn drop(&mut self) {
        self.probe.record("drop");
    }
}
impl IPlugViewTrait for AttachedView {
    unsafe fn isPlatformTypeSupported(&self, _kind: FIDString) -> tresult {
        kResultOk
    }
    unsafe fn attached(&self, _parent: *mut c_void, _kind: FIDString) -> tresult {
        self.probe.record("attached");
        kResultOk
    }
    unsafe fn removed(&self) -> tresult {
        self.probe.record("removed");
        kResultOk
    }
    unsafe fn onWheel(&self, _distance: f32) -> tresult {
        kResultOk
    }
    unsafe fn onKeyDown(&self, _key: char16, _code: int16, _modifiers: int16) -> tresult {
        kResultOk
    }
    unsafe fn onKeyUp(&self, _key: char16, _code: int16, _modifiers: int16) -> tresult {
        kResultOk
    }
    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        self.probe.record("getSize");
        if let Ok(hook) = self.state.get_size.lock() {
            if let Some(hook) = hook.as_ref() {
                hook();
            }
        }
        let Ok(current) = self.state.size.lock() else {
            return kResultFalse;
        };
        if size.is_null() {
            return kInvalidArgument;
        }
        *size = ViewRect {
            left: 0,
            top: 0,
            right: current.0,
            bottom: current.1,
        };
        kResultOk
    }
    unsafe fn onSize(&self, rect: *mut ViewRect) -> tresult {
        self.probe.record("onSize");
        if let Ok(hook) = self.state.on_size.lock() {
            if let Some(hook) = hook.as_ref() {
                hook();
            }
        }
        if rect.is_null() {
            return kInvalidArgument;
        }
        let Ok(mut current) = self.state.size.lock() else {
            return kResultFalse;
        };
        *current = ((*rect).right - (*rect).left, (*rect).bottom - (*rect).top);
        kResultOk
    }
    unsafe fn onFocus(&self, _state: TBool) -> tresult {
        kResultOk
    }
    unsafe fn setFrame(&self, _frame: *mut IPlugFrame) -> tresult {
        self.probe.record("setFrame");
        kResultOk
    }
    unsafe fn canResize(&self) -> tresult {
        self.probe.record("canResize");
        kResultTrue
    }
    unsafe fn checkSizeConstraint(&self, _rect: *mut ViewRect) -> tresult {
        self.probe.record("checkSizeConstraint");
        kResultOk
    }
}
impl IPlugViewContentScaleSupportTrait for AttachedView {
    unsafe fn setContentScaleFactor(&self, _factor: f32) -> tresult {
        self.probe.record("setContentScaleFactor");
        self.state.scale_result.load(Ordering::Acquire)
    }
}

fn attach_mock_view(owner: &mut PluginImpl, trace: &Trace) -> Arc<ViewState> {
    let state = Arc::new(ViewState::default());
    *state.size.lock().unwrap() = (640, 480);
    state.scale_result.store(kResultOk, Ordering::Release);
    owner.control.plugin_view = Some(tracked(AttachedView {
        probe: Probe {
            name: "view",
            trace: trace.clone(),
        },
        state: state.clone(),
    }));
    state
}

#[test]
fn scoped_worker_public_session_reuses_one_worker_and_returns_original_audio_allocations() {
    let main = thread::current().id();
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut internal = plugin_fixture(&trace, &state);
    internal.start_processing().unwrap();
    let mut owner = crate::plugin::MainThreadPlugin::from_in_process(public_plugin(internal));
    let before = trace.calls().len();
    let report = owner
        .with_scoped_worker(&mut |_, io| {
            for frames in [16, 40, 0, 7] {
                let buffers = AudioBuffers::new(0, 1, frames, 48_000.0);
                let address = buffers.outputs[0].as_ptr();
                io.try_submit(WorkerCommand::Process {
                    buffers,
                    transport: None,
                })
                .unwrap();
                let denied = io.try_submit(WorkerCommand::TakeOutputEvents).unwrap_err();
                assert_eq!(denied.reason, CommandRejectionReason::Pending);
                assert!(matches!(denied.command, WorkerCommand::TakeOutputEvents));
                match io.recv_reply().unwrap() {
                    WorkerReply::Processed { buffers, result } => {
                        result.unwrap();
                        assert_eq!(buffers.outputs[0].as_ptr(), address);
                        assert_eq!(buffers.outputs[0], vec![1.0; frames]);
                    }
                    _ => panic!("wrong reply"),
                }
            }
            assert!(matches!(
                exchange(io, WorkerCommand::TakeOutputEvents),
                WorkerReply::OutputEvents { lost: false, .. }
            ));
            Ok(())
        })
        .unwrap();
    assert_eq!(report.completed_commands, 5);
    let calls = trace.calls();
    let workers: Vec<_> = calls[before..]
        .iter()
        .filter(|c| c.method == "process")
        .map(|c| c.thread)
        .collect();
    assert_eq!(workers.len(), 6);
    assert!(workers.iter().all(|id| *id == workers[0]));
    assert!(trace.methods()[before..]
        .iter()
        .all(|(_, method)| *method == "process"));
    assert!(owner.is_processing());
    drop(owner);
    assert_owner_ledger(&trace, main, &[workers[0]]);
    assert_teardown_before_module(&trace);
}

#[test]
fn scoped_worker_completes_while_actual_attached_get_size_or_resize_holds_350ms() {
    for resize in [false, true] {
        let main = thread::current().id();
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut internal = plugin_fixture(&trace, &state);
        internal.start_processing().unwrap();
        let view = attach_mock_view(&mut internal, &trace);
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let mut entered_rx = Some(entered_rx);
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let done_rx = Mutex::new(done_rx);
        let held = Arc::new(AtomicBool::new(false));
        let held_in_ui = held.clone();
        let held_in_producer = held.clone();
        let observed = Arc::new(AtomicBool::new(false));
        let observed_in_ui = observed.clone();
        let hook: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            // Never assert or panic through COM, including watchdog/latch failure.
            held_in_ui.store(true, Ordering::Release);
            let started = Instant::now();
            let sent = entered_tx.send(()).is_ok();
            thread::sleep(Duration::from_millis(350));
            let completed = done_rx
                .lock()
                .ok()
                .and_then(|rx| rx.recv_timeout(WATCHDOG).ok());
            observed_in_ui.store(
                sent && completed == Some(true) && started.elapsed() >= Duration::from_millis(350),
                Ordering::Release,
            );
            held_in_ui.store(false, Ordering::Release);
        });
        if resize {
            *view.on_size.lock().unwrap() = Some(hook);
        } else {
            *view.get_size.lock().unwrap() = Some(hook);
        }
        let mut owner = crate::plugin::MainThreadPlugin::from_in_process(public_plugin(internal));
        let mut producer_id = None;
        owner
            .with_scoped_worker(&mut |ui, io| {
                let entered_rx = entered_rx.take().unwrap();
                let done_tx = done_tx.clone();
                let held_in_producer = held_in_producer.clone();
                thread::scope(|scope| {
                    let producer = scope.spawn(move || {
                        entered_rx.recv_timeout(WATCHDOG).unwrap();
                        assert_process_ok(exchange(io, process_command(40)), 40);
                        done_tx
                            .send(held_in_producer.load(Ordering::Acquire))
                            .unwrap();
                        thread::current().id()
                    });
                    let request = if resize {
                        AttachedUiRequest::Resize {
                            width: 800,
                            height: 600,
                        }
                    } else {
                        AttachedUiRequest::AttachedSize
                    };
                    let reply = ui.request(request)?;
                    assert_eq!(
                        reply,
                        if resize {
                            AttachedUiReply::Resized((800, 600))
                        } else {
                            AttachedUiReply::AttachedSize((640, 480))
                        }
                    );
                    producer_id = Some(producer.join().unwrap());
                    Ok(())
                })
            })
            .unwrap();
        assert!(observed.load(Ordering::Acquire));
        assert!(!held.load(Ordering::Acquire));
        *view.get_size.lock().unwrap() = None;
        *view.on_size.lock().unwrap() = None;
        let workers: Vec<_> = trace
            .calls()
            .iter()
            .filter(|c| c.method == "process")
            .map(|c| c.thread)
            .collect();
        assert_eq!(workers.len(), 3);
        assert!(workers.iter().all(|id| *id == workers[0]));
        assert_ne!(workers[0], producer_id.unwrap());
        drop(owner);
        assert_owner_ledger(&trace, main, &[workers[0]]);
        assert_teardown_before_module(&trace);
    }
}

#[test]
fn scoped_worker_absent_attached_ui_never_attempts_create_view() {
    let trace = Trace::default();
    let mut internal = plugin_fixture(&trace, &Arc::new(MockState::default()));
    internal.start_processing().unwrap();
    let mut owner = crate::plugin::MainThreadPlugin::from_in_process(public_plugin(internal));
    trace.clear();
    owner
        .with_scoped_worker(&mut |ui, _| {
            for request in [
                AttachedUiRequest::AttachedSize,
                AttachedUiRequest::CanResize,
                AttachedUiRequest::Resize {
                    width: 800,
                    height: 600,
                },
                AttachedUiRequest::SetScale { factor: 2.0 },
                AttachedUiRequest::TakeResizeRequest,
            ] {
                assert!(ui.request(request).is_err());
            }
            assert_eq!(
                ui.request(AttachedUiRequest::ServiceRunLoop)?,
                AttachedUiReply::RunLoopServiced
            );
            Ok(())
        })
        .unwrap();
    assert!(trace.calls().is_empty());
}

#[test]
fn scoped_worker_scale_query_use_release_and_resize_delivery_stay_on_main() {
    let main = thread::current().id();
    let trace = Trace::default();
    let mut internal = plugin_fixture(&trace, &Arc::new(MockState::default()));
    internal.start_processing().unwrap();
    let view = attach_mock_view(&mut internal, &trace);
    *internal.control.editor_resize.lock().unwrap() = Some((700, 500));
    let mut owner = crate::plugin::MainThreadPlugin::from_in_process(public_plugin(internal));
    trace.clear();
    owner
        .with_scoped_worker(&mut |ui, _| {
            assert_eq!(
                ui.request(AttachedUiRequest::SetScale { factor: 2.0 })?,
                AttachedUiReply::ScaleAccepted(true)
            );
            view.scale_result.store(kResultFalse, Ordering::Release);
            assert_eq!(
                ui.request(AttachedUiRequest::SetScale { factor: 1.5 })?,
                AttachedUiReply::ScaleAccepted(false)
            );
            let before_invalid = trace.calls().len();
            assert!(ui
                .request(AttachedUiRequest::SetScale { factor: f32::NAN })
                .is_err());
            assert_eq!(trace.calls().len(), before_invalid);
            assert_eq!(
                ui.request(AttachedUiRequest::TakeResizeRequest)?,
                AttachedUiReply::ResizeRequest(Some((700, 500)))
            );
            assert_eq!(
                ui.request(AttachedUiRequest::TakeResizeRequest)?,
                AttachedUiReply::ResizeRequest(None)
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(
        trace.methods(),
        [
            ("view", "addRef"),
            ("view", "setContentScaleFactor"),
            ("view", "release"),
            ("view", "addRef"),
            ("view", "setContentScaleFactor"),
            ("view", "release")
        ]
    );
    trace.assert_thread(main);
    assert!(!trace
        .methods()
        .iter()
        .any(|(_, method)| *method == "createView"));
}

#[test]
fn scoped_worker_ordinary_midi_validation_preserves_existing_obligations() {
    for event in [
        OrdinaryMidiEvent::NoteOn {
            channel: MidiChannel::Ch1,
            note: 128,
            velocity: 1,
        },
        OrdinaryMidiEvent::NoteOn {
            channel: MidiChannel::Ch1,
            note: 60,
            velocity: 128,
        },
        OrdinaryMidiEvent::NoteOff {
            channel: MidiChannel::Ch1,
            note: 128,
            velocity: 0,
        },
        OrdinaryMidiEvent::NoteOff {
            channel: MidiChannel::Ch1,
            note: 60,
            velocity: 128,
        },
        OrdinaryMidiEvent::PolyAftertouch {
            channel: MidiChannel::Ch1,
            note: 128,
            pressure: 1,
        },
        OrdinaryMidiEvent::PolyAftertouch {
            channel: MidiChannel::Ch1,
            note: 60,
            pressure: 128,
        },
    ] {
        let trace = Trace::default();
        let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
        owner.start_processing().unwrap();
        owner.runtime.ordinary_note_counts[60] = 3;
        owner.runtime.active_notes.push((9, 0, 62));
        owner.runtime.next_note_id = 10;
        owner.set_process_transport(reset_transport()).unwrap();
        owner.queue_processor_parameter_at(7, 0.5, 17).unwrap();
        let handler = owner.control.component_handler.as_ref().unwrap().clone();
        assert_eq!(unsafe { handler.performEdit(8, 0.25) }, kResultOk);
        let native = handler.native_edits.snapshot();
        let before = trace.calls().len();
        let (result, reply) = internal_exchange(&mut owner, midi(event));
        assert!(matches!(
            reply,
            WorkerReply::MidiAdmission { result: Err(_) }
        ));
        assert!(matches!(
            result.unwrap_err().primary,
            ScopedWorkerFailure::Worker(_)
        ));
        assert_eq!(owner.runtime.ordinary_note_counts[60], 3);
        assert_eq!(owner.runtime.active_notes, [(9, 0, 62)]);
        assert_eq!(owner.runtime.next_note_id, 10);
        assert!(owner.runtime.input_events.is_empty());
        assert_eq!(owner.runtime.pending_param_changes.len(), 1);
        assert_eq!(owner.runtime.process_transport, Some(reset_transport()));
        assert_eq!(handler.native_edits.snapshot(), native);
        assert_eq!(trace.calls().len(), before);
    }
}

#[test]
fn scoped_worker_counter_overflow_is_retryable_after_velocity_zero_release() {
    let mut owner = plugin_fixture(&Trace::default(), &Arc::new(MockState::default()));
    owner.start_processing().unwrap();
    owner.runtime.ordinary_note_counts[60] = u16::MAX;
    let (failed, reply) = internal_exchange(&mut owner, midi(note(100)));
    assert!(
        matches!(reply, WorkerReply::MidiAdmission { result: Err(ref error) } if matches!(error.as_ref(), Error::EventInputRejected))
    );
    assert!(failed.is_err());
    assert!(owner.runtime.input_events.is_empty());
    assert_eq!(owner.runtime.ordinary_note_counts[60], u16::MAX);
    let levels = Mutex::new(crate::audio::AudioLevels::new(1));
    owner
        .with_scoped_worker(&levels, &mut |_, io| {
            assert_midi_ok(exchange(io, midi(note(0))));
            assert_midi_ok(exchange(io, midi(note(100))));
            assert_midi_ok(exchange(
                io,
                WorkerCommand::SendMidi {
                    event: OrdinaryMidiEvent::NoteOff {
                        channel: MidiChannel::Ch1,
                        note: 60,
                        velocity: 64,
                    },
                    sample_offset: -12,
                },
            ));
            assert_midi_ok(exchange(
                io,
                midi(OrdinaryMidiEvent::PolyAftertouch {
                    channel: MidiChannel::Ch1,
                    note: 60,
                    pressure: 127,
                }),
            ));
            Ok(())
        })
        .unwrap();
    assert_eq!(owner.runtime.ordinary_note_counts[60], u16::MAX - 1);
    assert_eq!(queued_note_releases(&owner), [(-1, 0, 60), (-1, 0, 60)]);
    let events = owner.runtime.input_events.events.lock().unwrap();
    assert_eq!(events.len(), 4);
    assert!(matches!(
        events[0].data,
        crate::midi::PluginEventData::NoteOff { .. }
    ));
    assert_eq!(events[2].sample_offset, 0);
    assert!(owner.runtime.active_notes.is_empty());
    assert_eq!(owner.runtime.next_note_id, 1);
}

#[test]
fn scoped_worker_full_input_queue_never_commits_failed_note_bookkeeping() {
    for event in [
        note(100),
        note(0),
        OrdinaryMidiEvent::NoteOff {
            channel: MidiChannel::Ch1,
            note: 60,
            velocity: 0,
        },
        OrdinaryMidiEvent::PolyAftertouch {
            channel: MidiChannel::Ch1,
            note: 60,
            pressure: 60,
        },
    ] {
        let mut owner = plugin_fixture(&Trace::default(), &Arc::new(MockState::default()));
        owner.start_processing().unwrap();
        owner.runtime.ordinary_note_counts[60] = 3;
        owner.runtime.active_notes.push((4, 0, 61));
        owner.runtime.next_note_id = 5;
        fill_admission_queue(&mut owner, MAX_QUEUED_EVENTS);
        let (failed, reply) = internal_exchange(&mut owner, midi(event));
        assert!(failed.is_err());
        assert!(
            matches!(reply, WorkerReply::MidiAdmission { result: Err(ref error) } if matches!(error.as_ref(), Error::EventInputRejected))
        );
        assert_eq!(owner.runtime.ordinary_note_counts[60], 3);
        assert_eq!(owner.runtime.active_notes, [(4, 0, 61)]);
        assert_eq!(owner.runtime.next_note_id, 5);
        assert_eq!(
            owner.runtime.input_events.events.lock().unwrap().len(),
            MAX_QUEUED_EVENTS
        );
    }
}

#[test]
fn scoped_worker_native_ack_tracks_sdk_result_and_sticky_parameter_fault_separately() {
    for (sdk_fails, output_fault) in [(false, false), (true, false), (false, true), (true, true)] {
        let state = Arc::new(MockState::default());
        state.no_alloc_process.store(true, Ordering::Release);
        state.fail_process.store(sdk_fails, Ordering::Release);
        state
            .output_points
            .store(if output_fault { 4097 } else { 0 }, Ordering::Release);
        let mut owner = plugin_fixture(&Trace::default(), &state);
        owner.start_processing().unwrap();
        let handler = owner.control.component_handler.as_ref().unwrap().clone();
        assert_eq!(unsafe { handler.performEdit(7, 0.5) }, kResultOk);
        let (session, reply) = internal_exchange(&mut owner, process_command(0));
        let WorkerReply::Processed { result, .. } = reply else {
            panic!("wrong reply");
        };
        assert_eq!(result.is_err(), sdk_fails || output_fault);
        if let Err(reply_error) = result {
            let session = session.unwrap_err();
            let ScopedWorkerFailure::Worker(primary) = session.primary else {
                panic!("worker must stay primary");
            };
            let Some(WorkerOutcome::Failed(secondary)) = session.secondary_worker else {
                panic!("missing independently retained worker failure");
            };
            assert!(Arc::ptr_eq(&reply_error, &primary));
            assert!(Arc::ptr_eq(&reply_error, &secondary));
            assert!(session.unconsumed_reply.is_none());
            if output_fault {
                assert!(matches!(primary.as_ref(), Error::ParameterOutputRejected));
            } else {
                assert!(
                    matches!(primary.as_ref(), Error::ProcessFailed(code) if *code == kResultFalse)
                );
            }
        } else {
            assert_eq!(session.unwrap().completed_commands, 1);
        }
        let native = handler.native_edits.snapshot();
        assert_eq!(native.applied, u64::from(!sdk_fails));
        assert_eq!(native.lost, sdk_fails);
        assert_eq!(owner.runtime.observe_parameter_output_fault(), output_fault);
        state.fail_process.store(false, Ordering::Release);
        state.output_points.store(0, Ordering::Release);
        owner.stop_processing().unwrap();
        owner.start_processing().unwrap();
        let levels = Mutex::new(crate::audio::AudioLevels::new(1));
        let reentry = owner.with_scoped_worker(&levels, &mut |_, _| Ok(()));
        assert_eq!(reentry.is_err(), output_fault);
        assert_eq!(handler.native_edits.snapshot(), native);
    }
}

#[test]
fn scoped_worker_output_event_loss_drains_without_changing_native_ack_or_session_success() {
    let state = Arc::new(MockState::default());
    state.no_alloc_process.store(true, Ordering::Release);
    state.output_event_attempts.store(4097, Ordering::Release);
    let mut internal = plugin_fixture(&Trace::default(), &state);
    internal.runtime.output_events_owned = Arc::new(ArrayQueue::new(4096));
    internal.start_processing().unwrap();
    let handler = internal.control.component_handler.as_ref().unwrap().clone();
    assert_eq!(unsafe { handler.performEdit(42, 0.25) }, kResultOk);
    let mut owner = crate::plugin::MainThreadPlugin::from_in_process(public_plugin(internal));
    let report = owner.with_scoped_worker(&mut |_, io| {
        match exchange(io, process_command(1)) {
            WorkerReply::Processed { result, .. } => result.unwrap(),
            _ => panic!("wrong process reply"),
        }
        match exchange(io, WorkerCommand::TakeOutputEvents) {
            WorkerReply::OutputEvents { events, lost } => {
                assert!(lost);
                assert_eq!(events.len(), 4096);
                for (index, event) in events.iter().enumerate() {
                    assert!(matches!(event.data, crate::midi::PluginEventData::NoteOff { note_id, .. } if note_id == index as i32));
                }
            }
            _ => panic!("wrong drain reply"),
        }
        assert!(matches!(exchange(io, WorkerCommand::TakeOutputEvents), WorkerReply::OutputEvents { events, lost: false } if events.is_empty()));
        assert!(!io.is_terminal());
        Ok(())
    }).unwrap();
    assert_eq!(report.completed_commands, 3);
    assert_eq!(state.rejected_output_events.load(Ordering::Acquire), 1);
    assert_eq!(handler.native_edits.snapshot().applied, 1);
    assert!(!handler.native_edits.snapshot().lost);
    assert!(owner.save_state().is_ok());
}

#[test]
fn scoped_worker_metering_updates_whole_successful_flat_and_bus_calls_including_zero() {
    for bus in [false, true] {
        let state = Arc::new(MockState::default());
        let mut internal = plugin_fixture(&Trace::default(), &state);
        internal.start_processing().unwrap();
        let layout = internal.audio_bus_layout().unwrap();
        let plugin = public_plugin(internal);
        let levels = plugin.audio_levels.clone();
        let mut owner = crate::plugin::MainThreadPlugin::from_in_process(plugin);
        owner
            .with_scoped_worker(&mut |_, io| {
                for frames in [40, 0] {
                    let command = if bus {
                        WorkerCommand::ProcessBuses {
                            buffers: BusAudioBuffers::new(&layout, frames, 48_000.0),
                            transport: None,
                        }
                    } else {
                        process_command(frames)
                    };
                    match exchange(io, command) {
                        WorkerReply::Processed { result, .. }
                        | WorkerReply::ProcessedBuses { result, .. } => result.unwrap(),
                        _ => panic!("wrong process reply"),
                    }
                    let levels = levels.lock().unwrap();
                    let expected = if frames == 0 { 0.0 } else { 1.0 };
                    assert_eq!(levels.channels[0].peak, expected);
                    assert_eq!(levels.channels[0].rms, expected);
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(state.processed_layouts.lock().unwrap().len(), 4);
    }
}

#[test]
fn scoped_worker_failed_or_partial_calls_return_allocations_without_updating_whole_call_meters() {
    for bus in [false, true] {
        for failure in ["sdk", "partial", "output"] {
            let state = Arc::new(MockState::default());
            let mut internal = plugin_fixture(&Trace::default(), &state);
            internal.start_processing().unwrap();
            let layout = internal.audio_bus_layout().unwrap();
            if failure == "sdk" {
                state.fail_process.store(true, Ordering::Release);
            }
            if failure == "output" {
                state.no_alloc_process.store(true, Ordering::Release);
                state.output_points.store(4097, Ordering::Release);
            }
            if failure == "partial" {
                let weak = Arc::downgrade(&state);
                *state.process_callback.lock().unwrap() = Some(Box::new(move || {
                    if let Some(state) = weak.upgrade() {
                        state.fail_process.store(true, Ordering::Release);
                    }
                }));
            }
            let plugin = public_plugin(internal);
            plugin.audio_levels.lock().unwrap().channels[0].peak = 0.375;
            let mut owner = crate::plugin::MainThreadPlugin::from_in_process(plugin);
            let mut returned = false;
            let result = owner.with_scoped_worker(&mut |_, io| {
                let (command, address) = if bus {
                    let mut buffers = BusAudioBuffers::new(&layout, 32, 48_000.0);
                    buffers.outputs[0].channels[0].fill(-0.25);
                    let address = buffers.outputs[0].channels[0].as_ptr();
                    (
                        WorkerCommand::ProcessBuses {
                            buffers,
                            transport: None,
                        },
                        address,
                    )
                } else {
                    let mut buffers = AudioBuffers::new(0, 1, 32, 48_000.0);
                    buffers.outputs[0].fill(-0.25);
                    let address = buffers.outputs[0].as_ptr();
                    (
                        WorkerCommand::Process {
                            buffers,
                            transport: None,
                        },
                        address,
                    )
                };
                let samples = match exchange(io, command) {
                    WorkerReply::Processed {
                        mut buffers,
                        result,
                    } => {
                        assert!(result.is_err());
                        buffers.outputs.remove(0)
                    }
                    WorkerReply::ProcessedBuses {
                        mut buffers,
                        result,
                    } => {
                        assert!(result.is_err());
                        buffers.outputs[0].channels.remove(0)
                    }
                    _ => panic!("wrong process reply"),
                };
                assert_eq!(samples.as_ptr(), address);
                if failure == "partial" {
                    assert_eq!(samples[..16], [1.0; 16]);
                    assert_eq!(samples[16..], [-0.25; 16]);
                }
                returned = true;
                Ok(())
            });
            assert!(returned);
            assert!(matches!(
                result.unwrap_err().primary,
                ScopedWorkerFailure::Worker(_)
            ));
            assert_eq!(owner.get_output_levels().channels[0].peak, 0.375);
        }
    }
}

#[test]
fn scoped_worker_poisoned_meter_mutex_skips_update_without_failing_processing() {
    for bus in [false, true] {
        let state = Arc::new(MockState::default());
        let mut internal = plugin_fixture(&Trace::default(), &state);
        internal.start_processing().unwrap();
        let layout = internal.audio_bus_layout().unwrap();
        let plugin = public_plugin(internal);
        let levels = plugin.audio_levels.clone();
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let mut levels = levels.lock().unwrap();
            levels.channels[0].peak = 0.625;
            panic!("intentional Rust mutex poison outside COM");
        }))
        .is_err());
        let mut owner = crate::plugin::MainThreadPlugin::from_in_process(plugin);
        owner
            .with_scoped_worker(&mut |_, io| {
                let command = if bus {
                    WorkerCommand::ProcessBuses {
                        buffers: BusAudioBuffers::new(&layout, 16, 48_000.0),
                        transport: None,
                    }
                } else {
                    process_command(16)
                };
                match exchange(io, command) {
                    WorkerReply::Processed { result, .. }
                    | WorkerReply::ProcessedBuses { result, .. } => result.unwrap(),
                    _ => panic!("wrong process reply"),
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(state.processed_layouts.lock().unwrap().len(), 1);
        assert_eq!(owner.get_output_levels().channels[0].peak, 0.625);
        assert!(levels.is_poisoned());
    }
}

#[test]
fn scoped_worker_either_registered_callback_rejects_before_com_queues_or_transport_change() {
    for callbacks in [1, 2, 3] {
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        state.capture_origin.store(true, Ordering::Release);
        let mut internal = plugin_fixture(&trace, &state);
        internal.start_processing().unwrap();
        internal.set_process_transport(reset_transport()).unwrap();
        internal.queue_processor_parameter_at(1, 0.25, 0).unwrap();
        internal
            .send_midi_event(MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: 60,
                velocity: 100,
            })
            .unwrap();
        let events = internal.runtime.input_events.clone();
        let handler = internal.control.component_handler.as_ref().unwrap().clone();
        assert_eq!(unsafe { handler.performEdit(3, 0.75) }, kResultOk);
        let native = handler.native_edits.snapshot();
        let mut plugin = public_plugin(internal);
        let called = Arc::new(AtomicUsize::new(0));
        if callbacks & 1 != 0 {
            let called = called.clone();
            plugin.on_audio_process(move |_| {
                called.fetch_add(1, Ordering::Relaxed);
            });
        }
        if callbacks & 2 != 0 {
            let called = called.clone();
            plugin.on_parameter_change(move |_, _| {
                called.fetch_add(1, Ordering::Relaxed);
            });
        }
        let mut owner = crate::plugin::MainThreadPlugin::from_in_process(plugin);
        let before = trace.calls().len();
        let mut entered = false;
        let result = owner.with_scoped_worker(&mut |_, _| {
            entered = true;
            Ok(())
        });
        assert!(matches!(
            result.unwrap_err().primary,
            ScopedWorkerFailure::Rejected(_)
        ));
        assert!(!entered);
        assert_eq!(trace.calls().len(), before);
        assert_eq!(called.load(Ordering::Acquire), 0);
        assert_eq!(events.events.lock().unwrap().len(), 1);
        assert_eq!(handler.native_edits.snapshot(), native);
        owner
            .process_audio(&mut AudioBuffers::new(0, 1, 16, 48_000.0))
            .unwrap();
        assert_eq!(
            state.origin_calls.lock().unwrap()[0].transport.unwrap().1,
            reset_transport().sample_position
        );
        assert_eq!(
            state.processed_parameters.lock().unwrap()[0],
            [(1, 0, 0.25), (3, 0, 0.75)]
        );
        assert_eq!(state.processed_event_count.load(Ordering::Acquire), 1);
    }
}

#[test]
fn scoped_worker_readiness_rejections_preserve_queues_transport_and_owner_references() {
    for mode in [
        "inactive",
        "not_processing",
        "output_fault",
        "wrong_thread",
        "missing_setup",
        "missing_data",
        "active_call",
    ] {
        let trace = Trace::default();
        let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
        owner.start_processing().unwrap();
        owner.set_process_transport(reset_transport()).unwrap();
        owner.queue_processor_parameter_at(7, 0.5, 8).unwrap();
        owner
            .send_midi_event(MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: 60,
                velocity: 100,
            })
            .unwrap();
        let main = owner.control.control_thread;
        match mode {
            "inactive" => owner.control.is_active = false,
            "not_processing" => owner.runtime.is_processing = false,
            "output_fault" => owner
                .runtime
                .parameter_output_fault
                .store(true, Ordering::Release),
            "wrong_thread" => {
                owner.control.control_thread =
                    thread::spawn(|| thread::current().id()).join().unwrap()
            }
            "missing_setup" => owner.control.applied_setup = None,
            "missing_data" => owner.runtime.process_data = None,
            _ => (),
        }
        let host = owner.control._host_app.clone();
        let mut gate = host.data_exchange_process_gate();
        let active = if mode == "active_call" {
            Some(gate.enter())
        } else {
            None
        };
        let levels = Mutex::new(crate::audio::AudioLevels::new(1));
        let before = trace.calls().len();
        let mut entered = false;
        let result = owner.with_scoped_worker(&levels, &mut |_, _| {
            entered = true;
            Ok(())
        });
        assert!(
            matches!(
                result.unwrap_err().primary,
                ScopedWorkerFailure::Rejected(_)
            ),
            "{mode}"
        );
        assert!(!entered, "{mode}");
        assert_eq!(trace.calls().len(), before, "{mode}");
        assert_eq!(
            owner.runtime.input_events.events.lock().unwrap().len(),
            1,
            "{mode}"
        );
        assert_eq!(owner.runtime.pending_param_changes.len(), 1, "{mode}");
        assert_eq!(owner.runtime.ordinary_note_counts[60], 1, "{mode}");
        assert_eq!(
            owner.runtime.process_transport,
            Some(reset_transport()),
            "{mode}"
        );
        drop(active);
        owner.control.control_thread = main;
        // Restore fixture-only readiness mutations so Drop follows the real active lifecycle.
        if mode == "inactive" {
            owner.control.is_active = true;
        }
        if mode == "not_processing" {
            owner.runtime.is_processing = true;
        }
    }
}

#[test]
fn scoped_worker_rejection_leaves_strict_main_thread_wrapper_callback_affinity_unchanged() {
    let main = thread::current().id();
    let trace = Trace::default();
    let mut internal = plugin_fixture(&trace, &Arc::new(MockState::default()));
    internal.start_processing().unwrap();
    let layout = internal.audio_bus_layout().unwrap();
    let mut plugin = public_plugin(internal);
    let levels = plugin.audio_levels.clone();
    let audio_calls = Arc::new(Mutex::new(Vec::new()));
    let observed = audio_calls.clone();
    plugin.on_audio_process(move |audio| {
        observed.lock().unwrap().push((
            thread::current().id(),
            audio.channels[0].peak,
            matches!(levels.try_lock(), Err(std::sync::TryLockError::WouldBlock)),
        ));
    });
    let parameter_calls = Arc::new(Mutex::new(Vec::new()));
    let observed = parameter_calls.clone();
    plugin.on_parameter_change(move |id, value| {
        observed
            .lock()
            .unwrap()
            .push((thread::current().id(), id, value));
    });
    let mut owner = crate::plugin::MainThreadPlugin::from_in_process(plugin);
    assert!(owner
        .with_scoped_worker(&mut |_, _| panic!("ineligible callback body"))
        .is_err());
    owner
        .process_audio(&mut AudioBuffers::new(0, 1, 40, 48_000.0))
        .unwrap();
    owner
        .process_audio(&mut AudioBuffers::new(0, 1, 0, 48_000.0))
        .unwrap();
    owner
        .process_bus_audio(&mut BusAudioBuffers::new(&layout, 16, 48_000.0))
        .unwrap();
    owner.set_parameter(7, 0.625).unwrap();
    assert_eq!(
        *audio_calls.lock().unwrap(),
        [(main, 1.0, true), (main, 0.0, true), (main, 1.0, true)]
    );
    assert_eq!(*parameter_calls.lock().unwrap(), [(main, 7, 0.625)]);
    trace.assert_thread(main);
}

#[test]
fn scoped_worker_reentry_preserves_zero_flush_and_positive_attempt_surge_restore_history() {
    for fail_positive in [false, true] {
        let state = Arc::new(MockState::default());
        let mut owner = plugin_fixture(&Trace::default(), &state);
        identify_surge_fixture(&mut owner);
        owner.start_processing().unwrap();
        for _ in 0..2 {
            let (result, reply) = internal_exchange(&mut owner, process_command(0));
            result.unwrap();
            assert_process_ok(reply, 0);
            assert!(!owner.runtime.has_processed_audio);
            assert!(owner.preflight_state_restore().is_ok());
        }
        let saved = owner.save_state().unwrap();
        owner.load_state(&saved).unwrap();
        state.fail_process.store(fail_positive, Ordering::Release);
        let (result, _) = internal_exchange(&mut owner, process_command(1));
        assert_eq!(result.is_err(), fail_positive);
        assert!(owner.runtime.has_processed_audio);
        assert!(owner.preflight_state_restore().is_err());
        state.fail_process.store(false, Ordering::Release);
        let (result, reply) = internal_exchange(&mut owner, process_command(0));
        result.unwrap();
        assert_process_ok(reply, 0);
        assert!(owner.runtime.has_processed_audio);
        assert!(owner.load_state(&saved).is_err());
    }
}

#[test]
fn scoped_worker_exit_never_stops_processing_and_owner_reset_state_work_after_rejoin() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut internal = plugin_fixture(&trace, &state);
    internal.start_processing().unwrap();
    let mut owner = crate::plugin::MainThreadPlugin::from_in_process(public_plugin(internal));
    let before = trace.calls().len();
    for _ in 0..2 {
        owner
            .with_scoped_worker(&mut |_, io| {
                assert_process_ok(exchange(io, process_command(16)), 16);
                Ok(())
            })
            .unwrap();
    }
    assert!(owner.is_processing());
    assert!(trace.methods()[before..]
        .iter()
        .all(|(_, method)| *method == "process"));
    let reset = owner.process_reset_origin(16, reset_transport()).unwrap();
    reset.ensure_success().unwrap();
    let saved = owner.save_state().unwrap();
    owner.load_state(&saved).unwrap();
    assert!(owner.is_processing());
    owner
        .with_scoped_worker(&mut |_, io| {
            assert_process_ok(exchange(io, process_command(16)), 16);
            Ok(())
        })
        .unwrap();
}

#[test]
fn scoped_worker_invalid_bus_command_rejects_before_sdk_and_native_ack() {
    let trace = Trace::default();
    let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
    owner.start_processing().unwrap();
    let handler = owner.control.component_handler.as_ref().unwrap().clone();
    assert_eq!(unsafe { handler.performEdit(5, 0.5) }, kResultOk);
    let native = handler.native_edits.snapshot();
    let layout = owner.audio_bus_layout().unwrap();
    let mut buffers = BusAudioBuffers::new(&layout, 16, 48_000.0);
    buffers.outputs[0].channels.clear();
    let before = trace.calls().len();
    let (result, reply) = internal_exchange(
        &mut owner,
        WorkerCommand::ProcessBuses {
            buffers,
            transport: None,
        },
    );
    assert!(result.is_err());
    assert!(matches!(
        reply,
        WorkerReply::ProcessedBuses { result: Err(_), .. }
    ));
    assert_eq!(trace.calls().len(), before);
    assert_eq!(handler.native_edits.snapshot(), native);
}

#[derive(Clone, Copy, Debug)]
enum ExitKind {
    Normal,
    BodyError,
    MainPanic,
}

fn exercise_shutdown(stage: &'static str, exit_kind: ExitKind, sdk_fails: bool) {
    use super::super::domain_session::scoped_worker::{run_for_test, WorkerTestPoint};
    let main = thread::current().id();
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    state.fail_process.store(sdk_fails, Ordering::Release);
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    let host = owner.control._host_app.clone();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let release_rx = Arc::new(Mutex::new(release_rx));
    let latch_ok = Arc::new(AtomicBool::new(true));
    let completed = Arc::new(AtomicBool::new(false));
    if stage == "inflight" {
        let entered_tx = entered_tx.clone();
        let release_rx = release_rx.clone();
        let latch_ok = latch_ok.clone();
        *state.process_entry_callback.lock().unwrap() = Some(Box::new(move || {
            let sent = entered_tx.send(()).is_ok();
            let released = release_rx
                .lock()
                .ok()
                .and_then(|rx| rx.recv_timeout(WATCHDOG).ok())
                .is_some();
            latch_ok.store(sent && released, Ordering::Release);
        }));
    }
    let finished_in_hook = completed.clone();
    let hook_ok = latch_ok.clone();
    let hook = Box::new(move |point| {
        if stage == "accepted_before_dispatch" && point == WorkerTestPoint::BeforeDispatch {
            let sent = entered_tx.send(()).is_ok();
            let released = release_rx
                .lock()
                .ok()
                .and_then(|rx| rx.recv_timeout(WATCHDOG).ok())
                .is_some();
            hook_ok.store(sent && released, Ordering::Release);
        }
        if point == WorkerTestPoint::AfterDispatchBeforeReply {
            finished_in_hook.store(true, Ordering::Release);
        }
        if stage == "unread" && point == WorkerTestPoint::AfterReply {
            let _ = entered_tx.send(());
        }
    });
    let body_leaving = Arc::new(AtomicBool::new(false));
    let pinned = Arc::new(AtomicBool::new(false));
    let levels = Mutex::new(crate::audio::AudioLevels::new(1));
    let before = trace.calls().len();
    let mut submitted_address = ptr::null();
    let outcome = thread::scope(|scope| {
        let body_leaving = body_leaving.clone();
        let pinned = pinned.clone();
        let trace = trace.clone();
        let body_leaving_in_releaser = body_leaving.clone();
        let releaser = scope.spawn(move || {
            let start = Instant::now();
            while !body_leaving_in_releaser.load(Ordering::Acquire) && start.elapsed() < WATCHDOG {
                thread::sleep(Duration::from_millis(1));
            }
            if stage != "unread" {
                thread::sleep(Duration::from_millis(30));
            }
            pinned.store(
                !trace.methods().iter().any(|(_, method)| {
                    matches!(*method, "terminate" | "drop" | "setProcessing(false)")
                }),
                Ordering::Release,
            );
            let _ = release_tx.send(());
        });
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_for_test(
                &mut owner,
                &levels,
                &mut |_, io| {
                    let buffers = AudioBuffers::new(0, 1, 16, 48_000.0);
                    submitted_address = buffers.outputs[0].as_ptr();
                    io.try_submit(WorkerCommand::Process {
                        buffers,
                        transport: None,
                    })
                    .unwrap();
                    entered_rx.recv_timeout(WATCHDOG).unwrap();
                    assert!(io.has_pending());
                    body_leaving.store(true, Ordering::Release);
                    match exit_kind {
                        ExitKind::Normal => Ok(()),
                        ExitKind::BodyError => Err(Error::ProcessFailed(-76_543)),
                        ExitKind::MainPanic => {
                            std::panic::panic_any("exact scoped-main panic payload")
                        }
                    }
                },
                Some(hook),
                false,
            )
        }));
        releaser.join().unwrap();
        result
    });
    assert!(latch_ok.load(Ordering::Acquire), "{stage} {exit_kind:?}");
    assert!(
        completed.load(Ordering::Acquire),
        "accepted work must finish before return/unwind"
    );
    assert!(
        pinned.load(Ordering::Acquire),
        "owner/module must outlive scoped join"
    );
    assert!(!host.is_data_exchange_in_process_for_test());
    assert!(owner.runtime.is_processing);
    assert!(owner.runtime.has_processed_audio);
    if matches!(exit_kind, ExitKind::MainPanic) {
        assert_eq!(
            outcome.unwrap_err().downcast_ref::<&'static str>(),
            Some(&"exact scoped-main panic payload")
        );
    } else {
        let error = outcome.unwrap().unwrap_err();
        match (&error.primary, exit_kind, sdk_fails) {
            (ScopedWorkerFailure::Body(Error::ProcessFailed(-76_543)), ExitKind::BodyError, _) => {}
            (ScopedWorkerFailure::Worker(_), ExitKind::Normal, true) => (),
            (ScopedWorkerFailure::UnconsumedReply, ExitKind::Normal, false) => (),
            other => panic!("wrong precedence: {other:?}"),
        }
        let Some(WorkerReply::Processed { buffers, result }) =
            error.unconsumed_reply.map(|reply| *reply)
        else {
            panic!("missing original unread audio");
        };
        assert_eq!(buffers.outputs[0].as_ptr(), submitted_address);
        assert_eq!(result.is_err(), sdk_fails);
        if sdk_fails {
            let Some(WorkerOutcome::Failed(secondary)) = error.secondary_worker else {
                panic!("lost worker failure");
            };
            let reply_error = result.unwrap_err();
            assert!(Arc::ptr_eq(&secondary, &reply_error));
            if let ScopedWorkerFailure::Worker(primary) = error.primary {
                assert!(Arc::ptr_eq(&primary, &reply_error));
            }
        } else {
            assert!(error.secondary_worker.is_none());
            assert_eq!(buffers.outputs[0], [1.0; 16]);
        }
    }
    assert!(trace.methods()[before..]
        .iter()
        .all(|(_, method)| *method == "process"));
    let workers: Vec<_> = trace.calls()[before..]
        .iter()
        .filter(|c| c.method == "process")
        .map(|c| c.thread)
        .collect();
    assert_eq!(workers.len(), 1);
    *state.process_entry_callback.lock().unwrap() = None;
    drop(host);
    drop(owner);
    assert_owner_ledger(&trace, main, &workers);
    assert_teardown_before_module(&trace);
}

#[test]
fn scoped_worker_normal_exit_drains_accepted_inflight_and_unread_success_or_failure() {
    for stage in ["accepted_before_dispatch", "inflight", "unread"] {
        for sdk_fails in [false, true] {
            exercise_shutdown(stage, ExitKind::Normal, sdk_fails);
        }
    }
}

#[test]
fn scoped_worker_body_error_stays_primary_after_draining_accepted_inflight_and_unread_work() {
    for stage in ["accepted_before_dispatch", "inflight", "unread"] {
        for sdk_fails in [false, true] {
            exercise_shutdown(stage, ExitKind::BodyError, sdk_fails);
        }
    }
}

#[test]
fn scoped_worker_main_rust_panic_resumes_exact_payload_only_after_all_accepted_work_joins() {
    for stage in ["accepted_before_dispatch", "inflight", "unread"] {
        for sdk_fails in [false, true] {
            exercise_shutdown(stage, ExitKind::MainPanic, sdk_fails);
        }
    }
}

#[test]
fn scoped_worker_rust_panic_is_terminal_before_join_and_preserves_prior_sdk_error() {
    use super::super::domain_session::scoped_worker::{run_for_test, WorkerTestPoint};
    for point in [
        WorkerTestPoint::BeforeDispatch,
        WorkerTestPoint::AfterDispatchBeforeReply,
    ] {
        for body_error in [false, true] {
            for sdk_fails in [false, true] {
                if point == WorkerTestPoint::BeforeDispatch && sdk_fails {
                    continue;
                }
                let main = thread::current().id();
                let trace = Trace::default();
                let state = Arc::new(MockState::default());
                state.fail_process.store(sdk_fails, Ordering::Release);
                let mut owner = plugin_fixture(&trace, &state);
                owner.start_processing().unwrap();
                attach_mock_view(&mut owner, &trace);
                let handler = owner.control.component_handler.as_ref().unwrap().clone();
                assert_eq!(unsafe { handler.performEdit(7, 0.5) }, kResultOk);
                let levels = Mutex::new(crate::audio::AudioLevels::new(1));
                let before = trace.calls().len();
                let hook = Box::new(move |at| {
                    if at == point {
                        panic!("intentional private worker Rust panic outside COM");
                    }
                });
                let result = run_for_test(
                    &mut owner,
                    &levels,
                    &mut |ui, io| {
                        io.try_submit(process_command(16)).unwrap();
                        // Exercise status-only observation, without recv/try_reply manufacturing it.
                        let start = Instant::now();
                        while !io.is_terminal() && start.elapsed() < WATCHDOG {
                            thread::sleep(Duration::from_millis(1));
                        }
                        assert!(io.is_terminal(), "worker unwind must close UI before join");
                        assert!(io.is_closed());
                        let before_ui = trace.calls().len();
                        assert!(ui.request(AttachedUiRequest::AttachedSize).is_err());
                        assert_eq!(trace.calls().len(), before_ui);
                        if body_error {
                            Err(Error::ProcessFailed(-76_543))
                        } else {
                            Ok(())
                        }
                    },
                    Some(hook),
                    false,
                )
                .unwrap_err();
                if body_error {
                    assert!(matches!(
                        result.primary,
                        ScopedWorkerFailure::Body(Error::ProcessFailed(-76_543))
                    ));
                } else {
                    assert!(matches!(
                        result.primary,
                        ScopedWorkerFailure::WorkerPanicked
                    ));
                }
                let Some(WorkerOutcome::Panicked { prior_error }) = result.secondary_worker else {
                    panic!("missing worker panic evidence");
                };
                assert_eq!(prior_error.is_some(), sdk_fails);
                if let Some(error) = prior_error {
                    assert!(
                        matches!(error.as_ref(), Error::ProcessFailed(code) if *code == kResultFalse)
                    );
                }
                assert!(result.unconsumed_reply.is_none());
                let dispatched = point == WorkerTestPoint::AfterDispatchBeforeReply;
                assert_eq!(
                    handler.native_edits.snapshot().applied,
                    u64::from(dispatched && !sdk_fails)
                );
                assert_eq!(
                    handler.native_edits.snapshot().lost,
                    dispatched && sdk_fails
                );
                assert_eq!(owner.runtime.has_processed_audio, dispatched);
                assert!(!owner
                    .control
                    ._host_app
                    .is_data_exchange_in_process_for_test());
                let workers: Vec<_> = trace.calls()[before..]
                    .iter()
                    .filter(|c| c.method == "process")
                    .map(|c| c.thread)
                    .collect();
                assert_eq!(workers.len(), usize::from(dispatched));
                drop(handler);
                drop(owner);
                assert_owner_ledger(&trace, main, &workers);
                assert_teardown_before_module(&trace);
            }
        }
    }
}

#[test]
fn scoped_worker_spawn_failure_does_not_enter_body_or_touch_prepared_owner() {
    use super::super::domain_session::scoped_worker::run_for_test;
    let trace = Trace::default();
    let mut owner = plugin_fixture(&trace, &Arc::new(MockState::default()));
    owner.start_processing().unwrap();
    owner.set_process_transport(reset_transport()).unwrap();
    owner.queue_processor_parameter_at(7, 0.5, 0).unwrap();
    let prepared = &**owner.runtime.process_data.as_ref().unwrap() as *const HostProcessData;
    let levels = Mutex::new(crate::audio::AudioLevels::new(1));
    let before = trace.calls().len();
    let mut entered = false;
    let result = run_for_test(
        &mut owner,
        &levels,
        &mut |_, _| {
            entered = true;
            Ok(())
        },
        None,
        true,
    )
    .unwrap_err();
    assert!(matches!(
        result.primary,
        ScopedWorkerFailure::Spawn(Error::IoError(_))
    ));
    assert!(result.secondary_worker.is_none());
    assert!(result.unconsumed_reply.is_none());
    assert!(!entered);
    assert_eq!(trace.calls().len(), before);
    assert_eq!(
        &**owner.runtime.process_data.as_ref().unwrap() as *const HostProcessData,
        prepared
    );
    assert_eq!(owner.runtime.pending_param_changes.len(), 1);
    assert_eq!(owner.runtime.process_transport, Some(reset_transport()));
    let (result, reply) = internal_exchange(&mut owner, process_command(16));
    result.unwrap();
    assert_process_ok(reply, 16);
}

#[test]
fn scoped_worker_public_commands_keep_full_split_transport_parameter_midi_and_gate_pipeline() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    state.capture_origin.store(true, Ordering::Release);
    let mut internal = plugin_fixture(&trace, &state);
    internal.start_processing().unwrap();
    internal.queue_processor_parameter_at(2, 0.5, 17).unwrap();
    let handler = internal.control.component_handler.as_ref().unwrap().clone();
    assert_eq!(unsafe { handler.performEdit(3, 0.75) }, kResultOk);
    let host = internal.control._host_app.clone();
    let observed_gate = Arc::new(AtomicBool::new(true));
    let gate_in_sdk = observed_gate.clone();
    let host_in_sdk = host.clone();
    *state.process_entry_callback.lock().unwrap() = Some(Box::new(move || {
        if !host_in_sdk.is_data_exchange_in_process_for_test() {
            gate_in_sdk.store(false, Ordering::Release);
        }
    }));
    let layout = internal.audio_bus_layout().unwrap();
    let mut owner = crate::plugin::MainThreadPlugin::from_in_process(public_plugin(internal));
    let transport = crate::plugin::ProcessTransport {
        sample_position: 1024,
        quarter_note_position: 4.0,
        tempo: 120.0,
        playing: true,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
    };
    let report = owner
        .with_scoped_worker(&mut |_, io| {
            assert_midi_ok(exchange(
                io,
                WorkerCommand::SendMidi {
                    event: note(100),
                    sample_offset: 1,
                },
            ));
            assert_midi_ok(exchange(
                io,
                WorkerCommand::SendMidi {
                    event: OrdinaryMidiEvent::NoteOff {
                        channel: MidiChannel::Ch1,
                        note: 60,
                        velocity: 0,
                    },
                    sample_offset: 33,
                },
            ));
            assert_process_ok(
                exchange(
                    io,
                    WorkerCommand::Process {
                        buffers: AudioBuffers::new(0, 1, 40, 48_000.0),
                        transport: Some(transport),
                    },
                ),
                40,
            );
            match exchange(
                io,
                WorkerCommand::ProcessBuses {
                    buffers: BusAudioBuffers::new(&layout, 16, 48_000.0),
                    transport: None,
                },
            ) {
                WorkerReply::ProcessedBuses { buffers, result } => {
                    result.unwrap();
                    assert_eq!(buffers.outputs[0].channels[0], [1.0; 16]);
                }
                _ => panic!("wrong bus reply"),
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(report.completed_commands, 4);
    assert!(observed_gate.load(Ordering::Acquire));
    assert!(!host.is_data_exchange_in_process_for_test());
    assert_eq!(
        *state.processed_parameters.lock().unwrap(),
        [vec![(3, 0, 0.75)], vec![(2, 1, 0.5)], vec![], vec![]]
    );
    let calls = state.origin_calls.lock().unwrap();
    assert_eq!(
        calls.iter().map(|call| call.frames).collect::<Vec<_>>(),
        [16, 16, 8, 16]
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.transport.unwrap().1)
            .collect::<Vec<_>>(),
        [1024, 1040, 1056, 1064]
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call.events.len())
            .collect::<Vec<_>>(),
        [1, 0, 1, 0]
    );
    assert_eq!(calls[0].events[0], (kNoteOnEvent as u16, 1, 60));
    assert_eq!(calls[2].events[0], (kNoteOffEvent as u16, 1, 60));
    assert_eq!(handler.native_edits.snapshot().applied, 1);
    assert!(!handler.native_edits.snapshot().lost);
    drop(calls);
    *state.process_entry_callback.lock().unwrap() = None;
}

#[test]
fn scoped_worker_current_and_retired_host_queue_release_counts_remain_distinct() {
    use vst3::com_scrape_types::Unknown;
    // The feature worker borrows its processor; it never owns or releases that interface.
    // A plugin-retained host queue is a separate SDK reference. A queue retired by an
    // owner-side rebuild can legitimately receive final Release/free inside later Process.
    // This bounds the affinity claim: it is not a whole-Process no-allocation/no-free claim.
    for retire_before_release in [false, true] {
        let main = thread::current().id();
        let trace = Trace::default();
        let state = Arc::new(MockState::default());
        let mut internal = plugin_fixture(&trace, &state);
        internal.start_processing().unwrap();
        let input = &internal
            .runtime
            .process_data
            .as_ref()
            .unwrap()
            .input_param_changes;
        let retained = unsafe {
            let raw = input.addParameterData(&7, ptr::null_mut());
            assert!(!raw.is_null());
            assert_eq!(IParamValueQueue::add_ref(raw), 2);
            ComPtr::from_raw(raw).unwrap()
        };
        if retire_before_release {
            internal.stop_processing().unwrap();
            internal.reconfigure(44_100.0, 32).unwrap();
            internal.start_processing().unwrap();
        }
        let retained = Mutex::new(Some(retained));
        let releases = Arc::new(Mutex::new(Vec::new()));
        let ledger = releases.clone();
        let release_recorded = Arc::new(AtomicBool::new(false));
        let recorded_in_sdk = release_recorded.clone();
        *state.process_callback.lock().unwrap() = Some(Box::new(move || {
            // Do not panic through COM if the test's latch/ledger unexpectedly fails.
            let retained = retained
                .lock()
                .ok()
                .and_then(|mut retained| retained.take());
            if let Some(retained) = retained {
                // SAFETY: exactly the owned SDK reference acquired by AddRef above.
                // into_raw transfers it without Release; release consumes it exactly once.
                // No dereference follows a possible final release. The SDK already grants
                // IParamValueQueue Send + Sync; no new unsafe auto-trait or owner is added.
                let remaining = unsafe { IParamValueQueue::release(retained.into_raw()) };
                if let Ok(mut ledger) = ledger.lock() {
                    ledger.push((thread::current().id(), remaining));
                    recorded_in_sdk.store(true, Ordering::Release);
                }
            }
        }));
        let mut owner = crate::plugin::MainThreadPlugin::from_in_process(public_plugin(internal));
        let before = trace.calls().len();
        let report = owner
            .with_scoped_worker(&mut |_, io| {
                assert_process_ok(
                    exchange(
                        io,
                        WorkerCommand::Process {
                            buffers: AudioBuffers::new(
                                0,
                                1,
                                0,
                                if retire_before_release {
                                    44_100.0
                                } else {
                                    48_000.0
                                },
                            ),
                            transport: None,
                        },
                    ),
                    0,
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(report.completed_commands, 1);
        assert!(release_recorded.load(Ordering::Acquire));
        let calls = trace.calls();
        let workers: Vec<_> = calls[before..]
            .iter()
            .filter(|call| call.method == "process")
            .map(|call| call.thread)
            .collect();
        assert_eq!(workers.len(), 1);
        let worker = workers[0];
        assert_ne!(worker, main);
        assert_eq!(
            *releases.lock().unwrap(),
            [(worker, if retire_before_release { 0 } else { 1 })]
        );
        assert_eq!(&trace.methods()[before..], &[("component", "process")]);
        *state.process_callback.lock().unwrap() = None;
        owner.stop_processing().unwrap();
        drop(owner);
        assert_owner_ledger(&trace, main, &[worker]);
        assert_teardown_before_module(&trace);
    }
}
