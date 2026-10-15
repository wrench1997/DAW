//! Complete saved-project App owner for the explicit Windows NOIRE driver.
//! This module never reduces the score, creates scan metadata, or replaces a
//! native receipt with a model count. Register under windows_noire_bounce_qa.
use super::windows_noire_full_inputs::{FullInputPins, FullInputs, ValidatedSource};
use super::*;
use crate::app::stopped_bounce_capture::LiveBinding;
use crate::export_options::WavExportOptions;
use crate::instrument_bounce::render::{MasterSongRenderOutcome, MasterSongRenderReport};
use crate::model::ProjectMigrationDiagnostic;
use crate::plugins::plugin_runtime::BounceSourceSettlement;

/// The single permitted model observation change during source initialization
/// is its session-only runtime_status. Persisted data and every other skipped
/// field remain exact, including selection and dormant asset waveform bits.
#[derive(PartialEq)]
struct FullProjectImage {
    persisted: Vec<u8>,
    legacy_notes: Vec<u8>,
    legacy_selection: Vec<bool>,
    pattern_selection: Vec<Vec<bool>>,
    runtime_statuses: Vec<(u64, PluginRuntimeStatus)>,
    waveform_bits: Vec<(u64, Vec<u32>)>,
    migrations: Vec<ProjectMigrationDiagnostic>,
}
impl FullProjectImage {
    fn take(project: &Project, source_runtime: Option<u64>) -> Check<Self> {
        Ok(Self {
            persisted: serde_json::to_vec(project).map_err(error)?,
            legacy_notes: serde_json::to_vec(&project.piano_notes).map_err(error)?,
            legacy_selection: project
                .piano_notes
                .iter()
                .map(|note| note.selected)
                .collect(),
            pattern_selection: project
                .patterns
                .iter()
                .map(|pattern| pattern.notes.iter().map(|note| note.selected).collect())
                .collect(),
            runtime_statuses: project
                .plugin_instances
                .iter()
                .filter(|instance| Some(instance.id) != source_runtime)
                .map(|instance| (instance.id, instance.runtime_status))
                .collect(),
            waveform_bits: project
                .audio_assets
                .iter()
                .map(|asset| {
                    (
                        asset.id,
                        asset
                            .waveform_peaks
                            .iter()
                            .map(|value| value.to_bits())
                            .collect(),
                    )
                })
                .collect(),
            migrations: project.migration_diagnostics.clone(),
        })
    }
}

#[derive(PartialEq)]
struct FullHistoryImage {
    project: FullProjectImage,
    history: FullProjectImage,
    undo: Vec<FullProjectImage>,
    redo: Vec<FullProjectImage>,
    history_fingerprint: u64,
    project_fingerprint: u64,
    dirty: bool,
    saved_path: Option<PathBuf>,
}
impl FullHistoryImage {
    fn take(app: &CitrusApp, source: u64) -> Check<Self> {
        Ok(Self {
            project: FullProjectImage::take(&app.project, Some(source))?,
            history: FullProjectImage::take(&app.history_snapshot, None)?,
            undo: app
                .undo_stack
                .iter()
                .map(|project| FullProjectImage::take(project, None))
                .collect::<Check<_>>()?,
            redo: app
                .redo_stack
                .iter()
                .map(|project| FullProjectImage::take(project, None))
                .collect::<Check<_>>()?,
            history_fingerprint: app.history_fingerprint,
            project_fingerprint: app.project_fingerprint,
            dirty: app.dirty,
            saved_path: app.saved_path.clone(),
        })
    }
    fn unchanged(&self, app: &CitrusApp, source: u64) -> Check {
        require(
            Self::take(app, source)? == *self,
            "complete Project, history, dirty state or saved path changed during the native workflow",
        )
    }
}

