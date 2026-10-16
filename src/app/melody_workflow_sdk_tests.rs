//! Explicit no-device creation workflow: ordinary catalog/default load, live
//! Play, acknowledged generic edits, Manual Save, actual old joins, cold disk
//! Open, exact initialization ACKs, then the existing Whole strict owner.
//! The closed-audit and genuine-default factory cases share the complete workflow
//! and PCM oracle; only the audited case installs a renderer override.
use super::*;
use crate::{
    export::serial_master::automation_test_support::{self as master_automation, Mode},
    export_options::{WavLevelPolicy, WavSampleFormat},
    mixer_graph::{MASTER_MIXER_TRACK_ID, MixerRoute, MixerRouteDestination, MixerRouteTap},
    model::{Clip, ClipKind, PianoNote},
    plugin_parameter_editor::PluginParameterCatalogPhase,
    plugins::plugin_runtime::{
        PluginChainControl, PluginWorkerGuard, RetainedJoinObservation,
        whole_serial_sdk_tests::AppSerialSdkAudit,
    },
};
use std::{
    cell::RefCell,
    mem::ManuallyDrop,
    sync::atomic::{AtomicBool, Ordering},
};

const RATE: u32 = 48_000;
// A normal one-bar song leaves room for the real three-endpoint Realtime
// bridge path to sound before Manual Stop, without changing processing budgets.
const MUSIC: usize = RATE as usize * 2;
const TAIL: usize = 768;
const GENERATED: usize = (MUSIC + TAIL + 96).div_ceil(128) * 128;
const GAINS: [f32; 3] = [0.625, 0.8125, 0.9375];
const WAIT: Duration = Duration::from_secs(30);
const GEN_CLASS: &str = "43545253454658440000000000000002";
const FX_CLASS: &str = "43545253454658440000000000000001";
static CLEANUP_FAILED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
enum WorkflowRenderer {
    ClosedReadOnlyAudit,
    GenuineProduction,
}

