//! Real fake-worker receipts through App polling and its history observer. No device or DLL.

use super::*;
use plugins::plugin_runtime::{BackendSlot, PluginBackend, PluginChain, PluginPrepareConfig};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

struct NormalizingStateBackend {
    inner: ConfigCaptureBackend,
    saves: Arc<AtomicUsize>,
    queries: Arc<AtomicUsize>,
    generation: u64,
}

impl PluginBackend for NormalizingStateBackend {
    fn name(&self) -> &str {
        "normalized state history fixture"
    }
    fn prepare(&mut self, config: PluginPrepareConfig) -> Result<(), String> {
        self.inner.prepare(config)
    }
    fn process(&mut self, l: &mut [f32], r: &mut [f32], n: usize) -> Result<(), String> {
        self.inner.process(l, r, n)
    }
    fn send_midi(&mut self, message: plugins::plugin_runtime::MidiMessage) -> Result<(), String> {
        self.inner.send_midi(message)
    }
    fn set_parameter(&mut self, id: u32, value: f32) -> Result<(), String> {
        self.inner.set_parameter(id, value)
    }
    fn get_parameter(&mut self, id: u32) -> Result<f32, String> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        self.inner.get_parameter(id)
    }
    fn parameter_catalog_snapshot(
        &mut self,
    ) -> Result<plugins::plugin_runtime::PluginParameterCatalogPage, String> {
        self.inner.parameter_catalog_snapshot()
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        let mut bytes = self.inner.save_state()?;
        // Identical sound and parameter value, but a normalization byte differs from the
        // authoritative loaded blob, as real SDK state round-trips can do.
        let serial = self.saves.fetch_add(1, Ordering::SeqCst);
        bytes.push(0xa5_u8.wrapping_add(serial as u8));
        Ok(bytes)
    }
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.inner
            .load_state(bytes.get(..4).ok_or("invalid fixture state")?)
    }
    fn native_editor(
        &mut self,
        command: NativeEditorCommand,
    ) -> Result<plugins::plugin_runtime::NativeEditorState, String> {
        if matches!(command, NativeEditorCommand::Open { .. }) {
            self.generation = self.generation.saturating_add(1);
        }
        let mut state = self.inner.native_editor(command)?;
        state.generation = self.generation;
        Ok(state)
    }
    fn native_editor_feedback(
        &mut self,
    ) -> Result<plugins::plugin_runtime::NativeEditorFeedback, String> {
        let mut feedback = self.inner.native_editor_feedback()?;
        feedback.state.generation = self.generation;
        Ok(feedback)
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

struct StateHistoryWorker {
    _audio: plugins::plugin_runtime::AudioThreadEndpoint,
    saves: Arc<AtomicUsize>,
    queries: Arc<AtomicUsize>,
    value: Arc<AtomicU32>,
    dirty: Arc<AtomicU64>,
    channel_id: u32,
}

fn attach_state_history_worker(
    app: &mut CitrusApp,
    instance_id: u64,
    endpoint_id: u64,
    native_open_before_ready: bool,
) -> StateHistoryWorker {
    // The Project's persisted .25 base must still replay despite skipping state readback.
    let value = Arc::new(AtomicU32::new(0.1f32.to_bits()));
    let dirty = Arc::new(AtomicU64::new(0));
    let saves = Arc::new(AtomicUsize::new(0));
    let queries = Arc::new(AtomicUsize::new(0));
    let backend_value = value.clone();
    let backend_dirty = dirty.clone();
    let backend_saves = saves.clone();
    let backend_queries = queries.clone();
    let PluginChain {
        audio,
        control,
        guard,
    } = PluginChain::spawn_identified_with_backend_factory(
        &[instance_id],
        move || {
            vec![BackendSlot::new(Box::new(NormalizingStateBackend {
                inner: ConfigCaptureBackend {
                    value: backend_value,
                    dirty: backend_dirty,
                    open: false,
                    note_on: false,
                    fail_metadata: false,
                },
                saves: backend_saves,
                queries: backend_queries,
                generation: 0,
            }))]
        },
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 128,
        },
    )
    .unwrap();
    if native_open_before_ready {
        assert!(control.request_native_editor(
            0,
            700,
            NativeEditorCommand::Open { owner: None },
            &[0]
        ));
        wait_config_condition(|| {
            control
                .native_editor_snapshot(0)
                .is_some_and(|s| s.state.open && s.pending_request.is_none())
        });
    }
    let channel_id = app.project.channels[0].id;
    app.running_generator_chains.insert(
        channel_id,
        RunningGeneratorChain {
            project_session: app.project_session,
            endpoint_id,
            instance_id,
            mixer_track: 1,
            control,
            guard,
        },
    );
    StateHistoryWorker {
        _audio: audio,
        saves,
        queries,
        value,
        dirty,
        channel_id,
    }
}

