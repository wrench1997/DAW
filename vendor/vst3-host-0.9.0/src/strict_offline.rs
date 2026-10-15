//! Disabled-by-default Linux/Windows client for one disposable offline generator.
//!
//! This closed development API has no App caller. One process-wide reservation
//! admits at most one strict child, including abandoned or pending-cleanup owners.
//! Every exchange uses its caller's original absolute deadline and is never
//! replayed. Deadlines are cooperative: allocation, spawn, scheduling and reap
//! are not universal hard-time guarantees. The host must keep SIGPIPE ignored;
//! this module checks that precondition and never installs a signal handler.
//!
//! Variable in-flight exchange payload is charged up to 256 MiB; caller storage,
//! previously returned replies, allocator overhead and fixed error envelopes are
//! outside that bound. The supported codec rejects nonempty output-event arrays.
//! Audio bits are preserved, including nonfinite values; renderer policy must
//! validate all returned samples before publishing anything.

use crate::audio::{BusDirection, MediaType};
use crate::fresh_helper::{self, FreshHelperSession, ReplyShape, RequestView, StateContextView};
use crate::midi::MidiEvent;
use crate::plugin::{
    CheckedAudioBusLayout, EventBusLayout, ProcessMode, ProcessTransport, ResetOriginSupport,
};
use crate::process_isolation::HostResponse;
use crate::Parameter;
use std::process::Command;
use std::time::Instant;

pub use crate::fresh_helper::{LimitEvidence, LimitSite};
use crate::helper_lifecycle::unsent;
pub use crate::helper_lifecycle::{
    CancelHandle, CleanupObservation, CleanupState, Delivery, Failure, FailureKind, LoadedPlugin,
    RequestContext,
};
pub use crate::process_isolation::{
    MetadataGetter, SelectedMetadataAdmission, SelectedMetadataPolicy, StrictMetadataSnapshot,
};

/// A pure admission outcome does not impersonate a failed SDK exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedMetadataOutcome {
    /// Four complete fresh getter results.
    Complete(StrictMetadataSnapshot),
    /// Original early admission failed; later getters were not called.
    Rejected(SelectedMetadataAdmission),
}

/// Still-owning spawn failure. Store the cleanup client before returning failure.
#[must_use = "a returned cleanup client retains the process-wide reservation"]
pub struct SpawnFailure {
    /// Original setup/admission failure, before any LoadPlugin request.
    pub failure: Failure,
    /// The actual child owner when spawn succeeded but setup failed.
    pub cleanup: Option<Box<Client>>,
}

/// Borrowed load data. The initial configured maximum is fixed at 2048 frames.
#[derive(Debug, Clone, Copy)]
pub struct LoadSpec<'a> {
    /// Exact resolved plugin bundle path.
    pub path: &'a str,
    /// Requested sample rate.
    pub sample_rate: f64,
    /// Requested transport tempo.
    pub tempo: f64,
    /// Requested meter numerator.
    pub time_sig_numerator: i32,
    /// Requested meter denominator.
    pub time_sig_denominator: i32,
    /// Exact current class identifier, never an automatic fallback.
    pub class_id: &'a str,
}

/// Exact Q128 output, available only after a successful strict exchange.
#[derive(Debug)]
pub struct AudioBlock {
    /// Exact expected mono/stereo channels, each containing 128 raw samples.
    pub outputs: Vec<Vec<f32>>,
    /// Supported responses have zero events; nonempty arrays fail the exchange.
    pub output_event_count: usize,
    /// Preserve explicit output loss evidence for renderer policy.
    pub output_events_lost: bool,
    /// Preserve whether this request's explicit transport was applied.
    pub transport_applied: bool,
}

/// Compare controller readback at deliberately captured f32 precision. This is
/// neither a clamp/tolerance nor proof that an SDK Process applied the parameter.
pub fn captured_base_matches(raw: f64, captured: f32) -> bool {
    raw.is_finite()
        && (0.0..=1.0).contains(&raw)
        && captured.is_finite()
        && (0.0..=1.0).contains(&captured)
        && raw as f32 == captured
}

