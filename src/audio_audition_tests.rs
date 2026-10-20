// Real queued commands, device-free production callback, and an independent
// recording instrument worker. No native plug-in or speaker acceptance claim.
use std::sync::Mutex;

fn native_fixture() -> (AudioEngine, TestAudioCallback) {
    let (mut engine, mut callback) = AudioEngine::test_engine_with_callback();
    assert!(engine.set_project_session(7));
    callback.pump(&engine, 128);
    (engine, callback)
}

fn native_destination(channel_id: u32, mixer_track: usize) -> AuditionDestination {
    AuditionDestination::Native { channel_id, mixer_track }
}

fn audible(output: &[[f32; 2]]) -> bool {
    output.iter().any(|sample| sample[0].abs() > 0.0001 || sample[1].abs() > 0.0001)
}

#[test]
fn owned_native_callback_sustains_without_os_midi_and_releases_only_its_voice() {
    let (mut engine, mut callback) = native_fixture();
    // A paused, unrelated timeline/native voice must not advance merely because
    // the keyboard becomes audible. Its track PDC history must freeze too.
    callback.dsp.voices[31] = Voice {
        sampler: None,
        phase: 0.23, phase_step: 0.01, envelope: 0.5, decay: 0.999,
        active: true, mixer_track: 3, timeline_note_id: Some(991),
        timeline_channel_id: Some(9), audition_token_id: None,
    };
    callback.dsp.pdc_raw_track_delays[3].process_sample([0.3; 2]);
    let frozen_phase = callback.dsp.voices[31].phase;
    let frozen_envelope = callback.dsp.voices[31].envelope;
    let frozen_delay = callback.dsp.pdc_raw_track_delays[3].test_history_at_tap(7);
    let token = engine.begin_audition(native_destination(7, 1), 60, 0.75).unwrap();
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Pending));
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Accepted));
    for _ in 0..400 {
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
    }
    assert!(audible(callback.last_output()), "a held key must survive the former quarter-beat tap duration");
    assert!(!engine.snapshot().transport_playing);
    assert_eq!(engine.snapshot().timeline_frame, 0);
    assert_eq!(callback.dsp.voices[31].phase, frozen_phase);
    assert_eq!(callback.dsp.voices[31].envelope, frozen_envelope);
    assert_eq!(callback.dsp.pdc_raw_track_delays[3].test_history_at_tap(7), frozen_delay);
    assert!(engine.end_audition(token));
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Released));
    assert!(audible(callback.last_output()), "pointer-up must retain the short oscillator release");
    for _ in 0..450 {
        if !callback.dsp.has_paused_owned_auditions() { break; }
        callback.pump(&engine, 128);
    }
    assert!(!audible(callback.last_output()));
    assert_eq!(callback.dsp.voices[31].phase, frozen_phase);
    assert!(callback.dsp.voices[31].active);
    assert!(!callback.dsp.has_paused_owned_auditions());
    for _ in 0..8 {
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
        assert_eq!(callback.dsp.voices[31].phase, frozen_phase,
            "retiring the last monitor cannot reopen paused Legacy rendering");
        assert_eq!(callback.dsp.voices[31].envelope, frozen_envelope);
        assert_eq!(callback.dsp.pdc_raw_track_delays[3].test_history_at_tap(7), frozen_delay);
        assert!(!audible(callback.last_output()));
    }
    engine.finish_audition_receipt(token);
    assert_eq!(engine.audition_status(token), None);
}

#[test]
fn owned_native_begin_and_end_are_bounded_and_old_allocators_do_not_steal_a_hold() {
    let (mut engine, mut callback) = native_fixture();
    let mut tokens = Vec::new();
    for pitch in 48..48 + MAX_OWNED_AUDITIONS as u8 {
        tokens.push(engine.begin_audition(native_destination(7, 1), pitch, 0.8).unwrap());
    }
    callback.pump(&engine, 128);
    for token in &tokens { assert_eq!(engine.audition_status(*token), Some(AuditionStatus::Accepted)); }
    let rejected = engine.begin_audition(native_destination(7, 1), 90, 0.8).unwrap();
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(rejected), Some(AuditionStatus::Rejected));
    // Exercise both production old allocators under pressure.
    for index in 0..96_u64 {
        assert!(engine.command(AudioCommand::NoteOn { note: 72, velocity: 0.3, mixer_track: 2 }));
        callback.pump(&engine, 1);
        callback.dsp.start_timeline_native_note(chased_note(10_000 + index, 9, 74));
    }
    for token in &tokens {
        assert!(callback.dsp.voices.iter().any(|voice| voice.active && voice.audition_token_id == Some(token.id) && voice.decay == 1.0));
    }
    let first = tokens[0];
    assert!(engine.end_audition(first));
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(first), Some(AuditionStatus::Released));
    for token in &tokens[1..] { assert_eq!(engine.audition_status(*token), Some(AuditionStatus::Accepted)); }
}