fn consume_state_history_capture(app: &mut CitrusApp, worker: &StateHistoryWorker, count: usize) {
    wait_config_condition(|| {
        app.poll_plugin_runtime_events();
        app.project.plugin_instances[0].runtime_status == PluginRuntimeStatus::Loaded
    });
    // This actual FIFO query sits after Ready's base replay and any state request.
    // Completion proves startup work has run, rather than asserting no capture after a delay.
    let previous_queries = worker.queries.load(Ordering::SeqCst);
    assert!(
        app.running_generator_chains[&worker.channel_id]
            .control
            .query_parameter_tagged(0, 0, 799)
    );
    wait_config_condition(|| worker.queries.load(Ordering::SeqCst) > previous_queries);
    // Independent native polling runs BEFORE draining the state-event queue, exercising
    // the worker's native-publication-before-State ordering without a scheduling gamble.
    app.poll_native_editor_snapshots();
    app.poll_plugin_runtime_events();
    assert_eq!(worker.saves.load(Ordering::SeqCst), count);
    app.last_history_check = Instant::now() - Duration::from_millis(100);
    app.capture_history();
}

fn state_history_fixture(empty: bool) -> (UiHarness, u64) {
    let mut ui = UiHarness::floating();
    piano_clipboard_fixture(&mut ui);
    ui.app.project.active_pattern_mut().notes.truncate(1);
    let note = &mut ui.app.project.active_pattern_mut().notes[0];
    note.start = 15.0;
    note.length = 1.0;
    note.group_id = None;
    let id = config_capture_plugin(
        &mut ui.app.project,
        PluginPickerTarget::ChannelDevice { channel: 0 },
        50,
    );
    ui.app.project.plugin_instances[0]
        .parameters
        .insert(0, 0.25);
    if !empty {
        ui.app.project.plugin_instances[0].opaque_state = 0.25f32.to_le_bytes().to_vec();
    }
    ui.app.sync_history_observer();
    ui.app.undo_stack.clear();
    ui.app.redo_stack.clear();
    ui.app.dirty = false;
    ui.settle();
    (ui, id)
}

#[test]
fn startup_state_history_restored_echo_preserves_note_undo_and_redo() {
    let (mut ui, id) = state_history_fixture(false);
    ui.app.piano_viewport.x.reveal(14.0, 18.0, 0.0).unwrap();
    ui.settle();
    let original = project_fingerprint(&ui.app.project);
    let resize = ui
        .ctx
        .read_response(Id::new(("piano-note", 90_001_u64)).with("resize"))
        .unwrap()
        .rect
        .center();
    let delta = ui.app.piano_viewport.x.pixels_per_unit() as f32;
    ui.drag_pointer(resize, Vec2::new(delta, 0.0));
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 32.0);
    let edited = project_fingerprint(&ui.app.project);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 16.0);
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert_eq!((ui.app.undo_stack.len(), ui.app.redo_stack.len()), (0, 1));
    let worker = attach_state_history_worker(&mut ui.app, id, 901, false);
    consume_state_history_capture(&mut ui.app, &worker, 0);
    assert_eq!(f32::from_bits(worker.value.load(Ordering::SeqCst)), 0.25);
    assert_eq!(
        project_fingerprint(&ui.app.project),
        original,
        "a restored SDK echo cannot become an edit"
    );
    assert_eq!((ui.app.undo_stack.len(), ui.app.redo_stack.len()), (0, 1));
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 32.0);
    assert_eq!(project_fingerprint(&ui.app.project), edited);
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 16.0);
    assert_eq!(project_fingerprint(&ui.app.project), original);
}

