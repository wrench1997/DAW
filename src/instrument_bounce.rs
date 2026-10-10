//! Dormant, bounded preparation for a selected-instrument bounce.
//!
//! There is deliberately no App action, renderer or file publication in this checkpoint.
//! A capture is delivered on a private one-shot channel, never through project-save/native
//! publications. The caller must keep session actions fenced while this coordinator is pending.

#[path = "instrument_bounce_render.rs"]
#[allow(dead_code)] // Dormant engine; experimental strict clone has no App/UI caller.
pub(crate) mod render;

use std::{
    collections::BTreeMap,
    hash::{DefaultHasher, Hash, Hasher},
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};

use crate::{
    model::{PluginFormat, Project},
    plugins::plugin_runtime::{BounceStateReceiver, PluginChainControl},
    tempo_map::TempoMap,
    timeline::{CompiledTimeline, TimelineCompileOptions},
};

#[cfg(any(feature = "vst3", test))]
use crate::plugins::plugin_runtime::PluginLoadSpec;

pub const BOUNCE_PREPARED_MAXIMUM: usize = 2048;
pub const BOUNCE_QUANTUM: usize = 128;
pub const MAX_BOUNCE_SECONDS: u64 = 600;
pub const MAX_BOUNCE_TAIL_SECONDS: u32 = 30;
pub const MAX_BOUNCE_LATENCY_SECONDS: u32 = 1;
pub const MAX_BOUNCE_FRAMES: u64 = 24_000_000;
pub const MAX_BOUNCE_STATE_BYTES: usize = 64 * 1024 * 1024;
pub const BOUNCE_CAPTURE_TIMEOUT: Duration = Duration::from_secs(35);

/// Exact App-owned source stamp. None of these identities is derived from display names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BounceSourceIdentity {
    pub project_session: u64,
    pub project_fingerprint: u64,
    pub timeline_revision: u64,
    pub epoch: u64,
    pub endpoint_id: u64,
    pub channel_id: u32,
    pub instance_id: u64,
}

impl BounceSourceIdentity {
    fn validate(self) -> Result<(), String> {
        if self.project_session == 0
            || self.timeline_revision == 0
            || self.epoch == 0
            || self.endpoint_id == 0
            || self.channel_id == 0
            || self.instance_id == 0
        {
            return Err("Instrument bounce requires a complete, confirmed source identity".into());
        }
        Ok(())
    }
}

/// Observed App state, checked both at admission and when consuming the exact receipt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BounceSourceStatus {
    pub confirmed_stopped: bool,
    pub exact_timeline: bool,
    pub source_ready: bool,
    pub parameter_edits_pending: bool,
    pub native_editor_open_or_pending: bool,
    pub session_transition: bool,
    pub live_midi_or_recording: bool,
}

impl BounceSourceStatus {
    fn validate(self) -> Result<(), String> {
        if !self.confirmed_stopped || !self.exact_timeline || !self.source_ready {
            return Err(
                "Instrument bounce requires a ready source at an exact stopped Timeline epoch"
                    .into(),
            );
        }
        if self.parameter_edits_pending
            || self.native_editor_open_or_pending
            || self.session_transition
            || self.live_midi_or_recording
        {
            return Err("Close the native editor and finish parameter, MIDI, recording and session transitions before instrument bounce".into());
        }
        Ok(())
    }
}

/// One declared audio bus, retained in component order even when inactive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BounceAudioBus {
    pub channel_count: usize,
    pub is_main: bool,
    pub active: Option<bool>,
}

impl BounceAudioBus {
    pub(crate) const fn new(channel_count: usize, is_main: bool, active: Option<bool>) -> Self {
        Self {
            channel_count,
            is_main,
            active,
        }
    }
}

/// The single bounded dry-generator policy used for capture and clone admission.
/// Unknown activation is never projected to false or removed from the declaration.
pub(crate) fn validate_bounce_audio_topology(
    inputs: &[BounceAudioBus],
    outputs: &[BounceAudioBus],
) -> Result<(), String> {
    if inputs.iter().chain(outputs).any(|bus| bus.active.is_none()) {
        return Err("Instrument bounce activation is unknown; a fresh load with explicit inactive-bus acknowledgment is required".into());
    }
    if inputs.len() > 1
        || inputs
            .iter()
            .any(|bus| !bus.is_main || !matches!(bus.channel_count, 1 | 2))
        || !(1..=3).contains(&outputs.len())
        || !outputs.first().is_some_and(|bus| {
            bus.is_main && bus.active == Some(true) && matches!(bus.channel_count, 1 | 2)
        })
        || outputs.iter().skip(1).any(|bus| {
            bus.is_main || bus.active != Some(false) || !matches!(bus.channel_count, 1 | 2)
        })
    {
        return Err("Instrument bounce requires at most one known main mono/stereo input, one known-active main mono/stereo output, and at most two known-inactive auxiliary mono/stereo outputs".into());
    }
    Ok(())
}

