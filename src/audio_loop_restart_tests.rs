// Source-only loop restart regressions. Independent real worker gates hold SDK
// reset/admin/process operations; only the production callback submits audio.
use super::*;

const LOOP_END: u64 = 16_384;

#[test]
fn loop_restart_pending_is_part_of_the_coherent_transport_publication() {
    const PUBLISHES: u64 = 20_000;
    let status = Arc::new(AudioStatus::default());
    let writer_status = Arc::clone(&status);
    let writer = thread::spawn(move || {
        for value in 1..=PUBLISHES {
            let pending = (value % 2 == 0).then_some(PendingLoopRestart {
                prepared: PreparedPlaybackToken::from_words([1, 7, 71, value, value, 1, 1, 1, 0]).unwrap(),
                identity: LoopRestartIdentity {
                    request_id: value,
                    discontinuity_id: 0,
                    loop_start_q32: 0,
                    loop_end_q32: 1,
                    loop_start_frame: 0,
                    loop_end_frame: 128,
                    loop_token: 1,
                },
            });
            RealtimeTransport {
                request: TransportRequest { request_id: value, ..TransportRequest::default() },
                device_frame: value,
                timeline_frame: value * 2,
                epoch: value,
                loop_count: value * 3,
                pending_loop_restart: pending,
                ..RealtimeTransport::default()
            }.publish(&writer_status);
        }
    });
    loop {
        let snapshot = status.transport_fields();
        let value = snapshot.applied_request;
        if value != 0 {
            assert_eq!(snapshot.device_frame, value);
            assert_eq!(snapshot.timeline_frame, value * 2);
            assert_eq!(snapshot.epoch, value);
            assert_eq!(snapshot.loop_count, value * 3);
            assert_eq!(snapshot.loop_restart_pending, value % 2 == 0);
            assert!(!snapshot.playing);
        }
        if value == PUBLISHES { break; }
    }
    writer.join().unwrap();
}

fn pump_callback(f: &mut StoppedSafetyFixture, frames: usize) {
    let engine = f.engine.as_ref().unwrap();
    let callback = f.callback.as_mut().unwrap();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(engine, frames));
}

fn begin_loop_wait(first_segment: usize, raw_frames: usize) -> StoppedSafetyFixture {
    let mut f = StoppedSafetyFixture::new();
    f.activate_with_loop_at(2, false, LOOP_END, LOOP_END - first_segment as u64);
    f.pump(128);
    f.settle(2);
    f.pump(128);
    let token = f.engine.as_ref().unwrap().prepared_playback_token().unwrap();
    for worker in &f.workers {
        worker.probe.reset_gate.hold();
    }
    assert!(f.engine.as_ref().unwrap().start_prepared_playback(token).is_some());
    f.pump(raw_frames);
    let wrapped = f.engine.as_ref().unwrap().snapshot();
    assert_eq!(wrapped.timeline_frame, 0);
    assert_eq!(wrapped.transport_epoch, 3);
    assert_eq!(wrapped.transport_loop_count, 1);
    assert!(!wrapped.transport_playing);
    assert!(wrapped.transport_loop_restart_pending);
    assert!(wrapped.plugin_processing_fault.is_none());
    f
}

fn settle_loop_safety(f: &mut StoppedSafetyFixture) {
    for worker in &f.workers {
        worker.probe.reset_gate.release();
    }
    f.settle(3);
    assert!(f.callback().dsp.stopped_playback_workers_settled());
}