#[test]
fn startup_state_history_new_plugin_adopts_initial_state() {
    let (mut ui, id) = state_history_fixture(true);
    let before = project_fingerprint(&ui.app.project);
    let worker = attach_state_history_worker(&mut ui.app, id, 902, false);
    consume_state_history_capture(&mut ui.app, &worker, 1);
    let mut normalized = 0.25f32.to_le_bytes().to_vec();
    normalized.push(0xa5);
    assert_eq!(ui.app.project.plugin_instances[0].opaque_state, normalized);
    assert_ne!(project_fingerprint(&ui.app.project), before);
    assert!(ui.app.dirty);
    assert_eq!(ui.app.undo_stack.len(), 1);
}

#[test]
fn startup_state_history_explicit_save_and_native_edits_remain_authoritative() {
    let (mut ui, id) = state_history_fixture(false);
    let worker = attach_state_history_worker(&mut ui.app, id, 903, false);
    consume_state_history_capture(&mut ui.app, &worker, 0);
    let request_id = ui.app.allocate_save_state_request_id();
    worker.value.store(0.5f32.to_bits(), Ordering::SeqCst);
    assert!(
        ui.app.running_generator_chains[&worker.channel_id]
            .control
            .request_state_tagged(0, request_id)
    );
    consume_state_history_capture(&mut ui.app, &worker, 1);
    assert_eq!(
        ui.app.project.plugin_instances[0].opaque_state[..4],
        0.5f32.to_le_bytes()
    );
    let control = &ui.app.running_generator_chains[&worker.channel_id].control;
    assert!(control.request_native_editor(0, 701, NativeEditorCommand::Open { owner: None }, &[0]));
    wait_config_condition(|| {
        control
            .native_editor_snapshot(0)
            .is_some_and(|s| s.state.open && s.pending_request.is_none())
    });
    worker.value.store(0.73f32.to_bits(), Ordering::SeqCst);
    worker.dirty.store(1, Ordering::SeqCst);
    assert!(control.request_native_editor(0, 702, NativeEditorCommand::Close, &[0]));
    wait_config_condition(|| {
        control.native_editor_snapshot(0).is_some_and(|s| {
            s.dirty_revision == 1 && s.captured_dirty_revision == 1 && s.pending_request.is_none()
        })
    });
    ui.app.poll_plugin_runtime_events();
    ui.app.last_history_check = Instant::now() - Duration::from_millis(100);
    ui.app.capture_history();
    assert_eq!(
        ui.app.project.plugin_instances[0].opaque_state[..4],
        0.73f32.to_le_bytes()
    );
    assert_eq!(ui.app.project.plugin_instances[0].parameters[&0], 0.73);
    assert!(ui.app.dirty);
}