/// Actual loaded metadata. Project/cache role and path-based IDs cannot construct this receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BouncePluginMetadata {
    pub class_uid: String,
    pub version: String,
    pub name: String,
    pub category: String,
    /// Current component event buses: MIDI channel count, main/aux, host-attested activation.
    pub event_input_buses: Vec<(u32, bool, Option<bool>)>,
    pub event_output_buses: Vec<(u32, bool, Option<bool>)>,
    /// Preserve every declared bus and its exact role and activation evidence.
    pub input_buses: Vec<BounceAudioBus>,
    pub output_buses: Vec<BounceAudioBus>,
    pub latency_samples: u32,
    pub tail_samples: u32,
}

impl BouncePluginMetadata {
    /// Flat input storage contains only participating channels; no external audio is read.
    pub(crate) fn participating_input_channels(&self) -> usize {
        self.input_buses
            .iter()
            .filter(|bus| bus.active == Some(true))
            .map(|bus| bus.channel_count)
            .sum()
    }

    pub(crate) fn validate(&self, sample_rate: u32) -> Result<(), String> {
        if self.class_uid.len() != 32 || !self.class_uid.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("Instrument bounce requires an actual VST3 factory class UID".into());
        }
        if !self
            .category
            .split('|')
            .any(|token| token.trim() == "Instrument")
            || self.event_input_buses.len() != 1
            || !matches!(
                self.event_input_buses.first(),
                Some((1..=16, true, Some(true)))
            )
            || !self.event_output_buses.is_empty()
        {
            return Err("Instrument bounce requires an actual VST3 instrument with MIDI input and no MIDI output".into());
        }
        validate_bounce_audio_topology(&self.input_buses, &self.output_buses)?;
        if self.latency_samples > sample_rate.saturating_mul(MAX_BOUNCE_LATENCY_SECONDS) {
            return Err("Instrument bounce plug-in latency exceeds the one-second bound".into());
        }
        Ok(())
    }
}

/// A capture rejection is nonmutating; an actual backend/IPC failure must remain visible
/// to the source worker's normal fault handling rather than being relabeled as cancellation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BounceCaptureError {
    Rejected(String),
    BackendFailed(String),
}

impl std::fmt::Display for BounceCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(message) | Self::BackendFailed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for BounceCaptureError {}

/// Returned only from the dedicated worker capture, never written back into Project.
#[derive(Clone, Debug, PartialEq)]
pub struct BouncePluginState {
    pub(crate) metadata: BouncePluginMetadata,
    pub(crate) opaque_state: Vec<u8>,
    pub(crate) parameter_bases: BTreeMap<u32, f32>,
    pub(crate) native_generation: u64,
    pub(crate) native_dirty_revision: u64,
}

impl BouncePluginState {
    pub fn metadata(&self) -> &BouncePluginMetadata {
        &self.metadata
    }

