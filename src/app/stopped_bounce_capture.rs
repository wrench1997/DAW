//! Shared App-owned stopped source preparation and private capture actor.
//! Queue/confirmation provenance is accepted only through the existing actual
//! Timeline notification path. Snapshots never acknowledge activation. The
//! caller retains publication, transaction and abnormal source-retirement owners.
use super::*;
#[cfg(test)]
use crate::plugins::plugin_runtime::BounceSourceSettlementObservation;
use crate::{
    instrument_bounce::{
        BounceSourceIdentity, BounceSourceStatus, CapturePollError, CapturedInstrumentBounce,
        InstrumentBounceCapture, InstrumentBouncePlan,
    },
    occurrence_request::OccurrenceStamp,
    plugins::plugin_runtime::{
        BounceSourceCallbackIdentity, BounceSourceSettlement, PluginEndpointSnapshot,
    },
};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

pub(in crate::app) const STOP_BUDGET: Duration = Duration::from_secs(35);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct LiveBinding {
    pub(in crate::app) stamp: OccurrenceStamp,
    pub(in crate::app) control_revision: u64,
    pub(in crate::app) endpoint: PluginEndpointSnapshot,
    pub(in crate::app) sample_rate: u32,
    pub(in crate::app) asset_generation: u64,
}
impl LiveBinding {
    /// Review explicitly authorizes one preparatory stopped chase, never a
    /// change to source, model, control requests, device, or native authority.
    fn same_except_epoch(self, mut other: Self) -> bool {
        other.stamp.epoch = self.stamp.epoch;
        self == other
    }
}

/// Provenance for this job's one preparatory chase. The mailbox request and
/// Timeline activation request are separate identity domains. Only the actual
/// successful production queue/confirmation boundaries may fill activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct StopActivation {
    pub(in crate::app) request_id: u64,
    pub(in crate::app) minimum_epoch: u64,
    pub(in crate::app) committed_epoch: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct StopPreparation {
    pub(in crate::app) generation: u64,
    pub(in crate::app) seek_serial: u64,
    pub(in crate::app) frame: u64,
    pub(in crate::app) transport_request: u64,
    pub(in crate::app) activation: Option<StopActivation>,
    pub(in crate::app) superseded: bool,
}

/// Produced only after actual Timeline confirmation and source settlement.
/// Geometry/plan builders use this exact reviewed snapshot and frozen binding.
pub(in crate::app) struct StoppedCaptureReady {
    snapshot: Arc<[u8]>,
    binding: LiveBinding,
}

impl StoppedCaptureReady {
    pub(in crate::app) fn binding(&self) -> LiveBinding {
        self.binding
    }

    pub(in crate::app) fn snapshot(&self) -> &Arc<[u8]> {
        &self.snapshot
    }
}

/// Owns exactly the reviewed source snapshot, preparatory stop and actual
/// private capture receipt. It owns no source worker, publication or history;
/// the caller's retained resource fence must continue to cover those owners.
pub(in crate::app) struct StoppedBounceCapture {
    pub(in crate::app) snapshot: Arc<[u8]>,
    pub(in crate::app) initial: LiveBinding,
    pub(in crate::app) started_at: Instant,
    pub(in crate::app) old_epoch: Option<u64>,
    pub(in crate::app) stop_transport_request: Option<u64>,
    pub(in crate::app) stop_preparation: Option<StopPreparation>,
    pub(in crate::app) settlement_target: Option<BounceSourceCallbackIdentity>,
    #[cfg(test)]
    pub(in crate::app) stop_settlement: Option<BounceSourceSettlementObservation>,
    pub(in crate::app) binding: Option<LiveBinding>,
    pub(in crate::app) capture: Option<InstrumentBounceCapture>,
    stopping: bool,
    ready_binding: Option<LiveBinding>,
    capture_identity: Option<BounceSourceIdentity>,
    capture_abort_reason: Option<CapturePollError>,
}