#[test]
fn owned_receipt_does_not_claim_callback_acceptance_and_full_ring_keeps_release_retryable() {
    let (mut engine, mut callback) = native_fixture();
    let token = engine.begin_audition(native_destination(7, 1), 60, 0.5).unwrap();
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Pending));
    callback.pump(&engine, 128);
    while engine.command(AudioCommand::SetTrackGain { track: 2, gain: 0.5 }) {}
    assert!(!engine.end_audition(token));
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Accepted));
    callback.pump(&engine, 128);
    assert!(engine.end_audition(token));
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Released));
}

#[test]
fn owned_native_project_epoch_and_transport_resets_cancel_exact_tokens() {
    let (mut engine, mut callback) = native_fixture();
    let token = engine.begin_audition(native_destination(7, 1), 60, 0.5).unwrap();
    callback.pump(&engine, 128);
    assert!(engine.set_project_session(8));
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Cancelled));
    let new_token = engine.begin_audition(native_destination(7, 1), 60, 0.5).unwrap();
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(new_token), Some(AuditionStatus::Accepted));
    assert!(engine.end_audition(token));
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(new_token), Some(AuditionStatus::Accepted));
    callback.dsp.apply_transport_epoch(&engine.status, 2, 0, false);
    assert_eq!(engine.audition_status(new_token), Some(AuditionStatus::Cancelled));
    assert!(callback.dsp.owned_auditions.iter().all(Option::is_none));
}

#[test]
fn missing_assigned_generator_rejects_callback_admission_without_native_fallback() {
    let (mut engine, mut callback) = native_fixture();
    let destination = AuditionDestination::Generator(MidiGeneratorRouteStamp {
        project_session: 7, channel_id: 7, endpoint_id: 70, plugin_instance_id: 700, slot: Some(0),
    });
    let token = engine.begin_audition(destination, 60, 0.5).unwrap();
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Pending));
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(token), Some(AuditionStatus::Rejected));
    assert!(!audible(callback.last_output()));
    assert!(callback.dsp.voices.iter().all(|voice| !voice.active));
    assert!(callback.dsp.owned_auditions.iter().all(Option::is_none));
}

struct RecordingInstrument {
    trace: Arc<Mutex<Vec<[u8; 3]>>>,
    held: [bool; 128],
    release: f32,
    declared_tail: u32,
    constant_release: bool,
}

impl PluginBackend for RecordingInstrument {
    fn name(&self) -> &str { "owned audition recording instrument" }
    fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> { Ok(()) }
    fn midi_capabilities(&self) -> (bool, bool) { (true, false) }
    fn process(&mut self, left: &mut [f32], right: &mut [f32], frames: usize) -> Result<(), String> {
        for index in 0..frames {
            let sample = if self.held.iter().any(|held| *held) { 0.15 } else {
                let sample = self.release;
                if !self.constant_release { self.release *= 0.995; }
                if self.release < 0.00001 { self.release = 0.0; }
                sample
            };
            left[index] += sample;
            right[index] += sample;
        }
        Ok(())
    }
    fn send_midi(&mut self, message: MidiMessage) -> Result<(), String> {
        self.trace.lock().unwrap().push(message.data);
        let pitch = usize::from(message.data[1]);
        match message.data[0] & 0xf0 {
            0x90 if pitch < 128 && message.data[2] != 0 => self.held[pitch] = true,
            0x80 | 0x90 if pitch < 128 => { self.held[pitch] = false; self.release = 0.15; }
            0xb0 if message.data[1] == 123 => { self.held.fill(false); self.release = 0.0; }
            _ => {}
        }
        Ok(())
    }
    fn set_parameter(&mut self, _id: u32, _normalized: f32) -> Result<(), String> { Ok(()) }
    fn get_parameter(&mut self, _id: u32) -> Result<f32, String> { Ok(0.0) }
    fn save_state(&mut self) -> Result<Vec<u8>, String> { Ok(Vec::new()) }
    fn load_state(&mut self, _state: &[u8]) -> Result<(), String> { Ok(()) }
    fn latency_samples(&self) -> u32 { 0 }
    fn tail_samples(&self) -> u32 { self.declared_tail }
}