    pub(crate) fn validate(&self, expected_ids: &[u32], sample_rate: u32) -> Result<(), String> {
        self.metadata.validate(sample_rate)?;
        if self.opaque_state.is_empty() || self.opaque_state.len() > MAX_BOUNCE_STATE_BYTES {
            return Err("Instrument bounce requires nonempty authoritative state within the 64 MiB limit; default-state fallback is forbidden".into());
        }
        if self.parameter_bases.len() != expected_ids.len()
            || !self
                .parameter_bases
                .keys()
                .copied()
                .eq(expected_ids.iter().copied())
            || self
                .parameter_bases
                .values()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(
                "Instrument bounce did not capture exactly the current generic parameter bases"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct BounceStateReceipt {
    pub(crate) epoch: u64,
    pub(crate) state: BouncePluginState,
}

/// Prepared solely from an immutable project view; compilation retains original channel order.
pub struct InstrumentBouncePlan {
    project_fingerprint: u64,
    channel_id: u32,
    instance_id: u64,
    sample_rate: u32,
    tail_frames: u32,
    timeline: CompiledTimeline,
    #[cfg(any(feature = "vst3", test))]
    spec: PluginLoadSpec,
    parameter_ids: Vec<u32>,
}

impl InstrumentBouncePlan {
    pub fn prepare(
        project: &Project,
        channel_id: u32,
        sample_rate: u32,
        tail_frames: u32,
    ) -> Result<Self, String> {
        if !(8_000..=192_000).contains(&sample_rate)
            || tail_frames > sample_rate.saturating_mul(MAX_BOUNCE_TAIL_SECONDS)
        {
            return Err("Instrument bounce requires 8000–192000 Hz and an explicit tail of at most 30 seconds".into());
        }
        project
            .validate_mixer_graph()
            .map_err(|error| error.to_string())?;
        if let Some(lane) = project
            .automation_lanes
            .iter()
            .find(|lane| lane.lane.is_enabled() && !lane.lane.points().is_empty())
        {
            return Err(format!(
                "Instrument bounce does not support active automation, including Tempo (lane {}: {})",
                lane.id, lane.name
            ));
        }
        if project.plugin_instances.iter().any(|instance| {
            instance.midi_ports.input.is_some() || instance.midi_ports.output.is_some()
        }) {
            return Err("Instrument bounce does not support configured MIDI-port routing".into());
        }
        let mut channels = project
            .channels
            .iter()
            .filter(|channel| channel.id == channel_id);
        let channel = channels
            .next()
            .ok_or("Instrument bounce channel is missing")?;
        if channels.next().is_some() || channel.muted {
            return Err("Instrument bounce requires one unmuted channel identity".into());
        }
        let instance_id = channel
            .instrument_plugin_instance_id
            .ok_or("The selected channel has no VST3 instrument")?;
        let instance = project
            .plugin_instances
            .iter()
            .find(|instance| instance.id == instance_id)
            .ok_or("Instrument bounce plug-in is missing")?;
        if instance.format != PluginFormat::Vst3
            || !instance.enabled
            || instance.bypass
            || instance.wet != 1.0
            || instance.midi_ports.audio_monitor_muted
        {
            return Err("Instrument bounce requires an enabled, fully wet, unbypassed VST3 generator with audio monitoring enabled".into());
        }
        if instance.parameters.len()
            > crate::plugins::plugin_runtime::MAX_PLUGIN_PARAMETER_CATALOG_ITEMS
        {
            return Err("Too many stored generic parameter bases for instrument bounce".into());
        }
        let tempo_map =
            TempoMap::from_project(project, sample_rate).map_err(|error| error.to_string())?;
        let timeline =
            CompiledTimeline::from_project(project, &tempo_map, TimelineCompileOptions::default())
                .map_err(|error| error.to_string())?;
        if !timeline.diagnostics().is_empty() || timeline.stats().invalid_items_skipped != 0 {
            return Err(format!(
                "Instrument bounce refuses Timeline diagnostics rather than omitting content: {:?}",
                timeline.diagnostics().first()
            ));
        }
        let total = timeline
            .duration_frames()
            .checked_add(u64::from(tail_frames))
            .and_then(|n| n.checked_add(u64::from(sample_rate)))
            .ok_or("Instrument bounce length overflow")?;
        if timeline.duration_frames() == 0
            || timeline.duration_frames() > u64::from(sample_rate) * MAX_BOUNCE_SECONDS
            || total > MAX_BOUNCE_FRAMES
        {
            return Err("Instrument bounce exceeds its ten-minute / 24-million-frame bound or has zero length".into());
        }
        #[cfg(any(feature = "vst3", test))]
        let spec = crate::plugins::plugin_load_spec_from_instance(instance);
        let mut hasher = DefaultHasher::new();
        serde_json::to_vec(project)
            .map_err(|error| error.to_string())?
            .hash(&mut hasher);
        Ok(Self {
            project_fingerprint: hasher.finish(),
            channel_id,
            instance_id,
            sample_rate,
            tail_frames,
            timeline,
            #[cfg(any(feature = "vst3", test))]
            spec,
            parameter_ids: instance.parameters.keys().copied().collect(),
        })
    }

    pub fn channel_id(&self) -> u32 {
        self.channel_id
    }
    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn tail_frames(&self) -> u32 {
        self.tail_frames
    }
    pub fn timeline(&self) -> &CompiledTimeline {
        &self.timeline
    }
    pub fn project_fingerprint(&self) -> u64 {
        self.project_fingerprint
    }
}

/// Owns one bounded receipt and an immutable preparation. Dropping/cancelling this value
/// abandons the reply without publishing captured bytes or mutating the source.
pub struct InstrumentBounceCapture {
    identity: BounceSourceIdentity,
    plan: Option<InstrumentBouncePlan>,
    receiver: Option<BounceStateReceiver>,
    started_at: Instant,
    terminal: bool,
    abort_reason: Option<String>,
}

pub struct CapturedInstrumentBounce {
    identity: BounceSourceIdentity,
    pub(crate) plan: InstrumentBouncePlan,
    pub(crate) state: BouncePluginState,
}

impl CapturedInstrumentBounce {
    pub fn identity(&self) -> BounceSourceIdentity {
        self.identity
    }
    pub fn plan(&self) -> &InstrumentBouncePlan {
        &self.plan
    }
    pub fn metadata(&self) -> &BouncePluginMetadata {
        self.state.metadata()
    }
    pub fn prepare_offline(
        &self,
    ) -> Result<crate::plugins::plugin_runtime::OfflineInstrument, String> {
        crate::plugins::plugin_runtime::OfflineInstrument::from_capture(self)
    }
    /// Borrow only captured load identity and the original requested base-ID set.
    /// The strict clone borrows authoritative state separately; no source endpoint
    /// or mutable capture authority escapes and no opaque state is copied here.
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub(crate) fn strict_load_inputs(&self) -> (&PluginLoadSpec, &[u32]) {
        (&self.plan.spec, &self.plan.parameter_ids)
    }

    #[cfg(any(feature = "vst3", test))]
    pub(crate) fn render_spec(&self) -> PluginLoadSpec {
        let mut spec = self.plan.spec.clone();
        spec.initial_state.clone_from(&self.state.opaque_state);
        // Pin this new clone to the factory class that actually supplied the captured state.
        spec.class_uid = Some(self.state.metadata.class_uid.clone());
        spec
    }
}

impl InstrumentBounceCapture {
    pub fn begin(
        plan: InstrumentBouncePlan,
        identity: BounceSourceIdentity,
        status: BounceSourceStatus,
        control: &PluginChainControl,
        now: Instant,
    ) -> Result<Self, String> {
        identity.validate()?;
        status.validate()?;
        if identity.channel_id != plan.channel_id
            || identity.instance_id != plan.instance_id
            || identity.project_fingerprint != plan.project_fingerprint
        {
            return Err("Instrument bounce source differs from the prepared selection".into());
        }
        let receiver = control.request_instrument_bounce_state(
            identity.instance_id,
            identity.epoch,
            plan.parameter_ids.clone(),
        )?;
        Ok(Self {
            identity,
            plan: Some(plan),
            receiver: Some(receiver),
            started_at: now,
            terminal: false,
            abort_reason: None,
        })
    }

    /// Request cancellation, retaining preparation and source ownership until worker ack.
    /// A future App adapter must retain this coordinator/session fence while poll is pending.
    pub fn cancel(&mut self) {
        self.request_abort("Instrument bounce capture was cancelled".into());
    }

    pub fn termination_acknowledged(&self) -> bool {
        self.receiver
            .as_ref()
            .map_or(self.terminal, BounceStateReceiver::termination_acknowledged)
    }

    pub fn is_cancelling(&self) -> bool {
        self.abort_reason.is_some() && !self.terminal
    }

    fn request_abort(&mut self, reason: String) {
        if !self.terminal && self.abort_reason.is_none() {
            self.abort_reason = Some(reason);
        }
        if let Some(receiver) = &self.receiver {
            receiver.cancel();
        }
    }

    pub fn poll(
        &mut self,
        identity: BounceSourceIdentity,
        status: BounceSourceStatus,
        now: Instant,
    ) -> Result<Option<CapturedInstrumentBounce>, String> {
        if self.terminal {
            return Err("Instrument bounce capture is no longer pending".into());
        }
        let result = self.poll_pending(identity, status, now);
        if !matches!(result, Ok(None)) {
            self.terminal = true;
            self.plan = None;
            self.receiver = None;
        }
        result
    }

    fn poll_pending(
        &mut self,
        identity: BounceSourceIdentity,
        status: BounceSourceStatus,
        now: Instant,
    ) -> Result<Option<CapturedInstrumentBounce>, String> {
        if identity != self.identity {
            self.request_abort("Instrument bounce source changed during capture".into());
        }
        if let Err(reason) = status.validate() {
            self.request_abort(reason);
        }
        if now.saturating_duration_since(self.started_at) >= BOUNCE_CAPTURE_TIMEOUT {
            self.request_abort("Instrument bounce state capture timed out".into());
        }
        let receipt = match self
            .receiver
            .as_ref()
            .ok_or("Instrument bounce capture is unavailable")?
            .try_recv()
        {
            Ok(Err(BounceCaptureError::BackendFailed(message))) => {
                // The worker already published its real fault; cancellation cannot replace it.
                return Err(message);
            }
            Ok(result) => {
                if let Some(reason) = &self.abort_reason {
                    return Err(reason.clone());
                }
                result.map_err(|error| error.to_string())?
            }
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                return Err("Instrument bounce source worker disconnected".into());
            }
        };
        if receipt.epoch != identity.epoch {
            return Err("Instrument bounce received a stale epoch receipt".into());
        }
        let plan = self
            .plan
            .as_ref()
            .ok_or("Instrument bounce preparation was cancelled")?;
        receipt
            .state
            .validate(&plan.parameter_ids, plan.sample_rate)?;
        Ok(Some(CapturedInstrumentBounce {
            identity: self.identity,
            plan: self.plan.take().expect("checked above"),
            state: receipt.state,
        }))
    }
}

#[cfg(all(test, target_os = "linux", feature = "experimental-fresh-helper"))]
#[path = "instrument_bounce_strict_backend_tests.rs"]
mod strict_backend_tests;