/// Main/owner-thread strict session. No Clone, Send, Sync, raw owner or generic
/// command interface. Dropping transfers unresolved ownership to quarantine.
#[must_use = "complete checked disposal; Drop is not evidence of reap"]
pub struct Client {
    session: FreshHelperSession,
    // Once explicit selection starts, no default participation may substitute
    // for its completed owner ACKs. This bounded retained layout is not in-flight
    // codec payload, and is never returned as a substitute for a fresh query.
    audio_selection_required: bool,
    selected_audio_layout: Option<CheckedAudioBusLayout>,
}
impl Client {
    /// Spawn exactly once, retaining any post-spawn failure owner. Required stdin
    /// and stdout pipes and inherited stderr override the supplied Command stdio.
    /// Other executable/argument/environment policy belongs to the future caller.
    pub fn spawn(command: &mut Command, deadline: Instant) -> Result<Self, SpawnFailure> {
        FreshHelperSession::spawn(command, deadline)
            .map(|session| Self {
                session,
                audio_selection_required: false,
                selected_audio_layout: None,
            })
            .map_err(|failure| SpawnFailure {
                failure: unsent(failure.cause),
                cleanup: failure.cleanup.map(|session| {
                    Box::new(Self {
                        session: *session,
                        audio_selection_required: false,
                        selected_audio_layout: None,
                    })
                }),
            })
    }
    /// Obtain cancellation-only authority before the first load exchange.
    pub fn cancellation(&self) -> CancelHandle {
        CancelHandle(self.session.cancellation())
    }
    fn exchange(
        &mut self,
        request: RequestView<'_>,
        shape: ReplyShape,
        deadline: Instant,
    ) -> Result<HostResponse, Failure> {
        let context = RequestContext::from_request(request);
        self.session
            .exchange_view(request, shape, deadline)
            .map_err(|error| {
                let mut failure = Failure::from(error);
                failure.request_context = Some(context);
                failure
            })
    }
    fn success(&mut self, request: RequestView<'_>, deadline: Instant) -> Result<(), Failure> {
        match self.exchange(request, ReplyShape::Success, deadline)? {
            HostResponse::Success { .. } => Ok(()),
            _ => unreachable!("strict codec enforces expected success"),
        }
    }
    fn reject(&mut self, kind: fresh_helper::ErrorKind, deadline: Instant) -> Failure {
        self.session.reject_request(kind, deadline).into()
    }
    fn verify_selected_audio_layout(&mut self, deadline: Instant) -> Result<(), Failure> {
        if !self.audio_selection_required {
            return Ok(());
        }
        if self.selected_audio_layout.is_none() {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        let observed = self.checked_audio_layout(deadline)?;
        if self.selected_audio_layout.as_ref() != Some(&observed) {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        Ok(())
    }
    /// Load the chosen class with configured maximum2048; never retry or recover.
    pub fn load_plugin(
        &mut self,
        spec: LoadSpec<'_>,
        deadline: Instant,
    ) -> Result<LoadedPlugin, Failure> {
        self.selected_audio_layout = None;
        if spec.path.is_empty()
            || spec.class_id.len() != 32
            || !spec.class_id.bytes().all(|b| b.is_ascii_hexdigit())
            || !spec.sample_rate.is_finite()
            || spec.sample_rate <= 0.0
            || !spec.tempo.is_finite()
            || spec.tempo <= 0.0
            || spec.time_sig_numerator <= 0
            || !matches!(spec.time_sig_denominator, 1 | 2 | 4 | 8 | 16)
        {
            return Err(self
                .session
                .reject_request(fresh_helper::ErrorKind::InvalidRequest, deadline)
                .into());
        }
        let request = RequestView::LoadPlugin {
            path: spec.path,
            sample_rate: spec.sample_rate,
            block_size: 2048,
            tempo: spec.tempo,
            time_sig_numerator: spec.time_sig_numerator,
            time_sig_denominator: spec.time_sig_denominator,
            class_id: Some(spec.class_id),
        };
        Ok(LoadedPlugin::from_plugin_info(self.exchange(
            request,
            ReplyShape::PluginInfo,
            deadline,
        )?))
    }
    /// Set Offline mode on the helper owner before preparation/start.
    pub fn set_offline(&mut self, deadline: Instant) -> Result<(), Failure> {
        self.success(
            RequestView::SetProcessMode {
                mode: ProcessMode::Offline,
            },
            deadline,
        )
    }
    /// Query configured reset capability/status; this never processes reset audio.
    pub fn reset_origin_support(
        &mut self,
        deadline: Instant,
    ) -> Result<ResetOriginSupport, Failure> {
        match self.exchange(
            RequestView::ResetOriginSupport,
            ReplyShape::ResetOriginSupport,
            deadline,
        )? {
            HostResponse::ResetOriginSupport { support } => Ok(support),
            _ => unreachable!("strict reset support shape"),
        }
    }
    /// Borrow opaque project state synchronously without an eager payload copy.
    pub fn load_state(&mut self, data: &[u8], deadline: Instant) -> Result<(), Failure> {
        self.selected_audio_layout = None;
        self.success(
            RequestView::LoadState {
                data,
                context: StateContextView::Project,
            },
            deadline,
        )
    }
    /// Start prepared processing through the helper's existing owner path.
    pub fn start_processing(&mut self, deadline: Instant) -> Result<(), Failure> {
        self.verify_selected_audio_layout(deadline)?;
        self.success(RequestView::StartProcessing, deadline)?;
        self.verify_selected_audio_layout(deadline)
    }
    /// Stop a healthy clone. A sealed/cancelled session cannot send this request.
    pub fn stop_processing(&mut self, deadline: Instant) -> Result<(), Failure> {
        self.success(RequestView::StopProcessing, deadline)?;
        self.verify_selected_audio_layout(deadline)
    }
    /// Non-consuming observation through the same closed client and original budget.
    pub fn async_state_snapshot(
        &mut self,
        deadline: Instant,
    ) -> Result<crate::plugin::AsyncStateSnapshot, Failure> {
        match self.exchange(
            RequestView::AsyncStateSnapshot,
            ReplyShape::AsyncStateSnapshot,
            deadline,
        )? {
            HostResponse::AsyncStateSnapshot { snapshot } => Ok(snapshot),
            _ => unreachable!("strict async-state snapshot shape"),
        }
    }
    /// Clone-only explicit drain; never use on a realtime instance with another consumer.
    pub fn discard_progress_notifications(&mut self, deadline: Instant) -> Result<(), Failure> {
        self.success(RequestView::DiscardProgressNotifications, deadline)
    }
    /// Arm the actual DSP-boundary guard, not a UI-side cached readiness flag.
    pub fn arm_offline_state_readiness(
        &mut self,
        expected_revision: u64,
        deadline: Instant,
    ) -> Result<(), Failure> {
        self.success(
            RequestView::ArmOfflineStateReadiness { expected_revision },
            deadline,
        )
    }

    /// Read dirty revision evidence; this alone is not an enforced edit fence.
    pub fn native_dirty_revision(&mut self, deadline: Instant) -> Result<u64, Failure> {
        match self.exchange(
            RequestView::NativeDirtyRevision,
            ReplyShape::NativeDirtyRevision,
            deadline,
        )? {
            HostResponse::NativeDirtyRevision { revision } => Ok(revision),
            _ => unreachable!("strict revision shape"),
        }
    }
    /// Read a bounded catalog retaining every exposed Parameter field.
    pub fn parameters(&mut self, deadline: Instant) -> Result<Vec<Parameter>, Failure> {
        match self.exchange(
            RequestView::GetAllParameters,
            ReplyShape::Parameters,
            deadline,
        )? {
            HostResponse::Parameters { params } => Ok(params),
            _ => unreachable!("strict catalog shape"),
        }
    }
    /// Set one finite normalized captured f32 base through the existing owner path.
    pub fn set_parameter(&mut self, id: u32, value: f32, deadline: Instant) -> Result<(), Failure> {
        self.success(
            RequestView::SetParameter {
                id,
                value: f64::from(value),
            },
            deadline,
        )
    }
    /// Read the raw controller scalar; caller checks captured precision explicitly.
    pub fn parameter(&mut self, id: u32, deadline: Instant) -> Result<f64, Failure> {
        match self.exchange(
            RequestView::GetParameter { id },
            ReplyShape::ParameterValue,
            deadline,
        )? {
            HostResponse::ParameterValue { value } => Ok(value),
            _ => unreachable!("strict parameter shape"),
        }
    }
    /// Read actual/prepared audio layout. Raw main/aux and unknown activation remain.
    pub fn checked_audio_layout(
        &mut self,
        deadline: Instant,
    ) -> Result<CheckedAudioBusLayout, Failure> {
        match self.exchange(
            RequestView::CheckedAudioBusLayout,
            ReplyShape::CheckedAudioBusLayout,
            deadline,
        )? {
            HostResponse::CheckedAudioBusLayout { layout } => Ok(layout),
            _ => unreachable!("strict checked audio shape"),
        }
    }
    /// Read four fresh getters for an explicitly selected stereo clone.
    /// This client requires completed audio selection; factory identity is
    /// enforced by the StrictBackend dispatch and the helper's loaded owner.
    /// Generic backend dispatch retains its four independent exchanges.
    pub fn selected_metadata_snapshot(
        &mut self,
        deadline: Instant,
    ) -> Result<SelectedMetadataOutcome, Failure> {
        if !self.audio_selection_required || self.selected_audio_layout.is_none() {
            return Err(self.reject(fresh_helper::ErrorKind::InvalidRequest, deadline));
        }
        match self.exchange(
            RequestView::StrictMetadataSnapshot {
                policy: SelectedMetadataPolicy::Kontakt841SelectedStereo,
            },
            ReplyShape::StrictMetadataSnapshot,
            deadline,
        )? {
            HostResponse::StrictMetadataSnapshot { snapshot } => {
                Ok(SelectedMetadataOutcome::Complete(snapshot))
            }
            HostResponse::StrictMetadataAdmissionRejected { stage } => {
                Ok(SelectedMetadataOutcome::Rejected(stage))
            }
            _ => unreachable!("strict selected metadata shape"),
        }
    }
    /// Select output bus zero as the sole stereo output of a stopped clone.
    ///
    /// `expected` must be the complete fresh clone layout, retaining all bus
    /// indices. Inputs are limited to none or one main mono/stereo silent input.
    /// Every audio bus is explicitly acknowledged by the SDK owner: input zero
    /// participates when present; output zero participates and all other outputs
    /// are disabled. Event outputs are neither discarded nor disabled here.
    ///
    /// The existing owner binds each ACK to its complete SDK descriptor and
    /// prepared storage. Missing ACKs, topology drift, or any failed setter seal
    /// this disposable session; callers must dispose it and publish no audio.
    /// Selected start/stop and every Q128 process also re-query the layout before
    /// accepting a result. This API does not prove source/clone state equivalence.
    pub fn select_main_stereo_output(
        &mut self,
        expected: &CheckedAudioBusLayout,
        deadline: Instant,
    ) -> Result<CheckedAudioBusLayout, Failure> {
        self.audio_selection_required = true;
        self.selected_audio_layout = None;
        let supported_inputs = expected.inputs.is_empty()
            || (expected.inputs.len() == 1
                && expected.inputs[0].is_main
                && matches!(expected.inputs[0].channel_count, 1 | 2));
        let supported_outputs = !expected.outputs.is_empty()
            && expected.outputs.len() <= 32
            && expected.outputs[0].is_main
            && expected.outputs[0].channel_count == 2
            && expected.outputs.iter().all(|bus| bus.channel_count > 0)
            && expected
                .outputs
                .iter()
                .try_fold(0usize, |total, bus| total.checked_add(bus.channel_count))
                .is_some_and(|total| total <= 64);
        if !supported_inputs || !supported_outputs {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        if self.reset_origin_support(deadline)?.processing {
            return Err(self.reject(fresh_helper::ErrorKind::InvalidRequest, deadline));
        }
        if self.checked_audio_layout(deadline)? != *expected {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        for index in 0..expected.inputs.len() {
            self.success(
                RequestView::SetBusActive {
                    media_type: MediaType::Audio,
                    direction: BusDirection::Input,
                    bus_index: index as i32,
                    active: true,
                },
                deadline,
            )?;
        }
        for index in 0..expected.outputs.len() {
            self.success(
                RequestView::SetBusActive {
                    media_type: MediaType::Audio,
                    direction: BusDirection::Output,
                    bus_index: index as i32,
                    active: index == 0,
                },
                deadline,
            )?;
        }
        let selected = self.checked_audio_layout(deadline)?;
        let side_matches = |before: &[crate::plugin::CheckedAudioBusConfig],
                            after: &[crate::plugin::CheckedAudioBusConfig],
                            input: bool| {
            before.len() == after.len()
                && before
                    .iter()
                    .zip(after)
                    .enumerate()
                    .all(|(index, (old, new))| {
                        old.channel_count == new.channel_count
                            && old.is_main == new.is_main
                            && new.active == Some(input || index == 0)
                    })
        };
        if !side_matches(&expected.inputs, &selected.inputs, true)
            || !side_matches(&expected.outputs, &selected.outputs, false)
        {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        self.selected_audio_layout = Some(selected.clone());
        Ok(selected)
    }
    /// Read actual event buses on the helper owner thread.
    pub fn event_layout(&mut self, deadline: Instant) -> Result<EventBusLayout, Failure> {
        match self.exchange(
            RequestView::EventBusLayout,
            ReplyShape::EventBusLayout,
            deadline,
        )? {
            HostResponse::EventBusLayout { layout } => Ok(layout),
            _ => unreachable!("strict event layout shape"),
        }
    }
    /// Fresh owner-thread latency read, with no zero fallback.
    pub fn latency_samples(&mut self, deadline: Instant) -> Result<u32, Failure> {
        match self.exchange(
            RequestView::LatencySamples,
            ReplyShape::LatencySamples,
            deadline,
        )? {
            HostResponse::LatencySamples { samples } => Ok(samples),
            _ => unreachable!("strict latency shape"),
        }
    }
    /// Fresh owner-thread tail read, retaining legitimate u32::MAX.
    pub fn tail_samples(&mut self, deadline: Instant) -> Result<u32, Failure> {
        match self.exchange(RequestView::TailSamples, ReplyShape::TailSamples, deadline)? {
            HostResponse::TailSamples { samples } => Ok(samples),
            _ => unreachable!("strict tail shape"),
        }
    }
    /// Admit only ordinary NoteOn/NoteOff at offset0..127. Other MIDI is rejected.
    pub fn send_note(
        &mut self,
        event: MidiEvent,
        sample_offset: i32,
        deadline: Instant,
    ) -> Result<(), Failure> {
        self.success(
            RequestView::SendMidiAt {
                event,
                sample_offset,
            },
            deadline,
        )
    }
    /// Process one Q128 generator block without participating input channels.
    /// Finite/audio-loss acceptance remains a caller policy; no buffers are padded.
    pub fn process(
        &mut self,
        output_channels: usize,
        transport: ProcessTransport,
        deadline: Instant,
    ) -> Result<AudioBlock, Failure> {
        self.process_silent_generator(0, output_channels, transport, deadline)
    }
    /// Process one Q128 dry-generator block with explicit transport and exact shape.
    /// Accepts only input counts 0..=2 and output counts 1..=2. Each participating input
    /// is the same immutable positive-zero Q128 channel; caller audio cannot enter.
    /// Finite/audio-loss acceptance remains a caller policy; no buffers are padded.
    ///
    /// ```compile_fail
    /// use std::time::Instant;
    /// use vst3_host::{ProcessTransport, strict_offline::Client};
    /// fn cannot_supply_audio(client: &mut Client, samples: &[Vec<f32>],
    ///                       transport: ProcessTransport, deadline: Instant) {
    ///     client.process_silent_generator(samples, 2, transport, deadline);
    /// }
    /// ```
    pub fn process_silent_generator(
        &mut self,
        input_channels: usize,
        output_channels: usize,
        transport: ProcessTransport,
        deadline: Instant,
    ) -> Result<AudioBlock, Failure> {
        // Shape validation remains inside the budgeted session, sealing invalid
        // requests with NotSent and preserving deadline/cancellation precedence.
        if input_channels > 2 || !matches!(output_channels, 1 | 2) {
            return Err(self
                .session
                .reject_request(fresh_helper::ErrorKind::Shape, deadline)
                .into());
        }
        self.verify_selected_audio_layout(deadline)?;
        if let Some(layout) = &self.selected_audio_layout {
            let selected_inputs = layout.inputs.first().map_or(0, |bus| bus.channel_count);
            if input_channels != selected_inputs || output_channels != 2 {
                return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
            }
        }
        let reply = self.exchange(
            RequestView::silent_generator(input_channels, transport),
            ReplyShape::Audio {
                channels: output_channels,
                frames: 128,
            },
            deadline,
        )?;
        // Do not let a block escape if Process changed descriptors or activation.
        self.verify_selected_audio_layout(deadline)?;
        match reply {
            HostResponse::AudioOutput {
                outputs,
                output_events,
                output_events_lost,
                transport_applied,
            } => Ok(AudioBlock {
                outputs,
                output_event_count: output_events.len(),
                output_events_lost,
                transport_applied,
            }),
            _ => unreachable!("strict audio shape"),
        }
    }
    /// Seal admission, optionally send one healthy Shutdown before the earlier
    /// protocol/cleanup deadline, close pipes, then observe/kill/reap within the
    /// independent cleanup deadline. A successful healthy Shutdown write gets
    /// up to three seconds for natural exit, leaving one second of the original
    /// cleanup budget for kill/reap. This never extends either absolute deadline.
    /// Prior terminal/cancelled/failed Shutdown paths retain the 100 ms policy.
    pub fn dispose(
        &mut self,
        protocol_deadline: Instant,
        cleanup_deadline: Instant,
    ) -> CleanupObservation {
        self.session
            .dispose_with_deadlines_and_natural_grace(
                protocol_deadline,
                cleanup_deadline,
                std::time::Duration::from_secs(3),
                std::time::Duration::from_secs(1),
            )
            .into()
    }
    /// After disposal began, perform one nonwaiting observation/eligible kill.
    /// Never renew grace or send protocol; successful kill alone remains Pending.
    pub fn continue_cleanup(&mut self) -> Result<CleanupObservation, Failure> {
        self.session
            .continue_cleanup()
            .map(Into::into)
            .map_err(unsent)
    }
    /// Recover retained abandoned ownership under an absolute cleanup deadline.
    /// This never sends Shutdown, and blocks any new strict child until reap.
    pub fn cleanup_abandoned(deadline: Instant) -> CleanupObservation {
        FreshHelperSession::cleanup_abandoned(deadline).into()
    }
}