fn recording_chain(instance_id: u64, trace: Arc<Mutex<Vec<[u8; 3]>>>) -> PluginChain {
    recording_chain_with_tail(instance_id, trace, 4096, false)
}

fn recording_chain_with_tail(instance_id: u64, trace: Arc<Mutex<Vec<[u8; 3]>>>, declared_tail: u32, constant_release: bool) -> PluginChain {
    let chain = PluginChain::spawn_identified_with_backend_factory(&[instance_id], move || {
        vec![BackendSlot::new(Box::new(RecordingInstrument {
            trace, held: [false; 128], release: 0.0, declared_tail, constant_release,
        }))]
    }, PluginPrepareConfig { sample_rate: 48_000.0, max_block_frames: 128 }).unwrap();
    wait_until(|| chain.control.plugin_latency_snapshot().is_some());
    chain
}

struct GeneratorFixture {
    callback: TestAudioCallback,
    engine: AudioEngine,
    control: PluginChainControl,
    guard: Option<PluginWorkerGuard>,
    trace: Arc<Mutex<Vec<[u8; 3]>>>,
}

impl GeneratorFixture {
    fn new() -> Self {
        Self::with_tail(4096, false)
    }
    fn with_tail(declared_tail: u32, constant_release: bool) -> Self {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let PluginChain { audio, control, guard } = recording_chain_with_tail(700, Arc::clone(&trace), declared_tail, constant_release);
        let (engine, mut callback) = native_fixture();
        let mut endpoint = fixed_adapter(audio);
        endpoint.project_session = 7;
        assert_eq!(endpoint.epoch(), 1);
        callback.dsp.generator_endpoints[0] = Some(GeneratorEndpointSlot {
            channel_id: 7, endpoint_id: 70, plugin_instance_id: 700, mixer_track: 1,
            endpoint, pdc_delay: test_pdc_delay(), pdc_initialized: false, suppress_output_frames: 0,
        });
        Self { callback, engine, control, guard: Some(guard), trace }
    }
    fn destination(&self) -> AuditionDestination {
        AuditionDestination::Generator(MidiGeneratorRouteStamp {
            project_session: 7, channel_id: 7, endpoint_id: 70, plugin_instance_id: 700, slot: Some(0),
        })
    }
    fn pump(&mut self) {
        self.pump_frames(128);
    }
    fn pump_frames(&mut self, frames: usize) {
        let submitted = self.control.stats().submitted;
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| self.callback.pump(&self.engine, frames));
        let after = self.control.stats().submitted;
        if after > submitted { wait_until(|| self.control.stats().completed >= after); }
    }
    fn count(&self, status: u8, pitch: u8) -> usize {
        self.trace.lock().unwrap().iter().filter(|midi| midi[0] & 0xf0 == status && midi[1] == pitch).count()
    }
    fn timeline(&mut self, note_on: bool) {
        let mut plan = TimelineRenderPlan::new();
        if note_on { plan.note_on(chased_note(991, 7, 60), 0); }
        else { plan.note_off(chased_note(991, 7, 60), 0); }
        let routes = timeline_generator_route_table(1, 1, 7, 700);
        assert!(DspState::stage_timeline_generator_plan_with_auditions(&mut plan,
            &mut self.callback.dsp.generator_endpoints, &mut self.callback.dsp.timeline_generator_notes,
            &self.callback.dsp.owned_auditions, &routes, 1, 1));
    }
}

impl Drop for GeneratorFixture {
    fn drop(&mut self) {
        drop(self.callback.dsp.generator_endpoints[0].take());
        self.engine.reclaim_retired_insert_endpoints();
        if let Some(guard) = self.guard.take() { guard.shutdown(); }
    }
}