/// Validate a persisted request, not loaded metadata. The short source probe
/// must obtain factory/version/topology evidence from this run's actual worker.
fn requested_saved_source_intent(
    project: &Project,
    source: &ValidatedSource,
) -> Check<(String, PathBuf, String)> {
    require(
        source.plugin_instance_id != 0 && source.channel_id != 0,
        "complete source has an invalid stable identity",
    )?;
    let mut instances = project
        .plugin_instances
        .iter()
        .filter(|instance| instance.id == source.plugin_instance_id);
    let instance = instances
        .next()
        .ok_or("complete source instance is absent")?;
    require(
        instances.next().is_none(),
        "complete source instance identity is ambiguous",
    )?;
    let mut channels = project
        .channels
        .iter()
        .filter(|channel| channel.id == source.channel_id);
    let channel = channels.next().ok_or("complete source channel is absent")?;
    require(
        channels.next().is_none()
            && channel.instrument_plugin_instance_id == Some(instance.id)
            && project
                .channels
                .iter()
                .filter(|item| item.instrument_plugin_instance_id == Some(instance.id))
                .count()
                == 1,
        "complete source channel assignment is absent or ambiguous",
    )?;
    let persisted = plugins::plugin_load_identity_from_instance(instance).descriptor;
    require(
        instance.uid == KONTAKT_UID
            && instance.format == ModelPluginFormat::Vst3
            && instance.role == PluginRole::Instrument
            && instance.enabled
            && !instance.bypass
            && instance.wet.to_bits() == 1.0_f32.to_bits()
            && persisted.id == source.descriptor.id
            && persisted.path == source.descriptor.path
            && persisted.format == source.descriptor.format
            && instance.opaque_state.len() == source.state_bytes
            && oracles::sha256_hex(&instance.opaque_state) == source.state_sha256,
        "complete source changed, or is not supported by actual fresh-source initialization",
    )?;
    state_envelope(&instance.opaque_state)?;
    require(
        project.mixer_runtime_slot(channel.mixer_track).is_some(),
        "complete source has no valid mixer destination",
    )?;
    project.validate_mixer_graph().map_err(error)?;
    require(
        !persisted.id.is_empty()
            && persisted.id.len() <= 4096
            && persisted.path.is_absolute()
            && persisted.path.as_os_str().len() <= 4096,
        "complete fresh-source descriptor exceeds its identity bounds",
    )?;
    Ok((persisted.id, persisted.path, instance.uid.clone()))
}

/// Match the persisted source using stable IDs. The original full selector
/// still requires its separate checked, healthy loaded-default scan descriptor.
fn complete_source_intent(
    project: &Project,
    source: &ValidatedSource,
    scanned: &PluginDescriptor,
) -> Check<(String, PathBuf, String)> {
    let (descriptor_id, path, requested_uid) = requested_saved_source_intent(project, source)?;
    let metadata = scanned
        .vst3_metadata
        .as_ref()
        .ok_or("complete source requires an actual scanner result with VST3 factory metadata")?;
    require(
        scanned.verified
            && scanned.scan_error.is_none()
            && scanned.format == plugins::PluginFormat::Vst3
            && scanned.is_instrument
            && scanned.path == path
            && metadata.class_uid == requested_uid
            && metadata.class_uid.len() == 32
            && metadata
                .class_uid
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && metadata
                .category
                .split('|')
                .any(|part| part.trim() == "Instrument")
            && metadata.has_midi_input,
        "actual scan descriptor does not match the complete persisted Kontakt source",
    )?;
    Ok((descriptor_id, path, metadata.class_uid.clone()))
}

fn register_and_enqueue_complete_source(
    app: &mut CitrusApp,
    source: &ValidatedSource,
    scanned: &PluginDescriptor,
    enqueue: impl FnOnce(&mut CitrusApp, u32, Project, Option<ChannelGeneratorState>) -> Check,
) -> Check {
    let intent = complete_source_intent(&app.project, source, scanned)?;
    register_and_enqueue_source_intent(app, source, intent, enqueue)
}

fn register_and_enqueue_source_intent(
    app: &mut CitrusApp,
    source: &ValidatedSource,
    intent: (String, PathBuf, String),
    enqueue: impl FnOnce(&mut CitrusApp, u32, Project, Option<ChannelGeneratorState>) -> Check,
) -> Check {
    require(
        cfg!(all(windows, feature = "experimental-fresh-helper")),
        "complete source initialization requires the explicit Windows experimental fresh-helper build",
    )?;
    require(
        !app.fresh_bounce_sources
            .contains_key(&source.plugin_instance_id)
            && !app
                .clip_render_restorations
                .contains_key(&source.plugin_instance_id)
            && !app
                .pending_generator_chains
                .contains_key(&source.channel_id)
            && !app
                .running_generator_chains
                .contains_key(&source.channel_id)
            && !app
                .pending_generator_removals
                .contains_key(&source.channel_id),
        "complete source already has an owner, preparation or callback request",
    )?;
    require(
        app.fresh_bounce_sources.len() < 4096,
        "complete source registry has reached its existing 4096-entry bound",
    )?;
    app.fresh_bounce_sources.try_reserve(1).map_err(error)?;
    let previous = snapshot_channel_generator(&app.project, source.channel_id)
        .ok_or("complete source channel disappeared before initialization")?;
    let candidate = app.project.clone();
    let (descriptor_id, path, class_uid) = intent;
    app.fresh_bounce_sources.insert(
        source.plugin_instance_id,
        (descriptor_id, path, FreshSourceClass::Exact(class_uid)),
    );
    // A failed queue may already own a spawned worker in retirement storage.
    // Keep the session intent; never discard its guard or call failure joined.
    enqueue(app, source.channel_id, candidate, Some(previous))
}

