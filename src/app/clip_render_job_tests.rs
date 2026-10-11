//! Source/mock-only tests of the actual App job. The source uses the production
//! identified worker with a deterministic PluginBackend; the callback, dedicated
//! captures, renderer owner, create-new writer, verifier, and history seam are real.
//! No SDK, helper process, device, editor, or genuine plugin is invoked.
#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[path = "clip_render_genuine_topology_app_tests.rs"]
mod genuine_topology_app_tests;
#[path = "clip_render_stop_settlement_tests.rs"]
mod stop_settlement_tests;
use super::*;
use crate::instrument_bounce::render::{
    BackendFailure, BlockReply, DisposalObservation, RenderBackend,
};
use crate::instrument_bounce::{BounceAudioBus, BouncePluginMetadata, BouncePluginState};
use crate::plugins::plugin_runtime::{
    PluginPrepareConfig, PluginTransport, test_capture_source::TestCaptureSource,
};
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicU64},
};

#[test]
fn app_cancel_and_commit_have_one_independent_winner() {
    let operation = AppOperationControl::default();
    assert!(operation.cancel());
    assert!(operation.cancel());
    assert!(!operation.seal());
    let operation = AppOperationControl::default();
    assert!(operation.seal());
    assert!(!operation.cancel());
    assert!(!operation.seal());
}

#[test]
fn every_inflight_phase_is_nonterminal_including_source_join_and_playback() {
    for phase in [
        ClipRenderPhase::Review,
        ClipRenderPhase::StoppingDraining,
        ClipRenderPhase::Capturing,
        ClipRenderPhase::RenderingDisposing,
        ClipRenderPhase::VerifyingMedia,
        ClipRenderPhase::RecapturingSource,
        ClipRenderPhase::ReadyToCommit,
        ClipRenderPhase::Removing,
        ClipRenderPhase::CommittedSourceJoining,
        ClipRenderPhase::CommittedPlaybackPending,
    ] {
        assert!(!phase.terminal());
    }
}

struct Backend {
    metadata: BouncePluginMetadata,
    disposed: bool,
}
impl RenderBackend for Backend {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        _: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        assert_eq!(capture.metadata(), &self.metadata);
        Ok(self.metadata.clone())
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        Ok(self.metadata.clone())
    }
    fn send_midi(&mut self, _: [u8; 3], _: u16) -> Result<(), BackendFailure> {
        Ok(())
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        assert_eq!(frames, 128);
        assert!(transport.sample_position >= 24_000);
        Ok(BlockReply {
            channels: vec![vec![0.125; frames], vec![-0.125; frames]],
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        })
    }
    fn dispose(&mut self, _: Instant) -> DisposalObservation {
        self.disposed = true;
        DisposalObservation {
            terminated: true,
            error: None,
            diagnostics: Vec::new(),
        }
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        assert!(
            self.disposed,
            "mock renderer must complete checked disposal"
        );
    }
}

fn source_state() -> BouncePluginState {
    BouncePluginState {
        metadata: BouncePluginMetadata {
            class_uid: "ABCDEF019182FAEB566D624153675854".into(),
            version: "fixture-1".into(),
            name: "Controller mock".into(),
            category: "Instrument|Synth".into(),
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![],
            input_buses: vec![],
            output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            latency_samples: 0,
            tail_samples: 0,
        },
        opaque_state: vec![7, 8, 9],
        parameter_bases: BTreeMap::from([(17, 0.75)]),
        native_generation: 2,
        native_dirty_revision: 9,
    }
}