struct SourceOwner {
    control: PluginChainControl,
    guard: PluginWorkerGuard,
}
struct WorkflowApp {
    app: Option<Box<CitrusApp>>,
    callback: Option<audio::TestAudioCallback>,
    ctx: egui::Context,
    retiring: Vec<SourceOwner>,
    identities: Vec<PluginControlIdentity>,
}
struct RetainedFailure {
    _app: Option<Box<CitrusApp>>,
    _callback: Option<audio::TestAudioCallback>,
    _retiring: Vec<SourceOwner>,
    _identities: Vec<PluginControlIdentity>,
}
thread_local! {
    static RETAINED_FAILURE: RefCell<Vec<ManuallyDrop<RetainedFailure>>> = const { RefCell::new(Vec::new()) };
}
impl WorkflowApp {
    fn new() -> Self {
        assert!(
            !CLEANUP_FAILED.load(Ordering::Acquire),
            "An original SDK workflow owner remains unproven"
        );
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        assert!(
            app.audio.is_none()
                && app.autosave_path.is_none()
                && app.plugin_cache_path.is_none()
                && app.recordings_dir.is_none()
        );
        let (mut audio, callback) = AudioEngine::test_engine_with_production_callback_capacity();
        assert!(audio.set_project_session(app.project_session));
        assert_eq!(audio.snapshot().sample_rate, RATE);
        app.audio = Some(audio);
        Self {
            app: Some(app),
            callback: Some(callback),
            ctx,
            retiring: Vec::new(),
            identities: Vec::new(),
        }
    }
    fn app(&self) -> &CitrusApp {
        self.app.as_deref().expect("Original App retained")
    }
    fn app_mut(&mut self) -> &mut CitrusApp {
        self.app.as_deref_mut().expect("Original App retained")
    }
    fn tick(&mut self) {
        if let (Some(callback), Some(audio)) = (
            &mut self.callback,
            self.app.as_ref().and_then(|app| app.audio.as_ref()),
        ) {
            callback.pump(audio, 128);
        }
        let ctx = self.ctx.clone();
        let app = self.app_mut();
        app.poll_plugin_processing_health();
        app.retry_deferred_plugin_rebuilds();
        app.poll_insert_endpoint_events();
        app.poll_generator_endpoint_events();
        app.poll_plugin_runtime_events();
        app.drive_clip_session_retirement();
        app.poll_clip_restorations();
        app.poll_plugin_parameter_callback_receipts();
        app.poll_plugin_parameter_worker_receipts();
        app.drive_plugin_parameter_edit_drafts();
        app.drive_plugin_parameter_catalog();
        app.poll_save_barrier();
        app.poll_native_editor_snapshots();
        app.sync_generator_routes();
        app.refresh_transport_clock();
        app.poll_timeline_compile(&ctx);
        app.poll_audio_asset_events();
        app.poll_timeline_audio_runtime(&ctx);
        app.poll_prepared_playback_start(&ctx);
        app.poll_loop_restart_wait(&ctx);
        app.drive_whole_song_export();
        app.apply_automation();
        app.sync_generator_audibility();
        app.update_transport(&ctx);
        app.update_project_state();
        // Pace the real callback at its device-free Q128/48k cadence, not an
        // arbitrary delay that changes processing deadlines or skips checks.
        std::thread::sleep(Duration::from_secs_f64(128.0 / f64::from(RATE)));
    }
    fn until(&mut self, mut condition: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !condition(self) {
            assert!(
                Instant::now() < deadline,
                "SDK App phase timed out: {}",
                self.diagnostic()
            );
            self.tick();
        }
    }
    fn diagnostic(&self) -> String {
        let app = self.app();
        format!(
            "session={} generators={} pendingGen={} inserts={} pendingFx={} initializationPending={} failed={} timeline={:?} desired={} confirmed={:?} transport={:?} save={} toast={:?} whole={:?}",
            app.project_session,
            app.running_generator_chains.len(),
            app.pending_generator_chains.len(),
            app.running_insert_chains.len(),
            app.pending_insert_chains.len(),
            app.plugin_initializations_pending(),
            app.plugin_initialization_failed(),
            app.timeline_audio_sync.phase,
            app.timeline_desired_generation,
            app.audio
                .as_ref()
                .and_then(AudioEngine::confirmed_timeline_identity),
            app.audio.as_ref().map(AudioEngine::snapshot).map(|s| (
                s.transport_playing,
                s.timeline_frame,
                s.plugin_processing_fault
            )),
            app.save_barrier.is_some(),
            app.toast.as_ref().map(|v| &v.0),
            app.whole_song_export_job
                .as_ref()
                .map(|j| (j.phase, j.failure.as_deref()))
        )
    }
    fn ready(&self) -> bool {
        let app = self.app();
        !app.plugin_topology_is_settling()
            && !app.plugin_initialization_failed()
            && app.running_generator_chains.len() == 1
            && app.running_insert_chains.len() == 2
            && app
                .project
                .plugin_instances
                .iter()
                .all(|p| p.runtime_status == PluginRuntimeStatus::Loaded)
            && app.timeline_audio_matches_desired()
            && app
                .audio
                .as_ref()
                .and_then(AudioEngine::prepared_playback_token)
                .is_some()
    }
    fn remember_sources(&mut self) -> Vec<PluginControlIdentity> {
        let ids: Vec<_> = self
            .app()
            .running_generator_chains
            .values()
            .map(|c| c.control.owner_identity())
            .chain(
                self.app()
                    .running_insert_chains
                    .values()
                    .map(|c| c.control.owner_identity()),
            )
            .collect();
        assert_eq!(ids.len(), 3);
        self.identities.extend(ids.iter().cloned());
        ids
    }
    fn cleanup(&mut self) -> Result<(), String> {
        if self.app.is_some() {
            self.app_mut().cancel_whole_song_export();
            let deadline = Instant::now() + WAIT;
            while self
                .app()
                .whole_song_export_job
                .as_ref()
                .is_some_and(WholeSongExportJob::pending)
            {
                if Instant::now() >= deadline {
                    return Err(
                        "Original Whole owner did not terminate before SDK App cleanup".into(),
                    );
                }
                self.tick();
            }
            // Destroy actual callback endpoints first, on this control thread.
            // Retain every actual guard/control instead of ordinary guard Drop.
            self.callback = None;
            let app = self.app_mut();
            app.audio = None;
            let mut owners = Vec::new();
            for (_, c) in app.running_generator_chains.drain() {
                owners.push(SourceOwner {
                    control: c.control,
                    guard: c.guard,
                });
            }
            for (_, c) in app.pending_generator_chains.drain() {
                owners.push(SourceOwner {
                    control: c.control,
                    guard: c.guard,
                });
            }
            for (_, c) in app.running_insert_chains.drain() {
                owners.push(SourceOwner {
                    control: c.control,
                    guard: c.guard,
                });
            }
            for (_, c) in app.pending_insert_chains.drain() {
                owners.push(SourceOwner {
                    control: c.control,
                    guard: c.guard,
                });
            }
            self.identities
                .extend(owners.iter().map(|c| c.control.owner_identity()));
            self.retiring.extend(owners);
            drop(self.app.take()); // The old App really exits before cold construction.
        }
        let deadline = Instant::now() + WAIT;
        loop {
            let mut pending = false;
            for owner in &mut self.retiring {
                match owner.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => pending = true,
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err(
                            "Original SDK source worker panicked during actual retained Join"
                                .into(),
                        );
                    }
                    RetainedShutdownPoll::Joined => {}
                }
            }
            if !pending {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Original SDK source guard did not actually Join".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        for identity in &self.identities {
            assert_joined(identity);
        }
        self.retiring.clear();
        Ok(())
    }
}
impl Drop for WorkflowApp {
    fn drop(&mut self) {
        let was_panicking = std::thread::panicking();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.cleanup()));
        if !matches!(result, Ok(Ok(()))) {
            CLEANUP_FAILED.store(true, Ordering::Release);
            let retained = RetainedFailure {
                _app: self.app.take(),
                _callback: self.callback.take(),
                _retiring: std::mem::take(&mut self.retiring),
                _identities: std::mem::take(&mut self.identities),
            };
            RETAINED_FAILURE.with(|slot| slot.borrow_mut().push(ManuallyDrop::new(retained)));
            if !was_panicking {
                panic!("SDK workflow cleanup unproven; actual owners and files retained");
            }
        }
    }
}
fn assert_joined(identity: &PluginControlIdentity) {
    assert!(matches!(
        identity.genuine_retained_join_observation(),
        RetainedJoinObservation::HealthyJoinReturned
            | RetainedJoinObservation::HealthyJoinObserverCallReturned
    ));
    let stats = identity.stats();
    assert_eq!(
        (
            stats.detached_workers,
            stats.shutdown_timeouts,
            stats.faults
        ),
        (0, 0, 0)
    );
}