#[test]
fn owned_generator_paused_callback_outputs_long_hold_and_real_release_tail() {
    let mut fixture = GeneratorFixture::new();
    let token = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
    assert_eq!(fixture.engine.audition_status(token), Some(AuditionStatus::Pending));
    for _ in 0..180 { fixture.pump(); }
    assert_eq!(fixture.engine.audition_status(token), Some(AuditionStatus::Accepted));
    assert!(audible(fixture.callback.last_output()));
    assert_eq!(fixture.count(0x90, 60), 1);
    assert_eq!(fixture.engine.snapshot().timeline_frame, 0);
    assert!(fixture.engine.end_audition(token));
    fixture.pump();
    assert_eq!(fixture.engine.audition_status(token), Some(AuditionStatus::Released));
    assert!(audible(fixture.callback.last_output()));
    for _ in 0..450 { fixture.pump(); }
    assert_eq!(fixture.count(0x80, 60), 1);
    assert_eq!(fixture.count(0xb0, 123), 0, "owned release must not panic the channel");
    assert!(!audible(fixture.callback.last_output()));
    assert!(!fixture.callback.dsp.has_paused_owned_auditions());
}

#[test]
fn owned_generator_and_timeline_same_pitch_keep_the_other_owner_in_both_off_orders() {
    for timeline_first in [true, false] {
        let mut fixture = GeneratorFixture::new();
        if timeline_first { fixture.timeline(true); }
        let token = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
        fixture.pump();
        if !timeline_first { fixture.timeline(true); fixture.pump(); }
        assert_eq!(fixture.count(0x90, 60), 1, "each physical pitch starts once");
        if timeline_first {
            fixture.timeline(false);
            fixture.pump();
            assert_eq!(fixture.count(0x80, 60), 0, "Timeline Off must keep the held owner");
            assert!(fixture.engine.end_audition(token));
        } else {
            assert!(fixture.engine.end_audition(token));
            fixture.pump();
            assert_eq!(fixture.count(0x80, 60), 0, "held Off must keep the Timeline owner");
            fixture.timeline(false);
        }
        for _ in 0..12 { fixture.pump(); }
        assert_eq!(fixture.count(0x80, 60), 1, "only the last owner releases the pitch");
        assert_eq!(fixture.count(0xb0, 123), 0);
    }
}

#[test]
fn owned_generator_live_lane_full_rejects_begin_without_panic_and_retries_end() {
    let mut fixture = GeneratorFixture::new();
    let held = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
    for _ in 0..12 { fixture.pump(); }
    fixture.timeline(true);
    // Real bounded staging, not the old stage() failure/CC123 path.
    let endpoint = &mut fixture.callback.dsp.generator_endpoints[0].as_mut().unwrap().endpoint;
    let events = [FrameEvent::midi(0, Some(0), [0xb0, 1, 0]); 16];
    let classes = [EndpointEventClass::Live; 16];
    assert!(endpoint.can_stage_batch(&events, &classes));
    endpoint.stage_preflighted_batch(&events, &classes);
    let rejected = fixture.engine.begin_audition(fixture.destination(), 62, 0.5).unwrap();
    fixture.pump();
    assert_eq!(fixture.engine.audition_status(rejected), Some(AuditionStatus::Rejected));
    assert_eq!(fixture.engine.audition_status(held), Some(AuditionStatus::Accepted));
    assert_eq!(fixture.count(0x90, 62), 0);
    assert_eq!(fixture.count(0xb0, 123), 0);
    fixture.timeline(false);
    // The consumed half-Q reservation remains full until the real adapter drains.
    let endpoint = &mut fixture.callback.dsp.generator_endpoints[0].as_mut().unwrap().endpoint;
    assert!(endpoint.can_stage_batch(&events, &classes));
    endpoint.stage_preflighted_batch(&events, &classes);
    let before = fixture.control.stats().submitted;
    fixture.callback.pump(&fixture.engine, 64);
    let after = fixture.control.stats().submitted;
    if after > before { wait_until(|| fixture.control.stats().completed >= after); }
    assert_eq!(fixture.callback.dsp.generator_endpoints[0].as_ref().unwrap().endpoint.input_phase_frames(), 64);
    assert!(fixture.engine.end_audition(held));
    fixture.callback.pump(&fixture.engine, 1);
    assert_eq!(fixture.engine.audition_status(held), Some(AuditionStatus::ReleasePending));
    for _ in 0..12 { fixture.pump(); }
    assert_eq!(fixture.engine.audition_status(held), Some(AuditionStatus::Released));
    assert_eq!(fixture.count(0x80, 60), 1);
    assert_eq!(fixture.count(0xb0, 123), 0);
}

