//! Experimental whole-song export owner; not qualified as an App/NI feature.
//! The shared stopped-capture actor owns real callback/worker provenance. This
//! job owns the frozen master request and the only final file-publication CAS.
//! Normal completion never removes/reloads a source or changes Project/history.
use super::stopped_bounce_capture::{
    CapturedStoppedEffectChain, StoppedEffectChainCapture, StoppedEffectSourceLease,
};
use super::*;
use super::{
    generator_loaded_identity::LoadedGeneratorIdentity,
    stopped_bounce_capture::{LiveBinding, StoppedBounceCapture},
};
use crate::instrument_bounce::{CapturedInstrumentBounce, effect::EffectCapturePollError};
use crate::plugins::plugin_runtime::{
    BounceSourceSettlement, PluginControlIdentity, StoppedSourceWorkTicket,
};
use crate::{
    export_job::{ExportControl, SourceCommitOwner},
    export_options::WavExportOptions,
    instrument_bounce::{
        BounceSourceIdentity, BounceSourceStatus, CapturePollError, InstrumentBounceCapture,
        InstrumentBouncePlan,
        render::{
            BackendFailure, MasterSongRenderOutcome, MasterSongRenderReceipt, RenderAdmission,
            RenderControl, RenderFailure, RenderStatus,
        },
    },
    plugins::plugin_runtime::{LoadedVst3Identity, RetainedShutdownPoll},
};

const RENDER_BUDGET: Duration = Duration::from_secs(300);

/// Project the retained typed evidence without replacing the primary failure.
/// These fields are fixed host vocabulary; plug-in diagnostics belong in the
/// visible error message, never in the opt-in control-plane stderr event.
fn render_backend_context(failure: &BackendFailure) -> String {
    let context = format!("kind={:?}, delivery={:?}", failure.kind, failure.delivery);
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    let context = format!(
        "{context}, stage={:?}, strict_kind={:?}, wire_request={:?}, diagnostic_unavailable={:?}",
        failure.strict_stage,
        failure.strict_kind,
        failure.last_wire_request,
        failure.strict_diagnostic_unavailable,
    );
    context
}

fn whole_render_failure_message(failure: &RenderFailure) -> String {
    let mut lines = vec![failure.primary.clone()];
    if let Some(backend) = &failure.backend {
        lines.push(format!(
            "Primary backend: {}",
            render_backend_context(backend)
        ));
    }
    if let Some(cleanup) = &failure.cleanup_backend {
        if !lines.iter().any(|line| line == cleanup.diagnostic()) {
            lines.push(cleanup.diagnostic().to_owned());
        }
        lines.push(format!(
            "Clone cleanup: {}",
            render_backend_context(cleanup)
        ));
    }
    for diagnostic in &failure.cleanup {
        if !lines.contains(diagnostic) {
            lines.push(diagnostic.clone());
        }
    }
    lines.join("\n")
}