#[test]
fn startup_state_history_clean_native_startup_echo_does_not_dirty_restored_project() {
    let (mut ui, id) = state_history_fixture(false);
    let before = project_fingerprint(&ui.app.project);
    let worker = attach_state_history_worker(&mut ui.app, id, 904, true);
    consume_state_history_capture(&mut ui.app, &worker, 0);
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn startup_state_history_audio_restart_retains_nonempty_request_zero_verification() {
    let (mut ui, id) = state_history_fixture(false);
    let before = project_fingerprint(&ui.app.project);
    ui.app.restart_initial_state_waiting.insert(id);
    let worker = attach_state_history_worker(&mut ui.app, id, 905, false);
    consume_state_history_capture(&mut ui.app, &worker, 1);
    assert!(
        ui.app.restart_initial_state_waiting.is_empty(),
        "the actual request-zero State receipt must clear restart verification"
    );
    assert_eq!(project_fingerprint(&ui.app.project), before);
    assert_eq!(f32::from_bits(worker.value.load(Ordering::SeqCst)), 0.25);
    assert!(!ui.app.dirty);
    assert!(ui.app.undo_stack.is_empty());
}

#[test]
fn startup_state_history_autosave_normalization_preserves_one_step_piano_undo_redo() {
    let files = Fixture::new();
    let (mut ui, id) = state_history_fixture(false);
    let worker = attach_state_history_worker(&mut ui.app, id, 906, false);
    consume_state_history_capture(&mut ui.app, &worker, 0);
    ui.app.piano_viewport.x.reveal(14.0, 18.0, 0.0).unwrap();
    ui.settle();
    let resize = ui
        .ctx
        .read_response(Id::new(("piano-note", 90_001_u64)).with("resize"))
        .unwrap()
        .rect
        .center();
    let delta = ui.app.piano_viewport.x.pixels_per_unit() as f32;
    ui.drag_pointer(resize, Vec2::new(delta, 0.0));
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 32.0);
    assert_eq!(ui.app.undo_stack.len(), 1);

    let recovery = files.0.join("normalized-state-only-recovery.citrus");
    ui.app.autosave_path = Some(recovery.clone());
    // Advance the production cadence and observer coalescing clocks, without sleeping
    // for thirty seconds. The real autosave/barrier/worker/filesystem paths remain used.
    ui.app.last_autosave = Instant::now() - AUTOSAVE_INTERVAL - Duration::from_secs(1);
    ui.app.last_history_capture = Instant::now() - Duration::from_secs(1);
    ui.app.update_project_state();
    assert!(ui.app.save_barrier.is_some());
    wait_config_condition(|| {
        ui.app.poll_save_barrier();
        ui.app.poll_plugin_runtime_events();
        ui.app.poll_save_barrier();
        ui.app.save_barrier.is_none()
    });
    assert_eq!(worker.saves.load(Ordering::SeqCst), 1);
    let saved = Project::load(&recovery).unwrap();
    let mut normalized = 0.25f32.to_le_bytes().to_vec();
    normalized.push(0xa5);
    assert_eq!(saved.plugin_instances[0].opaque_state, normalized);
    assert_eq!(ui.app.project.plugin_instances[0].opaque_state, normalized);
    assert_eq!(saved.active_pattern().notes[0].length, 2.0);
    ui.app.last_history_check = Instant::now() - Duration::from_millis(100);
    ui.app.capture_history();
    ui.key(egui::Key::Z, piano_clipboard_command());
    assert_eq!(
        pattern_playback_length_beats(&ui.app.project),
        16.0,
        "automatic normalized SaveState must not insert a state-only Undo above the note resize"
    );
    assert_eq!(ui.app.project.active_pattern().notes[0].length, 1.0);
    assert_eq!(ui.app.redo_stack.len(), 1);
    ui.key(egui::Key::Y, piano_clipboard_command());
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 32.0);
    assert_eq!(ui.app.project.active_pattern().notes[0].length, 2.0);
}

fn begin_due_state_history_autosave(app: &mut CitrusApp, path: PathBuf) {
    app.autosave_path = Some(path);
    app.dirty = true;
    app.last_autosave = Instant::now() - AUTOSAVE_INTERVAL - Duration::from_secs(1);
    app.last_history_capture = Instant::now() - Duration::from_secs(1);
    app.update_project_state();
    assert!(app.save_barrier.is_some());
    app.poll_save_barrier();
}

fn finish_state_history_autosave(app: &mut CitrusApp) {
    wait_config_condition(|| {
        app.poll_plugin_runtime_events();
        app.poll_save_barrier();
        app.save_barrier.is_none()
    });
    app.last_history_check = Instant::now() - Duration::from_millis(100);
    app.capture_history();
}

#[test]
fn startup_state_history_autosave_normalization_keeps_existing_redo() {
    let files = Fixture::new();
    let (mut ui, id) = state_history_fixture(false);
    let mut candidate = ui.app.project.clone();
    candidate.active_pattern_mut().notes[0].length = 2.0;
    ui.app.commit_editor_project(candidate);
    ui.app.undo();
    assert_eq!(ui.app.redo_stack.len(), 1);
    let worker = attach_state_history_worker(&mut ui.app, id, 907, false);
    consume_state_history_capture(&mut ui.app, &worker, 0);
    begin_due_state_history_autosave(&mut ui.app, files.0.join("redo-recovery.citrus"));
    finish_state_history_autosave(&mut ui.app);
    assert!(ui.app.undo_stack.is_empty());
    assert_eq!(ui.app.redo_stack.len(), 1);
    ui.app.redo();
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 32.0);
    assert_eq!(ui.app.project.active_pattern().notes[0].length, 2.0);
}