fn queue_loop_activation(f: &mut StoppedSafetyFixture, epoch: u64, playing: bool) -> u64 {
    let topology = f.callback().dsp.plugin_topology_revision;
    let engine = f.engine.as_mut().unwrap();
    let timing = engine.prepare_plugin_timing();
    let request_id = engine.transport_request_id();
    let controller = engine.timeline_runtime.as_mut().unwrap();
    let mut chase = controller.prepare_chase(&f.timeline, SAFETY_REVISION, epoch, 0,
        TimelineChaseOptions::default()).unwrap();
    chase.plugin_timing = timing;
    chase.plugin_topology_revision = topology;
    controller.install_chase(chase).unwrap();
    controller.activate_transport(TimelineTransportActivationSpec {
        revision: SAFETY_REVISION,
        target_epoch: epoch,
        minimum_epoch: epoch,
        frame: 0,
        beat_q32: 0,
        loop_start_frame: 0,
        loop_end_frame: LOOP_END,
        loop_start_q32: 0,
        loop_end_q32: beat_to_q32(frame_as_beat_f64(LOOP_END)),
        loop_token: f.loop_token,
        loop_enabled: true,
        playing,
        mixer_pan_release: TimelineMixerPanRelease::EMPTY,
    }, request_id).unwrap()
}

fn assert_playing_activation_cannot_bypass_loop_guard(cancel_first: bool) {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    if cancel_first {
        f.engine.as_ref().unwrap().set_playing(false);
        f.pump(128);
    }
    let before = f.engine.as_ref().unwrap().snapshot();
    let rejected_request = queue_loop_activation(&mut f, 4, true);
    f.pump(128);
    let rejected = f.engine.as_ref().unwrap().snapshot();
    assert!(!rejected.transport_playing);
    assert!(!rejected.transport_loop_restart_pending);
    assert_eq!(rejected.transport_epoch, 3);
    assert_eq!(rejected.timeline_frame, 0);
    assert_eq!(rejected.plugin_epoch_resets, before.plugin_epoch_resets,
        "the rejected activation cannot request another SDK reset");
    assert!(f.callback().transport.loop_restart_requires_prepared_playback);
    assert!(f.callback().transport.loop_restart_safety_only);
    let mut rejection = None;
    while let Some(event) = f.engine.as_mut().unwrap().next_timeline_runtime_event() {
        if let TimelineRuntimeEvent::TransportActivationRejected { request_id, reason, .. } = event
            && request_id == rejected_request
        { rejection = Some(reason); }
    }
    assert_eq!(rejection, Some(TimelineTransportActivationRejectReason::PlaybackPreparationRequired));
    for _ in 0..f.callback().dsp.plugin_timing.lookahead_quanta + 4 { f.pump(128); }
    assert!(f.engine.as_ref().unwrap().snapshot().plugin_processing_fault.is_none());
    settle_loop_safety(&mut f);
    for _ in 0..4 { f.pump(128); }
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
}

#[test]
fn loop_restart_queued_playing_activation_cannot_bypass_held_reset() {
    assert_playing_activation_cannot_bypass_loop_guard(false);
}

#[test]
fn loop_restart_queued_playing_activation_cannot_revive_a_canceled_wait() {
    assert_playing_activation_cannot_bypass_loop_guard(true);
}

#[test]
fn loop_restart_explicit_stopped_activation_then_fresh_prepared_play_is_allowed() {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    let activated = queue_loop_activation(&mut f, 4, false);
    f.pump(128);
    let stopped = f.engine.as_ref().unwrap().snapshot();
    assert!(!stopped.transport_playing);
    assert!(!stopped.transport_loop_restart_pending);
    assert_eq!(stopped.transport_epoch, 4);
    assert_eq!(stopped.timeline_frame, 0);
    assert!(f.engine.as_ref().unwrap().prepared_playback_token().is_none());
    let mut applied = false;
    while let Some(event) = f.engine.as_mut().unwrap().next_timeline_runtime_event() {
        if let TimelineRuntimeEvent::TransportActivationApplied { request_id, epoch: 4, .. } = event
            && request_id == activated
        { applied = true; }
    }
    assert!(applied);
    for _ in 0..f.callback().dsp.plugin_timing.lookahead_quanta + 4 { f.pump(128); }
    assert!(f.engine.as_ref().unwrap().prepared_playback_token().is_none());
    for worker in &f.workers { worker.probe.reset_gate.release(); }
    f.settle(4);
    f.pump(128);
    let token = f.engine.as_ref().unwrap().prepared_playback_token().unwrap();
    assert_eq!(token.epoch, 4);
    let resets: Vec<_> = f.workers.iter().map(|w| w.control().stats().worker_epoch_resets).collect();
    assert!(f.engine.as_ref().unwrap().start_prepared_playback(token).is_some());
    f.pump(128);
    let playing = f.engine.as_ref().unwrap().snapshot();
    assert!(playing.transport_playing);
    assert_eq!(playing.transport_epoch, 4);
    assert_eq!(playing.timeline_frame, 128);
    assert_eq!(f.workers.iter().map(|w| w.control().stats().worker_epoch_resets).collect::<Vec<_>>(), resets);
}