fn write_whole_render_failure_diagnostic(
    sink: &mut impl std::io::Write,
    failure: &RenderFailure,
) -> std::io::Result<()> {
    // Only typed context and counts: no path, plug-in error text, state, PCM,
    // parameter values or inferred delivery/exit success are logged here.
    writeln!(
        sink,
        "WHOLE_EXPORT_RENDER_FAILED kind={:?} primary_backend=[{}] cleanup_backend=[{}] cleanup_grace_elapsed={} cleanup_details={}",
        failure.kind,
        failure
            .backend
            .as_deref()
            .map(render_backend_context)
            .unwrap_or_else(|| "none".into()),
        failure
            .cleanup_backend
            .as_deref()
            .map(render_backend_context)
            .unwrap_or_else(|| "none".into()),
        failure.cleanup_grace_elapsed,
        failure.cleanup.len(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WholeExportPhase {
    Stopping,
    Capturing,
    CapturingEffects,
    RenderingDisposing,
    JoiningLostSource,
    Complete,
    Failed,
    Cancelled,
}
impl WholeExportPhase {
    fn name(self) -> &'static str {
        match self {
            Self::Stopping => "Stopping",
            Self::Capturing => "Capturing",
            Self::CapturingEffects => "CapturingEffects",
            Self::RenderingDisposing => "RenderingDisposing",
            Self::JoiningLostSource => "JoiningLostSource",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
    fn terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Failed | Self::Cancelled)
    }
    fn label(self) -> &'static str {
        match self {
            Self::Stopping => "Stopping and draining source",
            Self::Capturing => "Capturing stopped instrument",
            Self::CapturingEffects => "Capturing ordered stopped Effects",
            Self::RenderingDisposing => "Rendering full master / disposing clone",
            Self::JoiningLostSource => "Waiting for lost source worker shutdown",
            Self::Complete => "Whole-song master exported; playback paused",
            Self::Failed => "Whole-song export failed; playback paused",
            Self::Cancelled => "Whole-song export cancelled; playback paused",
        }
    }
}

/// Control-plane call sites, not inferred keyboard state or plug-in contents.
/// The first accepted request belongs to this job's frozen actor identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WholeExportCancelSource {
    Programmatic,
    EscapeKey,
    TransportStop,
    CancelButton,
    WindowCloseRequest,
    ArmedWindowClose,
    LateParameterReceipt,
    LateAudioAssetResult,
    AppDrop,
    JobDrop,
}
impl WholeExportCancelSource {
    fn label(self) -> &'static str {
        match self {
            Self::Programmatic => "programmatic cancellation",
            Self::EscapeKey => "Escape key",
            Self::TransportStop => "transport Stop",
            Self::CancelButton => "Cancel export button",
            Self::WindowCloseRequest => "window close request",
            Self::ArmedWindowClose => "armed window close dispatch",
            Self::LateParameterReceipt => "late parameter receipt",
            Self::LateAudioAssetResult => "late audio asset result",
            Self::AppDrop => "App destruction",
            Self::JobDrop => "export job destruction",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WholeExportCancellation {
    source: WholeExportCancelSource,
    phase: WholeExportPhase,
}
impl WholeExportCancellation {
    fn summary(self) -> String {
        format!("{} during {}", self.source.label(), self.phase.name())
    }
}

/// Forced App destruction is an exceptional control-owner transfer. It cannot
/// drop a private capture receiver or source guard and call that termination.
struct WholeShutdownOwners {
    capture: Option<(InstrumentBounceCapture, BounceSourceIdentity)>,
    effect_capture: Option<StoppedEffectChainCapture>,
    effect_capture_failure: Option<EffectCapturePollError>,
    render: Option<MasterSongRenderReceipt>,
    admission: RenderAdmission,
    source: Option<RunningGeneratorChain>,
    inserts: Vec<RunningInsertChain>,
    // An identity invariant failure retains the original actor indefinitely;
    // it never guesses a binding or releases a still-pending private receipt.
    unresolved_actor: Option<StoppedBounceCapture>,
}
impl Drop for WholeSongExportJob {
    fn drop(&mut self) {
        self.cancel(WholeExportCancelSource::JobDrop);
        let empty_actor = StoppedBounceCapture::new(
            Arc::clone(&self.actor.snapshot),
            self.actor.initial,
            Instant::now(),
        );
        let mut actor = std::mem::replace(&mut self.actor, empty_actor);
        let capture = actor.take_cancelled_capture_for_shutdown();
        let effect_capture = actor.take_cancelled_effect_capture_for_shutdown();
        let unresolved_actor = actor.owns_pending_capture().then_some(actor);
        let owners = WholeShutdownOwners {
            capture,
            effect_capture,
            effect_capture_failure: self.effect_capture_failure.take(),
            render: self.render.take(),
            admission: self.admission.clone(),
            source: self.retired_source.take(),
            inserts: std::mem::take(&mut self.retired_inserts),
            unresolved_actor,
        };
        if owners.capture.is_none()
            && owners.effect_capture.is_none()
            && owners.render.is_none()
            && !owners.admission.is_busy()
            && owners.source.is_none()
            && owners.inserts.is_empty()
            && owners.unresolved_actor.is_none()
        {
            return;
        }
        if owners.unresolved_actor.is_some() {
            // This fault has no valid draining identity. Preserve the exact
            // owner instead of making a false terminal/drop assertion.
            let _ = Box::leak(Box::new(owners));
            return;
        }
        let slot = Arc::new(std::sync::Mutex::new(Some(owners)));
        let worker_slot = Arc::clone(&slot);
        let spawn = std::thread::Builder::new()
            .name("whole-export-shutdown-owner".into())
            .spawn(move || {
                let Some(mut owners) = worker_slot
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                else {
                    return;
                };
                loop {
                    if let Some(capture) = &mut owners.effect_capture {
                        let result = capture.poll_cancelled();
                        if let Err(failure) = &result { owners.effect_capture_failure.get_or_insert_with(|| failure.clone()); }
                        if capture.termination_acknowledged() && !matches!(result, Ok(false)) {
                            owners.effect_capture = None;
                        }
                    }
                    if let Some((capture, identity)) = &mut owners.capture {
                        // Already cancelled. Default status denies readiness and
                        // is used only to consume the original terminal ACK.
                        if !matches!(
                            capture.poll_typed(
                                *identity,
                                BounceSourceStatus::default(),
                                Instant::now()
                            ),
                            Ok(None)
                        ) {
                            owners.capture = None;
                        }
                    }
                    if let Some(render) = &owners.render {
                        let result = render.try_recv();
                        if render.termination_acknowledged()
                            && !owners.admission.is_busy()
                            && !matches!(result, Err(mpsc::TryRecvError::Empty))
                        {
                            owners.render = None;
                        }
                    }
                    if owners.capture.is_none()
                        && owners.effect_capture.is_none()
                        && owners.render.is_none()
                        && !owners.admission.is_busy()
                    {
                        if let Some(source) = owners.source.as_mut()
                            && !matches!(
                                source.guard.poll_shutdown_retained(),
                                RetainedShutdownPoll::Pending
                            )
                        {
                            owners.source = None;
                        }
                        if owners.source.is_none() {
                            owners.inserts.retain_mut(|source| {
                                matches!(
                                    source.guard.poll_shutdown_retained(),
                                    RetainedShutdownPoll::Pending
                                )
                            });
                            if owners.inserts.is_empty() {
                                if let Some(failure) = &owners.effect_capture_failure
                                    && std::env::var_os("CITRUS_STUDIO_PLUGIN_DIAGNOSTICS").is_some_and(|value| value == "1") {
                                    use std::io::Write as _;
                                    let kind = match failure {
                                        EffectCapturePollError::Cancelled => "cancelled",
                                        EffectCapturePollError::Deadline => "deadline",
                                        EffectCapturePollError::Stale(_) => "stale",
                                        EffectCapturePollError::Invalid(_) => "invalid",
                                        EffectCapturePollError::Backend(_) => "backend",
                                        EffectCapturePollError::Disconnected => "disconnected",
                                    };
                                    let _ = writeln!(std::io::stderr().lock(), "WHOLE_EXPORT_DROP_EFFECT_CAPTURE_TERMINATED kind={kind} all_retired_sources_joined=true");
                                }
                                return;
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
        if spawn.is_err() {
            // The spawned closure held only an Arc. Its failure cannot destroy
            // the source/capture/render owners retained in this original slot.
            let _ = Box::leak(Box::new(slot));
        }
    }
}

/// Constructed before the native Save dialog. The selected source, full Project,
/// device sample rate and options are frozen together; Save chooses only path.
pub(super) struct WholeSongExportReview {
    project: Project,
    snapshot: Arc<[u8]>,
    initial: LiveBinding,
    loaded_identity: Option<LoadedGeneratorIdentity>,
    options: WavExportOptions,
    reviewed_fixed_tail: bool,
    inserts: Vec<WholeInsertSourceBinding>,
}

struct WholeInsertSourceBinding {
    track: usize,
    session: u64,
    endpoint_id: u64,
    instance_ids: Vec<u64>,
    owner: PluginControlIdentity,
}
impl WholeInsertSourceBinding {
    fn matches(&self, source: &RunningInsertChain) -> bool {
        source.project_session == self.session
            && source.endpoint_id == self.endpoint_id
            && source.instance_ids == self.instance_ids
            && self.owner.matches(&source.control)
    }
}

/// Retained for this App run independently of the transient notification and
/// the next export job. File-menu review reopens the same metadata-qualified
/// warning; no Project/history record or extra output file is created.
pub(super) struct WholeSongTailResult {
    open: bool,
    path: PathBuf,
    selected_tail_milliseconds: u32,
    report: crate::instrument_bounce::render::MasterSongRenderReport,
    source_warning: Option<String>,
}
impl WholeSongTailResult {
    fn text(&self) -> String {
        let dry = &self.report.dry_render;
        let reported = if dry.reported_tail == u32::MAX {
            "unknown / unbounded (u32::MAX)".to_owned()
        } else {
            format!(
                "{} frames ({:.3} ms at {} Hz)",
                dry.reported_tail,
                1000.0 * f64::from(dry.reported_tail)
                    / f64::from(self.report.master_output.sample_rate),
                self.report.master_output.sample_rate
            )
        };
        // Preserve the general/native summary implementation. Its source-voice
        // flag describes a different predicate; Whole has a metadata comparison.
        let mut level = self.report.master_output;
        level.tail_limit_reached = false;
        format!(
            "Fixed-window whole-song file: {}\nSelected tail: {} ms ({} frames). Reported plug-in tail: {reported}.\nThe reported tail exceeds this fixed window, or is unknown; release may be truncated. This comparison uses plug-in metadata. No tail-end acoustic silence measurement was performed, and increasing a finite window does not guarantee a complete unknown release.\n{} Hz / {}. {}{}",
            self.path.display(),
            self.selected_tail_milliseconds,
            dry.fixed_tail,
            level.sample_rate,
            self.report.sample_format.label(),
            level.level_summary(),
            self.source_warning
                .as_ref()
                .map_or(String::new(), |reason| format!(
                    "\nSource/session warning: {reason}"
                ))
        )
    }
}

pub(super) struct WholeSongExportJob {
    actor: StoppedBounceCapture,
    loaded_identity: Option<LoadedGeneratorIdentity>,
    frozen_project: Option<Project>,
    options: WavExportOptions,
    #[cfg(any(
        test,
        all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        )
    ))]
    reviewed_fixed_tail: bool,
    path: PathBuf,
    export: ExportControl,
    admission: RenderAdmission,
    render: Option<MasterSongRenderReceipt>,
    phase: WholeExportPhase,
    cancel_requested: bool,
    first_cancellation: Option<WholeExportCancellation>,
    failure_pause_requested: bool,
    failure: Option<String>,
    #[allow(dead_code)]
    // Retain the original typed private-capture evidence after its UI projection.
    capture_failure: Option<CapturePollError>,
    outcome: Option<MasterSongRenderOutcome>,
    publication_unknown: bool,
    retired_source: Option<RunningGeneratorChain>,
    retired_inserts: Vec<RunningInsertChain>,
    insert_sources: Vec<WholeInsertSourceBinding>,
    captured_generator: Option<CapturedInstrumentBounce>,
    captured_effects: Vec<CapturedStoppedEffectChain>,
    effect_leases: Vec<StoppedEffectSourceLease>,
    generator_ticket: Option<StoppedSourceWorkTicket>,
    effect_capture_failure: Option<EffectCapturePollError>,
    render_deadline: Option<Instant>,
    source_commit: Option<SourceCommitOwner>,
    result_announced: bool,
    #[cfg(test)]
    render_starter: Option<TestWholeRenderStarter>,
    #[cfg(all(
        test,
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    serial_render_starter: Option<TestSerialWholeRenderStarter>,
}
#[cfg(all(
    test,
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
type TestSerialWholeRenderStarter = Box<
    dyn FnOnce(
        &RenderAdmission,
        Project,
        CapturedInstrumentBounce,
        Vec<crate::instrument_bounce::render::serial::CapturedSerialEffectChain>,
        WavExportOptions,
        PathBuf,
        RenderControl,
        bool,
    ) -> Result<MasterSongRenderReceipt, String>,
>;
#[cfg(test)]
type TestWholeRenderStarter = Box<
    dyn FnOnce(
        &RenderAdmission,
        Project,
        crate::instrument_bounce::CapturedInstrumentBounce,
        WavExportOptions,
        PathBuf,
        RenderControl,
        bool,
    ) -> Result<MasterSongRenderReceipt, String>,
>;
impl WholeSongExportJob {
    pub(super) fn pending(&self) -> bool {
        !self.phase.terminal()
            || self.actor.owns_pending_capture()
            || self.render.is_some()
            || self.admission.is_busy()
            || self.retired_source.is_some()
            || !self.retired_inserts.is_empty()
    }
    pub(super) fn owns_source(&self, source: &RunningGeneratorChain) -> bool {
        let stamp = self.actor.binding.unwrap_or(self.actor.initial).stamp;
        self.pending()
            && source.project_session == stamp.project_session
            && source.endpoint_id == stamp.endpoint_id
            && source.instance_id == stamp.instance_id
    }
    pub(super) fn owns_insert_source(&self, source: &RunningInsertChain) -> bool {
        self.pending()
            && self
                .insert_sources
                .iter()
                .any(|binding| binding.matches(source))
    }
    pub(super) fn observe_stop_activation_queued(
        &mut self,
        request_id: u64,
        generation: u64,
        seek_serial: u64,
        minimum_epoch: u64,
        frame: u64,
    ) {
        if self.phase == WholeExportPhase::Stopping {
            self.actor.observe_stop_activation_queued(
                request_id,
                generation,
                seek_serial,
                minimum_epoch,
                frame,
            );
        }
    }
    pub(super) fn observe_stop_activation_confirmed(
        &mut self,
        request_id: u64,
        generation: u64,
        epoch: u64,
        frame: u64,
    ) {
        if self.phase == WholeExportPhase::Stopping {
            self.actor
                .observe_stop_activation_confirmed(request_id, generation, epoch, frame);
        }
    }
    fn cancel(&mut self, source: WholeExportCancelSource) -> bool {
        let accepted = self.export.cancel();
        if accepted && self.first_cancellation.is_none() {
            let cancellation = WholeExportCancellation {
                source,
                phase: self.phase,
            };
            self.first_cancellation = Some(cancellation);
            if std::env::var_os("CITRUS_STUDIO_PLUGIN_DIAGNOSTICS")
                .is_some_and(|value| value == "1")
            {
                use std::io::Write as _;
                let stamp = self.actor.initial.stamp;
                // One event per job. No file path, state bytes, parameter value
                // or display name is exposed; a closed stderr is non-fatal.
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "WHOLE_EXPORT_CANCEL source={source:?} phase={} job_started={:?} project_session={} engine_session={} channel={} endpoint={} instance={} initial_epoch={} stop_request={:?}",
                    self.phase.name(),
                    self.actor.started_at,
                    stamp.project_session,
                    stamp.engine_session,
                    stamp.channel_id,
                    stamp.endpoint_id,
                    stamp.instance_id,
                    stamp.epoch,
                    self.actor.stop_transport_request,
                );
            }
        }
        self.cancel_requested |= accepted;
        self.actor.cancel_capture();
        if let Some(render) = &self.render {
            render.cancel();
        }
        accepted
    }
    fn fail(&mut self, reason: impl Into<String>) {
        if self.failure.is_none() {
            self.failure = Some(reason.into());
        }
        self.export.cancel();
        self.actor.cancel_capture();
        if let Some(render) = &self.render {
            render.cancel();
        }
    }
    fn finish(&mut self) {
        if self.actor.owns_pending_capture() || self.render.is_some() || self.admission.is_busy() {
            return;
        }
        if self.retired_source.is_some() || !self.retired_inserts.is_empty() {
            self.phase = WholeExportPhase::JoiningLostSource;
            return;
        }
        self.phase = if matches!(self.outcome, Some(MasterSongRenderOutcome::Complete(_))) {
            WholeExportPhase::Complete
        } else if self.cancel_requested && self.failure.is_none() {
            WholeExportPhase::Cancelled
        } else {
            WholeExportPhase::Failed
        };
        self.frozen_project = None;
        self.captured_generator = None;
        self.captured_effects.clear();
        self.effect_leases.clear();
        self.insert_sources.clear();
        self.generator_ticket = None;
        self.source_commit = None;
    }
}

/// Pure source/content/options admission shared before Save and after actual
/// stopped Ready. It does not derive a plan from a reduced/disabled copy.
fn pure_master_plan(
    project: &Project,
    channel_id: u32,
    initial_rate: u32,
    options: WavExportOptions,
) -> Result<InstrumentBouncePlan, String> {
    pure_master_plan_with_loaded_identity(project, channel_id, initial_rate, options, None)
}

/// Loaded identity is an additional whole-export admission, not a Project role
/// update or authority to capture. Legacy Instrument callers still use None.
fn pure_master_plan_with_loaded_identity(
    project: &Project,
    channel_id: u32,
    initial_rate: u32,
    options: WavExportOptions,
    loaded_identity: Option<&LoadedVst3Identity>,
) -> Result<InstrumentBouncePlan, String> {
    options.validate().map_err(|error| error.to_string())?;
    if options.sample_rate != initial_rate {
        return Err(format!(
            "Whole-song VST export requires the current source rate {initial_rate} Hz"
        ));
    }
    let master = project
        .mixer_tracks
        .iter()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .ok_or("MASTER identity is missing")?;
    if !master.volume.is_finite() || !(0.0..=1.2).contains(&master.volume) {
        return Err("Whole-song MASTER volume must be within the realtime range 0..1.2".into());
    }
    let mut channels = project
        .channels
        .iter()
        .filter(|channel| channel.id == channel_id);
    let channel = channels
        .next()
        .ok_or("The sole VST source channel is absent")?;
    if channels.next().is_some() {
        return Err("The VST channel identity is ambiguous".into());
    }
    let id = channel
        .instrument_plugin_instance_id
        .ok_or("The sole source is not a VST generator")?;
    let mut instances = project
        .plugin_instances
        .iter()
        .filter(|instance| instance.id == id);
    let instance = instances
        .next()
        .ok_or("The VST source instance is absent")?;
    if instances.next().is_some() {
        return Err("Whole-song export requires one distinct Instrument instance identity".into());
    }
    if loaded_identity.is_some_and(|identity| identity.path() != instance.path.as_path()) {
        return Err("The actual loaded VST3 identity belongs to a different source path".into());
    }
    match instance.role {
        PluginRole::Instrument => {}
        PluginRole::Unknown
            if loaded_identity.is_some_and(LoadedVst3Identity::is_instrument_with_midi_input) => {}
        _ => {
            return Err("Whole-song export requires an Instrument instance, or an Unknown instance with actual loaded Instrument and MIDI-input identity".into());
        }
    }
    let frames = u32::try_from(options.tail_frames().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    if project.mixer_insert_slots.is_empty() {
        InstrumentBouncePlan::prepare_master_song(project, channel_id, initial_rate, frames)
    } else {
        InstrumentBouncePlan::prepare_master_song_serial(project, channel_id, initial_rate, frames)
    }
}

fn recheck_loaded_identity(
    app: &CitrusApp,
    identity: Option<&LoadedGeneratorIdentity>,
) -> Result<(), String> {
    if identity.is_some_and(|identity| !identity.matches_current(app)) {
        return Err("The reviewed actual loaded VST3 source or control owner changed".into());
    }
    Ok(())
}

fn check_captured_loaded_identity(
    expected: Option<&LoadedVst3Identity>,
    actual: &crate::instrument_bounce::BouncePluginMetadata,
) -> Result<(), String> {
    if expected.is_some_and(|expected| {
        expected.class_uid() != actual.class_uid.as_str()
            || expected.version() != actual.version.as_str()
    }) {
        return Err(
            "The actual captured VST3 class/version differs from the reviewed loaded identity"
                .into(),
        );
    }
    Ok(())
}

impl CitrusApp {
    pub(super) fn wav_export_running(&self) -> bool {
        self.export_job.is_running() || self.whole_song_export_fence_active()
    }
    fn whole_song_export_operation_admission(&self) -> Result<(), String> {
        if self.transport_mode != TransportMode::Song {
            return Err("Switch to SONG mode before exporting a whole VST song".into());
        }
        if self.clip_operation_fence_active()
            || self.project_snapshot_transition_pending()
            || self.project_lifecycle_barriers_active()
            || self.export_job.is_running()
            || self.prepared_playback_start.is_some()
            || self.loop_restart_wait.is_some()
            || self.audio_import_receiver.is_some()
            || self.pending_midi_import.is_some()
            || self.pending_midi_export.is_some()
            || self.piano_roll_transform.is_some()
            || self.piano_note_properties.is_some()
            || self.playlist_gesture_before.is_some()
            || self.piano_roll_gesture_before.is_some()
            || self.automation_point_drag_active
            || self.project_media.open
            || self.recovery_available
            || self.queued_save_after_midi_recording.is_some()
            || !self.loading_audio_assets.is_empty()
            || !self.pending_audio_asset_registrations.is_empty()
            || !self.prepared_audio_asset_registrations.is_empty()
            || !self.pending_audio_asset_unregistrations.is_empty()
            || self.pending_audio_asset_clear.is_some()
        {
            return Err("Finish the current save, import, gesture, asset, playback preparation or session operation first".into());
        }
        if self.plugin_parameter_edits.reserved_len() != 0
            || !self.plugin_parameter_edit_draft_desired.is_empty()
        {
            return Err("Wait for parameter edits and unsent drafts before exporting".into());
        }
        if !self.midi_input.state.is_disconnected()
            || !self.midi_recording.is_idle()
            || self.recording
            || self.input_recorder.is_some()
            || self.pending_recording.is_some()
            || self.master_capture.is_some()
            || self.pending_master_capture.is_some()
        {
            return Err(
                "Disconnect MIDI and finish recording/input/master capture before exporting".into(),
            );
        }
        Ok(())
    }
    pub(super) fn prepare_whole_song_export_review(
        &self,
        options: WavExportOptions,
        reviewed_fixed_tail: bool,
    ) -> Result<WholeSongExportReview, String> {
        if !cfg!(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        )) {
            return Err(
                "Whole-song VST export requires experimental-fresh-helper on Linux or Windows"
                    .into(),
            );
        }
        self.whole_song_export_operation_admission()?;
        let mut active = self.project.channels.iter().filter(|channel| {
            channel.instrument_plugin_instance_id.is_some_and(|id| {
                self.project
                    .plugin_instances
                    .iter()
                    .any(|instance| instance.id == id && instance.enabled && !instance.bypass)
            })
        });
        let channel_id = active
            .next()
            .ok_or("Whole-song export requires exactly one active VST generator")?
            .id;
        if active.next().is_some() {
            return Err("Whole-song export supports exactly one active VST generator".into());
        }
        // Same complete bounded/round-trip snapshot contract as the shared
        // actor, before settings or native destination admission. No occurrence
        // geometry, source reduction or history candidate is constructed.
        let snapshot =
            clip_render::project_snapshot(&self.project).map_err(|error| error.to_string())?;
        let initial = self.stopped_bounce_observe(channel_id)?;
        // This bridge authorizes only persistent Unknown. A classified
        // Instrument keeps its original Review/capture contract even when the
        // runtime can also publish an optional loaded identity.
        let loaded_identity = if self.project.plugin_instances.iter().any(|instance| {
            instance.id == initial.stamp.instance_id && instance.role == PluginRole::Unknown
        }) {
            Some(
                self.loaded_generator_identity(channel_id)
                    .ok_or_else(|| self.loaded_generator_identity_unavailable_reason(channel_id))?,
            )
        } else {
            None
        };
        recheck_loaded_identity(self, loaded_identity.as_ref())?;
        let plan = match &loaded_identity {
            Some(identity) => pure_master_plan_with_loaded_identity(
                &self.project,
                channel_id,
                initial.sample_rate,
                options,
                Some(identity.identity()),
            ),
            None => pure_master_plan(&self.project, channel_id, initial.sample_rate, options),
        }?;
        if plan.instance_id() != initial.stamp.instance_id {
            return Err("The exact source instance binding changed".into());
        }
        let mut inserts = Vec::new();
        if plan.is_serial_master() {
            let serial = crate::export::serial_master::StaticSerialMasterPlan::prepare(
                &self.project,
                channel_id,
            )
            .map_err(|e| e.to_string())?;
            for node in serial.nodes.iter().filter(|n| !n.placements.is_empty()) {
                let source = self
                    .running_insert_chains
                    .get(&node.runtime_track)
                    .ok_or("Wait for the actual Insert endpoint to load")?;
                if source.project_session != self.project_session
                    || source
                        .instance_ids
                        .iter()
                        .copied()
                        .ne(node.placements.iter().map(|p| p.plugin_instance_id))
                {
                    return Err(
                        "The actual ordered Insert source differs from the reviewed graph".into(),
                    );
                }
                inserts.push(WholeInsertSourceBinding {
                    track: node.runtime_track,
                    session: source.project_session,
                    endpoint_id: source.endpoint_id,
                    instance_ids: source.instance_ids.clone(),
                    owner: source.control.owner_identity(),
                });
            }
        }
        Ok(WholeSongExportReview {
            project: self.project.clone(),
            snapshot,
            initial,
            loaded_identity,
            options,
            reviewed_fixed_tail,
            inserts,
        })
    }
    pub(super) fn begin_whole_song_export(
        &mut self,
        review: WholeSongExportReview,
        path: PathBuf,
    ) -> Result<(), String> {
        if !path.is_absolute() {
            return Err("Choose an absolute WAV destination".into());
        }
        if !review.reviewed_fixed_tail {
            return Err("Confirm the selected fixed VST tail window in Export Review before choosing a destination or stopping the source".into());
        }
        // Called after Save returns: the same full snapshot and actual initial
        // binding must still exist before publishing the preparatory Stop.
        let loaded_identity = review.loaded_identity;
        let mut actor = StoppedBounceCapture::new(review.snapshot, review.initial, Instant::now());
        actor.begin_stop(self, |app| {
            Self::whole_song_export_operation_admission(app)?;
            recheck_loaded_identity(app, loaded_identity.as_ref())?;
            if review.inserts.iter().any(|binding| {
                app.running_insert_chains
                    .get(&binding.track)
                    .is_none_or(|source| !binding.matches(source))
            }) {
                return Err("A reviewed Insert source changed before Stop".into());
            }
            Ok(())
        })?;
        self.whole_song_export_job = Some(WholeSongExportJob {
            actor,
            loaded_identity,
            frozen_project: Some(review.project),
            options: review.options,
            #[cfg(any(
                test,
                all(
                    any(target_os = "linux", windows),
                    feature = "experimental-fresh-helper"
                )
            ))]
            reviewed_fixed_tail: review.reviewed_fixed_tail,
            path,
            export: ExportControl::default(),
            admission: RenderAdmission::default(),
            render: None,
            phase: WholeExportPhase::Stopping,
            cancel_requested: false,
            first_cancellation: None,
            failure_pause_requested: false,
            failure: None,
            capture_failure: None,
            outcome: None,
            publication_unknown: false,
            retired_source: None,
            retired_inserts: Vec::new(),
            insert_sources: review.inserts,
            captured_generator: None,
            captured_effects: Vec::new(),
            effect_leases: Vec::new(),
            generator_ticket: None,
            source_commit: None,
            effect_capture_failure: None,
            render_deadline: None,
            result_announced: false,
            #[cfg(test)]
            render_starter: None,
            #[cfg(all(
                test,
                any(target_os = "linux", windows),
                feature = "experimental-fresh-helper"
            ))]
            serial_render_starter: None,
        });
        Ok(())
    }
    pub(super) fn whole_song_export_fence_active(&self) -> bool {
        self.whole_song_export_job
            .as_ref()
            .is_some_and(WholeSongExportJob::pending)
    }
    pub(super) fn whole_song_export_blocks_playback(&self) -> bool {
        self.whole_song_export_fence_active()
    }
    pub(super) fn whole_song_export_owns_current_pause_request(&self) -> bool {
        self.whole_song_export_job
            .as_ref()
            .is_some_and(|job| job.pending() && job.actor.owns_current_pause_request(self))
    }
    pub(super) fn whole_song_export_allows_timeline_dispatch(&self) -> bool {
        self.whole_song_export_job
            .as_ref()
            .is_none_or(|job| !job.pending() || job.phase == WholeExportPhase::Stopping)
    }
    pub(super) fn whole_song_export_allows_asset_dispatch(&self) -> bool {
        !self.whole_song_export_fence_active()
    }
    pub(super) fn cancel_whole_song_export(&mut self) -> bool {
        self.cancel_whole_song_export_from(WholeExportCancelSource::Programmatic)
    }
    pub(super) fn cancel_whole_song_export_from(
        &mut self,
        source: WholeExportCancelSource,
    ) -> bool {
        self.whole_song_export_job
            .as_mut()
            .is_some_and(|job| job.cancel(source))
    }
    #[allow(clippy::result_large_err)]
    pub(super) fn retain_whole_song_source_on_engine_loss(
        &mut self,
        source: RunningGeneratorChain,
    ) -> Result<(), RunningGeneratorChain> {
        let Some(job) = self.whole_song_export_job.as_mut() else {
            return Err(source);
        };
        if !job.owns_source(&source) || job.retired_source.is_some() {
            return Err(source);
        }
        job.retired_source = Some(source);
        job.fail("Audio/session loss retained the exact source worker until capture, clone disposal and source join terminate");
        Ok(())
    }
    #[allow(clippy::result_large_err)]
    pub(super) fn retain_whole_song_insert_on_engine_loss(
        &mut self,
        source: RunningInsertChain,
    ) -> Result<(), RunningInsertChain> {
        let Some(job) = self.whole_song_export_job.as_mut() else {
            return Err(source);
        };
        if !job.owns_insert_source(&source)
            || job
                .retired_inserts
                .iter()
                .any(|s| s.endpoint_id == source.endpoint_id)
        {
            return Err(source);
        }
        job.retired_inserts.push(source);
        job.fail("Audio/session loss retained every participating Insert source until capture, clone cleanup and actual source join");
        Ok(())
    }
    pub(super) fn drive_whole_song_export(&mut self) {
        let Some(mut job) = self.whole_song_export_job.take() else {
            return;
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.drive_whole_song_export_owned(&mut job)
        }));
        if result.is_err() {
            job.fail("Whole-song export controller panicked; original owners remain retained");
            if !job.actor.owns_pending_capture() && job.render.is_none() && !job.admission.is_busy()
            {
                job.finish();
            }
        }
        if !job.pending() && !job.result_announced {
            job.result_announced = true;
            match &job.outcome {
                Some(MasterSongRenderOutcome::Complete(report)) if report.master_output.tail_limit_reached => {
                    self.whole_song_export_tail_result = Some(WholeSongTailResult {
                        open: true, path: job.path.clone(), selected_tail_milliseconds: job.options.tail_milliseconds,
                        report: report.clone(), source_warning: job.failure.clone(),
                    });
                    self.notify("Whole-song fixed-window file completed with a tail warning; the retained result explains the metadata limit".into());
                }
                Some(MasterSongRenderOutcome::Complete(report)) => self.notify(format!("Exported {}. {}{}", job.path.display(),
                    report.master_output.level_summary(), job.failure.as_ref().map_or(String::new(), |reason| format!(" Source/session warning: {reason}")))),
                _ if job.publication_unknown => self.export_error = Some("Renderer terminated without a file outcome; destination status is unknown".into()),
                _ if job.phase == WholeExportPhase::Cancelled => self.notify(format!(
                    "Whole-song export cancelled ({}); destination was not changed; playback remains paused",
                    job.first_cancellation.map_or_else(|| "source unavailable".into(), WholeExportCancellation::summary),
                )),
                _ => self.export_error = Some(job.failure.clone().unwrap_or_else(|| "Whole-song export failed".into())),
            }
        }
        self.whole_song_export_job = Some(job);
    }
    fn drive_whole_song_export_owned(&mut self, job: &mut WholeSongExportJob) {
        if !job.pending() {
            return;
        }
        // Normal ticks preserve the exact stopped mailbox request. Actual late
        // playback invalidates that admission and publishes one recovery pause,
        // while the private capture/render owner still drains to its real ACK.
        if job.actor.binding.is_some()
            && (self.playing
                || self.transport_clock_playing
                || self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.snapshot().transport_playing))
        {
            job.fail("Playback changed after the exact stopped whole-song export admission");
            self.playing = false;
            self.transport_clock_playing = false;
            if !job.failure_pause_requested {
                job.failure_pause_requested = true;
                if let Some(audio) = &self.audio {
                    audio.set_playing(false);
                }
            }
        }
        // Recheck exact source/intent on every normal render tick.
        if job.actor.binding.is_some()
            && job.failure.is_none()
            && let Err(reason) = job
                .actor
                .live_recheck(self)
                .and_then(|_| job.actor.stop_intent_current(self))
                .and_then(|_| recheck_loaded_identity(self, job.loaded_identity.as_ref()))
        {
            job.fail(reason);
        }
        let mut serial_sources_ready = true;
        if job.failure.is_none()
            && job.insert_sources.iter().any(|binding| {
                self.running_insert_chains
                    .get(&binding.track)
                    .is_none_or(|source| !binding.matches(source))
            })
        {
            job.fail("The reviewed actual Insert endpoint changed during Whole export");
        }
        if job.failure.is_none()
            && matches!(
                job.phase,
                WholeExportPhase::CapturingEffects | WholeExportPhase::RenderingDisposing
            )
        {
            let recheck = (|| {
                if let Some(ticket) = job.generator_ticket {
                    let source = self
                        .running_generator_chains
                        .get(&job.actor.initial.stamp.channel_id)
                        .ok_or("Captured Generator disappeared")?;
                    match source.control.stopped_source_work_settlement(ticket) {
                        BounceSourceSettlement::Invalid => {
                            return Err(
                                "Captured Generator worker history changed during serial Whole"
                                    .to_owned(),
                            );
                        }
                        BounceSourceSettlement::Waiting => serial_sources_ready = false,
                        BounceSourceSettlement::Ready => {}
                    }
                }
                for captured in &job.captured_effects {
                    serial_sources_ready &= captured.recheck(self, true)?;
                }
                for lease in &job.effect_leases {
                    serial_sources_ready &= lease.recheck(self)?;
                }
                Ok::<_, String>(())
            })();
            if let Err(reason) = recheck {
                job.fail(reason);
            }
        }
        match job.phase {
            WholeExportPhase::Stopping => {
                // No private capture/render owner exists yet. An accepted
                // cancellation must not issue another StopAll/chase or wait
                // for Ready and later be relabelled as a stop timeout.
                if job.cancel_requested || job.failure.is_some() {
                    job.finish();
                    return;
                }
                if let Err(reason) = recheck_loaded_identity(self, job.loaded_identity.as_ref()) {
                    job.fail(reason);
                    job.finish();
                    return;
                }
                match job.actor.poll_stop(self) {
                    Ok(None) => (),
                    Err(reason) => {
                        job.fail(reason);
                        job.finish();
                    }
                    Ok(Some(ready)) => {
                        if job.cancel_requested || job.failure.is_some() {
                            job.finish();
                            return;
                        }
                        let binding = ready.binding();
                        let prepare = (|| {
                            if job.options.sample_rate != job.actor.initial.sample_rate
                                || job.options.sample_rate != binding.sample_rate
                                || ready.snapshot().as_ref() != job.actor.snapshot.as_ref()
                            {
                                return Err("Reviewed source/sample rate/snapshot differs from actual stopped Ready".into());
                            }
                            recheck_loaded_identity(self, job.loaded_identity.as_ref())?;
                            let plan = match &job.loaded_identity {
                                Some(identity) => pure_master_plan_with_loaded_identity(
                                    &self.project,
                                    binding.stamp.channel_id,
                                    binding.sample_rate,
                                    job.options,
                                    Some(identity.identity()),
                                ),
                                None => pure_master_plan(
                                    &self.project,
                                    binding.stamp.channel_id,
                                    binding.sample_rate,
                                    job.options,
                                ),
                            }?;
                            job.actor.stop_intent_current(self)?;
                            if plan.is_serial_master() {
                                let source = self
                                    .running_generator_chains
                                    .get(&binding.stamp.channel_id)
                                    .ok_or("Stopped Generator source is absent")?;
                                job.generator_ticket = Some(
                                    source
                                        .control
                                        .stopped_source_work_ticket(binding.stamp.epoch)
                                        .and_then(
                                            StoppedSourceWorkTicket::following_private_capture,
                                        )
                                        .ok_or(
                                            "Wait for exact stopped Generator worker settlement",
                                        )?,
                                );
                            }
                            job.actor.begin_capture(self, plan)
                        })();
                        match prepare {
                            Ok(()) => job.phase = WholeExportPhase::Capturing,
                            Err(reason) => {
                                job.fail(reason);
                                job.finish();
                            }
                        }
                    }
                }
            }
            WholeExportPhase::Capturing => match job.actor.poll_capture(self) {
                Ok(None) => (),
                Err(reason) => {
                    if matches!(
                        reason,
                        CapturePollError::Backend(_) | CapturePollError::Invalid(_)
                    ) || job.failure.is_none()
                        && (reason != CapturePollError::Cancelled || !job.cancel_requested)
                    {
                        job.failure = Some(reason.to_string());
                    }
                    job.capture_failure = Some(reason);
                    job.finish();
                }
                Ok(Some(captured)) => {
                    #[cfg(test)]
                    crate::genuine_app_observer::record_capture(
                        crate::genuine_app_observer::CaptureLabel::Initial,
                        &captured,
                    );
                    if job.cancel_requested || job.failure.is_some() {
                        drop(captured);
                        job.finish();
                        return;
                    }
                    if let Err(reason) = check_captured_loaded_identity(
                        job.loaded_identity
                            .as_ref()
                            .map(LoadedGeneratorIdentity::identity),
                        captured.metadata(),
                    ) {
                        job.capture_failure = Some(CapturePollError::Invalid(reason.clone()));
                        job.fail(reason);
                        drop(captured);
                        job.finish();
                        return;
                    }
                    if captured.plan().is_serial_master() {
                        job.render_deadline = Some(Instant::now() + RENDER_BUDGET);
                        job.captured_generator = Some(captured);
                        job.phase = WholeExportPhase::CapturingEffects;
                        return;
                    }
                    let project = job
                        .frozen_project
                        .take()
                        .expect("frozen full Project retained until capture terminal");
                    let control = RenderControl {
                        export: job.export.clone(),
                        deadline: Instant::now() + RENDER_BUDGET,
                    };
                    let start = start_whole_song_renderer(job, project, captured, control);
                    match start {
                        Ok(receipt) => {
                            job.render = Some(receipt);
                            job.phase = WholeExportPhase::RenderingDisposing;
                        }
                        Err(reason) => {
                            job.fail(reason);
                            job.finish();
                        }
                    }
                }
            },
            WholeExportPhase::CapturingEffects => {
                if job.actor.owns_pending_capture() {
                    match job.actor.poll_effect_chain_capture(self) {
                        Ok(None) => {
                            if job
                                .render_deadline
                                .is_none_or(|deadline| Instant::now() >= deadline)
                            {
                                job.fail("Serial Whole original render deadline elapsed during pending source capture");
                            }
                            return;
                        }
                        Ok(Some(captured)) => {
                            if captured.project_fingerprint
                                != super::stopped_bounce_capture::project_snapshot_fingerprint(
                                    &job.actor.snapshot,
                                )
                            {
                                job.fail("Captured Effect belongs to another frozen project");
                            } else {
                                job.captured_effects.push(captured);
                            }
                        }
                        Err(reason) => {
                            if matches!(
                                reason,
                                EffectCapturePollError::Backend(_)
                                    | EffectCapturePollError::Invalid(_)
                            ) || job.failure.is_none()
                                && (reason != EffectCapturePollError::Cancelled
                                    || !job.cancel_requested)
                            {
                                job.failure = Some(reason.to_string());
                            }
                            job.effect_capture_failure = Some(reason);
                        }
                    }
                }
                if job.cancel_requested || job.failure.is_some() {
                    job.finish();
                    return;
                }
                if job
                    .render_deadline
                    .is_none_or(|deadline| Instant::now() >= deadline)
                {
                    job.fail("Serial Whole original render deadline elapsed during source capture");
                    job.finish();
                    return;
                }
                if !serial_sources_ready {
                    return;
                }
                let recheck = job
                    .captured_effects
                    .iter()
                    .try_fold(true, |ready, capture| {
                        capture.recheck(self, true).map(|current| ready && current)
                    });
                match recheck {
                    Err(reason) => {
                        job.fail(reason);
                        job.finish();
                        return;
                    }
                    Ok(false) => return,
                    Ok(true) => {}
                }
                if self
                    .audio
                    .as_ref()
                    .and_then(AudioEngine::prepared_playback_token)
                    .is_none()
                {
                    return;
                }
                if job.captured_effects.len() < job.insert_sources.len() {
                    let track = job.insert_sources[job.captured_effects.len()].track;
                    if let Err(reason) = job.actor.begin_effect_chain_capture_before(
                        self,
                        track,
                        job.render_deadline.expect("One original render deadline"),
                    ) {
                        job.fail(reason);
                        job.finish();
                    }
                    return;
                }
                let captured = job
                    .captured_generator
                    .take()
                    .expect("Generator receipt retained before Effects");
                let project = job
                    .frozen_project
                    .take()
                    .expect("Original project retained before serial render");
                let control = RenderControl {
                    export: job.export.clone(),
                    deadline: job.render_deadline.expect("One original deadline"),
                };
                match start_whole_song_renderer(job, project, captured, control) {
                    Ok(receipt) => {
                        job.render = Some(receipt);
                        job.phase = WholeExportPhase::RenderingDisposing;
                    }
                    Err(reason) => {
                        job.fail(reason);
                        job.finish();
                    }
                }
            }
            WholeExportPhase::RenderingDisposing => {
                // The checked writer requests only at its original final CAS,
                // after every clone was reaped and the staged WAV was synced.
                // All original App/source/native/graph checks above must be
                // Ready now; a missing token or metadata in flight never grants
                // permission. The App executes CAS, not a cached Ready reply.
                if job.failure.is_none()
                    && !job.cancel_requested
                    && serial_sources_ready
                    && let Some(commit) = &job.source_commit
                    && commit.requested()
                    && let Err(reason) = commit
                        .commit_before(job.render_deadline.expect("One original serial deadline"))
                {
                    job.fail(reason.to_string());
                }
                let Some(receipt) = &job.render else {
                    if job.admission.is_busy() {
                        return;
                    }
                    job.publication_unknown = true;
                    job.fail("Original renderer receipt is unavailable");
                    job.finish();
                    return;
                };
                let result = receipt.try_recv();
                let terminated = receipt.termination_acknowledged();
                let busy = job.admission.is_busy();
                match result {
                    Err(mpsc::TryRecvError::Empty) => return,
                    Err(mpsc::TryRecvError::Disconnected) if !terminated || busy => return,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        job.publication_unknown = true;
                        job.fail("Terminated renderer returned no file outcome");
                    }
                    Ok(outcome) => {
                        if let MasterSongRenderOutcome::Failed(failure) = &outcome {
                            if std::env::var_os("CITRUS_STUDIO_PLUGIN_DIAGNOSTICS")
                                .is_some_and(|value| value == "1")
                            {
                                let _ = write_whole_render_failure_diagnostic(
                                    &mut std::io::stderr().lock(),
                                    failure,
                                );
                            }
                            if failure.kind
                                != crate::instrument_bounce::render::FailureKind::Cancelled
                                || job.failure.is_none() && !job.cancel_requested
                            {
                                job.failure = Some(whole_render_failure_message(failure));
                            }
                        }
                        job.outcome = Some(outcome);
                    }
                }
                if !terminated || busy {
                    return;
                }
                job.render = None;
                job.finish();
            }
            WholeExportPhase::JoiningLostSource => {
                if let Some(source) = job.retired_source.as_mut() {
                    match source.guard.poll_shutdown_retained() {
                        RetainedShutdownPoll::Pending => return,
                        RetainedShutdownPoll::WorkerPanicked => {
                            job.failure =
                                Some("Exact source worker joined after a shutdown panic".into())
                        }
                        RetainedShutdownPoll::Joined => {}
                    }
                    job.retired_source = None;
                }
                job.retired_inserts.retain_mut(|source| {
                    match source.guard.poll_shutdown_retained() {
                        RetainedShutdownPoll::Pending => true,
                        RetainedShutdownPoll::WorkerPanicked => {
                            job.failure =
                                Some("Exact Insert worker joined after a shutdown panic".into());
                            false
                        }
                        RetainedShutdownPoll::Joined => false,
                    }
                });
                job.finish();
            }
            _ => job.finish(),
        }
    }
    /// Independent Window keeps progress and Cancel enabled even while the
    /// main edit root is disabled by the operation/source ownership fence.
    pub(super) fn whole_song_export_progress_ui(&mut self, ctx: &egui::Context) {
        if let Some(job) = self
            .whole_song_export_job
            .as_mut()
            .filter(|job| job.pending())
        {
            ctx.request_repaint_after(Duration::from_millis(50));
            egui::Window::new("Whole-song master export")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(job.path.display().to_string());
                    ui.label(job.phase.label());
                    if let Some(receipt) = &job.render
                        && let RenderStatus::CleanupPending { diagnostics } = receipt.status()
                    {
                        ui.label("Clone cleanup remains pending; ownership is retained");
                        for diagnostic in diagnostics {
                            ui.label(diagnostic);
                        }
                    }
                    let progress = job.export.progress();
                    ui.add(
                        egui::ProgressBar::new(progress.basis_points as f32 / 10000.0)
                            .text(progress.phase()),
                    );
                    let cancel =
                        ui.add_enabled(progress.can_cancel, egui::Button::new("Cancel export"));
                    #[cfg(test)]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new("whole-export-cancel-button-test"),
                            cancel.rect,
                        )
                    });
                    if cancel.clicked() {
                        job.cancel(WholeExportCancelSource::CancelButton);
                    }
                    if let Some(cancellation) = job.first_cancellation {
                        ui.label(format!(
                            "Cancellation requested: {}",
                            cancellation.summary()
                        ));
                    }
                    if !progress.can_cancel && !progress.cancelling {
                        ui.label("Finalizing the destination");
                    }
                    if let Some(reason) = &job.failure {
                        ui.colored_label(egui::Color32::LIGHT_RED, reason);
                    }
                });
        }
        if let Some(result) = self
            .whole_song_export_tail_result
            .as_mut()
            .filter(|result| result.open)
        {
            let text = result.text();
            egui::Window::new("Whole-song fixed-window tail result")
                .id(egui::Id::new("whole-song-tail-result")).open(&mut result.open)
                .collapsible(false).default_width(540.0).show(ctx, |ui| {
                    ui.label(text);
                    ui.label("This result remains available from File → Last whole-song tail warning for this App run.");
                });
        }
    }
    pub(super) fn reopen_whole_song_tail_result(&mut self) {
        if let Some(result) = &mut self.whole_song_export_tail_result {
            result.open = true;
        }
    }
}