#[test]
fn old_timed_preview_off_cannot_cut_a_new_owned_generator_hold() {
    let mut fixture = GeneratorFixture::new();
    let held = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
    for _ in 0..12 { fixture.pump(); }
    assert!(fixture.engine.send_generator_midi(7, Some(0), [0x80, 60, 0], 0));
    fixture.pump();
    assert_eq!(fixture.count(0x80, 60), 0);
    assert_eq!(fixture.engine.audition_status(held), Some(AuditionStatus::Accepted));
    assert!(fixture.engine.end_audition(held));
    for _ in 0..12 { fixture.pump(); }
    assert_eq!(fixture.count(0x80, 60), 1);
}

#[test]
fn owned_short_preview_release_keeps_a_same_pitch_held_generator_voice() {
    let mut fixture = GeneratorFixture::new();
    let held = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
    for _ in 0..12 { fixture.pump(); }
    let mut destination = fixture.destination();
    // Broadcast and slot 0 reach the same single instrument. Ownership must
    // follow the physical endpoint/instance, not this addressing spelling.
    if let AuditionDestination::Generator(route) = &mut destination { route.slot = None; }
    let preview = fixture.engine.begin_audition(destination, 60, 0.75).unwrap();
    fixture.pump();
    assert_eq!(fixture.count(0x90, 60), 1);
    assert!(fixture.engine.end_audition(preview));
    for _ in 0..12 { fixture.pump(); }
    assert_eq!(fixture.engine.audition_status(preview), Some(AuditionStatus::Released));
    assert_eq!(fixture.engine.audition_status(held), Some(AuditionStatus::Accepted));
    assert_eq!(fixture.count(0x80, 60), 0);
    assert!(audible(fixture.callback.last_output()));
    assert!(fixture.engine.end_audition(held));
    for _ in 0..12 { fixture.pump(); }
    assert_eq!(fixture.count(0x80, 60), 1);
}

#[test]
fn owned_generator_pause_with_full_live_lane_cancels_physically_and_next_press_works_in_same_epoch() {
    let mut fixture = GeneratorFixture::new();
    let token = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
    for _ in 0..12 { fixture.pump(); }
    let endpoint = &mut fixture.callback.dsp.generator_endpoints[0].as_mut().unwrap().endpoint;
    let events = [FrameEvent::midi(0, Some(0), [0xb0, 1, 0]); 16];
    let classes = [EndpointEventClass::Live; 16];
    assert!(endpoint.can_stage_batch(&events, &classes));
    endpoint.stage_preflighted_batch(&events, &classes);
    fixture.callback.dsp.pause_timeline();
    assert_eq!(fixture.engine.audition_status(token), Some(AuditionStatus::ReleasePending));
    for _ in 0..12 { fixture.pump(); }
    assert_eq!(fixture.engine.audition_status(token), Some(AuditionStatus::Cancelled));
    assert_eq!(fixture.count(0x80, 60), 1);
    assert_eq!(fixture.count(0xb0, 123), 0);
    let next = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
    assert_eq!(next.transport_epoch, token.transport_epoch);
    for _ in 0..12 { fixture.pump(); }
    assert_eq!(fixture.engine.audition_status(next), Some(AuditionStatus::Accepted));
    assert_eq!(fixture.count(0x90, 60), 2);
    assert!(audible(fixture.callback.last_output()), "cancellation cannot fence a subsequent physical press until another epoch");
}