#[test]
fn startup_state_history_autosave_normalization_preserves_unobserved_notes_and_io_failure() {
    for fail_io in [false, true] {
        let files = Fixture::new();
        let (mut ui, id) = state_history_fixture(false);
        let worker = attach_state_history_worker(&mut ui.app, id, 908, false);
        consume_state_history_capture(&mut ui.app, &worker, 0);
        let recovery = if fail_io {
            files.0.clone()
        } else {
            files.0.join("pending-note-recovery.citrus")
        };
        begin_due_state_history_autosave(&mut ui.app, recovery.clone());
        // A real user edit can still arrive while the asynchronous state capture is
        // pending. Normalization must not synchronize away this unobserved note edit.
        ui.app.project.active_pattern_mut().notes[0].length = 2.0;
        finish_state_history_autosave(&mut ui.app);
        assert_eq!(ui.app.undo_stack.len(), 1);
        assert_eq!(ui.app.project.plugin_instances[0].opaque_state.len(), 5);
        assert!(ui.app.dirty);
        if fail_io {
            assert!(
                ui.app
                    .toast
                    .as_ref()
                    .unwrap()
                    .0
                    .starts_with("Autosave failed:")
            );
            assert!(recovery.is_dir());
            assert!(files.0.join("valid.wav").is_file());
        } else {
            assert_eq!(
                Project::load(&recovery).unwrap().active_pattern().notes[0].length,
                2.0
            );
        }
        ui.app.undo();
        assert_eq!(pattern_playback_length_beats(&ui.app.project), 16.0);
        assert_eq!(ui.app.redo_stack.len(), 1);
    }
}

#[test]
fn startup_state_history_autosave_normalization_after_native_edit_without_dirty_callback() {
    let files = Fixture::new();
    let (mut ui, id) = state_history_fixture(false);
    let worker = attach_state_history_worker(&mut ui.app, id, 909, false);
    consume_state_history_capture(&mut ui.app, &worker, 0);
    let control = &ui.app.running_generator_chains[&worker.channel_id].control;
    assert!(control.request_native_editor(0, 801, NativeEditorCommand::Open { owner: None }, &[0]));
    wait_config_condition(|| {
        control
            .native_editor_snapshot(0)
            .is_some_and(|s| s.state.open && s.pending_request.is_none())
    });
    worker.value.store(0.73f32.to_bits(), Ordering::SeqCst);
    // Deliberately omit a dirty callback. The existing native generation/capture
    // contract must still keep this real edit authoritative and undoable.
    assert_eq!(worker.dirty.load(Ordering::SeqCst), 0);
    assert!(control.request_native_editor(0, 802, NativeEditorCommand::Close, &[0]));
    wait_config_condition(|| {
        control
            .native_editor_snapshot(0)
            .is_some_and(|s| s.capture_serial == 1 && s.pending_request.is_none())
    });
    ui.app.poll_native_editor_snapshots();
    ui.app.last_history_check = Instant::now() - Duration::from_millis(100);
    ui.app.capture_history();
    assert_eq!(ui.app.project.plugin_instances[0].parameters[&0], 0.73);
    assert_eq!(ui.app.undo_stack.len(), 1);

    begin_due_state_history_autosave(&mut ui.app, files.0.join("native-recovery.citrus"));
    ui.app.project.active_pattern_mut().notes[0].length = 2.0;
    let control = &ui.app.running_generator_chains[&worker.channel_id].control;
    wait_config_condition(|| {
        control
            .native_editor_snapshot(0)
            .is_some_and(|s| s.capture_serial == 2)
    });
    // The snapshot is definitely available while its State event remains undrained.
    ui.app.poll_native_editor_snapshots();
    ui.app.last_history_check = Instant::now() - Duration::from_millis(100);
    ui.app.capture_history();
    assert_eq!(
        ui.app.undo_stack.len(),
        2,
        "native edit and pending note edit each keep one entry; normalization adds none"
    );
    finish_state_history_autosave(&mut ui.app);
    assert_eq!(ui.app.undo_stack.len(), 2);
    assert_eq!(ui.app.project.plugin_instances[0].opaque_state[4], 0xa6);
    assert_eq!(ui.app.project.plugin_instances[0].parameters[&0], 0.73);
    ui.app.undo();
    assert_eq!(pattern_playback_length_beats(&ui.app.project), 16.0);
    assert_eq!(ui.app.project.plugin_instances[0].parameters[&0], 0.73);
    ui.app.undo();
    assert_eq!(ui.app.project.plugin_instances[0].parameters[&0], 0.25);
    assert_eq!(ui.app.redo_stack.len(), 2);
}