fn source_project() -> Project {
    let mut project = Project {
        tempo: 120.0,
        song_length_beats: 4.0,
        ..Project::default()
    };
    project.channels.truncate(1);
    project.channels[0].instrument_plugin_instance_id = Some(41);
    project.channels[0].pan = 0.0;
    project.channels[0].volume = 0.75;
    project.channels[0].muted = false;
    project.plugin_instances = vec![PluginInstance {
        id: 41,
        format: crate::model::PluginFormat::Vst3,
        role: PluginRole::Instrument,
        path: "/fixture-only/controller-source.vst3".into(),
        uid: "descriptor-not-factory-class".into(),
        vendor: "Fixture".into(),
        name: "Controller mock".into(),
        enabled: true,
        bypass: false,
        wet: 1.0,
        parameters: BTreeMap::from([(17, 0.25)]),
        opaque_state: vec![1, 2, 3],
        runtime_status: PluginRuntimeStatus::Loaded,
        midi_ports: Default::default(),
    }];
    project.automation_lanes.clear();
    project.audio_assets.clear();
    project.patterns.truncate(1);
    project.patterns[0].length_steps = 4;
    project.patterns[0].notes.clear();
    project.patterns[0].channel_steps = vec![[false; 16]];
    project.patterns[0].channel_steps[0][0] = true;
    project.clips.truncate(1);
    project.clips[0].kind = ClipKind::Pattern;
    project.clips[0].muted = false;
    project.clips[0].start = 1.0;
    project.clips[0].length = 0.125;
    project.clips[0].source_offset = 0.0;
    project.clips[0].pattern_id = project.patterns[0].id;
    project
}

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
pub(in crate::app) static FIXTURE_POISONED: AtomicBool = AtomicBool::new(false);
pub(in crate::app) static FIXTURE_ADMISSION: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Default)]
struct OwnedApp(Option<Box<CitrusApp>>);
impl std::ops::Deref for OwnedApp {
    type Target = CitrusApp;
    fn deref(&self) -> &Self::Target {
        self.0.as_deref().expect("fixture App retained")
    }
}
impl std::ops::DerefMut for OwnedApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_deref_mut().expect("fixture App retained")
    }
}
#[derive(Default)]
struct OwnedSource(Option<TestCaptureSource>);
impl std::ops::Deref for OwnedSource {
    type Target = TestCaptureSource;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("fixture source retained")
    }
}
struct RetainedFailedFixture {
    _app: OwnedApp,
    _source: OwnedSource,
    _callback: Option<audio::TestAudioCallback>,
    _directory: PathBuf,
    _verifier: Option<crate::occurrence_media::VerificationTestGuard>,
    _admission: Option<std::sync::MutexGuard<'static, ()>>,
}
thread_local! {
    // At most one failed fixture can occupy this emergency owner. It never
    // silently detaches an unproven worker or deletes its files. Global poison
    // refuses later fixture admission; a retained failure cannot count as pass.
    static RETAINED_FAILED_FIXTURE: std::cell::RefCell<Option<std::mem::ManuallyDrop<RetainedFailedFixture>>>
        = const { std::cell::RefCell::new(None) };
}
pub(in crate::app) struct Fixture {
    app: OwnedApp,
    ctx: egui::Context,
    callback: Option<audio::TestAudioCallback>,
    source: OwnedSource,
    directory: PathBuf,
    verifier_guard: Option<crate::occurrence_media::VerificationTestGuard>,
    admission_guard: Option<std::sync::MutexGuard<'static, ()>>,
    cleanup_budget: Duration,
}
impl Fixture {
    pub(in crate::app) fn app(&self) -> &CitrusApp {
        &self.app
    }
    pub(in crate::app) fn app_mut(&mut self) -> &mut CitrusApp {
        &mut self.app
    }
    pub(in crate::app) fn ctx(&self) -> egui::Context {
        self.ctx.clone()
    }
    #[cfg(target_os = "linux")]
    pub(in crate::app) fn directory(&self) -> &Path {
        &self.directory
    }
    pub(in crate::app) fn callback(&self) -> &audio::TestAudioCallback {
        self.callback.as_ref().expect("fixture callback retained")
    }
    #[cfg(target_os = "linux")]
    pub(in crate::app) fn callback_mut(&mut self) -> &mut audio::TestAudioCallback {
        self.callback.as_mut().expect("fixture callback retained")
    }
    pub(in crate::app) fn source_handle(&self) -> &TestCaptureSource {
        &self.source
    }
    #[cfg(target_os = "linux")]
    pub(in crate::app) fn original_source_joined(&self) -> bool {
        self.app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.source_joined && job.retired_source.is_none())
    }
    pub(in crate::app) fn new() -> Self {
        let admission_deadline = Instant::now() + Duration::from_secs(10);
        let admission = loop {
            assert!(
                Instant::now() < admission_deadline,
                "App fixture admission timed out before any source was spawned"
            );
            assert!(
                !FIXTURE_POISONED.load(Ordering::Acquire),
                "prior App fixture cleanup is unproven; its owners/files remain retained"
            );
            match FIXTURE_ADMISSION.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1))
                }
            }
        };
        let verifier_guard = crate::occurrence_media::verification_test_guard();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        app.project = source_project();
        app.saved_path = None;
        app.transport_mode = TransportMode::Song;
        app.selected_clip = Some(app.project.clips[0].id);
        app.playlist_selection_ids = HashSet::from([app.project.clips[0].id]);
        let (mut audio, callback) = AudioEngine::test_engine_with_callback();
        assert!(audio.set_project_session(app.project_session));
        app.audio = Some(audio);
        let directory = std::env::temp_dir().join(format!(
            "citrus-app-clip-job-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let channel_id = app.project.channels[0].id;
        let instance_id = app.project.channels[0]
            .instrument_plugin_instance_id
            .unwrap();
        let mixer_track = app
            .project
            .mixer_runtime_slot(app.project.channels[0].mixer_track)
            .unwrap();
        let endpoint_id = app.allocate_generator_endpoint_id();
        let candidate_state = snapshot_channel_generator(&app.project, channel_id).unwrap();
        let (chain, source) = TestCaptureSource::new(
            instance_id,
            source_state(),
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: MAX_MIXER_BLOCK_FRAMES,
            },
        )
        .unwrap();
        let plugins::plugin_runtime::PluginChain {
            audio: endpoint,
            control,
            guard,
        } = chain;
        // Put the actual guard into the fixture-owned App before any assertion
        // or callback admission can fail. Running is established only by the
        // normal callback Installed receipt, never by a fabricated ready chain.
        app.pending_generator_chains.insert(
            channel_id,
            PendingGeneratorChain {
                project_session: app.project_session,
                endpoint_id,
                instance_id,
                mixer_track,
                control,
                guard,
                previous_state: None,
                candidate_state,
            },
        );
        let mut fixture = Self {
            app: OwnedApp(Some(app)),
            ctx,
            callback: Some(callback),
            source: OwnedSource(Some(source)),
            directory,
            verifier_guard: Some(verifier_guard),
            admission_guard: Some(admission),
            cleanup_budget: Duration::from_secs(10),
        };
        let project_session = fixture.app.project_session;
        assert!(
            fixture
                .app
                .audio
                .as_mut()
                .unwrap()
                .install_generator_endpoint_for_project(
                    project_session,
                    channel_id,
                    endpoint_id,
                    instance_id,
                    mixer_track,
                    endpoint
                )
        );
        fixture.app.request_immediate_timeline_compile();
        fixture.until(|fixture| {
            fixture.app.timeline_audio_matches_desired()
                && fixture
                    .app
                    .clip_render_observe(fixture.app.project.channels[0].id)
                    .is_ok()
        });
        // The mock's normal SlotReady initialization has finished. Simulate
        // current unnotified vendor state while keeping the Project stale.
        fixture.source.set_capture_state(source_state());
        fixture.app.project.plugin_instances[0]
            .parameters
            .insert(17, 0.25);
        fixture.app.project.plugin_instances[0].opaque_state = vec![1, 2, 3];
        fixture.app.history_snapshot = fixture.app.project.clone();
        fixture.app.history_fingerprint = project_fingerprint(&fixture.app.project);
        fixture.app.project_fingerprint = fixture.app.history_fingerprint;
        fixture.app.undo_stack.clear();
        fixture.app.redo_stack.clear();
        fixture.app.dirty = false;
        fixture
    }
    pub(in crate::app) fn pump_callback_only(&mut self) {
        if let (Some(callback), Some(audio)) = (&mut self.callback, &self.app.audio) {
            callback.pump(audio, 128);
        }
    }
    pub(in crate::app) fn tick(&mut self) {
        self.pump_callback_only();
        self.app.drive_clip_session_retirement();
        self.app.poll_clip_restorations();
        self.app.poll_generator_endpoint_events();
        self.app.poll_plugin_runtime_events();
        self.app.poll_plugin_processing_health();
        self.app.poll_timeline_compile(&self.ctx);
        self.app.poll_audio_asset_events();
        self.app.poll_timeline_audio_runtime(&self.ctx);
        self.app.drive_clip_render_job();
        std::thread::sleep(Duration::from_millis(2));
    }
    pub(in crate::app) fn until(&mut self, condition: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(self) && Instant::now() < deadline {
            self.tick();
        }
        assert!(
            condition(self),
            "bounded mock App condition failed; {}",
            self.timeout_metadata()
        );
    }
    fn timeout_metadata(&self) -> String {
        // Only bounded control metadata. Never format Project, PCM, native
        // state, captured parameters, or any producer-owned receipt payload.
        fn ids<'a>(values: impl Iterator<Item = &'a u64>) -> Vec<u64> {
            let mut ids = values.take(16).copied().collect::<Vec<_>>();
            ids.sort_unstable();
            ids
        }
        let job = self.app.clip_render_job.as_ref();
        let route = self.app.timeline_audio_source.as_ref().map(|source| {
            match self.app.timeline_generator_routes_state(&source.timeline) {
                RequiredGeneratorRouteState::Ready => "Ready".to_owned(),
                RequiredGeneratorRouteState::Pending {
                    channel_id,
                    plugin_instance_id,
                } => format!("Pending(channel={channel_id}, instance={plugin_instance_id})"),
                RequiredGeneratorRouteState::Failed(error) => {
                    format!("Failed({})", bounded_clip_timeline_text(&error))
                }
            }
        });
        format!(
            "phase={:?}, error={:?}, asset(queued,ack,operation)={:?}, source_joined={:?}, expected_generation={:?}, desired={}, compiled={:?}, source={:?}, confirmed={:?}, sync={:?}, sync_error={:?}, synchronized={:?}, needs_resync={:?}, route={:?}, loaded(count,ids)={:?}, prepared(count,ids)={:?}, registering(count,ids)={:?}, unregistering(count,ids)={:?}, loading(count,ids)={:?}, clearing={:?}, retry={}",
            job.map(|job| job.phase),
            job.and_then(|job| job.failure.as_deref())
                .map(bounded_clip_timeline_text),
            job.map(|job| (job.asset_queued, job.asset_ack, job.asset_operation)),
            job.map(|job| job.source_joined),
            job.and_then(|job| job.expected_timeline_generation),
            self.app.timeline_desired_generation,
            self.app.timeline_compiled_generation,
            self.app
                .timeline_audio_source
                .as_ref()
                .map(|source| source.generation),
            self.app
                .audio
                .as_ref()
                .and_then(AudioEngine::confirmed_timeline_identity),
            self.app.timeline_audio_sync.phase,
            self.app
                .timeline_audio_sync
                .last_error
                .as_deref()
                .map(bounded_clip_timeline_text),
            self.app
                .audio
                .as_ref()
                .map(AudioEngine::timeline_runtime_is_synchronized),
            self.app
                .audio
                .as_ref()
                .map(AudioEngine::timeline_runtime_needs_resync),
            route,
            (
                self.app.loaded_audio_assets.len(),
                ids(self.app.loaded_audio_assets.keys())
            ),
            (
                self.app.prepared_audio_asset_registrations.len(),
                ids(self.app.prepared_audio_asset_registrations.keys())
            ),
            (
                self.app.pending_audio_asset_registrations.len(),
                ids(self.app.pending_audio_asset_registrations.keys())
            ),
            (
                self.app.pending_audio_asset_unregistrations.len(),
                ids(self.app.pending_audio_asset_unregistrations.keys())
            ),
            (
                self.app.loading_audio_assets.len(),
                ids(self.app.loading_audio_assets.keys())
            ),
            self.app
                .pending_audio_asset_clear
                .map(|pending| (pending.queued, pending.operation)),
            self.app.plugin_processing_retry_required,
        )
    }
    pub(in crate::app) fn begin(&mut self) {
        let mut job = self.app.prepare_clip_render_review().unwrap();
        job.review.directory = self.directory.to_str().unwrap().to_owned();
        job.review.tail_acknowledged = true;
        let metadata = source_state().metadata;
        job.render_starter = Some(Box::new(move |admission, capture, path, control| {
            admission.start_occurrence(capture, path, control, move || Backend {
                metadata,
                disposed: false,
            })
        }));
        self.app.begin_clip_render_stop(&mut job).unwrap();
        self.app.clip_render_job = Some(job);
    }
    pub(in crate::app) fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            &self.app.project,
            &self.app.history_snapshot,
            self.app.history_fingerprint,
            &self.app.undo_stack,
            &self.app.redo_stack,
            self.app.dirty,
        ))
        .unwrap()
    }
}
impl Fixture {
    fn drain_retained(&mut self, deadline: Instant) -> Result<(), &'static str> {
        // Tests may exercise recovery policy, but fixture destruction must not
        // spawn/drop-save work or recreate diagnostic files after cleanup.
        self.app.autosave_path = None;
        if let Some(source) = self.source.0.as_ref() {
            source.release_gates();
        }
        if let Some(stage) = &mut self.app.clip_session_retirement {
            stage.abandon_test_intent();
        }
        self.app.cancel_clip_render_job();
        while self
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.owns_pending_resource())
        {
            if Instant::now() >= deadline {
                return Err("App asynchronous owner did not terminate");
            }
            self.tick();
        }
        self.callback = None;
        loop {
            let mut pending = false;
            for source in self.app.running_generator_chains.values_mut() {
                pending |= match source.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => true,
                    RetainedShutdownPoll::Joined => false,
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err("exact source worker panicked during fixture cleanup");
                    }
                };
            }
            for source in self.app.pending_generator_chains.values_mut() {
                pending |= match source.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => true,
                    RetainedShutdownPoll::Joined => false,
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err("exact source worker panicked during fixture cleanup");
                    }
                };
            }
            if let Some(source) = self
                .app
                .clip_render_job
                .as_mut()
                .and_then(|job| job.retired_source.as_mut())
            {
                pending |= match source.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => true,
                    RetainedShutdownPoll::Joined => false,
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err("exact source worker panicked during fixture cleanup");
                    }
                };
            }
            if let Some(stage) = &mut self.app.clip_session_retirement {
                pending |= !stage.poll_test_shutdown_retained()?;
            }
            for restoration in self.app.pending_clip_restorations.values_mut() {
                pending |= !restoration.poll_test_shutdown_retained();
            }
            for guard in &mut self.app.retired_clip_restorations {
                pending |= match guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => true,
                    RetainedShutdownPoll::Joined => false,
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err("retired restoration worker panicked during fixture cleanup");
                    }
                };
            }
            if !pending {
                break;
            }
            if Instant::now() >= deadline {
                return Err("exact source JoinHandle is still pending");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.verifier_guard = None;
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let was_panicking = std::thread::panicking();
        // Release gates before any potentially panicking App poll. Cleanup has
        // a bounded observation budget and never substitutes timeout for join.
        if let Some(source) = self.source.0.as_ref() {
            source.release_gates();
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.drain_retained(Instant::now() + self.cleanup_budget)
        }));
        if !matches!(result, Ok(Ok(()))) {
            FIXTURE_POISONED.store(true, Ordering::Release);
            let retained = RetainedFailedFixture {
                _app: std::mem::take(&mut self.app),
                _source: std::mem::take(&mut self.source),
                _callback: self.callback.take(),
                _directory: std::mem::take(&mut self.directory),
                _verifier: self.verifier_guard.take(),
                _admission: self.admission_guard.take(),
            };
            RETAINED_FAILED_FIXTURE.with(|slot| {
                *slot.borrow_mut() = Some(std::mem::ManuallyDrop::new(retained));
            });
            if !was_panicking {
                panic!(
                    "App fixture cleanup is unproven; exact owners and files retained; later fixture admission refused"
                );
            }
            return; // Preserve the original assertion panic without a double panic.
        }
        if !was_panicking {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
        // On assertion unwind retain diagnostic files, even after proven join.
    }
}