impl StoppedBounceCapture {
    pub(in crate::app) fn new(snapshot: Arc<[u8]>, initial: LiveBinding, now: Instant) -> Self {
        Self {
            snapshot,
            initial,
            started_at: now,
            old_epoch: None,
            stop_transport_request: None,
            stop_preparation: None,
            settlement_target: None,
            #[cfg(test)]
            stop_settlement: None,
            binding: None,
            capture: None,
            stopping: false,
            ready_binding: None,
            capture_identity: None,
            capture_abort_reason: None,
        }
    }

    /// Caller admission supplies the operation-specific review guards, in the
    /// same order as the former Clip path, before any transport mutation.
    pub(in crate::app) fn begin_stop(
        &mut self,
        app: &mut CitrusApp,
        admission: impl FnOnce(&CitrusApp) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.stopping || self.stop_transport_request.is_some() || self.owns_pending_capture() {
            return Err("The reviewed stopped capture actor has already begun".into());
        }
        let current = app.stopped_bounce_observe(self.initial.stamp.channel_id)?;
        if current != self.initial
            || !clip_render::project_matches_snapshot(&app.project, &self.snapshot)
                .map_err(|error| error.to_string())?
        {
            return Err(
                "The reviewed source or Project changed; review the conversion again".into(),
            );
        }
        admission(app)?;
        app.playing = false;
        app.transport_clock_playing = false;
        if let Some(audio) = &app.audio {
            audio.set_playing(false);
            // Bind the actual false mailbox request at issuance, before a drive
            // can observe an unrelated later legacy request.
            self.stop_transport_request = Some(audio.transport_request_id());
        }
        self.started_at = Instant::now();
        self.stopping = true;
        Ok(())
    }

    pub(in crate::app) fn owns_pending_capture(&self) -> bool {
        self.capture.is_some()
    }

    /// A cancel/Stop need not republish the exact pause already issued by this
    /// actor. A foreign mailbox request or actual late playback still needs a
    /// fresh pause; callback lag alone never substitutes for request identity.
    pub(in crate::app) fn owns_current_pause_request(&self, app: &CitrusApp) -> bool {
        let (Some(request), Some(audio)) = (self.stop_transport_request, app.audio.as_ref()) else {
            return false;
        };
        let stamp = self.initial.stamp;
        app.project_session == stamp.project_session
            && audio.project_session() == stamp.project_session
            && audio.engine_session() == stamp.engine_session
            && audio.transport_request_id() == request
            && !app.playing
            && !app.transport_clock_playing
            && !audio.snapshot().transport_playing
    }

    pub(in crate::app) fn cancel_capture(&mut self) {
        if let Some(capture) = &mut self.capture {
            capture.cancel();
        }
    }

    /// Transfer only the actual issued coordinator and its saved identity to a
    /// retained shutdown owner. If identity is unexpectedly absent, the receipt
    /// stays here; None must be checked against owns_pending_capture().
    #[allow(dead_code)] // The whole-song shutdown adapter is a separate candidate.
    pub(in crate::app) fn take_cancelled_capture_for_shutdown(
        &mut self,
    ) -> Option<(InstrumentBounceCapture, BounceSourceIdentity)> {
        self.cancel_capture();
        let identity = self.capture_identity?;
        let capture = self.capture.take()?;
        self.capture_identity = None;
        self.capture_abort_reason = None;
        Some((capture, identity))
    }

    pub(in crate::app) fn observe_stop_activation_queued(
        &mut self,
        request_id: u64,
        generation: u64,
        seek_serial: u64,
        minimum_epoch: u64,
        frame: u64,
    ) {
        if !self.stopping {
            return;
        }
        let Some(stop) = &mut self.stop_preparation else {
            return;
        };
        if (generation, seek_serial, frame) != (stop.generation, stop.seek_serial, stop.frame)
            || stop.activation.is_some()
        {
            stop.superseded = true;
            return;
        }
        stop.activation = Some(StopActivation {
            request_id,
            minimum_epoch,
            committed_epoch: None,
        });
    }