#[test]
fn loop_restart_missing_prepared_identity_at_wrap_stays_stopped_after_commands_settle() {
    let mut f = StoppedSafetyFixture::new();
    f.activate_with_loop_at(2, false, LOOP_END, LOOP_END - 128);
    f.pump(128);
    f.settle(2);
    f.pump(128);
    let token = f.engine.as_ref().unwrap().prepared_playback_token().unwrap();
    for worker in &f.workers { worker.probe.reset_gate.hold(); }
    assert!(f.engine.as_ref().unwrap().start_prepared_playback(token).is_some());
    let status = Arc::clone(&f.engine.as_ref().unwrap().status);
    let mailbox = Arc::clone(&f.engine.as_ref().unwrap().transport_mailbox);
    let callback = f.callback_mut();
    // Worker-only admin work may become busy after the token observation.
    // The callback deliberately defers the same prepared request in that case;
    // retry that production admission without rendering or advancing the loop.
    wait_until(|| {
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
            callback.transport.apply_latest_request(&mailbox, &status, &mut callback.dsp);
        });
        callback.transport.request.playing
    });
    assert!(f.callback().transport.request.playing);
    let controller = f.engine.as_mut().unwrap().timeline_runtime.as_mut().unwrap();
    let chase = controller.prepare_loop_chase(&f.timeline, SAFETY_REVISION, 0,
        TimelineChaseOptions::default()).unwrap();
    controller.install_loop_chase(chase).unwrap();
    let callback = f.callback_mut();
    // Model a control command arriving between the callback's command boundary
    // and its Loop wrap. Render is the production callback segment kernel; no
    // test-side reset, safety submission or identity is manufactured.
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        render_transport_chunk(&mut callback.dsp, &status, &mailbox,
            &mut callback.transport, 128, |_, block| {
                assert!(block.iter().all(|frame| *frame == [0.0; 2]));
            });
    });
    let wrapped = f.engine.as_ref().unwrap().snapshot();
    assert_eq!(wrapped.transport_loop_count, 1);
    assert_eq!(wrapped.transport_epoch, 3);
    assert_eq!(wrapped.timeline_frame, 0);
    assert!(!wrapped.transport_playing);
    assert!(!wrapped.transport_loop_restart_pending,
        "pending timeline ownership prevents creating a complete prepared identity");
    assert!(f.callback().transport.loop_restart_requires_prepared_playback);
    f.pump(128);
    settle_loop_safety(&mut f);
    for _ in 0..4 { f.pump(128); }
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
}