#[cfg(target_os = "linux")]
#[test]
fn actual_controller_two_captures_writer_verifier_removal_join_and_timeline() {
    let mut fixture = Fixture::new();
    let source_clip = fixture.app.project.clips[0].id;
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::Complete)
    });
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(fixture.source.capture_count(), 2);
    assert!(job.source_joined);
    assert!(job.retired_source.is_none());
    assert_eq!(job.asset_ack, Some(true));
    assert!(job.retained_path.as_ref().unwrap().exists());
    assert!(
        fixture
            .app
            .project
            .clips
            .iter()
            .find(|clip| clip.id == source_clip)
            .unwrap()
            .muted
    );
    assert!(!fixture.app.project.plugin_instances[0].enabled);
    assert_eq!(
        fixture.app.project.plugin_instances[0].parameters[&17],
        0.75
    );
    assert_eq!(
        fixture.app.project.plugin_instances[0].opaque_state,
        source_state().opaque_state
    );
    assert_eq!(fixture.app.undo_stack.len(), 1);
    let before = fixture.app.undo_stack.last().unwrap();
    assert!(before.plugin_instances[0].enabled);
    assert_eq!(before.plugin_instances[0].parameters[&17], 0.75);
    assert!(
        !before
            .clips
            .iter()
            .find(|clip| clip.id == source_clip)
            .unwrap()
            .muted
    );
    assert!(!fixture.app.playing);
    assert!(fixture.app.timeline_audio_matches_desired());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_controller_cancel_after_file_commit_keeps_original_history_and_file() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::VerifyingMedia)
    });
    let job = fixture.app.clip_render_job.as_mut().unwrap();
    assert!(
        !job.export.cancel(),
        "the actual file CAS already committed"
    );
    assert!(
        job.cancel(),
        "the independent App cancellation remains open"
    );
    assert!(
        job.verifier.is_some(),
        "cancellation must retain the worker receipt"
    );
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase.terminal())
    });
    assert_eq!(fixture.bytes(), before);
    assert!(fixture.app.retained_clip_media.is_empty());
    assert_eq!(fixture.app.retained_clip_pcm_bytes, 0);
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::RetainedFile);
    assert!(job.retained_path.as_ref().unwrap().exists());
    assert!(job.verifier.is_none());
    assert_eq!(fixture.source.capture_count(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn actual_controller_unnotified_opaque_and_base_drift_refuses_after_second_capture() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::VerifyingMedia)
    });
    let mut changed = source_state();
    changed.opaque_state.push(10);
    changed.parameter_bases.insert(17, 0.625);
    fixture.source.set_capture_state(changed);
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase.terminal())
    });
    assert_eq!(fixture.source.capture_count(), 2);
    assert_eq!(fixture.bytes(), before);
    assert!(fixture.app.retained_clip_media.is_empty());
    assert_eq!(fixture.app.retained_clip_pcm_bytes, 0);
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::RetainedFile);
    assert!(
        job.failure
            .as_deref()
            .unwrap()
            .contains("second actual source capture")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn actual_controller_cancel_ready_to_commit_does_not_remove_source() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::ReadyToCommit)
    });
    assert!(fixture.app.cancel_clip_render_job());
    fixture.tick();
    assert_eq!(fixture.bytes(), before);
    assert!(fixture.app.retained_clip_media.is_empty());
    assert_eq!(fixture.app.retained_clip_pcm_bytes, 0);
    assert_eq!(fixture.app.running_generator_chains.len(), 1);
    assert!(
        fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .removal
            .is_some()
    );
    assert!(!fixture.app.clip_render_job.as_ref().unwrap().removal_queued);
}

