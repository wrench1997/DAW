// Real callback, independent generator/insert workers and retained gate cleanup.
use super::*;

fn prepared(f: &mut StoppedSafetyFixture) -> PreparedPlaybackToken {
    f.activate(2, false);
    f.pump(128);
    f.settle(2);
    f.pump(128);
    f.engine
        .as_ref()
        .unwrap()
        .prepared_playback_token()
        .expect("real stopped worker settlement")
}

#[test]
fn playback_startup_waits_for_every_real_reset_and_safety_terminal_without_music_time() {
    let mut f = StoppedSafetyFixture::new();
    for worker in &f.workers {
        worker.probe.reset_gate.hold();
    }
    f.activate(2, false);
    f.pump(128);
    for worker in &f.workers {
        worker.probe.reset_gate.wait_entered();
    }
    for _ in 0..12 {
        f.pump(128);
        let snapshot = f.engine.as_ref().unwrap().snapshot();
        assert_eq!(snapshot.timeline_frame, 0);
        assert_eq!(snapshot.transport_epoch, 2);
        assert!(!snapshot.transport_playing);
        assert!(snapshot.plugin_processing_fault.is_none());
        assert!(
            f.engine
                .as_ref()
                .unwrap()
                .prepared_playback_token()
                .is_none()
        );
    }
    f.workers[0].probe.reset_gate.release();
    wait_until(|| f.workers[0].probe.process_count.load(Ordering::Acquire) == 1);
    f.pump(128);
    assert!(
        f.engine
            .as_ref()
            .unwrap()
            .prepared_playback_token()
            .is_none(),
        "the insert is still resetting"
    );
    f.workers[1].probe.reset_gate.release();
    f.settle(2);
    f.pump(128);
    let token = f
        .engine
        .as_ref()
        .unwrap()
        .prepared_playback_token()
        .unwrap();
    let reset_counts: Vec<_> = f
        .workers
        .iter()
        .map(|w| w.control().stats().worker_epoch_resets)
        .collect();
    assert!(
        f.engine
            .as_ref()
            .unwrap()
            .start_prepared_playback(token)
            .is_some()
    );
    f.pump(128);
    let snapshot = f.engine.as_ref().unwrap().snapshot();
    assert!(snapshot.transport_playing);
    assert_eq!(snapshot.transport_epoch, 2);
    assert_eq!(snapshot.timeline_frame, 128);
    assert_eq!(
        f.workers
            .iter()
            .map(|w| w.control().stats().worker_epoch_resets)
            .collect::<Vec<_>>(),
        reset_counts
    );
}

#[test]
fn playback_startup_latest_pause_cancels_a_guard_waiting_on_real_admin_work() {
    let mut f = StoppedSafetyFixture::new();
    let token = prepared(&mut f);
    f.workers[0].probe.admin_gate.hold();
    assert!(f.workers[0].control().request_state(0));
    f.workers[0].probe.admin_gate.wait_entered();
    let issued = f
        .engine
        .as_ref()
        .unwrap()
        .start_prepared_playback(token)
        .unwrap();
    f.pump(128);
    let waiting = f.engine.as_ref().unwrap().snapshot();
    assert!(!waiting.transport_playing);
    assert_ne!(waiting.applied_transport_request, issued);
    f.engine.as_ref().unwrap().set_playing(false);
    f.workers[0].probe.admin_gate.release();
    f.settle(2);
    for _ in 0..12 {
        f.pump(128);
    }
    let canceled = f.engine.as_ref().unwrap().snapshot();
    assert!(!canceled.transport_playing);
    assert_eq!(canceled.timeline_frame, 0);
    assert_eq!(canceled.transport_epoch, 2);
}

fn assert_nonplay_mutation_cancels_waiting_guard(seek: bool) {
    let mut f = StoppedSafetyFixture::new();
    let token = prepared(&mut f);
    f.workers[0].probe.admin_gate.hold();
    assert!(f.workers[0].control().request_state(0));
    f.workers[0].probe.admin_gate.wait_entered();
    let issued = f
        .engine
        .as_ref()
        .unwrap()
        .start_prepared_playback(token)
        .unwrap();
    f.pump(128);
    assert_ne!(
        f.engine
            .as_ref()
            .unwrap()
            .snapshot()
            .applied_transport_request,
        issued
    );
    let engine = f.engine.as_ref().unwrap();
    let canceled = if seek {
        engine.seek_transport_to_frame(0.0, 0)
    } else {
        engine.set_transport_loop_frames(0.0, 0, 4.0, 96_000, false)
    };
    f.pump(128);
    let snapshot = f.engine.as_ref().unwrap().snapshot();
    assert_eq!(snapshot.applied_transport_request, canceled);
    assert!(
        !snapshot.transport_playing,
        "a non-Play mutation must not downgrade guarded Play to ordinary Play"
    );
    assert_eq!(snapshot.timeline_frame, 0);
    f.workers[0].probe.admin_gate.release();
    f.settle(2);
    for _ in 0..12 {
        f.pump(128);
    }
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
}