#[test]
fn loop_restart_held_resets_exceed_k_without_music_time_or_additional_quanta() {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    for worker in &f.workers {
        worker.probe.reset_gate.wait_entered();
    }
    let before = f.engine.as_ref().unwrap().snapshot();
    let submitted: Vec<_> = f.workers.iter().map(|w| w.control().stats().submitted).collect();
    let plan = f.callback().dsp.plugin_timing;
    for _ in 0..plan.lookahead_quanta + 8 {
        f.pump(128);
        let waiting = f.engine.as_ref().unwrap().snapshot();
        assert_eq!(waiting.timeline_frame, 0);
        assert_eq!(waiting.beat_q32, 0);
        assert_eq!(waiting.transport_epoch, 3);
        assert_eq!(waiting.transport_loop_count, 1);
        assert!(waiting.transport_loop_restart_pending);
        assert!(!waiting.transport_playing);
        assert!(waiting.plugin_processing_fault.is_none());
    }
    assert!(f.engine.as_ref().unwrap().snapshot().device_frame > before.device_frame);
    assert_eq!(f.callback().dsp.plugin_timing, plan);
    assert_eq!(f.workers.iter().map(|w| w.control().stats().submitted).collect::<Vec<_>>(), submitted);

    f.workers[0].probe.reset_gate.release();
    wait_until(|| f.callback().genuine_snapshot().endpoints.iter()
            .find(|e| e.endpoint_id == SAFETY_GENERATOR_ENDPOINT).unwrap().input_work_quiescent(3));
    f.pump(128);
    assert!(f.engine.as_ref().unwrap().snapshot().transport_loop_restart_pending,
        "the independent insert is still resetting");
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);

    f.workers[1].probe.reset_gate.release();
    f.settle(3);
    let resets: Vec<_> = f.workers.iter().map(|w| w.control().stats().worker_epoch_resets).collect();
    f.pump(128);
    let resumed = f.engine.as_ref().unwrap().snapshot();
    assert!(resumed.transport_playing);
    assert!(!resumed.transport_loop_restart_pending);
    assert_eq!(resumed.transport_epoch, 3);
    assert_eq!(resumed.timeline_frame, 128);
    assert_eq!(resumed.transport_loop_count, 1);
    assert_eq!(f.workers.iter().map(|w| w.control().stats().worker_epoch_resets).collect::<Vec<_>>(), resets,
        "resuming cannot perform a second reset");
    for endpoint in &f.callback().genuine_snapshot().endpoints {
        assert_eq!(endpoint.fixed.bridge.next_sequence, 3, "music submitted sequence 2");
    }
}

#[test]
fn loop_restart_wrap_inside_raw_callback_services_only_the_remaining_safety_frames() {
    let mut f = begin_loop_wait(32, 128);
    let observed = f.callback().genuine_snapshot();
    for endpoint in &observed.endpoints {
        assert_eq!(endpoint.fixed.input_phase_frames, 96);
        assert_eq!(endpoint.stopped_epoch_safety_remaining_frames, 32);
        assert_eq!(endpoint.fixed.bridge.next_sequence, 1);
    }
    let segments = f.callback_mut().take_genuine_segments();
    assert!(segments.iter().any(|segment| segment.playing && segment.frames == 32 && segment.epoch == 2));
    assert!(segments.iter().any(|segment| !segment.playing && segment.frames == 96 && segment.epoch == 3));
    f.pump(32);
    for worker in &f.workers {
        worker.probe.reset_gate.wait_entered();
    }
    for endpoint in &f.callback().genuine_snapshot().endpoints {
        assert_eq!(endpoint.fixed.input_phase_frames, 0);
        assert_eq!(endpoint.fixed.bridge.next_sequence, 2, "one finite safety sequence 1");
    }
    for _ in 0..16 {
        f.pump(32);
    }
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
    assert!(f.engine.as_ref().unwrap().snapshot().plugin_processing_fault.is_none());
}

fn assert_mailbox_cancels_loop(mutation: usize) {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    let engine = f.engine.as_ref().unwrap();
    match mutation {
        0 => engine.set_playing(false),
        1 => { engine.stop_transport(); }
        2 => { engine.seek_transport_to_frame(1.0, 24_000); }
        3 => { engine.set_transport_loop_frames(0.0, 0, 1.0, 24_000, false); }
        4 => engine.set_playing(true),
        _ => unreachable!(),
    }
    f.pump(128);
    let canceled = f.engine.as_ref().unwrap().snapshot();
    assert!(!canceled.transport_loop_restart_pending);
    assert!(!canceled.transport_playing);
    assert_eq!(canceled.timeline_frame, 0);
    settle_loop_safety(&mut f);
    // Even another preserving mutation after cancellation still sees a mailbox
    // whose old accepted Play is true. It cannot downgrade the guarded restart.
    f.engine.as_ref().unwrap().set_transport_loop_frames(0.0, 0, 4.0, 96_000, false);
    for _ in 0..4 {
        f.pump(128);
    }
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
}