#[cfg(target_os = "linux")]
#[test]
fn actual_controller_edit_away_and_back_is_rejected_by_app_revision() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::VerifyingMedia)
    });
    fixture.app.clip_render_model_revision += 2;
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase.terminal())
    });
    assert_eq!(fixture.bytes(), before);
    assert!(fixture.app.retained_clip_media.is_empty());
    assert_eq!(fixture.app.retained_clip_pcm_bytes, 0);
    assert!(
        fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .commit
            .is_none()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn actual_controller_cancellation_during_second_capture_retains_capture_owner() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::VerifyingMedia)
    });
    fixture.source.pause_capture();
    fixture.until(|fixture| fixture.source.capture_count() == 2);
    assert!(fixture.app.cancel_clip_render_job());
    fixture.app.drive_clip_render_job();
    assert!(
        fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .capture
            .is_some()
    );
    assert!(fixture.app.clip_render_fence_active());
    assert_eq!(fixture.bytes(), before);
    fixture.source.release_gates();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase.terminal())
    });
    assert_eq!(fixture.bytes(), before);
    assert!(matches!(
        fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .failure_evidence,
        Some(JobFailureEvidence::Capture(CapturePollError::Cancelled))
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn actual_callback_stale_project_removal_refuses_without_model_swap() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::ReadyToCommit)
    });
    fixture.app.drive_clip_render_job();
    assert!(fixture.app.clip_render_job.as_ref().unwrap().removal_queued);
    assert!(
        !fixture.app.cancel_clip_render_job(),
        "App commit CAS is now sealed"
    );
    let other_session = fixture.app.project_session + 1;
    assert!(
        fixture
            .app
            .audio
            .as_mut()
            .unwrap()
            .set_project_session(other_session)
    );
    fixture.tick();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::RetainedFile);
    assert!(job.commit.is_none());
    assert_eq!(fixture.bytes(), before);
    assert!(fixture.app.retained_clip_media.is_empty());
    assert_eq!(fixture.app.retained_clip_pcm_bytes, 0);
    assert_eq!(fixture.app.running_generator_chains.len(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn actual_controller_committed_engine_failure_stays_paused_after_status_hidden() {
    let mut fixture = Fixture::new();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::CommittedSourceJoining)
    });
    assert!(
        fixture
            .app
            .clip_render_job
            .as_ref()
            .unwrap()
            .commit
            .is_some()
    );
    let other_session = fixture.app.project_session + 1;
    assert!(
        fixture
            .app
            .audio
            .as_mut()
            .unwrap()
            .set_project_session(other_session)
    );
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::CommittedPlaybackFailedPaused)
    });
    let job = fixture.app.clip_render_job.as_mut().unwrap();
    assert!(job.source_joined);
    assert!(job.retained_path.as_ref().unwrap().exists());
    job.status_visible = false;
    assert!(fixture.app.clip_render_blocks_playback());
    assert!(!fixture.app.project.plugin_instances[0].enabled);
    assert!(!fixture.app.cancel_clip_render_job());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_controller_duplicate_start_and_unknown_removal_keep_original_owner() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    let operation = fixture.app.clip_render_job.as_ref().unwrap().operation_id;
    fixture.app.open_selected_clip_render_review();
    assert_eq!(
        fixture.app.clip_render_job.as_ref().unwrap().operation_id,
        operation
    );
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::ReadyToCommit)
    });
    fixture.app.drive_clip_render_job();
    let old_audio = fixture.app.audio.take().unwrap();
    fixture.app.drive_clip_render_job();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::Removing);
    assert!(job.removal_queued && job.history.is_some());
    assert!(job.failure.as_deref().unwrap().contains("outcome unknown"));
    assert!(fixture.app.clip_render_fence_active());
    assert_eq!(fixture.bytes(), before);
    fixture.app.audio = Some(old_audio);
    // The same real callback is allowed to deliver its actual outcome; no
    // fake success/rejection is injected to discharge fixture ownership.
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase.terminal())
    });
}