fn explicit_fixture(variable: &str, name: &str) -> PathBuf {
    let supplied = PathBuf::from(
        std::env::var_os(variable)
            .unwrap_or_else(|| panic!("Set explicit {variable}; no installed fallback")),
    );
    assert!(supplied.is_absolute());
    let supplied = supplied
        .canonicalize()
        .expect("Actual source-built bundle exists");
    let expected = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vst3-effect/target/bundles")
        .join(name)
        .canonicalize()
        .unwrap();
    assert_eq!(
        supplied, expected,
        "Only the fixed repository source-built fixture is accepted"
    );
    supplied
}
fn check_helper_sibling() {
    let supplied = PathBuf::from(
        std::env::var_os("CITRUS_MELODY_HELPER")
            .expect("Explicit matching helper staged beside actual private test executable"),
    );
    assert!(supplied.is_absolute() && supplied.is_file());
    let name = if cfg!(windows) {
        "vst3-host-helper.exe"
    } else {
        "vst3-host-helper"
    };
    let expected = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join(name);
    assert_eq!(
        supplied.canonicalize().unwrap(),
        expected.canonicalize().unwrap()
    );
}

fn authored_project() -> Project {
    let mut p = Project::blank();
    p.name = "SDK saved melody with Master automation".into();
    p.tempo = 120.0;
    p.song_length_beats = 4.0;
    p.auto_song_length = false;
    p.channels[0].volume = 0.75;
    p.channels[0].pan = -0.25;
    p.channels[0].mixer_track = 1;
    p.patterns[0].length_steps = 16;
    p.patterns[0].notes = [(1, 63, 0.0, 1.0), (2, 95, 2.0, 2.0)]
        .into_iter()
        .map(|(id, note, start, length)| PianoNote {
            id,
            channel_id: Some(p.channels[0].id),
            group_id: None,
            note,
            start,
            length,
            velocity: 1.0,
            selected: false,
            muted: false,
        })
        .collect();
    p.clips.push(Clip {
        id: 1,
        track: 0,
        start: 0.0,
        length: 4.0,
        name: "Saved melody".into(),
        color: [90, 170, 220],
        kind: ClipKind::Pattern,
        pattern_length_policy: None,
        group_id: None,
        pattern_id: p.patterns[0].id,
        automation_id: None,
        audio_asset_id: None,
        source_offset: 0.0,
        audio_source_offset_frame: None,
        audio_source_reference: None,
        audio_length_reference: None,
        gain: 1.0,
        fade_in: 0.0,
        fade_out: 0.0,
        fade_in_reference: None,
        fade_out_reference: None,
        muted: false,
    });
    for track in &mut p.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
    }
    p.mixer_tracks[1].volume = 1.25;
    p.mixer_tracks[1].pan = 0.25;
    p.mixer_tracks[2].volume = 0.875;
    p.mixer_tracks[2].pan = -0.5;
    p.mixer_routes = [(1, 2, 0.75), (2, MASTER_MIXER_TRACK_ID, 1.0)]
        .into_iter()
        .enumerate()
        .map(|(i, (source, destination, gain))| MixerRoute {
            id: i as u64 + 1,
            runtime_slot: i as u8,
            source_mixer_track_id: source,
            destination: MixerRouteDestination::MainInput {
                mixer_track_id: destination,
            },
            tap: MixerRouteTap::PostFader,
            gain,
            enabled: true,
        })
        .collect();
    master_automation::configure(&mut p, MUSIC as u64, RATE, Mode::All);
    p
}