    pub(in crate::app) fn observe_stop_activation_confirmed(
        &mut self,
        request_id: u64,
        generation: u64,
        epoch: u64,
        frame: u64,
    ) {
        if !self.stopping {
            return;
        }
        let Some(stop) = &mut self.stop_preparation else {
            return;
        };
        let Some(activation) = &mut stop.activation else {
            stop.superseded = true;
            return;
        };
        if (request_id, generation, frame) != (activation.request_id, stop.generation, stop.frame)
            || !timeline_epoch_has_reached(epoch, activation.minimum_epoch)
            || activation.committed_epoch.is_some()
        {
            stop.superseded = true;
            return;
        }
        activation.committed_epoch = Some(epoch);
    }

    pub(in crate::app) fn live_recheck(&self, app: &CitrusApp) -> Result<LiveBinding, String> {
        let expected = self
            .binding
            .ok_or("The stopped source binding is missing")?;
        let current = app.stopped_bounce_observe(expected.stamp.channel_id)?;
        if current != expected
            || !clip_render::project_matches_snapshot(&app.project, &self.snapshot)
                .map_err(|error| error.to_string())?
        {
            return Err(
                "The exact source, native observation, control stamps, or Project changed".into(),
            );
        }
        let status = app.stopped_bounce_status(current);
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

    /// Check authoritative intent even before callback/worker acknowledgment.
    /// Worker lag must wait; a different App/source/request must never be adopted.
    pub(in crate::app) fn stop_intent_current(&self, app: &CitrusApp) -> Result<(), String> {
        let stop = self
            .stop_preparation
            .ok_or("The preparatory stop intent is missing")?;
        let initial = self.initial;
        let stamp = initial.stamp;
        let audio = app
            .audio
            .as_ref()
            .ok_or("Audio disappeared while stopping")?;
        let source = app
            .running_generator_chains
            .get(&stamp.channel_id)
            .ok_or("The preparatory source disappeared")?;
        if stop.superseded
            || app.playing
            || app.transport_clock_playing
            || app.project_session != stamp.project_session
            || audio.project_session() != stamp.project_session
            || audio.engine_session() != stamp.engine_session
            || audio.transport_request_id() != stop.transport_request
            || app.clip_render_model_revision != stamp.project_revision
            || app.clip_render_control_revision != initial.control_revision
            || app.next_plugin_parameter_request_id != stamp.parameter_control_request
            || app.audio_asset_generation != initial.asset_generation
            || app.timeline_desired_generation != stop.generation
            || app.timeline_audio_sync.source_generation != Some(stop.generation)
            || app
                .timeline_audio_source
                .as_ref()
                .is_none_or(|source| source.generation != stop.generation)
            || app.timeline_audio_sync.desired_seek.serial != stop.seek_serial
            || app.timeline_audio_sync.desired_seek.frame != stop.frame
            || source.project_session != stamp.project_session
            || source.endpoint_id != stamp.endpoint_id
            || source.instance_id != stamp.instance_id
            || matches!(
                app.timeline_audio_sync.phase,
                TimelineAudioSyncPhase::Unavailable
                    | TimelineAudioSyncPhase::Faulted
                    | TimelineAudioSyncPhase::FaultClearReady { .. }
                    | TimelineAudioSyncPhase::FaultClearQueued { .. }
            )
            || app.plugin_processing_blocks_playback()
            || !app.audio_restart_state.allows_realtime_session_actions()
        {
            return Err(
                "The exact preparatory stop intent, request, source, or session changed".into(),
            );
        }
        let stats = source.control.stats();
        if stats.faults != 0 || stats.stopped {
            return Err("The preparatory source faulted or stopped".into());
        }
        if !clip_render::project_matches_snapshot(&app.project, &self.snapshot)
            .map_err(|error| error.to_string())?
        {
            return Err("The Project changed during the preparatory stop".into());
        }
        if let Some(epoch) = stop
            .activation
            .and_then(|activation| activation.committed_epoch)
            && (!matches!(app.timeline_audio_sync.phase,
                TimelineAudioSyncPhase::Active { generation, epoch: active_epoch }
                    if generation == stop.generation && active_epoch == epoch)
                || audio.confirmed_timeline_identity() != Some((stop.generation, epoch)))
        {
            return Err("The acknowledged preparatory Timeline was superseded".into());
        }
        Ok(())
    }

    /// Wait for the exact callback identity, reset and bridge input terminal.
    /// Ready returns the frozen live binding; operation-specific plans remain
    /// the caller's responsibility and cannot be prepared before this boundary.
    pub(in crate::app) fn poll_stop(
        &mut self,
        app: &mut CitrusApp,
    ) -> Result<Option<StoppedCaptureReady>, String> {
        if self.started_at.elapsed() >= STOP_BUDGET {
            return Err(
                "Timed out waiting for parameter/preview drain and a settled stopped Timeline"
                    .into(),
            );
        }
        if app.plugin_parameter_edits.reserved_len() != 0
            || !app.plugin_parameter_edit_draft_desired.is_empty()
        {
            return Err("Parameter work appeared after the bound review".into());
        }
        if self.old_epoch.is_none() {
            let audio = app
                .audio
                .as_mut()
                .ok_or("Audio became unavailable before stopping")?;
            if Some(audio.transport_request_id()) != self.stop_transport_request {
                return Err("The preparatory stopped transport request was superseded".into());
            }
            let old_epoch = audio.snapshot().transport_epoch;
            if !audio.command(AudioCommand::StopAll) {
                return Ok(None);
            }
            app.pending_generator_note_offs.clear();
            self.old_epoch = Some(old_epoch);
            if !app.request_timeline_resume_chase() {
                return Err("No exact stopped Timeline chase is available".into());
            }
            // A desired seek is only intent; actual notifications below own its
            // queue and confirmation evidence in separate request-ID domains.
            let intent = app.timeline_audio_sync.desired_seek;
            self.stop_preparation = Some(StopPreparation {
                generation: app
                    .timeline_audio_source
                    .as_ref()
                    .expect("issued source")
                    .generation,
                seek_serial: intent.serial,
                frame: intent.frame,
                transport_request: self.stop_transport_request.expect("issued stopped request"),
                activation: None,
                superseded: false,
            });
            return Ok(None);
        }
        self.stop_intent_current(app)?;
        let stop = self.stop_preparation.expect("checked preparatory intent");
        let Some(epoch) = stop
            .activation
            .and_then(|activation| activation.committed_epoch)
        else {
            return Ok(None);
        };
        let audio = app.audio.as_ref().expect("checked audio");
        let snapshot = audio.snapshot();
        if self.settlement_target.is_some() && snapshot.timeline_frame != stop.frame {
            return Err("The frozen preparatory Timeline frame changed".into());
        }
        if snapshot.transport_playing
            || snapshot.timeline_frame != stop.frame
            || Some(snapshot.transport_epoch) == self.old_epoch
            || !app.timeline_audio_matches_desired()
        {
            return Ok(None);
        }
        if self.settlement_target.is_none() {
            let current = app.stopped_bounce_observe(self.initial.stamp.channel_id)?;
            if !self.initial.same_except_epoch(current)
                || current.stamp.timeline_revision != stop.generation
                || current.stamp.epoch != epoch
            {
                return Err("The source changed during the preparatory stop".into());
            }
            self.binding = Some(current);
            self.settlement_target = Some(BounceSourceCallbackIdentity {
                engine_session: current.stamp.engine_session,
                project_session: current.stamp.project_session,
                timeline_generation: stop.generation,
                epoch,
                transport_request: stop.transport_request,
                channel_id: current.stamp.channel_id,
                endpoint_id: current.stamp.endpoint_id,
                instance_id: current.stamp.instance_id,
            });
        }
        let current = self.live_recheck(app)?;
        self.stop_intent_current(app)?;
        let target = self.settlement_target.expect("frozen settlement target");
        let observation = app
            .running_generator_chains
            .get(&current.stamp.channel_id)
            .ok_or("The source disappeared before settlement")?
            .control
            .observe_bounce_source_settlement(target);
        #[cfg(test)]
        {
            self.stop_settlement = Some(observation);
        }
        match observation.settlement {
            BounceSourceSettlement::Waiting => return Ok(None),
            BounceSourceSettlement::Invalid => {
                return Err("The exact stopped source settlement became invalid".into());
            }
            BounceSourceSettlement::Ready => {}
        }
        // No plan, command or capture lease exists before this joined boundary.
        self.stop_intent_current(app)?;
        self.ready_binding = Some(current);
        Ok(Some(StoppedCaptureReady {
            snapshot: Arc::clone(&self.snapshot),
            binding: current,
        }))
    }

    /// The caller supplies a complete admitted operation plan. Preparation and
    /// capture ownership stay separate; this issues the existing private worker
    /// capture command without exposing or substituting a source endpoint.
    pub(in crate::app) fn begin_capture(
        &mut self,
        app: &CitrusApp,
        plan: InstrumentBouncePlan,
    ) -> Result<(), String> {
        let binding = self.live_recheck(app)?;
        if self.capture.is_some() {
            return Err("The original private capture owner is still pending".into());
        }
        if self.ready_binding != Some(binding)
            || plan.project_fingerprint() != project_snapshot_fingerprint(&self.snapshot)
        {
            return Err("The capture plan differs from the actual settled reviewed source".into());
        }
        let identity = BounceSourceIdentity {
            project_session: binding.stamp.project_session,
            project_fingerprint: plan.project_fingerprint(),
            timeline_revision: binding.stamp.timeline_revision,
            epoch: binding.stamp.epoch,
            endpoint_id: binding.stamp.endpoint_id,
            channel_id: binding.stamp.channel_id,
            instance_id: binding.stamp.instance_id,
        };
        let running = app
            .running_generator_chains
            .get(&binding.stamp.channel_id)
            .ok_or("The source disappeared")?;
        self.capture = Some(InstrumentBounceCapture::begin(
            plan,
            identity,
            app.stopped_bounce_status(binding),
            &running.control,
            Instant::now(),
        )?);
        self.capture_identity = Some(identity);
        self.capture_abort_reason = None;
        self.stopping = false;
        Ok(())
    }

    /// Missing binding cancels and drains the original receipt before returning
    /// its typed failure. Cancellation retains this receipt until actual worker
    /// termination acknowledgment, including when the audio engine disappears.
    pub(in crate::app) fn poll_capture(
        &mut self,
        app: &CitrusApp,
    ) -> Result<Option<CapturedInstrumentBounce>, CapturePollError> {
        if self.capture.is_none() {
            return Err(CapturePollError::Invalid(
                "Capture ownership was lost".into(),
            ));
        }
        let binding = self.binding.unwrap_or(self.initial);
        let identity = BounceSourceIdentity {
            project_session: binding.stamp.project_session,
            project_fingerprint: project_snapshot_fingerprint(&self.snapshot),
            timeline_revision: binding.stamp.timeline_revision,
            epoch: binding.stamp.epoch,
            endpoint_id: binding.stamp.endpoint_id,
            channel_id: binding.stamp.channel_id,
            instance_id: binding.stamp.instance_id,
        };
        let status = if self.binding.is_some() {
            app.stopped_bounce_status(binding)
        } else {
            self.capture_abort_reason.get_or_insert_with(|| {
                CapturePollError::Invalid("The stopped source binding is missing".into())
            });
            self.cancel_capture();
            BounceSourceStatus::default()
        };
        // The fallback identity is used only to drain cancellation, never to
        // admit a capture or replace actual queue/confirmation provenance.
        let identity = if self.binding.is_some() {
            identity
        } else {
            self.capture_identity.unwrap_or(identity)
        };
        let result = self
            .capture
            .as_mut()
            .expect("retained capture checked above")
            .poll_typed(identity, status, Instant::now());
        if !matches!(result, Ok(None)) {
            self.capture = None;
            self.capture_identity = None;
            if let Some(reason) = self.capture_abort_reason.take()
                && !matches!(result, Err(CapturePollError::Backend(_)))
            {
                return Err(reason);
            }
        }
        result
    }
}

impl CitrusApp {
    pub(in crate::app) fn stopped_bounce_observe(
        &self,
        channel_id: u32,
    ) -> Result<LiveBinding, String> {
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
    pub(in crate::app) fn stopped_bounce_status(&self, binding: LiveBinding) -> BounceSourceStatus {
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
                .stopped_bounce_observe(binding.stamp.channel_id)
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
}

pub(in crate::app) fn project_snapshot_fingerprint(snapshot: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    snapshot.hash(&mut hasher);
    hasher.finish()
}
