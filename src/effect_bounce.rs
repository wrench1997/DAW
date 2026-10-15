//! Private stopped Insert-chain source receipts. These are not clone admission or rendering.
use std::{collections::BTreeMap, path::PathBuf};

use super::{BounceAudioBus, MAX_BOUNCE_STATE_BYTES};
use crate::plugins::plugin_runtime::{PluginParameterDescriptor, SlotConfig};

/// Actual SDK factory identity, independent of the Project's declared role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectSourceIdentity {
    pub class_uid: String,
    pub vendor: String,
    pub name: String,
    pub version: String,
    pub category: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectSourceMetadata {
    pub identity: EffectSourceIdentity,
    pub path: PathBuf,
    pub requested_class_uid: Option<String>,
    pub sample_rate: f64,
    pub max_block_frames: usize,
    pub tempo: f64,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
    pub input_buses: Vec<BounceAudioBus>,
    pub output_buses: Vec<BounceAudioBus>,
    pub event_input_buses: Vec<(u32, bool, Option<bool>)>,
    pub event_output_buses: Vec<(u32, bool, Option<bool>)>,
    pub latency_samples: u32,
    /// Actual DSP tail, including u32::MAX; the future renderer owns tail policy.
    pub tail_samples: u32,
    pub async_started_revision: u64,
}

impl EffectSourceMetadata {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let identity = &self.identity;
        if identity.class_uid.len() != 32
            || !identity.class_uid.bytes().all(|b| b.is_ascii_hexdigit())
            || !identity.class_uid.bytes().any(|b| b != b'0')
            || [
                &identity.vendor,
                &identity.name,
                &identity.version,
                &identity.category,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 64 * 1024)
            || !identity.category.split('|').any(|s| s == "Fx")
            || identity.category.split('|').any(|s| s == "Instrument")
            || self.path.as_os_str().is_empty()
            || self
                .requested_class_uid
                .as_ref()
                .is_some_and(|uid| !uid.eq_ignore_ascii_case(&identity.class_uid))
        {
            return Err(
                "Effect capture requires an unambiguous actual VST3 Fx factory identity".into(),
            );
        }
        let stereo = |buses: &[BounceAudioBus]| matches!(buses, [bus] if bus.channel_count == 2 && bus.is_main && bus.active == Some(true));
        if !stereo(&self.input_buses)
            || !stereo(&self.output_buses)
            || !self.event_input_buses.is_empty()
            || !self.event_output_buses.is_empty()
        {
            return Err("Effect capture supports exactly one known-active main stereo input/output and no event buses".into());
        }
        if !(8_000.0..=192_000.0).contains(&self.sample_rate)
            || self.sample_rate.fract() != 0.0
            || self.max_block_frames != super::BOUNCE_PREPARED_MAXIMUM
            || !self.tempo.is_finite()
            || self.tempo <= 0.0
            || self.time_sig_numerator <= 0
            || self.time_sig_denominator <= 0
        {
            return Err("Effect capture requires an exact supported rate, prepared maximum and stopped musical context".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectPluginState {
    pub metadata: EffectSourceMetadata,
    pub opaque_state: Vec<u8>,
    /// Complete bounded catalog and all writable bases, at the VST3 f64 readback precision.
    pub catalog_revision: u64,
    pub catalog: Vec<PluginParameterDescriptor>,
    pub parameter_bases: BTreeMap<u32, f64>,
    pub native_generation: u64,
    pub native_dirty_revision: u64,
}

impl EffectPluginState {
    pub(crate) fn validate(&self, required_ids: &[u32]) -> Result<(), String> {
        self.metadata.validate()?;
        if self.opaque_state.is_empty() || self.opaque_state.len() > MAX_BOUNCE_STATE_BYTES {
            return Err(
                "Effect capture requires nonempty authoritative state within 64 MiB".into(),
            );
        }
        let writable: Vec<_> = self
            .catalog
            .iter()
            .filter(|p| !p.read_only)
            .map(|p| p.id)
            .collect();
        if self.catalog_revision == 0
            || self.catalog.len()
                > crate::plugins::plugin_runtime::MAX_PLUGIN_PARAMETER_CATALOG_ITEMS
            || self.catalog.windows(2).any(|p| p[0].id >= p[1].id)
            || self.catalog.iter().any(|p| {
                !p.current_normalized.is_finite()
                    || !(0.0..=1.0).contains(&p.current_normalized)
                    || p.default_normalized
                        .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
                    || p.name.len()
                        > crate::plugins::plugin_runtime::MAX_PLUGIN_PARAMETER_NAME_BYTES
                    || p.unit.len()
                        > crate::plugins::plugin_runtime::MAX_PLUGIN_PARAMETER_UNIT_BYTES
            })
            || !self.parameter_bases.keys().copied().eq(writable)
            || self.catalog.iter().filter(|p| !p.read_only).any(|p| {
                self.parameter_bases
                    .get(&p.id)
                    .is_none_or(|v| *v as f32 != p.current_normalized)
            })
            || required_ids.windows(2).any(|p| p[0] >= p[1])
            || required_ids
                .iter()
                .any(|id| !self.parameter_bases.contains_key(id))
            || self
                .parameter_bases
                .values()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err("Effect capture requires the complete unique writable catalog and exact finite parameter bases".into());
        }
        Ok(())
    }
}

/// Ordered physical worker slots. Project slot numbers are retained by the App binding.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EffectCaptureSlotRequest {
    pub instance_id: u64,
    pub config: SlotConfig,
    pub path: PathBuf,
    pub requested_class_uid: Option<String>,
    pub parameter_ids: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CapturedEffectSlot {
    pub request: EffectCaptureSlotRequest,
    pub state: EffectPluginState,
}

#[derive(Debug)]
pub(crate) struct EffectChainStateReceipt {
    pub epoch: u64,
    pub endpoint: crate::plugins::plugin_runtime::PluginEndpointSnapshot,
    pub slots: Vec<CapturedEffectSlot>,
    pub(crate) submitted: u64,
    pub(crate) admin_revision: u64,
    pub(crate) epoch_resets: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EffectCapturePollError {
    Cancelled,
    Deadline,
    Stale(String),
    Invalid(String),
    Backend(super::BounceCaptureError),
    Disconnected,
}
impl std::fmt::Display for EffectCapturePollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Effect source capture was cancelled"),
            Self::Deadline => f.write_str("Effect source capture timed out"),
            Self::Stale(s) | Self::Invalid(s) => f.write_str(s),
            Self::Backend(e) => e.fmt(f),
            Self::Disconnected => f.write_str("Effect source worker disconnected"),
        }
    }
}
impl std::error::Error for EffectCapturePollError {}