fn assert_persisted_melody(project: &Project) {
    let authored = authored_project();
    // Compare the complete authored musical payload, not just a fingerprint
    // or the plug-in state. Assignment adds instance IDs but cannot alter it.
    let musical_payload = |p: &Project| {
        serde_json::to_value((
            p.tempo,
            p.song_length_beats,
            p.auto_song_length,
            &p.patterns,
            &p.clips,
            &p.mixer_routes,
            &p.automation_lanes,
        ))
        .unwrap()
    };
    assert_eq!(musical_payload(project), musical_payload(&authored));
    assert_eq!(project.channels.len(), 1);
    let channel = &project.channels[0];
    assert_eq!(
        (
            channel.volume.to_bits(),
            channel.pan.to_bits(),
            channel.mixer_track,
            channel.muted,
            channel.solo,
        ),
        (0.75_f32.to_bits(), (-0.25_f32).to_bits(), 1, false, false)
    );
    assert_eq!(project.mixer_tracks.len(), authored.mixer_tracks.len());
    for (saved, expected) in project.mixer_tracks.iter().zip(&authored.mixer_tracks) {
        assert_eq!(
            (
                saved.id,
                saved.runtime_slot,
                saved.volume.to_bits(),
                saved.pan.to_bits(),
                saved.muted,
                saved.solo,
            ),
            (
                expected.id,
                expected.runtime_slot,
                expected.volume.to_bits(),
                expected.pan.to_bits(),
                expected.muted,
                expected.solo,
            )
        );
    }
}

fn scan_descriptor(bundle: &Path) -> PluginDescriptor {
    let report = plugins::scan_catalog_checked(&[bundle.to_path_buf()])
        .expect("Real production catalog scan completes");
    assert_eq!(report.plugins.len(), 1);
    assert_eq!(report.probe_lifecycle.len(), 1);
    assert!(
        report.probe_lifecycle[0].healthy_retirement(),
        "Catalog helper must naturally exit/reap"
    );
    let descriptor = report.plugins.into_iter().next().unwrap();
    assert!(descriptor.is_catalog_only() && descriptor.scan_error.is_none());
    descriptor
}
fn catalogue(
    f: &mut WorkflowApp,
    instance: u64,
) -> Vec<plugins::plugin_runtime::PluginParameterDescriptor> {
    f.app_mut().open_plugin_parameter_editor(instance);
    f.until(|f| {
        f.app()
            .plugin_parameter_editor
            .as_ref()
            .is_some_and(|e| e.catalog.phase() == PluginParameterCatalogPhase::Complete)
    });
    let editor = f.app().plugin_parameter_editor.as_ref().unwrap();
    assert!(editor.local_error.is_none() && editor.edit_error.is_none());
    editor.catalog.descriptors().to_vec()
}
fn edit_gain(f: &mut WorkflowApp, instance: u64, value: f32) {
    let catalog = catalogue(f, instance);
    assert!(catalog.iter().any(|p| p.id == 0 && !p.read_only));
    let binding = f
        .app()
        .plugin_parameter_editor
        .as_ref()
        .unwrap()
        .catalog
        .binding()
        .unwrap();
    let route = f
        .app()
        .plugin_parameter_edit_route(
            binding.project_session,
            binding.endpoint_id,
            instance,
            binding.slot,
            0,
            true,
        )
        .unwrap();
    f.app_mut().request_plugin_parameter_edit(route, value);
    assert_ne!(f.app().plugin_parameter_edits.reserved_len(), 0);
    f.until(|f| {
        f.app().plugin_parameter_edits.reserved_len() == 0
            && f.app()
                .project
                .plugin_instances
                .iter()
                .find(|p| p.id == instance)
                .unwrap()
                .parameters
                .get(&0)
                .is_some_and(|v| v.to_bits() == value.to_bits())
    });
    assert!(
        f.app()
            .plugin_parameter_editor
            .as_ref()
            .unwrap()
            .edit_error
            .is_none()
    );
    f.app_mut().plugin_parameter_editor = None;
}

fn generator_opaque_restore_proof(
    catalog: &[plugins::plugin_runtime::PluginParameterDescriptor],
) -> (f32, f32) {
    let loads = catalog.iter().find(|p| p.id == 1007).unwrap();
    let accepted_gain = catalog.iter().find(|p| p.id == 1008).unwrap();
    assert!(loads.read_only && accepted_gain.read_only);
    (loads.current_normalized, accepted_gain.current_normalized)
}

fn assert_default_loaded(f: &WorkflowApp, ids: [u64; 3]) {
    let app = f.app();
    assert!(app.fresh_bounce_sources.is_empty() && app.clip_render_restorations.is_empty());
    let channel = app.project.channels[0].id;
    let loaded = app
        .loaded_generator_identity(channel)
        .expect("Actual default Instrument Ready identity");
    assert_eq!(loaded.identity().class_uid(), GEN_CLASS);
    assert!(loaded.identity().is_instrument_with_midi_input());
    for id in ids {
        let instance = app
            .project
            .plugin_instances
            .iter()
            .find(|p| p.id == id)
            .unwrap();
        assert_eq!(instance.runtime_status, PluginRuntimeStatus::Loaded);
        assert!(
            plugins::plugin_load_spec_from_instance(instance)
                .class_uid
                .is_none()
        );
        // Factory declarations never promote the saved role. Ordinary catalog
        // assignment remains Unknown; actual Ready/capture identity proves use.
        assert_eq!(instance.role, PluginRole::Unknown);
    }
}