#[cfg(target_os = "linux")]
#[test]
fn actual_second_capture_backend_fault_keeps_precedence_over_app_cancel() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::VerifyingMedia)
    });
    fixture.source.pause_capture();
    fixture
        .source
        .fail_capture_after_gate("in-flight second capture backend failure".into());
    fixture.until(|fixture| fixture.source.capture_count() == 2);
    assert!(fixture.app.cancel_clip_render_job());
    fixture.source.release_gates();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase.terminal())
    });
    assert_eq!(fixture.bytes(), before);
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert!(matches!(
        job.failure_evidence,
        Some(JobFailureEvidence::Capture(CapturePollError::Backend(
            crate::instrument_bounce::BounceCaptureError::BackendFailed(_)
        )))
    ));
    assert_eq!(
        job.failure.as_deref(),
        Some("in-flight second capture backend failure")
    );
    assert!(job.retained_path.as_ref().unwrap().exists());
}

#[test]
fn intentional_assertion_unwind_releases_gate_and_joins_before_fixture_drop() {
    let mut retained_directory = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut fixture = Fixture::new();
        retained_directory = Some(fixture.directory.clone());
        fixture.source.pause_capture();
        fixture.begin();
        fixture.until(|fixture| fixture.source.capture_count() == 1);
        panic!("intentional fixture unwind");
    }));
    assert!(result.is_err());
    assert!(
        !FIXTURE_POISONED.load(Ordering::Acquire),
        "released mock owners must really terminate"
    );
    let directory = retained_directory.unwrap();
    assert!(
        directory.exists(),
        "assertion unwind preserves its diagnostic directory"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

fn actual_fault_projection(typed: bool) -> String {
    let mut fixture = Fixture::new();
    fixture.source.pause_capture();
    fixture
        .source
        .fail_capture_after_gate("actual in-flight capture fault projection".into());
    fixture.begin();
    fixture.until(|fixture| fixture.source.capture_count() == 1);
    assert!(fixture.app.cancel_clip_render_job());
    fixture.source.release_gates();
    let binding = fixture
        .app
        .clip_render_job
        .as_ref()
        .unwrap()
        .binding
        .unwrap();
    let status = fixture.app.clip_render_status(binding);
    let job = fixture.app.clip_render_job.as_mut().unwrap();
    let identity = BounceSourceIdentity {
        project_session: binding.stamp.project_session,
        project_fingerprint: project_snapshot_fingerprint(&job.review.snapshot),
        timeline_revision: binding.stamp.timeline_revision,
        epoch: binding.stamp.epoch,
        endpoint_id: binding.stamp.endpoint_id,
        channel_id: binding.stamp.channel_id,
        instance_id: binding.stamp.instance_id,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            Instant::now() < deadline,
            "actual capture projection did not terminate"
        );
        let capture = job.capture.as_mut().unwrap();
        let result = if typed {
            match capture.poll_typed(identity, status, Instant::now()) {
                Err(error) => {
                    assert!(matches!(
                        error,
                        CapturePollError::Backend(
                            crate::instrument_bounce::BounceCaptureError::BackendFailed(_)
                        )
                    ));
                    Err(error.to_string())
                }
                Ok(value) => Ok(value),
            }
        } else {
            capture.poll(identity, status, Instant::now())
        };
        match result {
            Ok(None) => std::thread::sleep(Duration::from_millis(1)),
            Ok(Some(_)) => panic!("in-flight backend fault cannot become success"),
            Err(message) => {
                job.capture = None;
                job.fail(message.clone());
                job.finish_unchanged();
                return message;
            }
        }
    }
}