fn start_whole_song_renderer(
    job: &mut WholeSongExportJob,
    project: Project,
    captured: crate::instrument_bounce::CapturedInstrumentBounce,
    control: RenderControl,
) -> Result<MasterSongRenderReceipt, String> {
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    if captured.plan().is_serial_master() {
        job.source_commit = Some(
            job.export
                .require_source_checked_commit()
                .map_err(|e| e.to_string())?,
        );
        let chains = std::mem::take(&mut job.captured_effects)
            .into_iter()
            .map(|captured| {
                let (lease, chain) = captured.split_for_render();
                job.effect_leases.push(lease);
                chain
            })
            .collect();
        #[cfg(test)]
        if let Some(starter) = job.serial_render_starter.take() {
            return starter(
                &job.admission,
                project,
                captured,
                chains,
                job.options,
                job.path.clone(),
                control,
                job.reviewed_fixed_tail,
            );
        }
        return job.admission.start_master_song_serial(
            project,
            captured,
            chains,
            job.options,
            job.path.clone(),
            control,
            job.reviewed_fixed_tail,
        );
    }
    #[cfg(test)]
    if let Some(starter) = job.render_starter.take() {
        return starter(
            &job.admission,
            project,
            captured,
            job.options,
            job.path.clone(),
            control,
            job.reviewed_fixed_tail,
        );
    }
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    {
        job.admission.start_master_song(
            project,
            captured,
            job.options,
            job.path.clone(),
            control,
            job.reviewed_fixed_tail,
            crate::plugins::strict_backend::StrictBackend::new,
        )
    }
    #[cfg(not(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    )))]
    {
        drop((job, project, captured, control));
        Err("Strict whole-song renderer is unavailable".into())
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[path = "whole_song_export_job_tests.rs"]
mod tests;

#[cfg(all(
    test,
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[path = "melody_workflow_sdk_tests.rs"]
mod melody_workflow_sdk_tests;

// Explicit Windows genuine Whole QA may only borrow actual retained evidence.
// None of these accessors consumes a receipt, pumps an owner, changes a fence,
// selects a factory, or manufactures stop/capture authority.
#[cfg(all(test, windows, feature = "experimental-fresh-helper"))]
impl WholeSongExportJob {
    pub(in crate::app) fn genuine_whole_outcome(&self) -> Option<&MasterSongRenderOutcome> {
        self.outcome.as_ref()
    }
    pub(in crate::app) fn genuine_whole_failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
    pub(in crate::app) fn genuine_whole_capture_failure(&self) -> Option<&CapturePollError> {
        self.capture_failure.as_ref()
    }
    pub(in crate::app) fn genuine_whole_production_factory_selected(&self) -> bool {
        self.render_starter.is_none() && self.serial_render_starter.is_none()
    }
    pub(in crate::app) fn genuine_whole_phase(&self) -> &'static str {
        self.phase.name()
    }
    pub(in crate::app) fn genuine_whole_terminal_owners_drained(&self) -> bool {
        self.phase.terminal()
            && !self.pending()
            && !self.actor.owns_pending_capture()
            && self.render.is_none()
            && !self.admission.is_busy()
            && self.retired_source.is_none()
            && self.retired_inserts.is_empty()
    }
    pub(in crate::app) fn genuine_whole_path(&self) -> &std::path::Path {
        &self.path
    }
    pub(in crate::app) fn genuine_whole_stop_preparation(
        &self,
    ) -> Option<super::stopped_bounce_capture::StopPreparation> {
        self.actor.stop_preparation
    }
    pub(in crate::app) fn genuine_whole_settlement(
        &self,
    ) -> Option<crate::plugins::plugin_runtime::BounceSourceSettlementObservation> {
        self.actor.stop_settlement
    }
    pub(in crate::app) fn genuine_whole_binding(&self) -> Option<LiveBinding> {
        self.actor.binding
    }
    pub(in crate::app) fn genuine_whole_old_epoch(&self) -> Option<u64> {
        self.actor.old_epoch
    }
    pub(in crate::app) fn genuine_whole_source_target(
        &self,
    ) -> Option<crate::plugins::plugin_runtime::BounceSourceCallbackIdentity> {
        self.actor.settlement_target
    }
}