#[test]
fn owned_generator_remove_and_replace_cancel_old_token_without_ghost_off() {
    let mut fixture = GeneratorFixture::new();
    let old = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
    for _ in 0..12 { fixture.pump(); }
    assert!(fixture.engine.remove_generator_endpoint(7));
    fixture.pump();
    assert_eq!(fixture.engine.audition_status(old), Some(AuditionStatus::Cancelled));
    fixture.engine.reclaim_retired_insert_endpoints();
    let new_trace = Arc::new(Mutex::new(Vec::new()));
    let PluginChain { audio, control, guard } = recording_chain(701, Arc::clone(&new_trace));
    fixture.guard.take().unwrap().shutdown();
    fixture.control = control;
    fixture.guard = Some(guard);
    assert!(fixture.engine.install_generator_endpoint_for_project(7, 7, 71, 701, 1, audio));
    fixture.pump();
    assert!(fixture.engine.end_audition(old));
    for _ in 0..8 { fixture.pump(); }
    assert!(!new_trace.lock().unwrap().iter().any(|midi| midi[0] & 0xf0 == 0x80 && midi[1] == 60));
    drop(fixture.callback.dsp.generator_endpoints[0].take());
    fixture.engine.reclaim_retired_insert_endpoints();
}

#[test]
fn owned_callback_fault_retires_receipt_and_suspended_begin_is_rejected() {
    let (mut engine, mut callback) = native_fixture();
    let held = engine.begin_audition(native_destination(7, 1), 60, 0.5).unwrap();
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(held), Some(AuditionStatus::Accepted));
    callback.dsp.latch_plugin_processing_fault(70, 1, PluginProcessingFaultReason::WorkerFault);
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
    assert_eq!(engine.audition_status(held), Some(AuditionStatus::Cancelled));
    assert!(!callback.dsp.has_paused_owned_auditions());
    assert!(!audible(callback.last_output()));

    let (mut engine, mut callback) = native_fixture();
    // A graph activation that has not installed channel identity is genuinely
    // suspended, separately from the existing topology-replan admission gate.
    callback.dsp.mixer_graph_was_activated = true;
    callback.dsp.timeline_channel_revision = None;
    let rejected = engine.begin_audition(native_destination(7, 1), 60, 0.5).unwrap();
    callback.pump(&engine, 128);
    assert_eq!(engine.audition_status(rejected), Some(AuditionStatus::Rejected));
    assert!(callback.dsp.voices.iter().all(|voice| !voice.active));
}

struct DelayedAuditionEffect {
    history: Vec<[f32; 2]>,
    cursor: usize,
}

impl PluginBackend for DelayedAuditionEffect {
    fn name(&self) -> &str { "one-and-a-half-second audition delay" }
    fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> { Ok(()) }
    fn midi_capabilities(&self) -> (bool, bool) { (false, false) }
    fn process(&mut self, left: &mut [f32], right: &mut [f32], frames: usize) -> Result<(), String> {
        for index in 0..frames {
            let input = [left[index], right[index]];
            let output = self.history[self.cursor];
            self.history[self.cursor] = input;
            self.cursor = (self.cursor + 1) % self.history.len();
            left[index] = output[0];
            right[index] = output[1];
        }
        Ok(())
    }
    fn send_midi(&mut self, _message: MidiMessage) -> Result<(), String> { Ok(()) }
    fn set_parameter(&mut self, _id: u32, _normalized: f32) -> Result<(), String> { Ok(()) }
    fn get_parameter(&mut self, _id: u32) -> Result<f32, String> { Ok(0.0) }
    fn save_state(&mut self) -> Result<Vec<u8>, String> { Ok(Vec::new()) }
    fn load_state(&mut self, _state: &[u8]) -> Result<(), String> { Ok(()) }
    fn latency_samples(&self) -> u32 { 0 }
    fn tail_samples(&self) -> u32 { 72_000 }
}