fn exact_complete_registry(
    source: &ValidatedSource,
    session: u64,
    mixer_slot: usize,
    observed: (u64, u32, u64, u64, usize, usize),
) -> Check {
    let (actual_session, channel, endpoint, instance, actual_mixer, worker) = observed;
    require(
        actual_session == session
            && session != 0
            && channel == source.channel_id
            && endpoint != 0
            && instance == source.plugin_instance_id
            && actual_mixer == mixer_slot
            && worker != 0,
        "callback-confirmed complete source registry is bound to a different identity",
    )
}

pub(super) struct FullReadySource {
    pub binding: LiveBinding,
    pub actual_worker_id: usize,
}

/// Run-local device-free owner of its admitted Project. The full selector passes
/// the entire Saved13; the separately named short probe passes its private derivative.
/// The observer is installed and owned by the outer run, never here.
pub(super) struct FullParent {
    app: Option<Box<CitrusApp>>,
    callback: Option<audio::TestAudioCallback>,
    pub ctx: egui::Context,
    pub source: ValidatedSource,
    pub pins: FullInputPins,
    original: FullHistoryImage,
    initialization_requested: bool,
    joins_complete: bool,
    pub pumps: usize,
    pub callback_segments: usize,
    pub lifecycle_receipts: usize,
}
impl FullParent {
    pub(super) fn new_retained(
        inputs: FullInputs,
        callback_token: [u8; 32],
    ) -> Check<RetainedFullParent> {
        require(
            !FULL_OWNER_ADMITTED.swap(true, Ordering::AcqRel),
            "only one complete native owner may be admitted per qualification process",
        )?;
        let FullInputs {
            project,
            source,
            pins,
        } = inputs;
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        // This new test App has no earlier user session. Establish the loaded
        // full input as its initial history, rather than laundering a reduced
        // scratch or clearing an already-existing App's undo/redo stack.
        app.history_snapshot = project.clone();
        app.history_fingerprint = project_fingerprint(&project);
        app.project_fingerprint = app.history_fingerprint;
        app.project = project;
        app.saved_path = Some(pins.project_path.clone());
        app.transport_mode = TransportMode::Song;
        app.dirty = false;
        let (engine, mut callback) = AudioEngine::test_engine_with_callback();
        callback
            .enable_genuine_observation(callback_token)
            .map_err(error)?;
        app.audio = Some(engine);
        let original = FullHistoryImage::take(&app, source.plugin_instance_id)?;
        let owner = Self {
            app: Some(app),
            callback: Some(callback),
            ctx,
            source,
            pins,
            original,
            initialization_requested: false,
            joins_complete: false,
            pumps: 0,
            callback_segments: 0,
            lifecycle_receipts: 0,
        };
        owner.inputs_unchanged()?;
        Ok(RetainedFullParent {
            owner: Some(ManuallyDrop::new(owner)),
        })
    }
    pub(super) fn app(&self) -> &CitrusApp {
        self.app.as_deref().expect("complete App remains owned")
    }
    pub(super) fn app_mut(&mut self) -> &mut CitrusApp {
        self.app.as_deref_mut().expect("complete App remains owned")
    }
    pub(super) fn callback(&self) -> Check<&audio::TestAudioCallback> {
        self.callback
            .as_ref()
            .ok_or("complete actual callback is no longer owned".into())
    }
    /// Actual callback destruction and guard joins, independently of the
    /// final model/input checks. A caller can fail qualification after this
    /// boundary without mistaking a model error for a still-live resource.
    pub(super) fn resources_joined(&self) -> bool {
        self.joins_complete
    }
    pub(super) fn paused(&self) -> bool {
        let app = self.app();
        let Some((generation, epoch)) = app
            .audio
            .as_ref()
            .and_then(AudioEngine::confirmed_timeline_identity)
        else {
            return false;
        };
        let Some(callback) = self.callback.as_ref() else {
            return false;
        };
        let snapshot = callback.genuine_snapshot();
        app.timeline_audio_matches_desired()
            && generation == app.timeline_desired_generation
            && !app.playing
            && !snapshot.playing
            && snapshot.generation == Some(generation)
            && snapshot.active_epoch == Some(epoch)
            && snapshot.epoch == epoch
            && snapshot.paused_work_quiescent(generation, epoch)
    }
    /// A fixed read-only boundary snapshot, not another poll, pump or SDK call.
    /// Keep missing callback/identity distinct from an observed false predicate.
    pub(super) fn source_readiness_evidence(&self) -> Value {
        let app = self.app();
        let state = app.genuine_timeline_app_state();
        let bounded_text = |text: &str| {
            let mut end = text.len().min(4096);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            json!({"text":&text[..end],"utf8_bytes":text.len(),"truncated":end<text.len()})
        };
        let callback = self.callback.as_ref().map(|callback| {
            let snapshot = callback.genuine_snapshot();
            let expected = state.confirmed_identity;
            let endpoints = snapshot.endpoints.iter().take(4).map(|endpoint| {
                let bridge = &endpoint.fixed.bridge;
                json!({"endpoint":endpoint.endpoint_id,"channel":endpoint.channel_id,
                    "instance":endpoint.plugin_instance_id,"worker":bridge.worker_id,
                    "callback_work_pending":endpoint.callback_work_pending(),
                    "input_work_quiescent":expected.map(|(_,epoch)|endpoint.input_work_quiescent(epoch)),
                    "callback_epoch":endpoint.fixed.epoch,"bridge_epoch":bridge.stats.current_epoch,
                    "worker_epoch":bridge.worker_epoch,"worker_busy":bridge.worker_busy,
                    "input_pending":bridge.input_pending,"submitted":bridge.stats.submitted,
                    "input_consumed":bridge.input_consumed,"operations_completed":bridge.operations_completed})
            }).collect::<Vec<_>>();
            json!({"generation":snapshot.generation,"active_epoch":snapshot.active_epoch,
                "epoch":snapshot.epoch,"frame":snapshot.start_frame,"playing":snapshot.playing,
                "callback_request_id":snapshot.request_id,
                "mailbox_request_id":app.audio.as_ref().map(AudioEngine::test_transport_request_id),
                "callback_work_pending":snapshot.callback_work_pending(),
                "pending_audio_commands":snapshot.pending_audio_commands,
                "pending_timeline_commands":snapshot.pending_timeline_commands,
                "pending_activation":snapshot.pending_timeline_activation,
                "fault":snapshot.fault.map(|fault|format!("{fault:?}")),"suspended":snapshot.suspended,
                "graph_binding_exact":snapshot.graph_binding_exact,
                "channel_binding_revision":snapshot.channel_binding_revision,
                "channel_binding_epoch":snapshot.channel_binding_epoch,
                "plugin_topology_replan_pending":snapshot.plugin_topology_replan_pending,
                "timeline_needs_resync":snapshot.timeline_stats.map(|stats|stats.ownership_needs_resync),
                "paused_work_quiescent":expected.map(|(generation,epoch)|snapshot.paused_work_quiescent(generation,epoch)),
                "endpoint_count":snapshot.endpoints.len(),"endpoint_row_limit":4,
                "endpoints_truncated":snapshot.endpoints.len()>endpoints.len(),"endpoints":endpoints})
        });
        json!({"read_only":true,"adds_callback_or_sdk_calls":false,
            "exact_paused":self.paused(),"app_playing":app.playing,
            "app_timeline_matches_desired":app.timeline_audio_matches_desired(),
            "runtime_dispatch_allowed":app.audio_restart_state.allows_runtime_dispatch(),
            "phase":format!("{:?}",state.phase),"project_session":state.project_session,
            "engine_session":state.engine_session,"sync_session":state.sync_session,
            "desired_generation":state.desired_generation,"observed_fingerprint":state.observed_fingerprint,
            "compiled_generation":state.compiled_generation,"compiled_fingerprint":state.compiled_fingerprint,
            "source_generation":state.source_generation,"source_fingerprint":state.source_fingerprint,
            "sync_source_generation":state.sync_source_generation,"confirmed_identity":state.confirmed_identity,
            "desired_seek_serial":state.desired_seek_serial,"desired_seek_frame":state.desired_seek_frame,
            "retry_required":state.retry_required,"retry_pending":state.retry_pending,
            "timeline_worker_available":app.timeline_worker_available,
            "timeline_pending_generation":app.timeline_pending_generation,
            "timeline_last_enqueued_generation":app.timeline_last_enqueued_generation,
            "timeline_deferred_job_present":app.timeline_deferred_job.is_some(),
            "timeline_compile_error":app.timeline_compile_error.as_deref().map(bounded_text),
            "timeline_sync_error":app.timeline_audio_sync.last_error.as_deref().map(bounded_text),
            "pending_generator_count":app.pending_generator_chains.len(),
            "running_generator_count":app.running_generator_chains.len(),
            "pumps":self.pumps,"callback":callback})
    }
    pub(super) fn model_unchanged(&self) -> Check {
        self.original
            .unchanged(self.app(), self.source.plugin_instance_id)
    }
    pub(super) fn inputs_unchanged(&self) -> Check {
        let bytes = read_bounded(&self.pins.project_path, 2 * 1024 * 1024)?;
        require(
            bytes.len() == self.pins.project_bytes
                && oracles::sha256_hex(&bytes) == self.pins.project_sha256,
            "complete original saved input changed after admission",
        )?;
        require(
            self.source.state_bytes == self.pins.saved_state_bytes
                && self.source.state_sha256 == self.pins.saved_state_sha256
                && self.source.descriptor.path.canonicalize().map_err(error)?
                    == self.pins.actual_bundle_path,
            "complete input source state or actual bundle binding changed",
        )
    }
    pub(super) fn begin_source_initialization(
        &mut self,
        scanned: &PluginDescriptor,
        observer: &mut ObservationRun,
        deadline: Instant,
    ) -> Check {
        self.begin_source_initialization_admitted(observer, deadline, |project, source| {
            complete_source_intent(project, source, scanned)
        })
    }
    /// Separate short-probe admission: the saved class is only a load request.
    /// No scan metadata is manufactured, and readiness still needs the actual
    /// source worker, callback, metadata and stopped-source guards below.
    pub(super) fn begin_saved_source_initialization(
        &mut self,
        observer: &mut ObservationRun,
        deadline: Instant,
    ) -> Check {
        self.begin_source_initialization_admitted(observer, deadline, requested_saved_source_intent)
    }
    fn begin_source_initialization_admitted(
        &mut self,
        observer: &mut ObservationRun,
        deadline: Instant,
        admission: impl FnOnce(&Project, &ValidatedSource) -> Check<(String, PathBuf, String)>,
    ) -> Check {
        require(
            !self.initialization_requested,
            "complete source initialization must not be retried on the same owner",
        )?;
        require(
            !self.app().playing && !self.app().transport_clock_playing,
            "complete source initialization requires the original App to stay paused",
        )?;
        self.inputs_unchanged()?;
        self.model_unchanged()?;
        let intent = admission(&self.app().project, &self.source)?;
        observer.set_stage(Stage::InitialSource).map_err(error)?;
        self.initialization_requested = true;
        let session = self.app().project_session;
        require(
            self.app_mut()
                .audio
                .as_mut()
                .ok_or("complete audio engine is absent")?
                .set_project_session(session),
            "actual complete project-session command rejected",
        )?;
        self.app_mut().rebuild_tempo_map();
        self.app_mut().request_immediate_timeline_compile();
        self.service_initial_mixer_commands(deadline)?;
        let source = &self.source;
        let app = self.app.as_deref_mut().unwrap();
        register_and_enqueue_source_intent(
            app,
            source,
            intent,
            |app, channel, candidate, previous| {
                app.queue_channel_generator_candidate(channel, candidate, previous)
            },
        )?;
        self.model_unchanged()
    }
    fn service_initial_mixer_commands(&mut self, deadline: Instant) -> Check {
        // The complete Project already names its generator, but no pending
        // source exists yet. A full Timeline poll here would permanently fault
        // that route as absent. Drain only these real mixer commands first:
        // enqueueing an unprepared source ahead of them can block the 64-slot
        // callback queue. Native source admission stays AFTER this prelude.
        for command in mixer_control_commands(&self.app().project) {
            require(
                self.app_mut().audio.as_mut().unwrap().command(command),
                "actual complete mixer command admission failed",
            )?;
            require(
                Instant::now() < deadline && self.pumps < MAX_PUMPS,
                "complete source polling deadline or callback pump bound exceeded",
            )?;
            self.pump_callback_work()?;
        }
        Ok(())
    }
    pub(super) fn wait_source_initialization(
        &mut self,
        observer: &ObservationRun,
        deadline: Instant,
    ) -> Check<FullReadySource> {
        loop {
            observer.with_snapshot(|snapshot| {
                require(
                    snapshot.failure.is_none(),
                    snapshot.failure.unwrap_or("complete observer failed"),
                )
            })?;
            match self.source_ready() {
                Ok(ready) => return Ok(ready),
                Err(reason) => {
                    if Instant::now() >= deadline {
                        return Err(format!("complete source initialization deadline: {reason}"));
                    }
                    if self.app().project.plugin_instances.iter().any(|instance| {
                        instance.id == self.source.plugin_instance_id
                            && instance.runtime_status == PluginRuntimeStatus::Crashed
                    }) {
                        return Err(format!("complete source initialization faulted: {reason}"));
                    }
                }
            }
            self.tick(deadline)?;
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    /// Drive existing production poll methods and the real device-free audio
    /// callback. No Installed/SlotReady/Timeline/native receipt is synthesized.
    pub(super) fn tick(&mut self, deadline: Instant) -> Check {
        // Full snapshots include opaque state and history. Validate them at
        // admission/ready/cleanup boundaries rather than repeatedly copying
        // the whole score while an asynchronous worker is still loading.
        self.drive_tick(deadline)
    }
    fn drive_tick(&mut self, deadline: Instant) -> Check {
        require(
            Instant::now() < deadline && self.pumps < MAX_PUMPS,
            "complete source polling deadline or callback pump bound exceeded",
        )?;
        let ctx = self.ctx.clone();
        let app = self.app_mut();
        app.drive_clip_session_retirement();
        app.poll_clip_restorations();
        app.poll_generator_endpoint_events();
        app.poll_plugin_runtime_events();
        app.poll_plugin_parameter_worker_receipts();
        app.poll_plugin_parameter_callback_receipts();
        app.drive_plugin_parameter_edit_drafts();
        app.poll_native_editor_snapshots();
        app.poll_plugin_processing_health();
        app.poll_timeline_compile(&ctx);
        app.poll_audio_asset_events();
        app.poll_timeline_audio_runtime(&ctx);
        app.drive_clip_render_job();
        app.drive_whole_song_export();
        self.pump_callback_work()
    }
    /// The unchanged live-work pump decision shared by the mixer-only prelude
    /// and full control polls. Worker-only settlement never grants a new pump.
    fn pump_callback_work(&mut self) -> Check {
        let callback = self
            .callback
            .as_mut()
            .ok_or("complete callback owner is absent")?;
        let app = self.app.as_ref().unwrap();
        let engine = app
            .audio
            .as_ref()
            .ok_or("complete audio engine disappeared")?;
        let snapshot = callback.genuine_snapshot();
        if snapshot.callback_work_pending()
            || snapshot.request_id != engine.test_transport_request_id()
        {
            callback.pump(engine, 128);
            self.pumps += 1;
            self.callback_segments += callback.take_genuine_segments().len();
            self.lifecycle_receipts += callback.take_genuine_lifecycle_receipts().len();
        }
        Ok(())
    }
    /// A running map alone is insufficient: the production stopped-source
    /// validator checks SlotReady, coherent active metadata, native quiescence,
    /// exact route, source health and acknowledged current Timeline epoch.
    pub(super) fn source_ready(&self) -> Check<FullReadySource> {
        require(
            self.initialization_requested,
            "complete source was never submitted for initialization",
        )?;
        require(
            self.paused(),
            "complete actual callback has not reached its exact paused Timeline epoch",
        )?;
        let app = self.app();
        let binding = app.stopped_bounce_observe(self.source.channel_id)?;
        let source = app
            .running_generator_chains
            .get(&self.source.channel_id)
            .ok_or("complete source has no callback-confirmed running owner")?;
        require(
            source.guard.requires_retained_fresh_shutdown(),
            "complete source did not use the actual fresh-worker owner path",
        )?;
        let worker = source.guard.genuine_worker_id();
        let channel = app
            .project
            .channels
            .iter()
            .find(|item| item.id == self.source.channel_id)
            .ok_or("complete source channel disappeared")?;
        exact_complete_registry(
            &self.source,
            app.project_session,
            app.project
                .mixer_runtime_slot(channel.mixer_track)
                .ok_or("complete mixer route disappeared")?,
            (
                source.project_session,
                binding.stamp.channel_id,
                source.endpoint_id,
                source.instance_id,
                source.mixer_track,
                worker,
            ),
        )?;
        require(
            binding.stamp.instance_id == self.source.plugin_instance_id
                && binding.stamp.endpoint_id == source.endpoint_id,
            "actual stopped-source receipt does not match the complete source owner",
        )?;
        self.model_unchanged()?;
        Ok(FullReadySource {
            binding,
            actual_worker_id: worker,
        })
    }
    /// Submit the unchanged complete Project through the production review,
    /// stopped-source capture and whole-song controller. Consent to the fixed
    /// tail is explicit at the caller; this backend test does not click a UI
    /// checkbox or claim that a native destination dialog was exercised.
    pub(super) fn begin_full_export(
        &mut self,
        observer: &mut ObservationRun,
        path: PathBuf,
        options: WavExportOptions,
        confirmed_fixed_tail: bool,
    ) -> Check<LiveBinding> {
        self.model_unchanged()?;
        self.inputs_unchanged()?;
        require(
            self.app().whole_song_export_job.is_none()
                && self.app().clip_render_job.is_none()
                && self.app().clip_session_retirement.is_none(),
            "complete export must start once, without an earlier render or retirement owner",
        )?;
        require(
            path.is_absolute() && !path.exists() && path.parent().is_some_and(Path::is_dir),
            "complete export requires a new absolute destination in an existing private directory",
        )?;
        let ready = self.source_ready()?;
        let review = self
            .app()
            .prepare_whole_song_export_review(options, confirmed_fixed_tail)?;
        observer.set_stage(Stage::WholeExport).map_err(error)?;
        self.app_mut().begin_whole_song_export(review, path)?;
        require(
            self.app()
                .whole_song_export_job
                .as_ref()
                .is_some_and(|job| job.genuine_whole_production_factory_selected()),
            "complete export selected a replacement render starter instead of the production strict backend",
        )?;
        self.model_unchanged()?;
        Ok(ready.binding)
    }
    /// Await the production typed publication outcome. A WAV file or an idle
    /// fence never substitutes for actual capture/render/disposal termination.
    pub(super) fn wait_full_export(
        &mut self,
        observer: &ObservationRun,
        expected_source: LiveBinding,
        expected_path: &Path,
        deadline: Instant,
    ) -> Check<MasterSongRenderReport> {
        loop {
            observer.with_snapshot(|snapshot| {
                require(
                    snapshot.failure.is_none(),
                    snapshot
                        .failure
                        .unwrap_or("complete export observer failed"),
                )
            })?;
            let job = self
                .app()
                .whole_song_export_job
                .as_ref()
                .ok_or("complete production export owner disappeared")?;
            require(
                job.genuine_whole_production_factory_selected(),
                "complete export no longer owns the production strict backend factory",
            )?;
            if let Some(failure) = job.genuine_whole_failure() {
                return Err(failure.to_owned());
            }
            if !self.app().whole_song_export_fence_active() {
                require(
                    job.genuine_whole_terminal_owners_drained(),
                    "complete export published before capture/render/disposal owners drained",
                )?;
                require(
                    job.genuine_whole_path() == expected_path,
                    "complete export output path differs from the reviewed private destination",
                )?;
                let report = match job.genuine_whole_outcome() {
                    Some(MasterSongRenderOutcome::Complete(report)) => report.clone(),
                    Some(MasterSongRenderOutcome::Failed(failure)) => {
                        return Err(error(&failure.primary));
                    }
                    None => {
                        return Err("complete export has no typed publication outcome".into());
                    }
                };
                self.full_export_ownership_check(expected_source)?;
                self.model_unchanged()?;
                self.inputs_unchanged()?;
                // This is the actual production typed report received by the
                // full driver, after controller owner retirement. No backend
                // query is performed to manufacture this observation.
                crate::genuine_app_observer::record_render_report(&report.dry_render);
                return Ok(report);
            }
            self.drive_tick(deadline)?;
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    pub(super) fn full_export_ownership_check(&self, expected: LiveBinding) -> Check {
        let app = self.app();
        let job = app
            .whole_song_export_job
            .as_ref()
            .ok_or("complete export owner is absent at its ownership boundary")?;
        let binding = job
            .genuine_whole_binding()
            .ok_or("complete export has no actual stopped-source binding")?;
        let stop = job
            .genuine_whole_stop_preparation()
            .ok_or("complete export has no actual stop preparation")?;
        let activation = stop
            .activation
            .ok_or("complete export has no actual stop queue receipt")?;
        let target = job
            .genuine_whole_source_target()
            .ok_or("complete export has no actual source settlement target")?;
        let settled = job
            .genuine_whole_settlement()
            .ok_or("complete export has no actual source settlement observation")?;
        require(
            binding.stamp.project_session == expected.stamp.project_session
                && binding.stamp.engine_session == expected.stamp.engine_session
                && binding.stamp.endpoint_id == expected.stamp.endpoint_id
                && binding.stamp.instance_id == expected.stamp.instance_id
                && binding.stamp.channel_id == expected.stamp.channel_id
                && binding.stamp.timeline_revision == expected.stamp.timeline_revision
                && binding.sample_rate == expected.sample_rate,
            "complete export changed the exact source or full Timeline binding",
        )?;
        require(
            !stop.superseded
                && activation.committed_epoch == Some(binding.stamp.epoch)
                && stop.generation == binding.stamp.timeline_revision
                && job.genuine_whole_old_epoch() != Some(binding.stamp.epoch)
                && settled.settlement == BounceSourceSettlement::Ready
                && settled.coherent
                && settled.callback_ready
                && settled.callback_identity == Some(target)
                && settled.worker_valid
                && !settled.exhausted
                && settled.busy == 0
                && settled.parameter_admission == 0
                && !settled.capture_busy,
            "complete export lacked actual stop confirmation, source drain or worker settlement",
        )?;
        require(
            app.running_generator_chains.len() == 1
                && app.pending_generator_chains.is_empty()
                && app.pending_generator_removals.is_empty()
                && app.clip_render_job.is_none()
                && self.paused(),
            "complete export did not preserve its paused original source without MIDI conversion",
        )?;
        self.model_unchanged()
    }
    /// A stop/failure does not commit a candidate or reset history. Actual
    /// guards remain in App ownership until cleanup completes a real join.
    pub(super) fn keep_paused(&mut self) -> Check {
        let app = self.app_mut();
        app.playing = false;
        app.transport_clock_playing = false;
        if let Some(audio) = &app.audio {
            audio.set_playing(false);
        }
        self.model_unchanged()
    }
    pub(super) fn cleanup(&mut self, observer: &mut ObservationRun, deadline: Instant) -> Check {
        let mut failure = observer.set_stage(Stage::Cleanup).map_err(error).err();
        if let Err(reason) = self.keep_paused() {
            failure.get_or_insert(reason);
        }
        self.app_mut().cancel_clip_render_job();
        self.app_mut().cancel_whole_song_export();
        while self.app().whole_song_export_fence_active()
            || self
                .app()
                .clip_render_job
                .as_ref()
                .is_some_and(ClipRenderJob::owns_pending_resource)
            || !self.app().pending_generator_removals.is_empty()
        {
            self.drive_tick(deadline)?;
            std::thread::sleep(Duration::from_millis(8));
        }
        require(
            self.app()
                .whole_song_export_job
                .as_ref()
                .is_none_or(|job| job.genuine_whole_terminal_owners_drained())
                && self.app().deferred_generator_rebuilds.is_empty()
                && self.app().pending_clip_restorations.is_empty()
                && self.app().clip_session_retirement.is_none(),
            "complete cleanup still owns deferred construction, capture, renderer or restoration",
        )?;
        // Destroy the actual callback endpoint before asking the worker guards
        // to finish; empty OS process lists never stand in for JoinHandle::join.
        self.callback.take();
        loop {
            require(
                Instant::now() < deadline,
                "complete actual worker joins remain pending",
            )?;
            let app = self.app_mut();
            let mut pending = false;
            for source in app.running_generator_chains.values_mut() {
                pending |= full_poll_guard(&mut source.guard, &mut failure);
            }
            for source in app.pending_generator_chains.values_mut() {
                pending |= full_poll_guard(&mut source.guard, &mut failure);
            }
            for chain in app.running_insert_chains.values_mut() {
                pending |= full_poll_guard(&mut chain.guard, &mut failure);
            }
            for chain in app.pending_insert_chains.values_mut() {
                pending |= full_poll_guard(&mut chain.guard, &mut failure);
            }
            for guard in &mut app.retired_fresh_source_candidates {
                pending |= full_poll_guard(guard, &mut failure);
            }
            for guard in &mut app.retired_clip_restorations {
                pending |= full_poll_guard(guard, &mut failure);
            }
            if !pending {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Resource completion is independent of final model/input equality.
        // A changed model still fails qualification after actual joins.
        self.joins_complete = true;
        self.model_unchanged()?;
        self.inputs_unchanged()?;
        failure.map_or(Ok(()), Err)
    }
}

static FULL_OWNER_ADMITTED: AtomicBool = AtomicBool::new(false);

thread_local! {
    // One qualification process owns one full native attempt. A deadline,
    // panic or failed cleanup retains the entire owner for diagnosis instead
    // of invoking App Drop or interpreting missing OS processes as a join.
    static FULL_OWNER_QUARANTINED: std::cell::RefCell<Option<ManuallyDrop<FullParent>>>
        = const { std::cell::RefCell::new(None) };
}

pub(super) struct RetainedFullParent {
    owner: Option<ManuallyDrop<FullParent>>,
}

impl std::ops::Deref for RetainedFullParent {
    type Target = FullParent;

    fn deref(&self) -> &Self::Target {
        self.owner.as_ref().expect("complete native owner retained")
    }
}

impl std::ops::DerefMut for RetainedFullParent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.owner.as_mut().expect("complete native owner retained")
    }
}

impl Drop for RetainedFullParent {
    fn drop(&mut self) {
        let Some(mut owner) = self.owner.take() else {
            return;
        };
        if !owner.initialization_requested || owner.joins_complete {
            // No native initialization was requested, or the actual callback
            // was destroyed and every retained guard has completed its join.
            owner.callback.take();
            unsafe { ManuallyDrop::drop(&mut owner) };
            return;
        }
        // A second owner cannot normally exist because admission is once-only.
        // Keep ManuallyDrop even if TLS is unavailable during thread exit, or
        // the slot is busy: this never becomes permission to release resources.
        let mut pending = Some(owner);
        let _ = FULL_OWNER_QUARANTINED.try_with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut()
                && slot.is_none()
            {
                *slot = pending.take();
            }
        });
    }
}

fn full_poll_guard(guard: &mut PluginWorkerGuard, failure: &mut Option<String>) -> bool {
    match guard.poll_shutdown_retained() {
        RetainedShutdownPoll::Pending => true,
        RetainedShutdownPoll::Joined => false,
        RetainedShutdownPoll::WorkerPanicked => {
            failure.get_or_insert_with(|| {
                "complete actual worker joined after a shutdown panic".into()
            });
            false
        }
    }
}

#[cfg(test)]
include!("windows_noire_full_owner_tests.rs");
