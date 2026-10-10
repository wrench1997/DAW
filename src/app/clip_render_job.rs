//! App-owned selected-occurrence conversion. Every asynchronous owner remains in
//! this job until its own terminal acknowledgment. Publication and insertion have
//! different cancellation boundaries; a published WAV never implies a model edit.
use super::clip_render::{
    OccurrencePlan,
    candidate::{
        OccurrenceHistoryCommit, PreparedOccurrenceHistory, prepare_rendered_occurrence_candidate,
        prepare_rendered_occurrence_history_transaction,
    },
};
use super::clip_render_cache::PreparedRetainedClipMedia;
use super::*;
use crate::{
    audio::{GeneratorRemovalRejectReason, GeneratorRemovalStamp},
    export_job::ExportControl,
    instrument_bounce::{
        BounceSourceIdentity, BounceSourceStatus, CapturePollError, CapturedInstrumentBounce,
        InstrumentBounceCapture, InstrumentBouncePlan,
        occurrence::OccurrenceCaptureAttestation,
        render::{
            OccurrenceRenderOutcome, OccurrenceRenderReceipt, RenderAdmission, RenderControl,
            RenderStatus,
        },
    },
    occurrence_media::{
        OccurrencePublicationFailure, PublishedOccurrenceRender, VerificationAdmission,
        VerificationOutcome, VerificationReceipt, VerificationStartFailure,
        VerifiedOccurrenceMedia,
    },
    occurrence_request::{FixedTailAcknowledgment, OccurrenceStamp},
    plugins::plugin_runtime::{PluginEndpointSnapshot, RetainedShutdownPoll},
};
use std::sync::atomic::{AtomicU8, Ordering};

const APP_OPEN: u8 = 0;
const APP_CANCELLED: u8 = 1;
const APP_COMMIT_ACCEPTED: u8 = 2;
const RENDER_BUDGET: Duration = Duration::from_secs(300);
const STOP_BUDGET: Duration = Duration::from_secs(35);

fn bounded_clip_timeline_text(text: &str) -> String {
    let mut chars = text.chars();
    let mut bounded = chars.by_ref().take(256).collect::<String>();
    if chars.next().is_some() {
        bounded.push('…');
    }
    bounded
}

/// One checked policy for review wording, acknowledgment, and the actual token.
fn checked_fixed_tail_frames(seconds: f64, sample_rate: u32) -> Result<u32, String> {
    if !seconds.is_finite()
        || !(0.0..=30.0).contains(&seconds)
        || !(8_000..=192_000).contains(&sample_rate)
    {
        return Err("The reviewed fixed tail or sample rate is invalid".into());
    }
    let frames = (seconds * f64::from(sample_rate)).round();
    if !frames.is_finite() || frames < 0.0 || frames > f64::from(sample_rate) * 30.0 {
        return Err("The reviewed fixed tail is invalid".into());
    }
    Ok(frames as u32)
}