#[test]
fn playback_startup_loop_change_cancels_an_unconsumed_guard() {
    assert_nonplay_mutation_cancels_waiting_guard(false);
}

#[test]
fn playback_startup_seek_none_cancels_an_unconsumed_guard() {
    assert_nonplay_mutation_cancels_waiting_guard(true);
}

#[test]
fn playback_startup_accepted_guard_loop_change_keeps_playing() {
    let mut f = StoppedSafetyFixture::new();
    let token = prepared(&mut f);
    let issued = f
        .engine
        .as_ref()
        .unwrap()
        .start_prepared_playback(token)
        .unwrap();
    f.pump(128);
    let playing = f.engine.as_ref().unwrap().snapshot();
    assert_eq!(playing.applied_transport_request, issued);
    assert!(playing.transport_playing);
    let updated = f
        .engine
        .as_ref()
        .unwrap()
        .set_transport_loop_frames(0.0, 0, 4.0, 96_000, false);
    f.pump(128);
    let continued = f.engine.as_ref().unwrap().snapshot();
    assert_eq!(continued.applied_transport_request, updated);
    assert!(continued.transport_playing);
    assert_eq!(continued.transport_epoch, 2);
    assert_eq!(continued.timeline_frame, 256);
}

#[test]
fn playback_startup_stale_epoch_token_is_consumed_stopped_and_cannot_play_later() {
    let mut f = StoppedSafetyFixture::new();
    let stale = prepared(&mut f);
    f.activate(3, false);
    let issued = f
        .engine
        .as_ref()
        .unwrap()
        .start_prepared_playback(stale)
        .unwrap();
    f.pump(128);
    let rejected = f.engine.as_ref().unwrap().snapshot();
    assert_eq!(rejected.applied_transport_request, issued);
    assert_eq!(rejected.transport_epoch, 3);
    assert!(!rejected.transport_playing);
    f.settle(3);
    for _ in 0..12 {
        f.pump(128);
    }
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
}

#[test]
fn playback_startup_rejected_guard_loop_change_cannot_resurrect_playback() {
    let mut f = StoppedSafetyFixture::new();
    let stale = prepared(&mut f);
    f.activate(3, false);
    let rejected = f
        .engine
        .as_ref()
        .unwrap()
        .start_prepared_playback(stale)
        .unwrap();
    f.pump(128);
    assert_eq!(
        f.engine
            .as_ref()
            .unwrap()
            .snapshot()
            .applied_transport_request,
        rejected
    );
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    f.settle(3);
    f.engine
        .as_ref()
        .unwrap()
        .set_transport_loop_frames(0.0, 0, 4.0, 96_000, false);
    for _ in 0..12 {
        f.pump(128);
    }
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
}

#[test]
fn playback_startup_slow_first_music_dsp_still_faults_at_the_original_deadline() {
    let mut f = StoppedSafetyFixture::new();
    let token = prepared(&mut f);
    let original_plan = f.callback().dsp.plugin_timing;
    f.workers[0].probe.process_gate.hold();
    assert!(
        f.engine
            .as_ref()
            .unwrap()
            .start_prepared_playback(token)
            .is_some()
    );
    f.pump(128);
    f.workers[0].probe.process_gate.wait_entered();
    for _ in 0..original_plan.lookahead_quanta + 2 {
        let engine = f.engine.as_ref().unwrap();
        f.callback.as_mut().unwrap().pump(engine, 128);
        if f.engine
            .as_ref()
            .unwrap()
            .snapshot()
            .plugin_processing_fault
            .is_some()
        {
            break;
        }
    }
    let fault = f
        .engine
        .as_ref()
        .unwrap()
        .snapshot()
        .plugin_processing_fault
        .unwrap();
    assert_eq!(fault.reason, PluginProcessingFaultReason::DeadlineMiss);
    assert_eq!(
        fault.expected_sequence, 2,
        "only stopped sequence 1 was fenced"
    );
    assert_eq!(f.callback().dsp.plugin_timing, original_plan);
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
}
