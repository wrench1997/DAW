//! Actual DSP-boundary opt-in, including a task that starts and finishes during
//! the foreign process call. This is not a cached UI readiness check.
use super::*;

fn offline() -> (PluginImpl, Arc<MockState>, Trace) {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.set_process_mode(crate::ProcessMode::Offline).unwrap();
    owner.start_processing().unwrap();
    trace.clear();
    (owner, state, trace)
}

fn progress_pointer(owner: &PluginImpl) -> usize {
    owner
        .control
        .component_handler
        .as_ref()
        .unwrap()
        .as_com_ref::<IComponentHandler>()
        .unwrap()
        .cast::<IProgress>()
        .unwrap()
        .as_ptr() as usize
}

unsafe fn start(pointer: usize, kind: u32) -> u64 {
    // The loading owner retains this exact COM allocation for the entire test;
    // each callback is synchronous and removed before its owner is dropped.
    let progress = vst3::ComRef::from_raw(pointer as *mut IProgress).unwrap();
    let mut id = 0;
    assert_eq!(progress.start(kind, ptr::null(), &mut id), kResultOk);
    id
}

#[test]
fn async_state_readiness_arm_checks_active_revision_and_mode_without_changing_legacy_processing() {
    let (mut owner, _, trace) = offline();
    let pointer = progress_pointer(&owner);
    let id = unsafe { start(pointer, IProgress_::ProgressType_::AsyncStateRestoration) };
    assert!(owner.arm_offline_state_readiness(0).is_err());
    assert!(owner.arm_offline_state_readiness(1).is_err());
    assert!(owner.runtime.offline_readiness_revision.is_none());
    // Opt-in applies to this new usage only; the existing unarmed Offline entry
    // point is not silently assigned a new progress prerequisite.
    owner
        .process(&mut AudioBuffers::new(1, 1, 16, 48_000.0))
        .unwrap();
    unsafe {
        assert_eq!(
            vst3::ComRef::from_raw(pointer as *mut IProgress)
                .unwrap()
                .finish(id),
            kResultOk
        );
    }
    assert!(owner.arm_offline_state_readiness(0).is_err());
    owner.arm_offline_state_readiness(1).unwrap();
    assert!(owner.arm_offline_state_readiness(1).is_err());
    assert_eq!(trace.methods(), [("component", "process")]);
}

#[test]
fn async_state_readiness_start_before_first_dsp_rejects_without_calling_the_processor() {
    let (mut owner, _, trace) = offline();
    owner.arm_offline_state_readiness(0).unwrap();
    let pointer = progress_pointer(&owner);
    unsafe {
        start(pointer, IProgress_::ProgressType_::AsyncStateRestoration);
    }
    let mut buffers = AudioBuffers::new(1, 1, 16, 48_000.0);
    buffers.outputs[0].fill(0.125);
    assert!(owner.process(&mut buffers).is_err());
    assert!(trace.methods().is_empty());
    assert_eq!(buffers.outputs[0], [0.125; 16]);
}

#[test]
fn async_state_readiness_start_and_finish_inside_first_dsp_rejects_output_and_later_blocks() {
    let (mut owner, state, trace) = offline();
    owner.arm_offline_state_readiness(0).unwrap();
    let pointer = progress_pointer(&owner);
    *state.process_callback.lock().unwrap() = Some(Box::new(move || unsafe {
        let id = start(pointer, IProgress_::ProgressType_::AsyncStateRestoration);
        assert_eq!(
            vst3::ComRef::from_raw(pointer as *mut IProgress)
                .unwrap()
                .finish(id),
            kResultOk
        );
    }));
    let mut buffers = AudioBuffers::new(1, 1, 16, 48_000.0);
    buffers.outputs[0].fill(0.125);
    assert!(owner.process(&mut buffers).is_err());
    assert_eq!(
        buffers.outputs[0], [0.125; 16],
        "unready SDK output must not be copied"
    );
    assert_eq!(owner.async_state_snapshot().unwrap().active_async, 0);
    assert_eq!(
        owner.async_state_snapshot().unwrap().async_started_revision,
        1
    );
    *state.process_callback.lock().unwrap() = None;
    owner.discard_progress_notifications().unwrap();
    assert!(
        owner.process(&mut buffers).is_err(),
        "drain/finish must not repair a stale guard"
    );
    assert_eq!(trace.methods(), [("component", "process")]);
}

#[test]
fn async_state_readiness_ui_background_work_during_dsp_does_not_block_audio() {
    let (mut owner, state, trace) = offline();
    owner.arm_offline_state_readiness(0).unwrap();
    let pointer = progress_pointer(&owner);
    *state.process_callback.lock().unwrap() = Some(Box::new(move || unsafe {
        start(pointer, IProgress_::ProgressType_::UIBackgroundTask);
    }));
    let mut buffers = AudioBuffers::new(1, 1, 16, 48_000.0);
    owner.process(&mut buffers).unwrap();
    assert_eq!(buffers.outputs[0], [1.0; 16]);
    let snapshot = owner.async_state_snapshot().unwrap();
    assert_eq!(
        (
            snapshot.active_async,
            snapshot.active_ui,
            snapshot.async_started_revision
        ),
        (0, 1, 0)
    );
    snapshot.ensure_ready().unwrap();
    *state.process_callback.lock().unwrap() = None;
    assert_eq!(trace.methods(), [("component", "process")]);
}

#[test]
fn async_state_readiness_guard_does_not_change_the_existing_realtime_entry_point() {
    let trace = Trace::default();
    let state = Arc::new(MockState::default());
    let mut owner = plugin_fixture(&trace, &state);
    owner.start_processing().unwrap();
    assert!(owner.arm_offline_state_readiness(0).is_err());
    let pointer = progress_pointer(&owner);
    unsafe {
        start(pointer, IProgress_::ProgressType_::AsyncStateRestoration);
    }
    owner
        .process(&mut AudioBuffers::new(1, 1, 16, 48_000.0))
        .unwrap();
}