/// This CAS is intentionally independent from ExportControl's file-publication CAS.
#[derive(Clone, Default)]
struct AppOperationControl(Arc<AtomicU8>);
impl AppOperationControl {
    fn cancel(&self) -> bool {
        matches!(
            self.0
                .compare_exchange(APP_OPEN, APP_CANCELLED, Ordering::AcqRel, Ordering::Acquire),
            Ok(_) | Err(APP_CANCELLED)
        )
    }
    fn seal(&self) -> bool {
        self.0
            .compare_exchange(
                APP_OPEN,
                APP_COMMIT_ACCEPTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == APP_CANCELLED
    }
    fn accepted(&self) -> bool {
        self.0.load(Ordering::Acquire) == APP_COMMIT_ACCEPTED
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClipRenderPhase {
    Review,
    StoppingDraining,
    Capturing,
    RenderingDisposing,
    VerifyingMedia,
    RecapturingSource,
    ReadyToCommit,
    Removing,
    CommittedSourceJoining,
    CommittedPlaybackPending,
    Complete,
    Refused,
    Cancelled,
    RetainedFile,
    CommittedPlaybackFailedPaused,
}
impl ClipRenderPhase {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Review => "Review clip render",
            Self::StoppingDraining => "Stopping and draining",
            Self::Capturing => "Capturing source",
            Self::RenderingDisposing => "Rendering / disposing",
            Self::VerifyingMedia => "Verifying published media",
            Self::RecapturingSource => "Recapturing source",
            Self::ReadyToCommit => "Ready to commit",
            Self::Removing => "Finishing: removing exact source",
            Self::CommittedSourceJoining => "Committed; waiting for source shutdown",
            Self::CommittedPlaybackPending => "Committed; waiting for media and Timeline",
            Self::Complete => "Conversion complete; playback paused",
            Self::Refused => "Conversion refused; Project unchanged",
            Self::Cancelled => "Conversion cancelled; Project unchanged",
            Self::RetainedFile => "Render retained; Project unchanged",
            Self::CommittedPlaybackFailedPaused => "Conversion committed; playback unavailable",
        }
    }
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Complete
                | Self::Refused
                | Self::Cancelled
                | Self::RetainedFile
                | Self::CommittedPlaybackFailedPaused
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LiveBinding {
    stamp: OccurrenceStamp,
    control_revision: u64,
    endpoint: PluginEndpointSnapshot,
    sample_rate: u32,
    asset_generation: u64,
}
impl LiveBinding {
    /// Review explicitly authorizes one preparatory stopped chase, never a
    /// change to source, model, control requests, device, or native authority.
    fn same_except_epoch(self, mut other: Self) -> bool {
        other.stamp.epoch = self.stamp.epoch;
        self == other
    }
}

struct Review {
    snapshot: Arc<[u8]>,
    initial: LiveBinding,
    clip_id: u32,
    clip_name: String,
    pattern_name: String,
    instrument_name: String,
    start_beat: f32,
    length_beats: f32,
    channel_gain: f32,
    directory: String,
    tail_seconds: f64,
    tail_acknowledged: bool,
}

/// Typed failures remain owned alongside any successfully published destination.
/// UI diagnostics never flatten a failed link into a claimed retained file.
/// Payloads intentionally retain typed failure/ownership evidence after the
/// bounded visible message is produced; no dummy read or payload erasure.
enum JobFailureEvidence {
    Capture(#[allow(dead_code)] CapturePollError),
    Render(#[allow(dead_code)] crate::instrument_bounce::render::RenderFailure),
    Publication(#[allow(dead_code)] OccurrencePublicationFailure),
    Verification(#[allow(dead_code)] crate::occurrence_media::MediaVerificationFailure),
    VerificationWorker(#[allow(dead_code)] crate::occurrence_media::RetainedMediaEvidence),
}

pub(super) struct ClipRenderJob {
    operation_id: u64,
    status_visible: bool,
    removal_outcome_unknown: bool,
    publication_outcome_unknown: bool,
    source_removed: bool,
    model_commit_started: bool,
    failure_pause_requested: bool,
    phase: ClipRenderPhase,
    review: Review,
    operation: AppOperationControl,
    export: ExportControl,
    render_admission: RenderAdmission,
    verification_admission: VerificationAdmission,
    started_at: Instant,
    old_epoch: Option<u64>,
    binding: Option<LiveBinding>,
    plan: Option<OccurrencePlan>,
    capture: Option<InstrumentBounceCapture>,
    attestation: Option<OccurrenceCaptureAttestation>,
    render: Option<OccurrenceRenderReceipt>,
    verifier: Option<VerificationReceipt>,
    verified: Option<VerifiedOccurrenceMedia>,
    publication: Option<PublishedOccurrenceRender>,
    retained_path: Option<PathBuf>,
    failure_evidence: Option<JobFailureEvidence>,
    failure: Option<String>,
    secondary_failure: Option<String>,
    history: Option<PreparedOccurrenceHistory>,
    restoration: Option<ClipRenderRestorationBinding>,
    removal: Option<GeneratorRemovalStamp>,
    removal_queued: bool,
    retired_source: Option<RunningGeneratorChain>,
    source_joined: bool,
    commit: Option<OccurrenceHistoryCommit>,
    staged_cache: Option<CachedAudioAssetRegistration>,
    cache_for_install: Option<CachedAudioAssetRegistration>,
    retained_media: Option<PreparedRetainedClipMedia>,
    asset_operation: Option<AudioAssetOperation>,
    asset_queued: bool,
    asset_ack: Option<bool>,
    expected_timeline_generation: Option<u64>,
    committed_fingerprint: Option<u64>,
    committed_model_revision: Option<u64>,
    #[cfg(test)]
    render_starter: Option<TestRenderStarter>,
}

#[cfg(test)]
type TestRenderStarter = Box<
    dyn FnOnce(
        &RenderAdmission,
        CapturedInstrumentBounce,
        PathBuf,
        RenderControl,
    ) -> Result<OccurrenceRenderReceipt, String>,
>;

impl ClipRenderJob {
    pub(super) fn owns_source(&self, source: &RunningGeneratorChain) -> bool {
        let stamp = self.binding.unwrap_or(self.review.initial).stamp;
        (self.pending() || self.owns_pending_resource())
            && source.project_session == stamp.project_session
            && source.endpoint_id == stamp.endpoint_id
            && source.instance_id == stamp.instance_id
    }
    pub(super) fn label(&self) -> &'static str {
        if self.commit.is_some() && self.retained_media.is_some() {
            return "Committed media ownership recovery required; playback and history held";
        }
        if self.phase == ClipRenderPhase::Removing && self.removal_outcome_unknown {
            return "Removal outcome unknown; exact acknowledgment still pending";
        }
        if self.commit.is_none()
            && matches!(
                self.phase,
                ClipRenderPhase::CommittedSourceJoining
                    | ClipRenderPhase::CommittedPlaybackFailedPaused
            )
        {
            return if self.model_commit_started {
                "Model commit outcome unknown; playback paused"
            } else if self.source_removed {
                "Source removed; Project unchanged; playback paused"
            } else {
                "Source shutdown pending; Project unchanged"
            };
        }
        self.phase.label()
    }
    #[cfg(test)]
    pub(super) fn phase(&self) -> ClipRenderPhase {
        self.phase
    }
    pub(super) fn pending(&self) -> bool {
        !self.phase.terminal()
    }
    fn owns_pending_resource(&self) -> bool {
        self.capture.is_some()
            || self.render.is_some()
            || self.render_admission.is_busy()
            || self.verifier.is_some()
            || self.verification_admission.owns_pending()
            || self.retired_source.is_some() && !self.source_joined
            || self.removal_queued && self.commit.is_none()
            // A defensive post-commit install refusal owns the only sealed PCM
            // outside the session map. Never clear this owner or admit history,
            // a new session, ordinary decoding, or close until exact recovery.
            || self.commit.is_some() && self.retained_media.is_some()
    }
    fn cancel(&mut self) -> bool {
        if !self.operation.cancel() {
            return false;
        }
        // False here means file publication began. It does NOT reject App cancel.
        self.export.cancel();
        if let Some(capture) = &mut self.capture {
            capture.cancel();
        }
        if let Some(render) = &self.render {
            render.cancel();
        }
        if let Some(verifier) = &self.verifier {
            verifier.cancel();
        }
        true
    }
    fn fail(&mut self, message: impl Into<String>) {
        if self.failure.is_none() {
            self.failure = Some(message.into());
        }
        if !self.operation.accepted() {
            self.export.cancel();
            if let Some(capture) = &mut self.capture {
                capture.cancel();
            }
            if let Some(render) = &self.render {
                render.cancel();
            }
            if let Some(verifier) = &self.verifier {
                verifier.cancel();
            }
        }
    }
    /// A completed producer fault outranks a generic observer diagnostic which
    /// may have seen the same worker become inactive just before polling it.
    fn establish_failure(&mut self, message: String) {
        if self
            .failure
            .as_ref()
            .is_some_and(|previous| previous != &message)
        {
            self.secondary_failure = self.failure.take();
        }
        self.failure = Some(message);
    }
    fn finish_unchanged(&mut self) {
        if self.retired_source.is_some() && !self.source_joined {
            self.phase = ClipRenderPhase::CommittedSourceJoining;
            return;
        }
        debug_assert!(!self.owns_pending_resource());
        self.phase = if self.retained_path.is_some() {
            ClipRenderPhase::RetainedFile
        } else if self.operation.cancelled() && self.failure.is_none() {
            ClipRenderPhase::Cancelled
        } else {
            ClipRenderPhase::Refused
        };
        self.attestation = None;
        self.verified = None;
        self.plan = None;
        self.history = None;
        self.restoration = None;
        self.retained_media = None;
        self.staged_cache = None;
        self.cache_for_install = None;
    }
    fn aborted(&self) -> bool {
        self.operation.cancelled() || self.failure.is_some()
    }
    fn output_path(&self) -> Result<PathBuf, String> {
        let directory = Path::new(self.review.directory.trim());
        if !directory.is_absolute()
            || directory.to_str().is_none()
            || self.review.directory.len() > 4096
        {
            return Err("Choose a permanent absolute UTF-8 render directory".into());
        }
        // Atomic create-new publication is authoritative. A collision refuses;
        // no pre-existence observation is treated as ownership or overwrite consent.
        Ok(directory.join(format!(
            "clip-{}-session-{}-render-{}.wav",
            self.review.clip_id, self.review.initial.stamp.project_session, self.operation_id
        )))
    }
}

impl CitrusApp {
    /// Engine-loss teardown transfers the exact source owner here instead of
    /// dropping its guard. The callback outcome remains unknown until its receipt.
    pub(super) fn retain_clip_render_source_on_engine_loss(
        &mut self,
        source: RunningGeneratorChain,
    ) -> Result<(), RunningGeneratorChain> {
        let Some(job) = self.clip_render_job.as_mut() else {
            return Err(source);
        };
        let expected = job.binding.unwrap_or(job.review.initial).stamp;
        if source.project_session != expected.project_session
            || source.endpoint_id != expected.endpoint_id
            || source.instance_id != expected.instance_id
            || job.retired_source.is_some()
        {
            return Err(source);
        }
        job.retired_source = Some(source);
        job.fail("Audio/session loss retained the exact source worker; cleanup and any pending removal acknowledgment must finish before another source can start");
        if !job.removal_queued
            && job.capture.is_none()
            && job.render.is_none()
            && job.verifier.is_none()
        {
            job.phase = ClipRenderPhase::CommittedSourceJoining;
        }
        Ok(())
    }
    pub(super) fn clip_render_blocks_playback(&self) -> bool {
        self.clip_render_job.as_ref().is_some_and(|job| {
            job.pending()
                || job.owns_pending_resource()
                || job.phase == ClipRenderPhase::CommittedPlaybackFailedPaused
        })
    }
    /// Only the parent restoration gate may clear this after it verifies a new
    /// exact endpoint/Timeline. Hiding the result window cannot clear it.
    pub(super) fn clear_clip_render_playback_failure_after_restoration(&mut self) {
        if self.clip_render_job.as_ref().is_some_and(|job| {
            job.phase == ClipRenderPhase::CommittedPlaybackFailedPaused
                && job.source_joined
                && !job.owns_pending_resource()
        }) {
            self.clip_render_job = None;
        }
    }
    fn clip_render_panic(&mut self, job: &mut ClipRenderJob) {
        self.playing = false;
        self.transport_clock_playing = false;
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
        job.fail("Clip conversion panicked; retained owners are being drained and playback remains paused");
        job.status_visible = true;
        if job.source_removed || job.model_commit_started || job.commit.is_some() {
            self.plugin_processing_retry_required = true;
            job.phase = if job.retired_source.is_some() || !job.source_joined {
                ClipRenderPhase::CommittedSourceJoining
            } else {
                ClipRenderPhase::CommittedPlaybackFailedPaused
            };
        } else if job.verification_admission.owns_pending() {
            job.phase = ClipRenderPhase::VerifyingMedia;
        } else if job.render_admission.is_busy() || job.render.is_some() {
            job.phase = ClipRenderPhase::RenderingDisposing;
        } else if !job.owns_pending_resource() {
            job.finish_unchanged();
        }
    }
    pub(super) fn clip_render_fence_active(&self) -> bool {
        self.clip_render_job
            .as_ref()
            .is_some_and(|job| job.pending() || job.owns_pending_resource())
    }
    pub(super) fn clip_render_busy_reason(&self) -> Option<&'static str> {
        self.clip_render_job
            .as_ref()
            .filter(|job| {
                job.pending()
                    || job.owns_pending_resource()
                    || job.phase == ClipRenderPhase::CommittedPlaybackFailedPaused
            })
            .map(ClipRenderJob::label)
    }
    pub(super) fn clip_render_allows_timeline_dispatch(&self) -> bool {
        self.clip_render_job.as_ref().is_none_or(|job| {
            !(job.commit.is_some() && job.retained_media.is_some())
                && matches!(
                    job.phase,
                    ClipRenderPhase::Review
                        | ClipRenderPhase::StoppingDraining
                        | ClipRenderPhase::CommittedPlaybackPending
                        | ClipRenderPhase::Complete
                        | ClipRenderPhase::Refused
                        | ClipRenderPhase::Cancelled
                        | ClipRenderPhase::RetainedFile
                        | ClipRenderPhase::CommittedPlaybackFailedPaused
                )
        })
    }
    pub(super) fn clip_render_allows_asset_dispatch(&self) -> bool {
        !self.clip_render_fence_active()
            || self.clip_render_job.as_ref().is_some_and(|job| {
                job.phase == ClipRenderPhase::CommittedPlaybackPending
                    && job.asset_ack == Some(true)
            })
    }
    pub(super) fn cancel_clip_render_job(&mut self) -> bool {
        self.clip_render_job
            .as_mut()
            .is_some_and(ClipRenderJob::cancel)
    }
    pub(super) fn clip_render_unavailable_reason(&self) -> Option<String> {
        if !cfg!(all(
            target_os = "linux",
            feature = "experimental-fresh-helper"
        )) {
            return Some(
                "Clip rendering requires Linux with experimental-fresh-helper enabled".into(),
            );
        }
        if let Some(reason) = self.clip_render_busy_reason() {
            return Some(reason.into());
        }
        self.clip_render_admission().err()
    }
    fn clip_render_admission(&self) -> Result<u32, String> {
        if !self.project.piano_notes.is_empty() {
            return Err(
                "Resolve legacy Piano Roll notes before rendering a Playlist occurrence".into(),
            );
        }
        if self.project_snapshot_transition_pending()
            || self.project_lifecycle_barriers_active()
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
            || self.export_job.is_running()
            || self.queued_save_after_midi_recording.is_some()
            || !self.loading_audio_assets.is_empty()
            || !self.pending_audio_asset_registrations.is_empty()
            || !self.prepared_audio_asset_registrations.is_empty()
            || !self.pending_audio_asset_unregistrations.is_empty()
            || self.pending_audio_asset_clear.is_some()
        {
            return Err(
                "Finish the current save, import, gesture, dialog, or session operation first"
                    .into(),
            );
        }
        if self.plugin_parameter_edits.reserved_len() != 0
            || !self.plugin_parameter_edit_draft_desired.is_empty()
        {
            return Err(
                "Wait for parameter edits and unsent drafts to finish before reviewing a render"
                    .into(),
            );
        }
        if !self.midi_input.state.is_disconnected()
            || !self.midi_recording.is_idle()
            || self.recording
            || self.input_recorder.is_some()
            || self.pending_recording.is_some()
            || self.master_capture.is_some()
            || self.pending_master_capture.is_some()
        {
            return Err("Disconnect MIDI and finish recording/input/master capture first".into());
        }
        if self.transport_mode != TransportMode::Song {
            return Err("Switch to SONG mode before rendering a Playlist occurrence".into());
        }
        if self.playlist_selection_ids.len() != 1 {
            return Err("Select exactly one unmuted Pattern Clip".into());
        }
        let clip_id = *self
            .playlist_selection_ids
            .iter()
            .next()
            .expect("one selection");
        if self.selected_clip != Some(clip_id) {
            return Err("The Inspector and Playlist selection differ".into());
        }
        self.clip_render_selected_channel(clip_id)?;
        Ok(clip_id)
    }
    fn clip_render_selected_channel(&self, clip_id: u32) -> Result<u32, String> {
        let clip = self
            .project
            .clips
            .iter()
            .find(|clip| clip.id == clip_id)
            .ok_or("The selected Clip disappeared")?;
        if clip.kind != ClipKind::Pattern || clip.muted {
            return Err("Select an unmuted Pattern Clip".into());
        }
        let pattern = self
            .project
            .patterns
            .iter()
            .find(|pattern| pattern.id == clip.pattern_id)
            .ok_or("The selected Pattern disappeared")?;
        if pattern.channel_steps.len() != self.project.channels.len() {
            return Err("The Pattern has ambiguous Channel membership".into());
        }
        let mut channel = None;
        for id in pattern
            .channel_steps
            .iter()
            .zip(&self.project.channels)
            .filter_map(|(steps, channel)| steps.iter().any(|on| *on).then_some(Some(channel.id)))
            .chain(
                pattern
                    .notes
                    .iter()
                    .filter(|note| !note.muted)
                    .map(|note| note.channel_id),
            )
        {
            let id = id.ok_or("The Pattern contains an unassigned note")?;
            if channel.is_some_and(|current| current != id) {
                return Err("The selected Pattern must contain exactly one musical Channel".into());
            }
            channel = Some(id);
        }
        channel.ok_or_else(|| "The selected Pattern has no musical Channel".into())
    }
    fn clip_render_observe(&self, channel_id: u32) -> Result<LiveBinding, String> {
        // Legacy notes are skipped by persistence and therefore require a live
        // check as well as review admission; snapshot equality cannot see them.
        if !self.project.piano_notes.is_empty() {
            return Err(
                "Resolve legacy Piano Roll notes before rendering a Playlist occurrence".into(),
            );
        }
        if self.clip_render_control_revision == 0 || self.clip_render_model_revision == 0 {
            return Err("App mutation identities are exhausted; open a new session".into());
        }
        let audio = self.audio.as_ref().ok_or("Audio is unavailable")?;
        if audio.project_session() != self.project_session
            || self.plugin_topology_is_settling()
            || !self.audio_restart_state.allows_realtime_session_actions()
            || self.plugin_processing_blocks_playback()
        {
            return Err("Wait for a healthy, stable audio/source session".into());
        }
        let channel = self
            .project
            .channels
            .iter()
            .find(|channel| channel.id == channel_id)
            .ok_or("The source Channel disappeared")?;
        let instance_id = channel
            .instrument_plugin_instance_id
            .ok_or("The Channel has no VST3 generator")?;
        let instance = self
            .project
            .plugin_instances
            .iter()
            .find(|instance| instance.id == instance_id)
            .ok_or("The source instance disappeared")?;
        let running = self
            .running_generator_chains
            .get(&channel_id)
            .ok_or("Wait for a callback-confirmed running generator")?;
        if running.project_session != self.project_session
            || running.instance_id != instance_id
            || self.project.mixer_runtime_slot(channel.mixer_track) != Some(running.mixer_track)
            || instance.runtime_status != PluginRuntimeStatus::Loaded
        {
            return Err(
                "The running generator does not match the Project assignment/route or SlotReady"
                    .into(),
            );
        }
        let endpoint = running
            .control
            .plugin_endpoint_snapshot()
            .ok_or("The source has no coherent active endpoint metadata")?;
        let stats = running.control.stats();
        if endpoint.slot_count() != 1
            || endpoint.manifest().instance_ids[0] != Some(instance_id)
            || endpoint.active_mask() != 1
            || stats.faults != 0
            || stats.stopped
        {
            return Err("The source endpoint is inactive, faulted, stopped, or ambiguous".into());
        }
        let native = running
            .control
            .native_editor_snapshot(0)
            .ok_or("The actual source native observation is unavailable")?;
        if native.state.open
            || native.pending_request.is_some()
            || native.capture_in_progress
            || native.error.is_some()
        {
            return Err("Close the native editor and finish its pending capture first".into());
        }
        let (timeline_revision, epoch) = audio
            .confirmed_timeline_identity()
            .ok_or("The exact Timeline has not been acknowledged")?;
        if !self.timeline_audio_matches_desired()
            || self.timeline_audio_source.as_ref().is_none_or(|source| {
                source.fingerprint != self.clip_timeline_fingerprint(self.timeline_sample_rate())
            })
        {
            return Err("Wait for the exact current Project Timeline acknowledgment".into());
        }
        let stamp = OccurrenceStamp {
            project_session: self.project_session,
            project_revision: self.clip_render_model_revision,
            timeline_revision,
            epoch,
            engine_session: audio.engine_session(),
            endpoint_id: running.endpoint_id,
            channel_id,
            instance_id,
            mixer_track_id: channel.mixer_track,
            native_generation: native.state.generation,
            app_native_dirty_revision: native.dirty_revision,
            native_capture_serial: native.capture_serial,
            parameter_capture_serial: native.parameter_capture_serial,
            native_control_request: native.completed_request,
            parameter_control_request: self.next_plugin_parameter_request_id,
        };
        stamp.validate()?;
        Ok(LiveBinding {
            stamp,
            control_revision: self.clip_render_control_revision,
            endpoint,
            sample_rate: audio.snapshot().sample_rate,
            asset_generation: self.audio_asset_generation,
        })
    }
    fn clip_render_status(&self, binding: LiveBinding) -> BounceSourceStatus {
        let audio = self.audio.as_ref();
        let status = audio.map(AudioEngine::snapshot);
        BounceSourceStatus {
            confirmed_stopped: !self.playing
                && status.as_ref().is_some_and(|snapshot| {
                    !snapshot.transport_playing && snapshot.transport_epoch == binding.stamp.epoch
                }),
            exact_timeline: audio.is_some_and(|audio| {
                audio.engine_session() == binding.stamp.engine_session
                    && audio.project_session() == binding.stamp.project_session
                    && audio.confirmed_timeline_identity()
                        == Some((binding.stamp.timeline_revision, binding.stamp.epoch))
            }) && self.timeline_audio_matches_desired(),
            source_ready: self
                .clip_render_observe(binding.stamp.channel_id)
                .is_ok_and(|current| current == binding),
            parameter_edits_pending: self.plugin_parameter_edits.reserved_len() != 0
                || !self.plugin_parameter_edit_draft_desired.is_empty()
                || self
                    .running_generator_chains
                    .get(&binding.stamp.channel_id)
                    .is_some_and(|source| source.control.outstanding_parameter_edits() != 0),
            native_editor_open_or_pending: self
                .native_editor_snapshot(binding.stamp.instance_id)
                .is_none_or(|native| {
                    native.state.open
                        || native.pending_request.is_some()
                        || native.capture_in_progress
                }),
            session_transition: self.plugin_topology_is_settling()
                || !self.audio_restart_state.allows_realtime_session_actions(),
            live_midi_or_recording: !self.midi_input.state.is_disconnected()
                || !self.midi_recording.is_idle()
                || self.recording
                || self.input_recorder.is_some()
                || self.pending_recording.is_some()
                || self.master_capture.is_some()
                || self.pending_master_capture.is_some()
                || !self.pending_generator_note_offs.is_empty(),
        }
    }
    pub(super) fn open_selected_clip_render_review(&mut self) {
        if self.clip_render_blocks_playback() {
            return;
        }
        if !cfg!(all(
            target_os = "linux",
            feature = "experimental-fresh-helper"
        )) {
            self.notify(
                "Clip rendering requires Linux with experimental-fresh-helper enabled".into(),
            );
            return;
        }
        match self.prepare_clip_render_review() {
            Ok(job) => self.clip_render_job = Some(job),
            Err(reason) => self.notify(format!("Clip render unavailable: {reason}")),
        }
    }
    fn prepare_clip_render_review(&mut self) -> Result<ClipRenderJob, String> {
        let clip_id = self.clip_render_admission()?;
        let channel_id = self.clip_render_selected_channel(clip_id)?;
        let initial = self.clip_render_observe(channel_id)?;
        let snapshot =
            clip_render::project_snapshot(&self.project).map_err(|error| error.to_string())?;
        // Full pure admission at review (the token is never used for capture).
        let provisional = Arc::new(FixedTailAcknowledgment::review(
            Arc::clone(&snapshot),
            initial.stamp,
            clip_id,
            initial.sample_rate,
            0,
        )?);
        drop(
            OccurrencePlan::prepare(
                &self.project,
                initial.stamp,
                clip_id,
                initial.sample_rate,
                0,
                Some(provisional),
            )
            .map_err(|error| error.to_string())?,
        );
        let clip = self
            .project
            .clips
            .iter()
            .find(|clip| clip.id == clip_id)
            .expect("admitted clip");
        let pattern = self
            .project
            .patterns
            .iter()
            .find(|pattern| pattern.id == clip.pattern_id)
            .expect("admitted pattern");
        let channel = self
            .project
            .channels
            .iter()
            .find(|channel| channel.id == channel_id)
            .expect("admitted channel");
        let instance = self
            .project
            .plugin_instances
            .iter()
            .find(|instance| instance.id == initial.stamp.instance_id)
            .expect("admitted source");
        let operation_id = self.next_clip_render_operation_id;
        self.next_clip_render_operation_id = operation_id
            .checked_add(1)
            .filter(|id| *id != 0)
            .ok_or("Clip render operation identities are exhausted; open a new session")?;
        let directory = self
            .saved_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf)
            .and_then(|path| path.to_str().map(str::to_owned))
            .unwrap_or_default();
        Ok(ClipRenderJob {
            operation_id,
            status_visible: true,
            removal_outcome_unknown: false,
            publication_outcome_unknown: false,
            source_removed: false,
            model_commit_started: false,
            failure_pause_requested: false,
            phase: ClipRenderPhase::Review,
            review: Review {
                snapshot,
                initial,
                clip_id,
                clip_name: clip.name.clone(),
                pattern_name: pattern.name.clone(),
                instrument_name: instance.name.clone(),
                start_beat: clip.start,
                length_beats: clip.length,
                channel_gain: channel.volume,
                directory,
                tail_seconds: 0.0,
                tail_acknowledged: false,
            },
            operation: AppOperationControl::default(),
            export: ExportControl::default(),
            render_admission: RenderAdmission::default(),
            verification_admission: VerificationAdmission::default(),
            started_at: Instant::now(),
            old_epoch: None,
            binding: None,
            plan: None,
            capture: None,
            attestation: None,
            render: None,
            verifier: None,
            verified: None,
            publication: None,
            retained_path: None,
            failure_evidence: None,
            failure: None,
            secondary_failure: None,
            history: None,
            restoration: None,
            removal: None,
            removal_queued: false,
            retired_source: None,
            source_joined: false,
            commit: None,
            staged_cache: None,
            cache_for_install: None,
            retained_media: None,
            asset_operation: None,
            asset_queued: false,
            asset_ack: None,
            expected_timeline_generation: None,
            committed_fingerprint: None,
            committed_model_revision: None,
            #[cfg(test)]
            render_starter: None,
        })
    }
    /// Explicit review acceptance. No pre-stop consent token survives this step.
    fn begin_clip_render_stop(&mut self, job: &mut ClipRenderJob) -> Result<(), String> {
        if !job.review.tail_acknowledged {
            return Err("Acknowledge the exact fixed-tail/cutoff choice first".into());
        }
        checked_fixed_tail_frames(job.review.tail_seconds, job.review.initial.sample_rate)?;
        job.output_path()?;
        let current = self.clip_render_observe(job.review.initial.stamp.channel_id)?;
        if current != job.review.initial
            || !clip_render::project_matches_snapshot(&self.project, &job.review.snapshot)
                .map_err(|error| error.to_string())?
        {
            return Err(
                "The reviewed source or Project changed; review the conversion again".into(),
            );
        }
        self.clip_render_admission()?;
        self.playing = false;
        self.transport_clock_playing = false;
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
        job.started_at = Instant::now();
        job.phase = ClipRenderPhase::StoppingDraining;
        Ok(())
    }

    pub(super) fn clip_render_review_ui(&mut self, ctx: &egui::Context) {
        let Some(mut job) = self.clip_render_job.take() else {
            return;
        };
        if !job.status_visible {
            self.clip_render_job = Some(job);
            return;
        }
        let mut start = false;
        let mut cancel = false;
        let mut dismiss = false;
        let ui_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            egui::Window::new("Render selected clip to audio")
            .id(egui::Id::new("selected_clip_render_review"))
            .collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label(job.label());
                if job.phase == ClipRenderPhase::Review {
                    ui.label(format!("Clip: {} · Pattern: {}", job.review.clip_name, job.review.pattern_name));
                    ui.label(format!("Instrument: {}", job.review.instrument_name));
                    ui.label(format!("Song beats {:.6}–{:.6} · {} Hz", job.review.start_beat,
                        job.review.start_beat + job.review.length_beats, job.review.initial.sample_rate));
                    ui.label("This stops playback and requests a stopped Timeline chase before capture.");
                    ui.label("Render a fresh dry instrument snapshot at this Clip's song position. Live voice/LFO history and random phase are not reproduced.");
                    ui.label(format!("Channel gain {:.6} transfers to the Audio Clip; MIDI Clip gain remains in velocity. No normalization.", job.review.channel_gain));
                    ui.label("The MIDI Clip will be muted and its retained generator disabled. Undo restores the captured generator state.");
                    ui.horizontal(|ui| {
                        ui.label("Permanent render directory");
                        ui.text_edit_singleline(&mut job.review.directory);
                        if ui.button("Choose directory…").clicked()
                            && let Some(directory) = rfd::FileDialog::new().pick_folder() {
                            if let Some(directory) = directory.to_str() { job.review.directory = directory.to_owned(); }
                            else { job.failure = Some("The render directory must be UTF-8".into()); }
                        }
                    });
                    ui.label("Use an existing permanent directory. The rendered file is retained after Undo or a cancelled insertion.");
                    if let Ok(path) = job.output_path() { ui.label(format!("Create-new destination: {}", path.display())); }
                    let previous_frames = checked_fixed_tail_frames(job.review.tail_seconds, job.review.initial.sample_rate).ok();
                    let tail = ui.add(egui::DragValue::new(&mut job.review.tail_seconds).range(0.0..=30.0)
                        .speed(0.1).suffix(" s fixed tail"));
                    let reviewed_frames = checked_fixed_tail_frames(job.review.tail_seconds, job.review.initial.sample_rate);
                    if tail.changed() || previous_frames != reviewed_frames.as_ref().ok().copied() { job.review.tail_acknowledged = false; }
                    match reviewed_frames {
                        Ok(frames) => {
                            ui.label(format!("Effective fixed tail: {} frames ({:.6} s)", frames,
                                f64::from(frames) / f64::from(job.review.initial.sample_rate)));
                            let acknowledgment = if frames == 0 {
                                "I acknowledge that audio is cut exactly at the Clip end (zero tail)"
                            } else { "I acknowledge this fixed tail may truncate the instrument, and it must fit the song" };
                            ui.checkbox(&mut job.review.tail_acknowledged, acknowledgment);
                        }
                        Err(reason) => { job.review.tail_acknowledged = false; ui.colored_label(Color32::YELLOW, reason); }
                    }
                    start = ui.add_enabled(job.review.tail_acknowledged && !job.review.directory.trim().is_empty(),
                        egui::Button::new("Render and convert")).clicked();
                }
                if let Some(RenderStatus::CleanupPending { diagnostics }) = job.render.as_ref()
                    .map(OccurrenceRenderReceipt::status).or_else(|| job.render_admission.status()) {
                    ui.colored_label(Color32::YELLOW, "Renderer cleanup pending. Its backend owner has not acknowledged destruction; the operation remains held.");
                    for diagnostic in diagnostics { ui.label(diagnostic); }
                }
                if let Some(reason) = &job.failure { ui.colored_label(Color32::YELLOW, reason); }
                if let Some(reason) = &job.secondary_failure { ui.label(format!("Additional observation: {reason}")); }
                if job.publication_outcome_unknown && let Ok(path) = job.output_path() {
                    ui.label(format!("Publication outcome unknown. Review destination: {}", path.display()));
                }
                if let Some(path) = &job.retained_path { ui.label(format!("Retained render: {}", path.display())); }
                if job.pending() {
                    if job.operation.accepted() { ui.label("Finishing. Cancellation is no longer available."); }
                    else {
                        cancel = ui.button(if job.operation.cancelled() { "Waiting for cleanup…" } else { "Cancel conversion" }).clicked();
                    }
                    if job.operation.cancelled() && job.owns_pending_resource() {
                        ui.label("Cleanup pending. Keep this window open until capture, render, or verification acknowledges termination.");
                    }
                } else { dismiss = ui.button("Close").clicked(); }
            })
        }));
        if ui_result.is_err() {
            self.clip_render_panic(&mut job);
        }
        if cancel {
            job.cancel();
        }
        if start {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.begin_clip_render_stop(&mut job)
            })) {
                Ok(Ok(())) => {}
                Ok(Err(reason)) => {
                    job.fail(reason);
                    job.finish_unchanged();
                }
                Err(_) => self.clip_render_panic(&mut job),
            }
        }
        if job.pending() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if dismiss && !job.owns_pending_resource() {
            job.status_visible = false;
        }
        self.clip_render_job = Some(job);
    }

    pub(super) fn drive_clip_render_job(&mut self) {
        let Some(mut job) = self.clip_render_job.take() else {
            return;
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.drive_clip_render_job_owned(&mut job)
        }));
        if result.is_err() {
            self.clip_render_panic(&mut job);
        }
        self.clip_render_job = Some(job);
    }

    fn clip_render_live_recheck(&self, job: &ClipRenderJob) -> Result<LiveBinding, String> {
        let expected = job.binding.ok_or("The stopped source binding is missing")?;
        let current = self.clip_render_observe(expected.stamp.channel_id)?;
        if current != expected
            || !clip_render::project_matches_snapshot(&self.project, &job.review.snapshot)
                .map_err(|error| error.to_string())?
        {
            return Err(
                "The exact source, native observation, control stamps, or Project changed".into(),
            );
        }
        let status = self.clip_render_status(current);
        if !status.confirmed_stopped
            || !status.exact_timeline
            || !status.source_ready
            || status.parameter_edits_pending
            || status.native_editor_open_or_pending
            || status.session_transition
            || status.live_midi_or_recording
        {
            return Err(
                "The source is no longer quiescent at its exact stopped Timeline epoch".into(),
            );
        }
        Ok(current)
    }

    fn begin_clip_render_capture(
        &self,
        job: &ClipRenderJob,
    ) -> Result<InstrumentBounceCapture, String> {
        let binding = self.clip_render_live_recheck(job)?;
        let request = Arc::clone(
            job.plan
                .as_ref()
                .ok_or("The occurrence plan is missing")?
                .request(),
        );
        let plan = InstrumentBouncePlan::prepare_occurrence(&self.project, request)?;
        let identity = BounceSourceIdentity {
            project_session: binding.stamp.project_session,
            project_fingerprint: plan.project_fingerprint(),
            timeline_revision: binding.stamp.timeline_revision,
            epoch: binding.stamp.epoch,
            endpoint_id: binding.stamp.endpoint_id,
            channel_id: binding.stamp.channel_id,
            instance_id: binding.stamp.instance_id,
        };
        let running = self
            .running_generator_chains
            .get(&binding.stamp.channel_id)
            .ok_or("The source disappeared")?;
        InstrumentBounceCapture::begin(
            plan,
            identity,
            self.clip_render_status(binding),
            &running.control,
            Instant::now(),
        )
    }

    fn pause_failed_clip_render(&mut self, job: &mut ClipRenderJob) {
        self.playing = false;
        self.transport_clock_playing = false;
        if !job.failure_pause_requested {
            job.failure_pause_requested = true;
            if let Some(audio) = &self.audio {
                audio.set_playing(false);
            }
        }
    }

    fn drive_clip_render_job_owned(&mut self, job: &mut ClipRenderJob) {
        if job.phase.terminal() {
            return;
        }
        if job.phase == ClipRenderPhase::Review {
            if job.operation.cancelled() {
                job.finish_unchanged();
            }
            return;
        }
        // The preparatory false request was published exactly once. Repeating
        // it here would create a new mailbox request immediately before stamped
        // removal and invalidate the callback's exact transport admission.
        if job.binding.is_some()
            && (self.playing
                || self.transport_clock_playing
                || self
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.snapshot().transport_playing))
        {
            job.fail("Playback changed after the exact stopped conversion admission");
            self.pause_failed_clip_render(job);
        }
        if job.binding.is_some()
            && !job.operation.accepted()
            && !job.aborted()
            && let Err(reason) = self.clip_render_live_recheck(job)
        {
            job.fail(reason);
        }
        if job.operation.cancelled() {
            if let Some(capture) = &mut job.capture {
                capture.cancel();
            }
            if let Some(render) = &job.render {
                render.cancel();
            }
            if let Some(verifier) = &job.verifier {
                verifier.cancel();
            }
        }
        match job.phase {
            ClipRenderPhase::StoppingDraining => self.drive_clip_render_stop(job),
            ClipRenderPhase::Capturing | ClipRenderPhase::RecapturingSource => {
                self.drive_clip_render_capture(job)
            }
            ClipRenderPhase::RenderingDisposing => self.drive_clip_render_render(job),
            ClipRenderPhase::VerifyingMedia => self.drive_clip_render_verification(job),
            ClipRenderPhase::ReadyToCommit => self.seal_clip_render_commit(job),
            ClipRenderPhase::Removing => self.queue_clip_render_removal(job),
            ClipRenderPhase::CommittedSourceJoining => self.drive_clip_render_source_join(job),
            ClipRenderPhase::CommittedPlaybackPending => self.drive_clip_render_playback(job),
            _ => {}
        }
    }

    fn drive_clip_render_stop(&mut self, job: &mut ClipRenderJob) {
        if job.aborted() {
            job.finish_unchanged();
            return;
        }
        if job.started_at.elapsed() >= STOP_BUDGET {
            job.fail("Timed out waiting for parameter/preview drain and a stopped Timeline acknowledgment");
            job.finish_unchanged();
            return;
        }
        if self.plugin_parameter_edits.reserved_len() != 0
            || !self.plugin_parameter_edit_draft_desired.is_empty()
        {
            job.fail("Parameter work appeared after the bound review");
            job.finish_unchanged();
            return;
        }
        if job.old_epoch.is_none() {
            let Some(audio) = self.audio.as_mut() else {
                job.fail("Audio became unavailable before stopping");
                job.finish_unchanged();
                return;
            };
            let old_epoch = audio.snapshot().transport_epoch;
            if !audio.command(AudioCommand::StopAll) {
                return;
            }
            self.pending_generator_note_offs.clear();
            job.old_epoch = Some(old_epoch);
            if !self.request_timeline_resume_chase() {
                job.fail("No exact stopped Timeline chase is available");
                job.finish_unchanged();
            }
            return;
        }
        let Some(audio) = self.audio.as_ref() else {
            job.fail("Audio disappeared while stopping");
            job.finish_unchanged();
            return;
        };
        let snapshot = audio.snapshot();
        if snapshot.transport_playing
            || Some(snapshot.transport_epoch) == job.old_epoch
            || !self.timeline_audio_matches_desired()
        {
            return;
        }
        let prepare = (|| {
            let current = self.clip_render_observe(job.review.initial.stamp.channel_id)?;
            if !job.review.initial.same_except_epoch(current)
                || !clip_render::project_matches_snapshot(&self.project, &job.review.snapshot)
                    .map_err(|error| error.to_string())?
            {
                return Err("The source or Project changed during the preparatory stop".to_owned());
            }
            let frames = checked_fixed_tail_frames(job.review.tail_seconds, current.sample_rate)?;
            let consent = Arc::new(FixedTailAcknowledgment::review(
                Arc::clone(&job.review.snapshot),
                current.stamp,
                job.review.clip_id,
                current.sample_rate,
                frames,
            )?);
            job.plan = Some(
                OccurrencePlan::prepare(
                    &self.project,
                    current.stamp,
                    job.review.clip_id,
                    current.sample_rate,
                    frames,
                    Some(consent),
                )
                .map_err(|error| error.to_string())?,
            );
            job.binding = Some(current);
            job.capture = Some(self.begin_clip_render_capture(job)?);
            Ok::<(), String>(())
        })();
        match prepare {
            Ok(()) => job.phase = ClipRenderPhase::Capturing,
            Err(reason) => {
                job.fail(reason);
                job.finish_unchanged();
            }
        }
    }

    fn drive_clip_render_capture(&mut self, job: &mut ClipRenderJob) {
        let binding = job.binding.expect("capture requires binding");
        let identity = BounceSourceIdentity {
            project_session: binding.stamp.project_session,
            project_fingerprint: project_snapshot_fingerprint(&job.review.snapshot),
            timeline_revision: binding.stamp.timeline_revision,
            epoch: binding.stamp.epoch,
            endpoint_id: binding.stamp.endpoint_id,
            channel_id: binding.stamp.channel_id,
            instance_id: binding.stamp.instance_id,
        };
        let status = self.clip_render_status(binding);
        let Some(capture) = job.capture.as_mut() else {
            job.fail("Capture ownership was lost");
            job.finish_unchanged();
            return;
        };
        let result = capture.poll_typed(identity, status, Instant::now());
        let captured = match result {
            Ok(None) => return,
            Err(reason) => {
                job.capture = None;
                if matches!(
                    reason,
                    CapturePollError::Backend(_) | CapturePollError::Invalid(_)
                ) {
                    job.establish_failure(reason.to_string());
                } else if reason != CapturePollError::Cancelled || !job.operation.cancelled() {
                    job.fail(reason.to_string());
                }
                job.failure_evidence = Some(JobFailureEvidence::Capture(reason));
                job.finish_unchanged();
                return;
            }
            Ok(Some(captured)) => captured,
        };
        job.capture = None;
        if job.aborted() {
            drop(captured);
            job.finish_unchanged();
            return;
        }
        if job.phase == ClipRenderPhase::RecapturingSource {
            let matched = match job
                .attestation
                .take()
                .expect("retained render attestation")
                .match_recapture(captured)
            {
                Ok(matched) => matched,
                Err(reason) => {
                    job.fail(reason);
                    job.finish_unchanged();
                    return;
                }
            };
            // The second capture has been destroyed before any Project image is cloned.
            match self.prepare_clip_render_transaction(job, &matched) {
                Ok(()) => job.phase = ClipRenderPhase::ReadyToCommit,
                Err(reason) => {
                    job.fail(reason);
                    job.finish_unchanged();
                }
            }
            return;
        }
        let attestation = match captured.retain_occurrence_attestation() {
            Ok(attestation) => attestation,
            Err(reason) => {
                job.fail(reason);
                job.finish_unchanged();
                return;
            }
        };
        // Raw native revision is first established by this dedicated receipt;
        // never relabel it as the App-relative dirty revision in OccurrenceStamp.
        job.attestation = Some(attestation);
        let path = match job.output_path() {
            Ok(path) => path,
            Err(reason) => {
                job.fail(reason);
                job.finish_unchanged();
                return;
            }
        };
        let control = RenderControl {
            export: job.export.clone(),
            deadline: Instant::now() + RENDER_BUDGET,
        };
        let start = start_occurrence_renderer(job, captured, path, control);
        match start {
            Ok(receipt) => {
                job.render = Some(receipt);
                job.phase = ClipRenderPhase::RenderingDisposing;
            }
            Err(reason) => {
                job.fail(reason);
                job.finish_unchanged();
            }
        }
    }

    fn drive_clip_render_render(&mut self, job: &mut ClipRenderJob) {
        if job.render.is_none() {
            if job.render_admission.is_busy() {
                return;
            }
            job.publication_outcome_unknown = true;
            job.fail(
                "Renderer receipt unavailable after termination; publication outcome is unknown",
            );
            job.finish_unchanged();
            return;
        }
        let result = job.render.as_ref().expect("render owner").try_recv();
        let outcome = match result {
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                if !job
                    .render
                    .as_ref()
                    .expect("render owner")
                    .termination_acknowledged()
                {
                    return;
                }
                job.render = None;
                job.fail("The terminated renderer returned no outcome");
                job.finish_unchanged();
                return;
            }
            Ok(outcome) => outcome,
        };
        // The original receipt/control is retained through the complete typed file outcome.
        job.render = None;
        match outcome {
            OccurrenceRenderOutcome::Complete { published, .. } => {
                job.retained_path = Some(published.path().to_path_buf());
                if job.aborted() {
                    job.publication = Some(published);
                    job.finish_unchanged();
                    return;
                }
                match job.verification_admission.start(published) {
                    Ok(receipt) => {
                        job.verifier = Some(receipt);
                        job.phase = ClipRenderPhase::VerifyingMedia;
                    }
                    Err(error) => {
                        match error {
                            VerificationStartFailure::Busy(published) => {
                                job.publication = Some(published);
                                job.fail("A media verifier still owns its prior operation");
                            }
                            VerificationStartFailure::Refused { published, problem } => {
                                job.fail(problem.to_string());
                                job.publication = Some(published);
                            }
                            VerificationStartFailure::Spawn { retained, problem } => {
                                job.fail(problem.to_string());
                                job.failure_evidence =
                                    Some(JobFailureEvidence::VerificationWorker(retained));
                            }
                        }
                        job.finish_unchanged();
                    }
                }
            }
            OccurrenceRenderOutcome::RenderFailed(error) => {
                // Preserve actual backend/cleanup failures even if App cancel raced them.
                if error.kind != crate::instrument_bounce::render::FailureKind::Cancelled
                    || !job.operation.cancelled()
                {
                    job.establish_failure(error.primary.clone());
                }
                job.failure_evidence = Some(JobFailureEvidence::Render(error));
                job.finish_unchanged();
            }
            OccurrenceRenderOutcome::PublicationFailed(error) => {
                job.retained_path = error
                    .writer
                    .as_ref()
                    .and_then(|writer| writer.evidence.retained_destination.clone());
                let message = error
                    .writer
                    .as_ref()
                    .map(|writer| writer.to_string())
                    .or_else(|| error.problem.as_ref().map(ToString::to_string))
                    .unwrap_or_else(|| "Occurrence publication failed".into());
                job.fail(message);
                job.failure_evidence = Some(JobFailureEvidence::Publication(error));
                job.finish_unchanged();
            }
        }
    }

    fn drive_clip_render_verification(&mut self, job: &mut ClipRenderJob) {
        let outcome = match job.verifier.as_mut() {
            Some(receipt) => receipt.poll(),
            None => job.verification_admission.poll_abandoned_owned(),
        };
        let Some(outcome) = outcome else {
            if job
                .verifier
                .as_ref()
                .is_some_and(VerificationReceipt::termination_acknowledged)
                || job.verifier.is_none() && !job.verification_admission.owns_pending()
            {
                job.verifier = None;
                job.fail("Verification outcome unavailable after worker termination; published render retained");
                job.finish_unchanged();
            }
            return;
        };
        // Both paths return only after the exact finished-handle join.
        debug_assert!(!job.verification_admission.owns_pending());
        job.verifier = None;
        match outcome {
            VerificationOutcome::Verified(media) => {
                job.verified = Some(media);
                if job.aborted() {
                    job.finish_unchanged();
                    return;
                }
                match self.begin_clip_render_capture(job) {
                    Ok(capture) => {
                        job.capture = Some(capture);
                        job.phase = ClipRenderPhase::RecapturingSource;
                    }
                    Err(reason) => {
                        job.fail(reason);
                        job.finish_unchanged();
                    }
                }
            }
            VerificationOutcome::Failed(error) => {
                job.fail(error.problem.to_string());
                job.failure_evidence = Some(JobFailureEvidence::Verification(error));
                job.finish_unchanged();
            }
            VerificationOutcome::WorkerPanicked { retained, problem } => {
                job.fail(problem.to_string());
                job.failure_evidence = Some(JobFailureEvidence::VerificationWorker(retained));
                job.finish_unchanged();
            }
        }
    }
    fn prepare_clip_render_transaction(
        &mut self,
        job: &mut ClipRenderJob,
        matched: &crate::instrument_bounce::occurrence::MatchedOccurrenceSource,
    ) -> Result<(), String> {
        let binding = self.clip_render_live_recheck(job)?;
        if job.aborted() {
            return Err("Insertion was cancelled before image preparation".into());
        }
        let media = job
            .verified
            .as_ref()
            .ok_or("Verified media ownership is missing")?;
        let plan = job
            .plan
            .as_ref()
            .ok_or("The exact occurrence plan is missing")?;
        let candidate = prepare_rendered_occurrence_candidate(
            &self.project,
            binding.stamp,
            plan,
            media,
            matched,
        )
        .map_err(|error| error.to_string())?;
        let history = prepare_rendered_occurrence_history_transaction(
            binding.stamp,
            &self.project,
            self.history_fingerprint,
            &mut self.undo_stack,
            &self.redo_stack,
            candidate,
        )
        .map_err(|error| error.to_string())?;
        let restoration = self.prepare_clip_render_restoration(
            binding.stamp.instance_id,
            matched.provenance().metadata(),
        )?;
        // Reserve every App-owned map insertion before removal. This does not
        // pretend to make serde/Project or the global allocator universally fallible.
        self.cached_audio_assets
            .try_reserve(1)
            .map_err(|_| "Unable to reserve converted media cache")?;
        self.loaded_audio_assets
            .try_reserve(1)
            .map_err(|_| "Unable to reserve converted media residency")?;
        let asset_id = history.result().asset_id;
        let path = history.result().published_path.clone();
        job.committed_model_revision = Some(
            self.clip_render_model_revision
                .checked_add(1)
                .ok_or("Project mutation identities are exhausted")?,
        );
        job.history = Some(history);
        job.restoration = Some(restoration);
        // PCM staging is deliberately pre-CommitAccepted. Its reviewed temporary
        // Vec+Arc peak and global allocator caveat are stated at the helper.
        self.stage_clip_render_pcm(job, asset_id, path)?;
        job.asset_operation = Some(self.next_audio_asset_operation());
        job.removal = Some(GeneratorRemovalStamp {
            operation_id: job.operation_id,
            engine_session: binding.stamp.engine_session,
            project_session: binding.stamp.project_session,
            channel_id: binding.stamp.channel_id,
            endpoint_id: binding.stamp.endpoint_id,
            plugin_instance_id: binding.stamp.instance_id,
            stopped_epoch: binding.stamp.epoch,
        });
        self.clip_render_live_recheck(job)?;
        if job.aborted() {
            return Err("Insertion was cancelled before sealing".into());
        }
        Ok(())
    }

    /// Reviewed Stop B ledger amendment: the THREE private history Projects
    /// already exist. During this one Vec→Arc conversion, V and the new Arc PCM
    /// overlap (at most 2 × 24,000,000 stereo frames × 8 bytes), plus the original
    /// snapshot/capture attestation/three images. The Vec is released before
    /// ReadyToCommit/removal. Stable Arc allocation is NOT a recoverable-OOM API;
    /// checked lengths and map try_reserve do not make the global allocator fallible.
    fn stage_clip_render_pcm(
        &mut self,
        job: &mut ClipRenderJob,
        asset_id: u64,
        path: PathBuf,
    ) -> Result<(), String> {
        if job.aborted() {
            return Err("Insertion was cancelled before PCM staging".into());
        }
        let media = job.verified.as_ref().ok_or("Verified PCM is missing")?;
        let frames = media.asset().metadata.frames;
        let samples = usize::try_from(frames)
            .ok()
            .and_then(|frames| frames.checked_mul(2))
            .ok_or("Verified PCM extent overflow")?;
        if frames == 0
            || frames > 24_000_000
            || samples != media.asset().samples.len()
            || samples.checked_mul(std::mem::size_of::<f32>()).is_none()
            || media.asset().metadata.channels != 2
        {
            return Err("Verified PCM exceeds the reviewed staging bound".into());
        }
        if media.path() != path {
            return Err("Verified PCM path differs from the prepared history asset".into());
        }
        let pcm = job
            .verified
            .take()
            .expect("checked media")
            .into_verified_pcm();
        let prepared = self
            .prepare_retained_clip_media(job.operation_id, asset_id, pcm)
            .map_err(|failure| {
                // The linked destination remains owned externally. Recoverable
                // refusal drops only this private verified-memory owner.
                drop(failure.pcm);
                failure.message
            })?;
        if Some(prepared.stamp()) != job.binding.map(|binding| binding.stamp) {
            return Err("Compact verified PCM differs from the exact stopped job binding".into());
        }
        // Small path copies are fallibly reserved before removal. All three
        // owners share the same producer-created PCM allocation.
        job.staged_cache = Some(prepared.cached()?);
        job.cache_for_install = Some(prepared.cached()?);
        job.retained_media = Some(prepared);
        if job.aborted() {
            return Err("Insertion was cancelled during PCM staging".into());
        }
        Ok(())
    }

    fn seal_clip_render_commit(&mut self, job: &mut ClipRenderJob) {
        if job.aborted() {
            job.finish_unchanged();
            return;
        }
        let validation = self.clip_render_live_recheck(job).and_then(|binding| {
            job.history
                .as_ref()
                .ok_or_else(|| "The prepared history transaction is missing".to_owned())?
                .revalidate(
                    &self.project,
                    binding.stamp,
                    self.history_fingerprint,
                    self.undo_stack.len(),
                    self.redo_stack.len(),
                )
                .map_err(|error| error.to_string())?;
            self.revalidate_retained_clip_media(
                job.retained_media
                    .as_ref()
                    .ok_or("The prepared verified-media owner is missing")?,
            )
        });
        if let Err(reason) = validation {
            job.fail(reason);
            job.finish_unchanged();
            return;
        }
        if !job.operation.seal() {
            job.finish_unchanged();
            return;
        }
        job.phase = ClipRenderPhase::Removing;
        self.queue_clip_render_removal(job);
    }

    fn queue_clip_render_removal(&mut self, job: &mut ClipRenderJob) {
        if job.removal_queued {
            let stamp = job.removal.expect("queued removal stamp");
            if self.project_session != stamp.project_session
                || self.audio.as_ref().is_none_or(|audio| {
                    audio.engine_session() != stamp.engine_session
                        || audio.project_session() != stamp.project_session
                })
            {
                job.removal_outcome_unknown = true;
                job.fail("Removal outcome unknown after engine/session loss; waiting for the exact callback acknowledgment and retaining the transaction/source owner");
            }
            return;
        }
        if job.aborted() {
            job.finish_unchanged();
            return;
        }
        // A full command ring leaves the same sealed transaction pending. It
        // never permits a different endpoint, reload, or second removal command.
        let validation = self.clip_render_live_recheck(job).and_then(|binding| {
            job.history
                .as_ref()
                .ok_or_else(|| "The prepared history transaction is missing".to_owned())?
                .revalidate(
                    &self.project,
                    binding.stamp,
                    self.history_fingerprint,
                    self.undo_stack.len(),
                    self.redo_stack.len(),
                )
                .map_err(|error| error.to_string())?;
            self.revalidate_retained_clip_media(
                job.retained_media
                    .as_ref()
                    .ok_or("The prepared verified-media owner is missing")?,
            )
        });
        if let Err(reason) = validation {
            job.fail(reason);
            job.finish_unchanged();
            return;
        }
        let stamp = job.removal.expect("sealed exact removal stamp");
        job.removal_queued = self
            .audio
            .as_mut()
            .is_some_and(|audio| audio.remove_generator_endpoint_stamped(stamp));
    }

    pub(super) fn consume_clip_render_removal_event(
        &mut self,
        stamp: GeneratorRemovalStamp,
        result: Result<(), GeneratorRemovalRejectReason>,
    ) {
        let Some(mut job) = self.clip_render_job.take() else {
            return;
        };
        let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.consume_clip_render_removal_owned(&mut job, stamp, result)
        }));
        if observed.is_err() {
            self.clip_render_panic(&mut job);
        }
        self.clip_render_job = Some(job);
    }

    fn consume_clip_render_removal_owned(
        &mut self,
        job: &mut ClipRenderJob,
        stamp: GeneratorRemovalStamp,
        result: Result<(), GeneratorRemovalRejectReason>,
    ) {
        if job.phase != ClipRenderPhase::Removing
            || !job.removal_queued
            || job.removal != Some(stamp)
        {
            return;
        }
        job.removal_queued = false;
        match result {
            Err(reason) => {
                job.fail(format!(
                    "The callback refused exact source removal: {reason:?}"
                ));
                self.pause_failed_clip_render(job);
                job.finish_unchanged();
            }
            Ok(()) => {
                job.source_removed = true;
                // No clone, serialization, reservation, file operation, or
                // fallible candidate work follows this exact successful ack.
                let current = self.audio.as_ref().is_some_and(|audio| {
                    audio.engine_session() == stamp.engine_session
                        && audio.project_session() == stamp.project_session
                }) && self.project_session == stamp.project_session;
                let exact = self
                    .running_generator_chains
                    .get(&stamp.channel_id)
                    .is_some_and(|source| {
                        source.project_session == stamp.project_session
                            && source.endpoint_id == stamp.endpoint_id
                            && source.instance_id == stamp.plugin_instance_id
                    });
                if !current || !exact {
                    // An engine loss cannot authorize a stale success. Retain
                    // the exact old source if still owned, join it, and stay paused.
                    if exact {
                        job.retired_source =
                            self.running_generator_chains.remove(&stamp.channel_id);
                    }
                    job.fail("The App engine/source identity changed before its removal receipt; playback remains paused");
                    self.plugin_processing_retry_required = true;
                    job.phase = ClipRenderPhase::CommittedSourceJoining;
                    if job.retired_source.is_none() {
                        // An invariant violation must retain the job/fence; never
                        // manufacture a source join that was not observed.
                        job.failure = Some("Exact removed source ownership is unavailable; shutdown is not verified".into());
                    }
                    return;
                }
                job.source_removed = true;
                job.retired_source = self.running_generator_chains.remove(&stamp.channel_id);
                job.model_commit_started = true;
                let history = job
                    .history
                    .take()
                    .expect("sealed transaction retained until exact acknowledgment");
                let result = history.apply(
                    &mut self.project,
                    &mut self.history_snapshot,
                    &mut self.history_fingerprint,
                    &mut self.undo_stack,
                    &mut self.redo_stack,
                    &mut self.dirty,
                );
                self.install_clip_render_restoration(
                    job.restoration
                        .take()
                        .expect("pre-reserved restoration binding"),
                );
                self.clip_render_model_revision = job
                    .committed_model_revision
                    .expect("prechecked model revision");
                self.project_fingerprint = self.history_fingerprint;
                job.committed_fingerprint = Some(self.history_fingerprint);
                job.commit = Some(result);
                let retained = job
                    .retained_media
                    .take()
                    .expect("pre-reserved verified PCM owner");
                if let Err(retained) = self.install_retained_clip_media(retained) {
                    // An impossible late ledger collision may not overwrite an
                    // existing authority or discard this committed asset's PCM.
                    job.retained_media = Some(retained);
                    job.fail("The model committed but verified-media ownership could not be installed; playback remains paused");
                }
                job.phase = ClipRenderPhase::CommittedSourceJoining;
            }
        }
    }

    fn drive_clip_render_source_join(&mut self, job: &mut ClipRenderJob) {
        let Some(source) = job.retired_source.as_mut() else {
            return;
        };
        match source.guard.poll_shutdown_retained() {
            RetainedShutdownPoll::Pending => return,
            RetainedShutdownPoll::WorkerPanicked => {
                job.source_joined = true;
                job.fail("The removed source worker panicked during shutdown; its handle was joined, playback remains paused");
            }
            RetainedShutdownPoll::Joined => job.source_joined = true,
        }
        // Exact terminal join is observed before any new audio, Undo, or source load.
        job.retired_source = None;
        if job.commit.is_none() {
            if job.source_removed || job.model_commit_started {
                self.plugin_processing_retry_required = true;
                job.phase = ClipRenderPhase::CommittedPlaybackFailedPaused;
            } else {
                job.finish_unchanged();
            }
            return;
        }
        if let Some(cache) = job.cache_for_install.take() {
            self.cached_audio_assets
                .insert(cache.project_asset_id, cache);
        }
        let binding = job.binding.expect("committed binding");
        if job.failure.is_some()
            || self.audio.as_ref().is_none_or(|audio| {
                audio.engine_session() != binding.stamp.engine_session
                    || audio.project_session() != binding.stamp.project_session
            })
            || self.project_session != binding.stamp.project_session
        {
            job.fail("The conversion is committed, but its audio session is unavailable");
            job.phase = ClipRenderPhase::CommittedPlaybackFailedPaused;
            return;
        }
        job.phase = ClipRenderPhase::CommittedPlaybackPending;
        self.drive_clip_render_playback(job);
    }

    pub(super) fn consume_clip_render_asset_event(&mut self, event: AudioAssetEvent) -> bool {
        let AudioAssetEvent::Registered {
            operation,
            id,
            success,
        } = event
        else {
            return false;
        };
        let Some(job) = self.clip_render_job.as_mut() else {
            return false;
        };
        if job.asset_operation != Some(operation)
            || job
                .commit
                .as_ref()
                .is_none_or(|commit| commit.asset_id != id)
        {
            return false;
        }
        if job.phase == ClipRenderPhase::CommittedPlaybackPending
            && job.asset_queued
            && job.asset_ack.is_none()
        {
            job.asset_ack = Some(success);
        }
        true
    }

    fn drive_clip_render_playback(&mut self, job: &mut ClipRenderJob) {
        // A coincident media/Timeline acknowledgment cannot erase an earlier
        // playback/session fault. Committed history and verified PCM stay owned.
        if job.aborted() {
            self.pause_failed_clip_render(job);
            job.phase = ClipRenderPhase::CommittedPlaybackFailedPaused;
            return;
        }
        if self.timeline_audio_sync.phase == TimelineAudioSyncPhase::Faulted {
            let detail = self
                .timeline_audio_sync
                .last_error
                .as_deref()
                .map(bounded_clip_timeline_text)
                .unwrap_or_else(|| {
                    "The Timeline runtime faulted during committed playback admission".into()
                });
            job.fail(format!(
                "The conversion is committed, but the Timeline runtime faulted during playback admission: {detail}"
            ));
            self.pause_failed_clip_render(job);
            job.phase = ClipRenderPhase::CommittedPlaybackFailedPaused;
            return;
        }
        let binding = job.binding.expect("committed source binding");
        let operation = job.asset_operation.expect("preissued media registration");
        if self.project_session != binding.stamp.project_session
            || Some(self.clip_render_model_revision) != job.committed_model_revision
            || Some(project_fingerprint(&self.project)) != job.committed_fingerprint
            || self.audio_asset_generation != binding.asset_generation
            || operation.engine_session != self.audio_asset_engine_session
            || self.audio.as_ref().is_none_or(|audio| {
                audio.engine_session() != binding.stamp.engine_session
                    || audio.project_session() != binding.stamp.project_session
            })
            || self.plugin_processing_blocks_playback()
            || self.timeline_compile_error.is_some()
        {
            job.fail("The conversion is committed, but media/Timeline admission lost its exact healthy session");
            job.phase = ClipRenderPhase::CommittedPlaybackFailedPaused;
            return;
        }
        let cached = job.staged_cache.as_ref().expect("retained staged PCM");
        if !job.asset_queued {
            job.asset_queued = self.audio.as_mut().is_some_and(|audio| {
                audio.register_asset(
                    operation,
                    cached.project_asset_id,
                    Arc::clone(&cached.samples),
                    cached.sample_rate,
                    cached.channels,
                )
            });
            return;
        }
        match job.asset_ack {
            None => return,
            Some(false) => {
                job.fail("The conversion is committed, but the callback rejected its exact PCM registration");
                job.phase = ClipRenderPhase::CommittedPlaybackFailedPaused;
                return;
            }
            Some(true) => {}
        }
        if job.expected_timeline_generation.is_none() {
            // Both map capacities were reserved before removal. Cache and callback
            // share the already staged Arc; no new PCM decode/copy is performed.
            self.loaded_audio_assets
                .insert(cached.project_asset_id, cached.project_asset_id);
            // The cache was moved into its reserved slot only after source join.
            self.request_immediate_timeline_compile();
            job.expected_timeline_generation = Some(self.timeline_desired_generation);
            return;
        }
        let generation = job
            .expected_timeline_generation
            .expect("requested exact generation");
        if self.timeline_desired_generation != generation {
            job.fail(
                "The conversion is committed, but its requested Timeline generation was superseded",
            );
            job.phase = ClipRenderPhase::CommittedPlaybackFailedPaused;
            return;
        }
        let ready = self.timeline_audio_matches_desired()
            && self
                .audio
                .as_ref()
                .and_then(AudioEngine::confirmed_timeline_identity)
                .is_some_and(|(confirmed, epoch)| {
                    confirmed == generation && epoch != binding.stamp.epoch
                })
            && self.timeline_audio_source.as_ref().is_some_and(|source| {
                source.generation == generation
                    && source.fingerprint
                        == self.clip_timeline_fingerprint(self.timeline_sample_rate())
                    && source
                        .timeline
                        .audio_clips()
                        .iter()
                        .any(|clip| clip.asset_id == cached.project_asset_id)
            })
            && self.loaded_audio_assets.get(&cached.project_asset_id)
                == Some(&cached.project_asset_id)
            && !self
                .running_generator_chains
                .contains_key(&binding.stamp.channel_id)
            && !self
                .pending_generator_chains
                .contains_key(&binding.stamp.channel_id);
        if ready {
            job.phase = ClipRenderPhase::Complete;
            self.last_history_capture = Instant::now();
            self.notify("Selected clip converted to audio. Exact media and Timeline are ready; playback remains paused.".into());
        }
    }
}

fn project_snapshot_fingerprint(snapshot: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    snapshot.hash(&mut hasher);
    hasher.finish()
}

fn start_occurrence_renderer(
    job: &mut ClipRenderJob,
    capture: CapturedInstrumentBounce,
    path: PathBuf,
    control: RenderControl,
) -> Result<OccurrenceRenderReceipt, String> {
    #[cfg(test)]
    if let Some(starter) = job.render_starter.take() {
        return starter(&job.render_admission, capture, path, control);
    }
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    {
        job.render_admission.start_occurrence(
            capture,
            path,
            control,
            crate::plugins::strict_backend::StrictBackend::new,
        )
    }
    #[cfg(not(all(target_os = "linux", feature = "experimental-fresh-helper")))]
    {
        let _ = (job, capture, path, control);
        Err("Strict occurrence rendering is unavailable on this build".into())
    }
}

#[cfg(test)]
#[path = "clip_render_job_tests.rs"]
pub(in crate::app) mod tests;