#[test]
fn actual_legacy_string_capture_poll_is_display_projection_of_typed_fault() {
    assert_eq!(
        actual_fault_projection(false),
        actual_fault_projection(true)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn verifier_owner_mismatch_cannot_drain_actual_abandoned_controller_worker() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::VerifyingMedia)
    });
    let other = VerificationAdmission::default();
    assert!(other.is_busy());
    assert!(!other.owns_pending());
    let job = fixture.app.clip_render_job.as_mut().unwrap();
    drop(job.verifier.take()); // Models an unwound receipt handoff; worker retained.
    job.fail("simulated receipt handoff unwind");
    assert!(job.verification_admission.owns_pending());
    assert!(job.owns_pending_resource());
    assert!(other.poll_abandoned_owned().is_none());
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase.terminal())
    });
    assert_eq!(fixture.bytes(), before);
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert!(!job.verification_admission.owns_pending());
    assert!(job.retained_path.as_ref().unwrap().exists());
}

#[test]
fn actual_backend_fault_precedes_its_inactive_publication_without_app_cancel() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.source.pause_capture();
    fixture
        .source
        .fail_capture_after_gate("authoritative fault before inactive observation".into());
    fixture.begin();
    fixture.until(|fixture| fixture.source.capture_count() == 1);
    fixture.source.release_gates();
    // Observe the real worker's reliable inactive publication without allowing
    // another App/controller poll to consume the dedicated result first.
    let deadline = Instant::now() + Duration::from_secs(5);
    let channel_id = fixture.app.project.channels[0].id;
    let captured_fault_ready = |fixture: &Fixture| {
        fixture.app.running_generator_chains[&channel_id]
            .control
            .plugin_endpoint_snapshot()
            .is_some_and(|snapshot| snapshot.active_mask() == 0)
            && fixture
                .app
                .clip_render_job
                .as_ref()
                .unwrap()
                .capture
                .as_ref()
                .unwrap()
                .termination_acknowledged()
    };
    while !captured_fault_ready(&fixture) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(captured_fault_ready(&fixture));
    assert_eq!(
        fixture.app.running_generator_chains[&channel_id]
            .control
            .plugin_endpoint_snapshot()
            .unwrap()
            .active_mask(),
        0
    );
    fixture.app.drive_clip_render_job();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert!(!job.operation.cancelled());
    assert_eq!(
        job.failure.as_deref(),
        Some("authoritative fault before inactive observation")
    );
    assert!(job.secondary_failure.is_some());
    assert!(matches!(
        job.failure_evidence,
        Some(JobFailureEvidence::Capture(CapturePollError::Backend(
            crate::instrument_bounce::BounceCaptureError::BackendFailed(_)
        )))
    ));
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_admission_refuses_unpersisted_legacy_piano_notes() {
    let mut fixture = Fixture::new();
    assert!(fixture.app.clip_render_admission().is_ok());
    let channel_id = fixture.app.project.channels[0].id;
    fixture.app.project.piano_notes.push(PianoNote {
        id: 999,
        channel_id: Some(channel_id),
        group_id: None,
        note: 64,
        start: 0.0,
        length: 0.25,
        velocity: 0.5,
        selected: false,
        muted: false,
    });
    let before = fixture.bytes();
    let error = fixture
        .app
        .prepare_clip_render_review()
        .err()
        .expect("legacy payload must refuse review");
    assert!(error.contains("legacy Piano Roll notes"));
    assert!(fixture.app.clip_render_job.is_none());
    assert_eq!(fixture.source.capture_count(), 0);
    assert_eq!(fixture.app.project.piano_notes.len(), 1);
    assert_eq!(fixture.bytes(), before);
}