fn independent_pcm() -> Vec<[f32; 2]> {
    // Notes at [0,N/4) and [N/2,N), true MIDI fixture32 FIFO,
    // two FIR32/160 Effects, actual callback control order, one final crop96.
    let generated = GENERATED;
    let mut raw = vec![[0.0; 2]; generated];
    for (frame, sample) in raw.iter_mut().enumerate() {
        let Some(j) = frame.checked_sub(32) else {
            continue;
        };
        let level = if j < MUSIC / 4 {
            0.5
        } else if (MUSIC / 2..MUSIC).contains(&j) {
            0.75
        } else {
            0.0
        };
        let left = GAINS[0] * level;
        *sample = [left, -0.5 * left];
        sample[0] = sample[0].clamp(-8.0, 8.0) * 0.75 * 1.0;
        sample[1] = sample[1].clamp(-8.0, 8.0) * 0.75 * 0.75;
    }
    for effect in 0..2 {
        let prior = raw;
        raw = vec![[0.0; 2]; generated];
        for (frame, sample) in raw.iter_mut().enumerate() {
            for (channel, output) in sample.iter_mut().enumerate() {
                let direct = frame.checked_sub(32).map_or(0.0, |i| prior[i][channel]);
                let echo = frame.checked_sub(160).map_or(0.0, |i| prior[i][channel]);
                let wet = (GAINS[effect + 1] * (direct + 0.5 * echo)).clamp(-8.0, 8.0);
                *output = if effect == 0 {
                    wet * 1.25 * if channel == 0 { 0.75 } else { 1.0 } * 0.75
                } else {
                    wet * 0.875 * if channel == 0 { 1.0 } else { 0.5 }
                };
            }
        }
    }
    raw[96..96 + MUSIC + TAIL]
        .iter()
        .copied()
        .enumerate()
        .map(|(j, s)| master_automation::expected(s, j, MUSIC, Mode::All))
        .collect()
}

#[test]
#[ignore = "explicit source-built Instrument-first + Effect bundle and matching sibling production helper; no device, GUI or installed fallback"]
fn actual_sdk_catalog_play_stop_manual_save_cold_open_whole_master_matches_pcm() {
    run_saved_melody_workflow(WorkflowRenderer::ClosedReadOnlyAudit);
}

#[test]
#[ignore = "explicit source-built Instrument-first + Effect bundle and matching sibling production helper; genuine default Whole factory, no device, GUI or installed fallback"]
fn actual_sdk_catalog_play_stop_manual_save_cold_open_genuine_whole_factory_matches_pcm() {
    run_saved_melody_workflow(WorkflowRenderer::GenuineProduction);
}