#[test]
fn startup_state_history_autosave_preserves_unobserved_native_edit() {
    let files = Fixture::new();
    let (mut ui, id) = state_history_fixture(false);
    let original = project_fingerprint(&ui.app.project);
    let worker = attach_state_history_worker(&mut ui.app, id, 910, false);
    consume_state_history_capture(&mut ui.app, &worker, 0);
    let control = &ui.app.running_generator_chains[&worker.channel_id].control;
    assert!(control.request_native_editor(0, 811, NativeEditorCommand::Open { owner: None }, &[0]));
    wait_config_condition(|| {
        control
            .native_editor_snapshot(0)
            .is_some_and(|s| s.state.open && s.pending_request.is_none())
    });
    worker.value.store(0.73f32.to_bits(), Ordering::SeqCst);
    assert_eq!(worker.dirty.load(Ordering::SeqCst), 0);
    assert!(control.request_native_editor(0, 812, NativeEditorCommand::Close, &[0]));
    wait_config_condition(|| {
        control
            .native_editor_snapshot(0)
            .is_some_and(|s| s.capture_serial == 1 && s.pending_request.is_none())
    });
    ui.app.poll_native_editor_snapshots();
    assert_eq!(ui.app.project.plugin_instances[0].parameters[&0], 0.73);
    assert_eq!(ui.app.project.plugin_instances[0].opaque_state[4], 0xa5);
    // Deliberately leave the real native edit outside the observer. The next
    // autosave preimage is .73, while the user's current history baseline is .25.
    assert_eq!(ui.app.history_fingerprint, original);
    assert!(ui.app.undo_stack.is_empty());

    let recovery = files.0.join("unobserved-native-recovery.citrus");
    begin_due_state_history_autosave(&mut ui.app, recovery.clone());
    let control = &ui.app.running_generator_chains[&worker.channel_id].control;
    wait_config_condition(|| {
        control
            .native_editor_snapshot(0)
            .is_some_and(|s| s.capture_serial == 2)
    });
    // Exercise the earlier native publication before the tagged State receipt.
    // Its normalization must not overwrite the mismatching .25 history preimage.
    ui.app.poll_native_editor_snapshots();
    assert_eq!(ui.app.history_fingerprint, original);
    ui.app.last_history_check = Instant::now() - Duration::from_millis(100);
    ui.app.capture_history();
    assert_eq!(ui.app.undo_stack.len(), 1);
    finish_state_history_autosave(&mut ui.app);
    assert_eq!(ui.app.undo_stack.len(), 1);
    assert_eq!(worker.saves.load(Ordering::SeqCst), 2);
    let saved = Project::load(&recovery).unwrap();
    assert_eq!(saved.plugin_instances[0].parameters[&0], 0.73);
    assert_eq!(saved.plugin_instances[0].opaque_state[4], 0xa6);

    ui.app.undo();
    assert_eq!(project_fingerprint(&ui.app.project), original);
    assert_eq!(ui.app.project.plugin_instances[0].parameters[&0], 0.25);
    assert_eq!(ui.app.redo_stack.len(), 1);
    ui.app.redo();
    assert_eq!(ui.app.project.plugin_instances[0].parameters[&0], 0.73);
    assert_eq!(ui.app.project.plugin_instances[0].opaque_state[4], 0xa6);
}