#[test]
fn loop_restart_pause_cancels_without_delayed_resurrection() { assert_mailbox_cancels_loop(0); }
#[test]
fn loop_restart_stop_cancels_without_delayed_resurrection() { assert_mailbox_cancels_loop(1); }
#[test]
fn loop_restart_seek_none_cannot_inherit_accepted_mailbox_play() { assert_mailbox_cancels_loop(2); }
#[test]
fn loop_restart_set_loop_cannot_inherit_accepted_mailbox_play() { assert_mailbox_cancels_loop(3); }
#[test]
fn loop_restart_plain_play_cannot_bypass_prepared_epoch_guard() { assert_mailbox_cancels_loop(4); }

#[test]
fn loop_restart_project_identity_change_cancels_even_without_a_mailbox_mutation() {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    assert!(f.engine.as_mut().unwrap().set_project_session(SAFETY_PROJECT + 1));
    f.pump(128);
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_loop_restart_pending);
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    settle_loop_safety(&mut f);
    for _ in 0..4 { f.pump(128); }
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
}

#[test]
fn loop_restart_canceled_safety_output_cannot_leak_into_live_midi_monitor() {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    for worker in &f.workers { worker.probe.reset_gate.wait_entered(); }
    // The real stopped safety Process will produce ±0.75. Only a later,
    // independently received Note On changes this mock to ±0.25.
    f.workers[0].probe.distinct_note_output.store(true, Ordering::Release);
    f.engine.as_ref().unwrap().set_playing(false);
    f.pump(128);
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_loop_restart_pending);
    assert!(f.callback().transport.loop_restart_safety_only);

    let (mut sender, receiver) = crate::midi_device::test_input_mailbox(8, 33);
    let route = PreparedMidiInputRoute::new(receiver, f.stamp()).unwrap();
    let engine = f.engine.as_mut().unwrap();
    assert!(engine.command(AudioCommand::SetMaster(1.0)));
    assert!(engine.command(AudioCommand::SetTrackGain { track: 1, gain: 1.0 }));
    engine.install_midi_input(5, route).unwrap();
    sender.send(1_000, &[0x90, 60, 100]);
    f.pump(128);
    assert!(matches!(f.engine.as_mut().unwrap().next_midi_input_route_event(),
        Some(MidiInputRouteEvent::Installed { route_id: 5, success: true, .. })));
    assert_eq!(f.workers[0].probe.note_output_quanta.load(Ordering::Acquire), 0,
        "a canceled loop still cannot monitor before the reset/safety terminal");
    settle_loop_safety(&mut f);
    assert_eq!(f.workers[0].probe.idle_output_quanta.load(Ordering::Acquire), 1);
    assert_eq!(f.workers[0].probe.note_output_quanta.load(Ordering::Acquire), 0);
    let before = f.engine.as_ref().unwrap().snapshot();
    let insert_submitted = f.workers[1].control().stats().submitted;
    let output_latency = usize::try_from(before.pdc_output_latency_samples).unwrap();
    let mut rendered = Vec::new();
    for _ in 0..output_latency.div_ceil(128) + 8 {
        pump_callback(&mut f, 128);
        rendered.extend_from_slice(f.callback().last_output());
        f.settle(3);
        let snapshot = f.engine.as_ref().unwrap().snapshot();
        assert!(!snapshot.transport_playing);
        assert!(!snapshot.transport_loop_restart_pending);
        assert_eq!(snapshot.transport_epoch, 3);
        assert_eq!(snapshot.timeline_frame, 0);
        assert_eq!(snapshot.pdc_plan_revision, before.pdc_plan_revision);
        assert!(snapshot.plugin_processing_fault.is_none());
    }
    let first_sound = rendered.iter().position(|frame| *frame != [0.0; 2]).unwrap();
    assert_eq!(first_sound, output_latency,
        "the old safety sequence 1 must stay silent until legal monitor sequence 2 arrives");
    let expected = [0.25_f32.tanh(), (-0.25_f32).tanh()];
    assert!(rendered[first_sound..].iter().all(|frame| *frame == expected),
        "only the distinct real MIDI-note output is audible; ±0.75 safety cannot leak");
    assert!(f.workers[0].probe.note_output_quanta.load(Ordering::Acquire) > 1);
    assert_eq!(f.workers[0].probe.idle_output_quanta.load(Ordering::Acquire), 1);
    assert_eq!(f.workers[1].control().stats().submitted, insert_submitted,
        "the unrelated insert receives no extra paused monitor Process");
    assert!(!f.callback().transport.loop_restart_safety_only);
    assert_eq!(f.callback().dsp.plugin_timing.revision, before.plugin_timing_revision);
}