fn run_saved_melody_workflow(renderer: WorkflowRenderer) {
    let genuine_factory = matches!(renderer, WorkflowRenderer::GenuineProduction);
    let _helper_admission = plugins::helper_transport_test_lock();
    check_helper_sibling();
    let generator_bundle =
        explicit_fixture("CITRUS_MELODY_FIXTURE", "CitrusLookaheadMIDIFixture.vst3");
    let effect_bundle = explicit_fixture("CITRUS_EFFECT_FIXTURE", "CitrusStereoDelayFixture.vst3");
    let generator_descriptor = scan_descriptor(&generator_bundle);
    let effect_descriptor = scan_descriptor(&effect_bundle);
    let directory = std::env::temp_dir().join(format!(
        "citrus-melody-workflow-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let saved_path = directory.join("saved-melody.citrus");
    let wav_path = directory.join("cold-whole.wav");
    let mut first = WorkflowApp::new();
    let session1 = first.app().audio.as_ref().unwrap().engine_session();
    first.app_mut().project = authored_project();
    first.app_mut().transport_mode = TransportMode::Song;
    first.app_mut().sync_history_observer();
    first.app_mut().sync_mixer_to_audio();
    assert!(
        first.app_mut().load_plugin_into_target(
            &generator_descriptor,
            PluginPickerTarget::ChannelDevice { channel: 0 }
        ),
        "Ordinary default Generator assignment failed: {}",
        first.diagnostic()
    );
    first.until(|f| {
        !f.app().plugin_topology_is_settling() && f.app().running_generator_chains.len() == 1
    });
    for track in [1, 2] {
        assert!(
            first.app_mut().load_plugin_into_target(
                &effect_descriptor,
                PluginPickerTarget::MixerSlot { track, slot: 0 }
            ),
            "Ordinary default Effect assignment failed: {}",
            first.diagnostic()
        );
        first.until(|f| {
            !f.app().plugin_topology_is_settling()
                && f.app().running_insert_chains.contains_key(&track)
        });
    }
    first.app_mut().request_immediate_timeline_compile();
    first.until(WorkflowApp::ready);
    let ids = [
        first.app().project.channels[0]
            .instrument_plugin_instance_id
            .unwrap(),
        first
            .app()
            .project
            .mixer_insert_slots
            .iter()
            .find(|s| s.track == 1)
            .unwrap()
            .plugin_instance_id,
        first
            .app()
            .project
            .mixer_insert_slots
            .iter()
            .find(|s| s.track == 2)
            .unwrap()
            .plugin_instance_id,
    ];
    assert_default_loaded(&first, ids);
    assert_eq!(
        first
            .app()
            .project
            .plugin_instances
            .iter()
            .find(|p| p.id == ids[0])
            .unwrap()
            .uid,
        generator_descriptor.id
    );
    // Default component/controller setup may legitimately accept default state.
    // Preserve its actual baseline instead of assuming zero initialization calls.
    let warm_generator_catalog = catalogue(&mut first, ids[0]);
    let warm_opaque = generator_opaque_restore_proof(&warm_generator_catalog);
    assert_ne!(warm_opaque.1.to_bits(), GAINS[0].to_bits());
    first.app_mut().plugin_parameter_editor = None;
    for (id, gain) in ids.into_iter().zip(GAINS) {
        edit_gain(&mut first, id, gain);
    }
    first.app_mut().request_immediate_timeline_compile();
    first.until(WorkflowApp::ready);
    let old_owners = first.remember_sources();
    first.app_mut().toggle_play();
    first
        .until(|f| f.app().playing && f.app().audio.as_ref().unwrap().snapshot().transport_playing);
    let mut audible_frames = 0;
    let play_deadline = Instant::now() + WAIT;
    while audible_frames < 256 {
        assert!(
            Instant::now() < play_deadline,
            "Real source Play remained silent: {}",
            first.diagnostic()
        );
        first.tick();
        let snapshot = first.app().audio.as_ref().unwrap().snapshot();
        assert!(
            snapshot.plugin_processing_fault.is_none(),
            "Real source Play fault: {:?}",
            snapshot.plugin_processing_fault
        );
        assert!(
            snapshot.transport_playing,
            "Manual Stop must follow actual still-playing source audio; audible={audible_frames}, frame={}, loop_count={}, per_endpoint_bridge={}, {}",
            snapshot.timeline_frame,
            snapshot.transport_loop_count,
            snapshot.plugin_bridge_latency_frames,
            first.diagnostic()
        );
        audible_frames += first
            .callback
            .as_ref()
            .unwrap()
            .last_output()
            .iter()
            .filter(|frame| frame.iter().any(|v| v.abs() > 1.0e-7))
            .count();
    }
    assert!(
        audible_frames > 0,
        "Actual callback source playback must sound"
    );
    first.app_mut().stop();
    first.until(|f| f.ready() && !f.app().playing && !f.app().plugin_state_save_transport_busy());
    for id in ids {
        let catalog = catalogue(&mut first, id);
        let probe = catalog.iter().find(|p| p.id == 1000).unwrap();
        assert!(
            probe.read_only && probe.current_normalized > 0.0,
            "Actual original SDK positive Process calls are required"
        );
        if id == ids[0] {
            let proof = generator_opaque_restore_proof(&catalog);
            assert_eq!(
                (proof.0.to_bits(), proof.1.to_bits()),
                (warm_opaque.0.to_bits(), warm_opaque.1.to_bits()),
                "Actual generic gain changes and Play must not manufacture accepted-state evidence"
            );
        }
        first.app_mut().plugin_parameter_editor = None;
    }
    first
        .app_mut()
        .begin_project_save(ProjectSaveRequest::Manual {
            path: saved_path.clone(),
            lifecycle: None,
        });
    first.until(|f| f.app().save_barrier.is_none() && f.app().queued_save_request.is_none());
    assert_eq!(first.app().saved_path.as_ref(), Some(&saved_path));
    assert!(
        !first.app().dirty,
        "Manual Save must be clean, not stale-state output: {}",
        first.diagnostic()
    );
    let saved_bytes = std::fs::read(&saved_path).unwrap();
    let saved = Project::load(&saved_path).unwrap();
    assert_persisted_melody(&saved);
    let saved_project_bytes = serde_json::to_vec(&saved).unwrap();
    assert_eq!(
        serde_json::to_vec(&first.app().project).unwrap(),
        saved_project_bytes,
        "Disk normalization must preserve the complete acknowledged saved Project"
    );
    let saved_fingerprint = project_fingerprint(&saved);
    assert_eq!(project_fingerprint(&first.app().project), saved_fingerprint);
    for (index, id) in ids.into_iter().enumerate() {
        let p = saved.plugin_instances.iter().find(|p| p.id == id).unwrap();
        assert_eq!(p.opaque_state.len(), 40);
        assert_eq!(
            p.parameters.len(),
            1,
            "Read-only probes must not become persisted bases"
        );
        assert_eq!(p.parameters[&0].to_bits(), GAINS[index].to_bits());
        assert!(
            plugins::plugin_load_spec_from_instance(p)
                .class_uid
                .is_none()
        );
    }
    first
        .cleanup()
        .expect("All original source guards actually Join before cold App creation");
    assert!(first.app.is_none() && first.callback.is_none() && first.retiring.is_empty());
    for owner in &old_owners {
        assert_joined(owner);
    }
    drop(first);

    let mut cold = WorkflowApp::new();
    assert_ne!(
        cold.app().audio.as_ref().unwrap().engine_session(),
        session1
    );
    assert!(
        cold.app().plugins.is_empty()
            && cold.app().fresh_bounce_sources.is_empty()
            && cold.app().clip_render_restorations.is_empty()
    );
    assert!(
        cold.app_mut()
            .finish_open_project(Project::load(&saved_path).unwrap(), saved_path.clone())
    );
    cold.app_mut().transport_mode = TransportMode::Song;
    cold.until(WorkflowApp::ready);
    assert_default_loaded(&cold, ids);
    for id in ids {
        let (session, endpoint, request, count) = cold
            .app()
            .completed_initialization_ack_for_sdk_test(id)
            .expect("Cold Loaded requires actual matched generic parameter initialization ACK");
        assert_eq!(session, cold.app().project_session);
        assert_eq!(
            endpoint,
            cold.app().plugin_parameter_runtime_binding(id).unwrap().0
        );
        assert!(request != 0 && count == 1);
    }
    let cold_generator_catalog = catalogue(&mut cold, ids[0]);
    let cold_opaque = generator_opaque_restore_proof(&cold_generator_catalog);
    assert!(
        cold_opaque.0 > 0.0,
        "Cold Generator must accept actual opaque state; generic Gain replay cannot qualify it"
    );
    assert_eq!(
        cold_opaque.1.to_bits(),
        GAINS[0].to_bits(),
        "The accepted opaque value must be the saved source value"
    );
    cold.app_mut().plugin_parameter_editor = None;
    let cold_owners = cold.remember_sources();
    for old in &old_owners {
        assert!(
            cold.app()
                .running_generator_chains
                .values()
                .all(|c| !old.matches(&c.control))
        );
        assert!(
            cold.app()
                .running_insert_chains
                .values()
                .all(|c| !old.matches(&c.control))
        );
    }
    assert_eq!(project_fingerprint(&cold.app().project), saved_fingerprint);
    let before_render = serde_json::to_vec(&cold.app().project).unwrap();
    assert_eq!(before_render, saved_project_bytes);
    assert_persisted_melody(&cold.app().project);
    let options = WavExportOptions {
        sample_rate: RATE,
        sample_format: WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds: 16,
    };
    let review = cold
        .app()
        .prepare_whole_song_export_review(options, true)
        .unwrap();
    cold.app_mut()
        .begin_whole_song_export(review, wav_path.clone())
        .unwrap();
    let audit = if genuine_factory {
        None
    } else {
        let audit = AppSerialSdkAudit::default();
        let worker_audit = audit.clone();
        cold.app_mut()
            .whole_song_export_job
            .as_mut()
            .unwrap()
            .serial_render_starter = Some(Box::new(
            move |admission, project, captured, chains, options, path, control, reviewed| {
                assert_eq!(
                    project_fingerprint(&project),
                    captured.identity().project_fingerprint
                );
                assert!(captured.strict_load_inputs().0.class_uid.is_none());
                assert_eq!(captured.state.opaque_state.len(), 40);
                assert_eq!(
                    captured.state.parameter_bases[&0].to_bits(),
                    GAINS[0].to_bits()
                );
                assert_eq!(chains.len(), 2);
                for (index, chain) in chains.iter().enumerate() {
                    let slot = &chain.receipt.slots[0];
                    assert!(slot.request.requested_class_uid.is_none());
                    assert_eq!(slot.state.metadata.identity.class_uid, FX_CLASS);
                    assert_eq!(slot.state.opaque_state.len(), 40);
                    assert_eq!(
                        slot.state.parameter_bases[&0].to_bits(),
                        f64::from(GAINS[index + 1]).to_bits()
                    );
                }
                worker_audit.start(
                    admission, project, captured, chains, options, path, control, reviewed,
                )
            },
        ));
        assert!(
            cold.app()
                .whole_song_export_job
                .as_ref()
                .unwrap()
                .serial_render_starter
                .is_some()
        );
        Some(audit)
    };
    // Source preparation keeps its existing bounded wait. Once the Whole owner
    // establishes its original render deadline, wait against that exact value,
    // never a new budget per clone or an arbitrary shorter source-phase timer.
    let source_deadline = Instant::now() + WAIT;
    let mut saw_serial_capture = false;
    while cold.app().whole_song_export_fence_active() {
        let job = cold.app().whole_song_export_job.as_ref().unwrap();
        if genuine_factory {
            // Check before EVERY drive, not merely after completion: an override
            // is consumed with take(), so its final absence cannot prove selection.
            assert!(job.render_starter.is_none() && job.serial_render_starter.is_none());
            if job.phase == WholeExportPhase::CapturingEffects {
                assert!(
                    job.captured_generator
                        .as_ref()
                        .unwrap()
                        .plan()
                        .is_serial_master()
                );
                saw_serial_capture = true;
            }
        }
        let deadline = job.render_deadline.unwrap_or(source_deadline);
        assert!(
            Instant::now() < deadline,
            "Actual Whole owner exceeded its original deadline: {}",
            cold.diagnostic()
        );
        cold.tick();
    }
    let job = cold.app().whole_song_export_job.as_ref().unwrap();
    assert_eq!(job.phase, WholeExportPhase::Complete, "{:?}", job.failure);
    if genuine_factory {
        assert!(
            saw_serial_capture,
            "The actual serial capture path must run"
        );
        assert!(job.render_starter.is_none() && job.serial_render_starter.is_none());
        assert!(
            job.failure.is_none()
                && job.capture_failure.is_none()
                && job.effect_capture_failure.is_none()
        );
        assert!(!job.pending() && job.render.is_none() && !job.admission.is_busy());
    }
    let Some(MasterSongRenderOutcome::Complete(report)) = &job.outcome else {
        panic!("No complete actual Whole WAV: {:?}", job.outcome)
    };
    assert_eq!(
        (
            report.dry_render.native_latency,
            report.dry_render.reported_tail,
            report.dry_render.generated_frames
        ),
        (96, 352, GENERATED as u64)
    );
    assert_eq!(
        (
            report.master_output.music_frames,
            report.master_output.tail_frames,
            report.master_output.frames
        ),
        (MUSIC, TAIL, MUSIC + TAIL)
    );
    assert_eq!(report.master_output.applied_gain, 1.0);
    assert!(!report.master_output.tail_limit_reached);
    if let Some(audit) = audit {
        audit.assert_natural_serial_reaps(&ids, job.render_deadline.unwrap());
    }
    let wav = crate::wav::read_wav(&wav_path).unwrap();
    let expected = independent_pcm();
    assert_eq!(
        (
            wav.metadata.sample_rate,
            wav.metadata.channels,
            wav.metadata.frames
        ),
        (RATE, 2, (MUSIC + TAIL) as u64)
    );
    assert_eq!(wav.samples.len(), expected.len() * 2);
    for (j, (actual, expected)) in wav
        .samples
        .as_chunks::<2>()
        .0
        .iter()
        .zip(&expected)
        .enumerate()
    {
        for channel in 0..2 {
            assert!(
                (actual[channel] - expected[channel]).abs() <= 3.0e-6,
                "Cold Whole PCM frame{j} channel{channel}: {} != {}",
                actual[channel],
                expected[channel]
            );
        }
    }
    assert_eq!(wav.samples[..2], [0.0; 2]);
    assert_eq!(
        wav.samples
            .as_chunks::<2>()
            .0
            .iter()
            .position(|s| s.iter().any(|v| v.abs() > 1.0e-7)),
        Some(1)
    );
    assert!(
        wav.samples[MUSIC / 4 * 2..MUSIC / 2 * 2]
            .iter()
            .all(|v| *v == 0.0)
    );
    assert!(
        wav.samples[MUSIC * 2..(MUSIC + 256) * 2]
            .iter()
            .any(|v| *v != 0.0)
    );
    assert!(wav.samples[(MUSIC + 256) * 2..].iter().all(|v| *v == 0.0));
    assert_eq!(
        serde_json::to_vec(&cold.app().project).unwrap(),
        before_render
    );
    assert_eq!(std::fs::read(&saved_path).unwrap(), saved_bytes);
    assert!(
        cold_owners
            .iter()
            .all(|id| id.genuine_retained_join_observation() == RetainedJoinObservation::NotPolled)
    );
    cold.cleanup()
        .expect("Cold App original source guards must also actually Join");
    for owner in &cold_owners {
        assert_joined(owner);
    }
    if genuine_factory {
        eprintln!(
            "REAL_APP_MELODY_SAVE_COLD_OPEN_GENUINE_WHOLE_FACTORY_PASS live_source_sdk_process_calls=positive_each; generic_base_contract=f32_ack; state=40_each; cold_generator_opaque=accepted_state_function_and_saved_gain_readonly_proof; old_app_dropped=true; old_source_joined=3; cold_init_exact_ack=3; cold_source_joined=3; source_class_selector=None_both; registry_reused=false; render_factory=default_production; genuine_whole_production_factory_selected=true; completion=typed_Complete_after_termination_ACK_and_admission_drained; strict_child_pid_audit=false; music={MUSIC}; tail={TAIL}; retained={}; generated={GENERATED}; native_latency=96; full_independent_pcm=true; output={}",
            MUSIC + TAIL,
            directory.display()
        );
    } else {
        eprintln!(
            "REAL_APP_MELODY_SAVE_COLD_OPEN_WHOLE_PASS live_source_sdk_process_calls=positive_each; generic_base_contract=f32_ack; state=40_each; cold_generator_opaque=accepted_state_function_and_saved_gain_readonly_proof; old_app_dropped=true; old_source_joined=3; cold_init_exact_ack=3; cold_source_joined=3; source_class_selector=None_both; registry_reused=false; render_factory=closed_read_only_audit_wrapper; genuine_whole_production_factory_selected=false; strict_children=3_natural_exit0_reaped; effect_offline_probe1005=1_each; max_live_strict_owner=1; music={MUSIC}; tail={TAIL}; retained={}; generated={GENERATED}; native_latency=96; full_independent_pcm=true; output={}",
            MUSIC + TAIL,
            directory.display()
        );
    }
}