#[test]
fn owned_paused_native_release_drains_a_delayed_effect_after_more_than_one_second_of_silence() {
    let PluginChain { audio, control, guard } = PluginChain::spawn_identified_with_backend_factory(
        &[800], || vec![BackendSlot::new(Box::new(DelayedAuditionEffect {
            history: vec![[0.0; 2]; 72_000], cursor: 0,
        }))], PluginPrepareConfig { sample_rate: 48_000.0, max_block_frames: 128 },
    ).unwrap();
    wait_until(|| control.plugin_latency_snapshot().is_some());
    let (mut engine, mut callback) = native_fixture();
    let mut endpoint = fixed_adapter(audio);
    endpoint.project_session = 7;
    callback.dsp.insert_endpoints[1] = Some(InsertEndpointSlot {
        endpoint_id: 80, endpoint, suppress_output_frames: 0,
    });
    let held = engine.begin_audition(native_destination(7, 1), 60, 0.75).unwrap();
    let mut audible_after_silent_gap = false;
    let mut monitored_past_one_second = false;
    let mut elapsed = 0_u64;
    for block in 0..1_300 {
        if block == 4 { assert!(engine.end_audition(held)); }
        let submitted = control.stats().submitted;
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
        let after = control.stats().submitted;
        if after > submitted { wait_until(|| control.stats().completed >= after); }
        elapsed += 128;
        if (50_000..60_000).contains(&elapsed) {
            assert!(!audible(callback.last_output()), "the delay still holds future audio");
            monitored_past_one_second |= callback.dsp.has_paused_owned_auditions();
        }
        if elapsed > 72_000 { audible_after_silent_gap |= audible(callback.last_output()); }
        assert_eq!(engine.snapshot().timeline_frame, 0);
    }
    assert!(monitored_past_one_second, "source silence must not prematurely stop its downstream delay");
    assert!(audible_after_silent_gap, "the effect's real delayed release must reach the callback Master");
    assert_eq!(engine.audition_status(held), Some(AuditionStatus::Released));
    assert_eq!(engine.audition_tail_limit_reached(), 0);
    assert!(!callback.dsp.has_paused_owned_auditions());
    drop(callback.dsp.insert_endpoints[1].take());
    guard.shutdown();
}

#[test]
fn owned_unknown_continuous_release_has_a_reported_finite_monitor_limit() {
    let mut fixture = GeneratorFixture::with_tail(u32::MAX, true);
    // Exercise thirty actual callback seconds at the smallest common PCM rate;
    // no device is opened and the test does not make a plug-in fidelity claim.
    fixture.callback.dsp.sample_rate = 8_000.0;
    let held = fixture.engine.begin_audition(fixture.destination(), 60, 0.5).unwrap();
    for _ in 0..12 { fixture.pump(); }
    assert!(fixture.engine.end_audition(held));
    // Offset release from the Q128/device boundary so the real callback must
    // preserve the final authorized prefix and silence its remaining samples.
    fixture.pump_frames(64);
    for _ in 0..1_874 { fixture.pump(); }
    assert!(fixture.callback.dsp.has_paused_owned_auditions());
    assert_eq!(fixture.engine.audition_tail_limit_reached(), 0);
    assert!(audible(fixture.callback.last_output()));
    fixture.pump();
    assert!(!fixture.callback.dsp.has_paused_owned_auditions());
    assert_eq!(fixture.engine.audition_tail_limit_reached(), 1,
        "bounded monitoring cannot silently impersonate a complete infinite tail");
    assert!(audible(&fixture.callback.last_output()[..64]));
    assert!(fixture.callback.last_output()[64..].iter().all(|frame| *frame == [0.0; 2]),
        "the sample prefix after the absolute deadline must already be silent before the limit receipt is published");
    for _ in 0..8 { fixture.pump(); }
    assert_eq!(fixture.engine.audition_tail_limit_reached(), 1);
    assert!(!audible(fixture.callback.last_output()));
    let frozen_submissions = fixture.control.stats().submitted;
    for _ in 0..8 {
        fixture.pump();
        assert!(!audible(fixture.callback.last_output()),
            "an expired monitor cannot fall through to Legacy generator audio");
    }
    assert_eq!(fixture.control.stats().submitted, frozen_submissions,
        "the expired generator itself must stay frozen after its monitor deadline");
    assert_eq!(fixture.count(0x80, 60), 1);
    assert_eq!(fixture.count(0xb0, 123), 0);
    let next = fixture.engine.begin_audition(fixture.destination(), 62, 0.5).unwrap();
    for _ in 0..12 { fixture.pump(); }
    assert_eq!(fixture.engine.audition_status(next), Some(AuditionStatus::Accepted));
    assert!(audible(fixture.callback.last_output()), "a fresh owned press is still authorized after a tail deadline");
    assert_eq!(fixture.engine.audition_tail_limit_reached(), 1);
    assert!(fixture.engine.end_audition(next));
    for _ in 0..8 { fixture.pump(); }
}