#[test]
fn loop_restart_replaced_installed_loop_token_cancels_the_exact_wait() {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    let controller = f.engine.as_mut().unwrap().timeline_runtime.as_mut().unwrap();
    let replacement = controller.prepare_loop_chase(&f.timeline, SAFETY_REVISION, 0,
        TimelineChaseOptions::default()).unwrap();
    let replacement_token = controller.install_loop_chase(replacement).unwrap();
    assert_ne!(replacement_token, f.loop_token);
    f.pump(128);
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_loop_restart_pending);
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
    settle_loop_safety(&mut f);
    for _ in 0..4 { f.pump(128); }
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
    assert!(!f.engine.as_ref().unwrap().snapshot().transport_playing);
}

#[test]
fn loop_restart_admin_work_after_safety_settlement_blocks_resume() {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    settle_loop_safety(&mut f);
    f.workers[1].probe.admin_gate.hold();
    assert!(f.workers[1].control().request_state(0));
    f.workers[1].probe.admin_gate.wait_entered();
    for _ in 0..f.callback().dsp.plugin_timing.lookahead_quanta + 4 { f.pump(128); }
    assert!(f.engine.as_ref().unwrap().snapshot().transport_loop_restart_pending);
    assert!(f.engine.as_ref().unwrap().snapshot().plugin_processing_fault.is_none());
    f.workers[1].probe.admin_gate.release();
    wait_until(|| !f.workers[1].control().has_outstanding_admin_work());
    f.settle(3);
    f.pump(128);
    assert!(f.engine.as_ref().unwrap().snapshot().transport_playing);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 128);
}

#[test]
fn loop_restart_first_musical_sequence_still_obeys_the_original_k_deadline() {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    settle_loop_safety(&mut f);
    let plan = f.callback().dsp.plugin_timing;
    f.workers[0].probe.process_gate.hold();
    f.pump(128);
    f.workers[0].probe.process_gate.wait_entered();
    for _ in 0..plan.lookahead_quanta + 2 {
        pump_callback(&mut f, 128);
        if f.engine.as_ref().unwrap().snapshot().plugin_processing_fault.is_some() { break; }
    }
    let snapshot = f.engine.as_ref().unwrap().snapshot();
    let fault = snapshot.plugin_processing_fault.unwrap();
    assert_eq!(fault.reason, PluginProcessingFaultReason::DeadlineMiss);
    assert_eq!(fault.epoch, 3);
    assert_eq!(fault.expected_sequence, 2, "only safety sequence 1 was fenced");
    assert_eq!(f.callback().dsp.plugin_timing, plan);
    assert!(!snapshot.transport_playing);
    assert!(!snapshot.transport_loop_restart_pending);
}