#[cfg(target_os = "linux")]
#[test]
fn actual_verified_pcm_over_budget_retains_file_without_model_or_cache_insertion() {
    let mut fixture = Fixture::new(); // Owns the shared admission before override.
    let _lower_limit = super::super::clip_render_cache::lower_test_pcm_limit(8);
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase.terminal())
    });
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(fixture.source.capture_count(), 2);
    assert_eq!(job.phase, ClipRenderPhase::RetainedFile);
    assert!(job.failure.as_deref().unwrap().contains("PCM limit"));
    assert!(job.retained_path.as_ref().unwrap().exists());
    assert!(!job.operation.accepted());
    assert!(!job.removal_queued);
    assert!(job.retained_media.is_none());
    assert!(fixture.app.retained_clip_media.is_empty());
    assert_eq!(fixture.app.retained_clip_pcm_bytes, 0);
    assert_eq!(fixture.app.running_generator_chains.len(), 1);
    assert_eq!(fixture.bytes(), before);
}

#[test]
#[cfg(not(all(target_os = "linux", feature = "experimental-fresh-helper")))]
fn actual_app_reports_unavailable_without_experimental_helper_feature() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    let channel = fixture.app.project.channels[0].id;
    let endpoint = fixture.app.running_generator_chains[&channel].endpoint_id;
    let operation = fixture.app.next_clip_render_operation_id;
    assert_eq!(
        fixture.app.clip_render_unavailable_reason().as_deref(),
        Some("Clip rendering requires Linux with experimental-fresh-helper enabled")
    );
    fixture.app.open_selected_clip_render_review();
    assert!(fixture.app.clip_render_job.is_none());
    assert_eq!(fixture.app.next_clip_render_operation_id, operation);
    assert_eq!(
        fixture.app.running_generator_chains[&channel].endpoint_id,
        endpoint
    );
    assert_eq!(fixture.source.capture_count(), 0);
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn checked_tail_uses_zero_cutoff_for_tiny_positive_seconds() {
    assert_eq!(checked_fixed_tail_frames(0.0, 48_000).unwrap(), 0);
    assert_eq!(checked_fixed_tail_frames(0.000000001, 48_000).unwrap(), 0);
    assert_eq!(
        checked_fixed_tail_frames(1.0 / 48_000.0, 48_000).unwrap(),
        1
    );
    assert_eq!(checked_fixed_tail_frames(30.0, 192_000).unwrap(), 5_760_000);
    assert!(checked_fixed_tail_frames(-0.0001, 48_000).is_err());
    assert!(checked_fixed_tail_frames(30.0001, 48_000).is_err());
    assert!(checked_fixed_tail_frames(f64::NAN, 48_000).is_err());
    assert!(checked_fixed_tail_frames(f64::INFINITY, 48_000).is_err());
    assert!(checked_fixed_tail_frames(0.0, 0).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_removal_enqueue_does_not_publish_a_new_transport_mailbox_request() {
    let mut fixture = Fixture::new();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::ReadyToCommit)
    });
    let request = fixture
        .app
        .audio
        .as_ref()
        .unwrap()
        .test_transport_request_id();
    assert_eq!(
        fixture
            .app
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .applied_transport_request,
        request
    );
    fixture.app.drive_clip_render_job();
    assert!(fixture.app.clip_render_job.as_ref().unwrap().removal_queued);
    assert_eq!(
        fixture
            .app
            .audio
            .as_ref()
            .unwrap()
            .test_transport_request_id(),
        request
    );
    fixture.pump_callback_only();
    fixture.app.poll_generator_endpoint_events();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert!(job.source_removed);
    assert!(job.commit.is_some());
    assert_eq!(
        fixture
            .app
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .applied_transport_request,
        request
    );
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::Complete)
    });
}