#[test]
fn loop_restart_first_mock_sound_keeps_the_prepared_pdc_output_latency() {
    let mut f = begin_loop_wait(128, 128);
    f.pump(128);
    settle_loop_safety(&mut f);
    let before = f.engine.as_ref().unwrap().snapshot();
    let output_latency = usize::try_from(before.pdc_output_latency_samples).unwrap();
    let plan = f.callback().dsp.plugin_timing;
    assert!(output_latency >= plan.bridge_latency_frames as usize);
    let mut rendered = Vec::new();
    for _ in 0..output_latency.div_ceil(128) + 3 {
        pump_callback(&mut f, 128);
        rendered.extend_from_slice(f.callback().last_output());
        f.settle(3);
        let snapshot = f.engine.as_ref().unwrap().snapshot();
        assert!(snapshot.plugin_processing_fault.is_none());
        assert!(snapshot.transport_playing);
        assert!(!snapshot.transport_loop_restart_pending);
        assert_eq!(snapshot.transport_epoch, 3);
        assert_eq!(snapshot.pdc_plan_revision, before.pdc_plan_revision);
        assert_eq!(snapshot.pdc_output_latency_samples, before.pdc_output_latency_samples);
    }
    let first_sound = rendered.iter().position(|frame| *frame != [0.0; 2]).unwrap();
    assert_eq!(first_sound, output_latency,
        "discarded safety output cannot become sound or shift the prepared PDC path");
    assert_eq!(f.callback().dsp.plugin_timing, plan);
}

#[test]
fn loop_restart_waiting_silence_remains_in_realtime_master_capture() {
    let mut f = begin_loop_wait(128, 128);
    let target = master_capture_test_path("loop-restart-wait");
    let session = MasterCaptureSession::start_to_path(&target, 48_000).unwrap();
    let (endpoint, control) = session.into_parts();
    let capture_id = control.session_id();
    assert!(f.engine.as_mut().unwrap().install_master_capture(capture_id, endpoint));
    for _ in 0..8 { f.pump(128); }
    assert!(matches!(f.engine.as_mut().unwrap().next_master_capture_event().unwrap(),
        MasterCaptureEndpointEvent::Installed { success: true, .. }));
    let captured = control.status();
    assert_eq!(captured.timeline_frames, 1024);
    assert!(!captured.invalid);
    assert!(captured.processing_fault.is_none());
    assert_eq!(captured.source_gap_frames, 0);
    assert_eq!(f.engine.as_ref().unwrap().snapshot().timeline_frame, 0);
    assert!(f.engine.as_mut().unwrap().stop_master_capture(capture_id));
    f.pump(128);
    let endpoint = match f.engine.as_mut().unwrap().next_master_capture_event().unwrap() {
        MasterCaptureEndpointEvent::Stopped { returned_endpoint: Some(endpoint), .. } => endpoint,
        event => panic!("unexpected capture stop: {event:?}"),
    };
    let metadata = control.stop_blocking(endpoint).unwrap();
    assert_eq!(metadata.frames, 1024);
    assert!(metadata.is_valid());
    let bytes = std::fs::read(&target).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8, bytes.len());
    let mut offset = 12;
    let mut audio_bytes = None;
    while offset + 8 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let end = offset + 8 + length;
        assert!(end <= bytes.len(), "RIFF chunk is truncated");
        if &bytes[offset..offset + 4] == b"data" {
            assert!(audio_bytes.is_none(), "duplicate audio data chunk");
            audio_bytes = Some(&bytes[offset + 8..end]);
        }
        offset = end + (length & 1);
    }
    assert_eq!(offset, bytes.len());
    let audio_bytes = audio_bytes.unwrap();
    assert_eq!(audio_bytes.len(), metadata.frames as usize * 2 * 3);
    assert!(audio_bytes.iter().all(|byte| *byte == 0));
    std::fs::remove_file(target).unwrap();
}