#[cfg(target_os = "linux")]
#[test]
fn actual_coincident_timeline_readiness_cannot_clear_committed_playback_drift_failure() {
    let mut fixture = Fixture::new();
    fixture.begin();
    fixture.until(|fixture| {
        fixture.app.clip_render_job.as_ref().is_some_and(|job| {
            job.phase == ClipRenderPhase::CommittedPlaybackPending
                && job.expected_timeline_generation.is_some()
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    // Consume real compile/asset/callback receipts without giving the controller
    // its final Ready observation yet.
    while !fixture.app.timeline_audio_matches_desired() {
        assert!(
            Instant::now() < deadline,
            "actual final Timeline did not become ready; {}",
            fixture.timeout_metadata()
        );
        fixture.pump_callback_only();
        fixture.app.poll_timeline_compile(&fixture.ctx);
        fixture.app.poll_audio_asset_events();
        fixture.app.poll_timeline_audio_runtime(&fixture.ctx);
        std::thread::sleep(Duration::from_millis(1));
    }
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::CommittedPlaybackPending);
    assert_eq!(job.asset_ack, Some(true));
    assert!(job.source_joined);
    let asset_id = job.commit.as_ref().unwrap().asset_id;
    let pcm = Arc::clone(&fixture.app.cached_audio_assets[&asset_id].samples);
    let before = fixture.bytes();
    fixture.app.playing = true; // Unexpected live App drift at the same Ready boundary.
    fixture.app.drive_clip_render_job();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::CommittedPlaybackFailedPaused);
    assert!(job.failure.as_deref().unwrap().contains("Playback changed"));
    assert_eq!(fixture.bytes(), before);
    assert!(Arc::ptr_eq(
        &pcm,
        &fixture.app.cached_audio_assets[&asset_id].samples
    ));
    assert!(fixture.app.retained_clip_media.contains_key(&asset_id));
    assert!(!fixture.app.playing);
    assert!(fixture.app.clip_render_blocks_playback());
    fixture.app.drive_clip_render_job();
    assert_eq!(
        fixture.app.clip_render_job.as_ref().unwrap().phase,
        ClipRenderPhase::CommittedPlaybackFailedPaused
    );
}

#[cfg(target_os = "linux")]
#[test]
fn actual_final_timeline_route_failure_is_sticky_after_commit_and_source_join() {
    let mut fixture = Fixture::new();
    fixture.begin();
    fixture.until(|fixture| {
        fixture.app.clip_render_job.as_ref().is_some_and(|job| {
            job.phase == ClipRenderPhase::CommittedPlaybackPending
                && job.expected_timeline_generation.is_some()
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.app.timeline_audio_matches_desired() {
        assert!(
            Instant::now() < deadline,
            "actual final Timeline did not become ready; {}",
            fixture.timeout_metadata()
        );
        fixture.pump_callback_only();
        fixture.app.poll_timeline_compile(&fixture.ctx);
        fixture.app.poll_audio_asset_events();
        fixture.app.poll_timeline_audio_runtime(&fixture.ctx);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(fixture.original_source_joined());
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.asset_ack, Some(true));
    let asset_id = job.commit.as_ref().unwrap().asset_id;
    let pcm = Arc::clone(&fixture.app.cached_audio_assets[&asset_id].samples);
    let before = fixture.bytes();

    // Compile the real persisted source assignment without the session playback
    // projection. Its actual missing endpoint must fail the ordinary route gate.
    // This reproduces the diagnosed fault without forged readiness or phase bits.
    let map = TempoMap::from_project(&fixture.app.project, 48_000).unwrap();
    let orphaned = CompiledTimeline::from_project(
        &fixture.app.project,
        &map,
        TimelineCompileOptions::default(),
    )
    .unwrap();
    assert!(orphaned.plugin_routes().iter().any(|route| {
        route.instance_id == 41
            && matches!(route.destination, PluginRouteDestination::Generator { .. })
    }));
    assert!(
        !fixture
            .app
            .ensure_timeline_generator_routes_ready(&orphaned)
    );
    assert!(matches!(
        fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::FaultClearReady { .. }
    ));
    fixture.app.drive_clip_render_job();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::CommittedPlaybackPending);
    assert!(job.failure.is_none(), "callback clear is still pending");
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::CommittedPlaybackFailedPaused)
    });
    assert_eq!(
        fixture.app.timeline_audio_sync.phase,
        TimelineAudioSyncPhase::Faulted
    );
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    let failure = job.failure.as_deref().unwrap().to_owned();
    assert!(failure.contains("no callback-confirmed Running endpoint"));
    assert!(fixture.original_source_joined());
    assert_eq!(fixture.bytes(), before);
    assert!(Arc::ptr_eq(
        &pcm,
        &fixture.app.cached_audio_assets[&asset_id].samples
    ));
    assert!(fixture.app.retained_clip_media.contains_key(&asset_id));
    assert!(!fixture.app.playing);
    assert!(fixture.app.clip_render_blocks_playback());
    fixture.app.clip_render_job.as_mut().unwrap().status_visible = false;
    fixture.tick();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::CommittedPlaybackFailedPaused);
    assert_eq!(job.failure.as_deref(), Some(failure.as_str()));
    assert_eq!(fixture.bytes(), before);
    assert!(fixture.app.clip_render_blocks_playback());
}

#[cfg(target_os = "linux")]
#[test]
fn actual_unissued_full_ring_removal_does_not_revive_after_recorded_failure() {
    let mut fixture = Fixture::new();
    let before = fixture.bytes();
    fixture.begin();
    fixture.until(|fixture| {
        fixture
            .app
            .clip_render_job
            .as_ref()
            .is_some_and(|job| job.phase == ClipRenderPhase::ReadyToCommit)
    });
    let mut full = false;
    for _ in 0..=64 {
        if !fixture
            .app
            .audio
            .as_mut()
            .unwrap()
            .command(AudioCommand::SetMaster(1.0))
        {
            full = true;
            break;
        }
    }
    assert!(full);
    fixture.app.drive_clip_render_job();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert_eq!(job.phase, ClipRenderPhase::Removing);
    assert!(job.operation.accepted());
    assert!(!job.removal_queued);
    fixture.app.playing = true;
    fixture.app.drive_clip_render_job();
    assert_eq!(
        fixture.app.clip_render_job.as_ref().unwrap().phase,
        ClipRenderPhase::RetainedFile
    );
    fixture.pump_callback_only(); // Clear queue pressure and apply the failure pause.
    fixture.app.poll_generator_endpoint_events();
    fixture.app.drive_clip_render_job();
    let job = fixture.app.clip_render_job.as_ref().unwrap();
    assert!(job.failure.as_deref().unwrap().contains("Playback changed"));
    assert!(!job.removal_queued);
    assert!(job.commit.is_none());
    assert!(!job.source_removed);
    assert_eq!(fixture.app.running_generator_chains.len(), 1);
    assert!(fixture.app.retained_clip_media.is_empty());
    assert_eq!(fixture.bytes(), before);
}
