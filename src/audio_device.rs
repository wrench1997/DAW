//! Stable audio-device identities, deterministic stream negotiation, and
//! allocation-free callback telemetry.
//!
//! Device enumeration and stream construction are control-thread operations.
//! The callback-facing type in this module contains atomics only.

use std::{
    fmt,
    str::FromStr,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};

use anyhow::{Result, anyhow};
use cpal::{
    BufferSize, Device, DeviceId, ErrorKind, Host, SampleFormat, StreamConfig, SupportedBufferSize,
    SupportedStreamConfig, SupportedStreamConfigRange,
    traits::{DeviceTrait, HostTrait},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const AUDIO_PREFERENCES_VERSION: u32 = 1;
pub const MAX_AUDIO_DEVICE_CAPABILITIES: usize = 512;
pub const MAX_AUDIO_DEVICE_CATALOG_ENTRIES: usize = 512;
pub const MAX_AUDIO_DEVICE_CATALOG_DIAGNOSTICS: usize = 128;
pub const MAX_AUDIO_DEVICES_PER_HOST: usize = 256;
pub const MAX_AUDIO_DEVICE_HOST_ID_BYTES: usize = 128;
pub const MAX_AUDIO_DEVICE_ID_BYTES: usize = 4_096;
pub const MAX_AUDIO_DEVICE_NAME_BYTES: usize = 512;
pub const MAX_AUDIO_DEVICE_DIAGNOSTIC_BYTES: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AudioDeviceDirection {
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioDeviceIdentity {
    pub host_id: String,
    /// Complete CPAL serialization (`host:backend-specific-id`).
    pub device_id: String,
}

impl AudioDeviceIdentity {
    pub fn from_cpal(id: &DeviceId) -> Self {
        Self {
            host_id: id.host().to_string(),
            device_id: id.to_string(),
        }
    }

    pub fn try_from_cpal(id: &DeviceId) -> Result<Self, AudioDeviceIdentityError> {
        let identity = Self::from_cpal(id);
        identity.parse()?;
        Ok(identity)
    }

    pub fn parse(&self) -> Result<DeviceId, AudioDeviceIdentityError> {
        if self.host_id.trim().is_empty() || self.device_id.trim().is_empty() {
            return Err(AudioDeviceIdentityError::Empty);
        }
        if self.host_id.len() > MAX_AUDIO_DEVICE_HOST_ID_BYTES
            || self.device_id.len() > MAX_AUDIO_DEVICE_ID_BYTES
        {
            return Err(AudioDeviceIdentityError::TooLong);
        }
        let id = DeviceId::from_str(&self.device_id)
            .map_err(|_| AudioDeviceIdentityError::InvalidDeviceId)?;
        let canonical_host = id.host().to_string();
        if !canonical_host.eq_ignore_ascii_case(&self.host_id) {
            return Err(AudioDeviceIdentityError::HostMismatch);
        }
        if canonical_host != self.host_id || id.to_string() != self.device_id {
            return Err(AudioDeviceIdentityError::NonCanonical);
        }
        Ok(id)
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum AudioDeviceIdentityError {
    #[error("audio device identity is empty")]
    Empty,
    #[error("audio device ID is invalid or belongs to an unavailable host")]
    InvalidDeviceId,
    #[error("audio device host and serialized ID do not match")]
    HostMismatch,
    #[error("audio device identity is not in CPAL's canonical serialized form")]
    NonCanonical,
    #[error("audio device identity exceeds the persisted size limit")]
    TooLong,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioDeviceSelection {
    #[default]
    SystemDefault,
    Stable(AudioDeviceIdentity),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioSampleRateRequest {
    #[default]
    DeviceDefault,
    Exact(u32),
    /// May select the closest rate inside a supported range.
    Nearest(u32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioChannelRequest {
    #[default]
    DeviceDefault,
    Exact(u16),
    /// May select the closest supported channel count.
    Nearest(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AudioSampleFormat {
    I8,
    F32,
    F64,
    I16,
    I24,
    I32,
    I64,
    U8,
    U16,
    U24,
    U32,
    U64,
}

impl AudioSampleFormat {
    pub const fn renderer_rank(self) -> u8 {
        match self {
            Self::F32 => 0,
            Self::I16 => 1,
            Self::U16 => 2,
            Self::F64 => 3,
            Self::I24 => 4,
            Self::I32 => 5,
            Self::I8 => 6,
            Self::I64 => 7,
            Self::U24 => 8,
            Self::U32 => 9,
            Self::U8 => 10,
            Self::U64 => 11,
        }
    }

    pub fn from_cpal(format: SampleFormat) -> Option<Self> {
        match format {
            SampleFormat::I8 => Some(Self::I8),
            SampleFormat::F32 => Some(Self::F32),
            SampleFormat::F64 => Some(Self::F64),
            SampleFormat::I16 => Some(Self::I16),
            SampleFormat::I24 => Some(Self::I24),
            SampleFormat::I32 => Some(Self::I32),
            SampleFormat::I64 => Some(Self::I64),
            SampleFormat::U8 => Some(Self::U8),
            SampleFormat::U16 => Some(Self::U16),
            SampleFormat::U24 => Some(Self::U24),
            SampleFormat::U32 => Some(Self::U32),
            SampleFormat::U64 => Some(Self::U64),
            _ => None,
        }
    }

    pub const fn supports_direction(self, direction: AudioDeviceDirection) -> bool {
        match direction {
            AudioDeviceDirection::Output => {
                matches!(self, Self::F32 | Self::I16 | Self::U16)
            }
            AudioDeviceDirection::Input => true,
        }
    }

    pub const fn to_cpal(self) -> SampleFormat {
        match self {
            Self::I8 => SampleFormat::I8,
            Self::F32 => SampleFormat::F32,
            Self::F64 => SampleFormat::F64,
            Self::I16 => SampleFormat::I16,
            Self::I24 => SampleFormat::I24,
            Self::I32 => SampleFormat::I32,
            Self::I64 => SampleFormat::I64,
            Self::U8 => SampleFormat::U8,
            Self::U16 => SampleFormat::U16,
            Self::U24 => SampleFormat::U24,
            Self::U32 => SampleFormat::U32,
            Self::U64 => SampleFormat::U64,
        }
    }
}

impl fmt::Display for AudioSampleFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::I8 => "i8",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::I16 => "i16",
            Self::I24 => "i24",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U24 => "u24",
            Self::U32 => "u32",
            Self::U64 => "u64",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioSampleFormatRequest {
    #[default]
    DeviceDefault,
    Exact(AudioSampleFormat),
    /// Selects the renderer's deterministic preference order (f32, i16, u16).
    Automatic,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioBufferSizeRequest {
    #[default]
    BackendDefault,
    /// Must be supported exactly; it is never silently clamped.
    Fixed(u32),
    /// May select the closest size when the backend publishes a range.
    Nearest(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioDeviceProfile {
    pub direction: AudioDeviceDirection,
    pub selection: AudioDeviceSelection,
    pub sample_rate: AudioSampleRateRequest,
    pub channels: AudioChannelRequest,
    pub sample_format: AudioSampleFormatRequest,
    pub buffer_size: AudioBufferSizeRequest,
}

impl AudioDeviceProfile {
    pub fn system_default_output() -> Self {
        Self {
            direction: AudioDeviceDirection::Output,
            selection: AudioDeviceSelection::SystemDefault,
            sample_rate: AudioSampleRateRequest::DeviceDefault,
            channels: AudioChannelRequest::DeviceDefault,
            sample_format: AudioSampleFormatRequest::DeviceDefault,
            buffer_size: AudioBufferSizeRequest::BackendDefault,
        }
    }

    pub fn system_default_input() -> Self {
        Self {
            direction: AudioDeviceDirection::Input,
            ..Self::system_default_output()
        }
    }
}

impl Default for AudioDeviceProfile {
    fn default() -> Self {
        Self::system_default_output()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AudioBufferCapability {
    Unknown,
    Range { min: u32, max: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AudioStreamCapability {
    pub channels: u16,
    pub sample_format: AudioSampleFormat,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    pub buffer_size: AudioBufferCapability,
}

impl AudioStreamCapability {
    const fn is_valid(self) -> bool {
        self.channels != 0
            && self.min_sample_rate != 0
            && self.min_sample_rate <= self.max_sample_rate
            && match self.buffer_size {
                AudioBufferCapability::Unknown => true,
                AudioBufferCapability::Range { min, max } => min != 0 && min <= max,
            }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioEffectiveBufferSize {
    BackendDefault,
    Fixed(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AudioEffectiveStreamConfig {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: AudioSampleFormat,
    pub buffer_size: AudioEffectiveBufferSize,
}

/// The backend's preferred dimensions, retained even when its preferred
/// sample format cannot be rendered in the requested direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioDefaultStreamHint {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: Option<AudioSampleFormat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioNegotiationFallback {
    DefaultConfigurationUnavailable,
    DefaultFormatUnavailable,
    DefaultChannelsUnavailable,
    DefaultSampleRateUnavailable,
    AutomaticFormat,
    NearestChannels,
    NearestSampleRate,
    NearestBufferSize,
    BackendDefaultBufferBecauseRangeUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioNegotiatedStreamConfig {
    pub requested: AudioDeviceProfile,
    pub resolved_device: AudioDeviceIdentity,
    pub effective: AudioEffectiveStreamConfig,
    pub fallback_reasons: Vec<AudioNegotiationFallback>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDeviceCatalogEntry {
    pub identity: AudioDeviceIdentity,
    pub name: String,
    pub direction: AudioDeviceDirection,
    pub is_system_default: bool,
    pub capabilities: Vec<AudioStreamCapability>,
    pub default_hint: Option<AudioDefaultStreamHint>,
    pub default_config: Option<AudioEffectiveStreamConfig>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioDeviceCatalog {
    pub entries: Vec<AudioDeviceCatalogEntry>,
    /// Partial host/device failures. A successful entry is never discarded
    /// because another device returned an error.
    pub diagnostics: Vec<String>,
}

pub struct ResolvedAudioDevice {
    pub device: Device,
    pub name: String,
    pub negotiated: AudioNegotiatedStreamConfig,
}

#[derive(Debug, Error)]
pub enum AudioDeviceResolveError {
    #[error(transparent)]
    InvalidIdentity(#[from] AudioDeviceIdentityError),
    #[error("unable to open audio host {host_id} ({kind:?}): {message}")]
    HostOpen {
        host_id: String,
        kind: AudioStreamFaultKind,
        message: String,
    },
    #[error("no default {direction:?} audio device is available")]
    DefaultUnavailable { direction: AudioDeviceDirection },
    #[error("unable to enumerate audio host {host_id} ({kind:?}): {message}")]
    DeviceEnumeration {
        host_id: String,
        kind: AudioStreamFaultKind,
        message: String,
    },
    #[error("requested {direction:?} audio device is unavailable: {device_id}")]
    StableDeviceNotFound {
        direction: AudioDeviceDirection,
        device_id: String,
    },
    #[error(
        "stable device lookup was incomplete because a device identity failed ({kind:?}): {message}"
    )]
    StableLookupIncomplete {
        kind: AudioStreamFaultKind,
        message: String,
    },
    #[error(
        "stable device lookup exceeded the bounded per-host device limit before the requested ID was found"
    )]
    StableLookupTruncated,
    #[error("selected audio device no longer exposes a stable identity ({kind:?}): {message}")]
    SelectedIdentityUnavailable {
        kind: AudioStreamFaultKind,
        message: String,
    },
    #[error("selected audio device changed host identity")]
    HostIdentityChanged,
    #[error("unable to query {direction:?} stream configurations ({kind:?}): {message}")]
    CapabilityQuery {
        direction: AudioDeviceDirection,
        kind: AudioStreamFaultKind,
        message: String,
    },
    #[error("requested audio stream configuration is unavailable: {0}")]
    Negotiation(#[from] AudioNegotiationError),
}

impl ResolvedAudioDevice {
    pub fn stream_config(&self) -> StreamConfig {
        StreamConfig {
            channels: self.negotiated.effective.channels,
            sample_rate: self.negotiated.effective.sample_rate,
            buffer_size: match self.negotiated.effective.buffer_size {
                AudioEffectiveBufferSize::BackendDefault => BufferSize::Default,
                AudioEffectiveBufferSize::Fixed(frames) => BufferSize::Fixed(frames),
            },
        }
    }

    pub const fn sample_format(&self) -> SampleFormat {
        self.negotiated.effective.sample_format.to_cpal()
    }
}

/// Enumerates all currently compiled/available CPAL hosts. This may call audio
/// drivers and therefore belongs on a catalog worker, never the UI/audio callback.
pub fn enumerate_audio_devices() -> Result<AudioDeviceCatalog> {
    let mut host_ids = cpal::available_hosts();
    if host_ids.is_empty() {
        return Err(anyhow!("No compiled audio host is available"));
    }
    let system_host_id = cpal::default_host().id();
    host_ids.sort_by_key(|host_id| u8::from(*host_id != system_host_id));
    let mut catalog = AudioDeviceCatalog::default();
    for host_id in host_ids {
        if catalog.entries.len() >= MAX_AUDIO_DEVICE_CATALOG_ENTRIES {
            push_catalog_diagnostic(
                &mut catalog.diagnostics,
                "Additional audio hosts were omitted because the catalog entry limit was reached"
                    .to_owned(),
            );
            break;
        }
        let host = match cpal::host_from_id(host_id) {
            Ok(host) => host,
            Err(error) => {
                push_catalog_diagnostic(
                    &mut catalog.diagnostics,
                    format!("Unable to open audio host {host_id}: {error}"),
                );
                continue;
            }
        };
        enumerate_host(
            &host,
            host_id == system_host_id,
            &mut catalog.entries,
            &mut catalog.diagnostics,
        );
    }
    catalog.entries.sort_by(|left, right| {
        left.direction
            .cmp(&right.direction)
            .then_with(|| right.is_system_default.cmp(&left.is_system_default))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.identity.device_id.cmp(&right.identity.device_id))
    });
    Ok(catalog)
}

fn enumerate_host(
    host: &Host,
    is_system_host: bool,
    entries: &mut Vec<AudioDeviceCatalogEntry>,
    diagnostics: &mut Vec<String>,
) {
    let default_input_device = is_system_host
        .then(|| host.default_input_device())
        .flatten();
    let default_output_device = is_system_host
        .then(|| host.default_output_device())
        .flatten();
    let default_input = default_input_device
        .as_ref()
        .and_then(|device| device.id().ok());
    let default_output = default_output_device
        .as_ref()
        .and_then(|device| device.id().ok());
    let mut prioritized_ids = Vec::with_capacity(2);
    for device in [
        default_output_device.as_ref(),
        default_input_device.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if entries.len() >= MAX_AUDIO_DEVICE_CATALOG_ENTRIES {
            break;
        }
        let Ok(id) = device.id() else {
            continue;
        };
        if prioritized_ids.contains(&id) {
            continue;
        }
        prioritized_ids.push(id.clone());
        collect_catalog_device(
            device,
            &id,
            host,
            0,
            default_input.as_ref(),
            default_output.as_ref(),
            entries,
            diagnostics,
        );
    }
    let devices = match host.devices() {
        Ok(devices) => devices,
        Err(error) => {
            push_catalog_diagnostic(
                diagnostics,
                format!("Unable to enumerate audio host {}: {error}", host.id()),
            );
            return;
        }
    };
    for (index, device) in devices.enumerate() {
        if index >= MAX_AUDIO_DEVICES_PER_HOST || entries.len() >= MAX_AUDIO_DEVICE_CATALOG_ENTRIES
        {
            push_catalog_diagnostic(
                diagnostics,
                format!(
                    "Audio host {} published more devices than the bounded catalog can retain",
                    host.id()
                ),
            );
            break;
        }
        let id = match device.id() {
            Ok(id) => id,
            Err(error) => {
                push_catalog_diagnostic(
                    diagnostics,
                    format!(
                        "Audio host {} device {} has no stable ID: {error}",
                        host.id(),
                        index + 1
                    ),
                );
                continue;
            }
        };
        if prioritized_ids.contains(&id) {
            continue;
        }
        collect_catalog_device(
            &device,
            &id,
            host,
            index,
            default_input.as_ref(),
            default_output.as_ref(),
            entries,
            diagnostics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_catalog_device(
    device: &Device,
    id: &DeviceId,
    host: &Host,
    index: usize,
    default_input: Option<&DeviceId>,
    default_output: Option<&DeviceId>,
    entries: &mut Vec<AudioDeviceCatalogEntry>,
    diagnostics: &mut Vec<String>,
) {
    let identity = match AudioDeviceIdentity::try_from_cpal(id) {
        Ok(identity) => identity,
        Err(_) => {
            push_catalog_diagnostic(
                diagnostics,
                format!(
                    "Audio host {} device {} has an oversized stable identity and was skipped",
                    host.id(),
                    index + 1
                ),
            );
            return;
        }
    };
    let name = device
        .description()
        .map(|description| bounded_copy(description.name(), MAX_AUDIO_DEVICE_NAME_BYTES))
        .unwrap_or_else(|_| {
            bounded_utf8(
                format!("Unnamed {} device {}", host.id(), index + 1),
                MAX_AUDIO_DEVICE_NAME_BYTES,
            )
        });
    let directions = if default_output == Some(id) {
        [AudioDeviceDirection::Output, AudioDeviceDirection::Input]
    } else {
        [AudioDeviceDirection::Input, AudioDeviceDirection::Output]
    };
    for direction in directions {
        collect_catalog_direction(
            device,
            &identity,
            &name,
            direction,
            match direction {
                AudioDeviceDirection::Input => default_input == Some(id),
                AudioDeviceDirection::Output => default_output == Some(id),
            },
            entries,
            diagnostics,
        );
    }
}

fn push_catalog_diagnostic(diagnostics: &mut Vec<String>, message: String) {
    if diagnostics.len() < MAX_AUDIO_DEVICE_CATALOG_DIAGNOSTICS {
        diagnostics.push(bounded_utf8(message, MAX_AUDIO_DEVICE_DIAGNOSTIC_BYTES));
    }
}

fn bounded_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

fn bounded_copy(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}

fn collect_catalog_direction(
    device: &Device,
    identity: &AudioDeviceIdentity,
    name: &str,
    direction: AudioDeviceDirection,
    is_system_default: bool,
    entries: &mut Vec<AudioDeviceCatalogEntry>,
    diagnostics: &mut Vec<String>,
) {
    let default_result = match direction {
        AudioDeviceDirection::Input => device.default_input_config(),
        AudioDeviceDirection::Output => device.default_output_config(),
    };
    let default_config = match &default_result {
        Ok(config) => Some(config),
        Err(error) => {
            if !matches!(error.kind(), ErrorKind::UnsupportedOperation) {
                push_catalog_diagnostic(
                    diagnostics,
                    format!(
                        "Unable to query the default {direction:?} format for {name} ({}): {error}",
                        identity.device_id
                    ),
                );
            }
            None
        }
    };
    let default_hint = default_config.and_then(default_stream_hint);
    let pinned = default_config.and_then(|config| default_capability(config, direction));
    let result = match direction {
        AudioDeviceDirection::Input => device
            .supported_input_configs()
            .map(|configs| canonical_capabilities(configs, direction, pinned, None, default_hint)),
        AudioDeviceDirection::Output => device
            .supported_output_configs()
            .map(|configs| canonical_capabilities(configs, direction, pinned, None, default_hint)),
    };
    let bounded = match result {
        Ok(bounded) => bounded,
        Err(error) => {
            if !matches!(error.kind(), ErrorKind::UnsupportedOperation) {
                push_catalog_diagnostic(
                    diagnostics,
                    format!(
                        "Unable to query {direction:?} formats for {name} ({}): {error}",
                        identity.device_id
                    ),
                );
            }
            return;
        }
    };
    if bounded.truncated {
        push_catalog_diagnostic(
            diagnostics,
            format!(
                "{name} published more than {MAX_AUDIO_DEVICE_CAPABILITIES} distinct renderer-compatible {direction:?} formats; the canonical catalog was bounded"
            ),
        );
    }
    let capabilities = bounded.values;
    if capabilities.is_empty() || entries.len() >= MAX_AUDIO_DEVICE_CATALOG_ENTRIES {
        return;
    }
    entries.push(AudioDeviceCatalogEntry {
        identity: identity.clone(),
        name: name.to_owned(),
        direction,
        is_system_default,
        capabilities,
        default_hint,
        default_config: default_config
            .and_then(|config| effective_default_config(config, direction)),
    });
}

#[derive(Default)]
struct BoundedCapabilities {
    values: Vec<AudioStreamCapability>,
    truncated: bool,
}

fn canonical_capabilities(
    configs: impl Iterator<Item = SupportedStreamConfigRange>,
    direction: AudioDeviceDirection,
    pinned: Option<AudioStreamCapability>,
    requested: Option<&AudioDeviceProfile>,
    default_hint: Option<AudioDefaultStreamHint>,
) -> BoundedCapabilities {
    let mut bounded = BoundedCapabilities {
        values: Vec::with_capacity(MAX_AUDIO_DEVICE_CAPABILITIES),
        truncated: false,
    };
    let mut request_pinned: Option<(AudioStreamCapability, Candidate)> = None;
    let mut stage_pins = [None; 4];
    for config in configs {
        let Some(capability) = capability_from_range(&config, direction) else {
            continue;
        };
        if let Some(request) = requested {
            let stage = exact_support_stage(capability, request);
            for stage_pin in stage_pins.iter_mut().take(stage) {
                if stage_pin.is_none_or(|current| capability < current) {
                    *stage_pin = Some(capability);
                }
            }
            if let Some(candidate) = candidate_for_capability(request, default_hint, capability)
                && request_pinned.is_none_or(|(_, current)| candidate.score < current.score)
            {
                request_pinned = Some((capability, candidate));
            }
        }
        bounded_insert_capability(&mut bounded, capability);
    }
    ensure_pinned_capabilities(
        &mut bounded,
        [
            pinned,
            request_pinned.map(|(capability, _)| capability),
            stage_pins[0],
            stage_pins[1],
            stage_pins[2],
            stage_pins[3],
        ],
    );
    bounded
}

fn ensure_pinned_capabilities<const N: usize>(
    bounded: &mut BoundedCapabilities,
    capabilities: [Option<AudioStreamCapability>; N],
) {
    let mut pinned = capabilities
        .into_iter()
        .flatten()
        .filter(|capability| capability.is_valid())
        .collect::<Vec<_>>();
    pinned.sort();
    pinned.dedup();
    for capability in pinned.iter().copied() {
        if let Err(index) = bounded.values.binary_search(&capability) {
            bounded.values.insert(index, capability);
        }
    }
    while bounded.values.len() > MAX_AUDIO_DEVICE_CAPABILITIES {
        let index = bounded
            .values
            .iter()
            .rposition(|capability| pinned.binary_search(capability).is_err())
            .expect("the fixed capability bound exceeds the pin count");
        bounded.values.remove(index);
        bounded.truncated = true;
    }
}

fn exact_support_stage(capability: AudioStreamCapability, requested: &AudioDeviceProfile) -> usize {
    if matches!(
        requested.sample_format,
        AudioSampleFormatRequest::Exact(format) if format != capability.sample_format
    ) {
        return 0;
    }
    if matches!(
        requested.channels,
        AudioChannelRequest::Exact(channels) if channels != capability.channels
    ) {
        return 1;
    }
    if matches!(
        requested.sample_rate,
        AudioSampleRateRequest::Exact(rate)
            if !(capability.min_sample_rate..=capability.max_sample_rate).contains(&rate)
    ) {
        return 2;
    }
    if matches!(
        requested.buffer_size,
        AudioBufferSizeRequest::Fixed(frames)
            if !matches!(
                capability.buffer_size,
                AudioBufferCapability::Range { min, max } if (min..=max).contains(&frames)
            )
    ) {
        return 3;
    }
    4
}

fn bounded_insert_capability(bounded: &mut BoundedCapabilities, capability: AudioStreamCapability) {
    let Err(index) = bounded.values.binary_search(&capability) else {
        return;
    };
    if bounded.values.len() < MAX_AUDIO_DEVICE_CAPABILITIES {
        bounded.values.insert(index, capability);
    } else {
        bounded.truncated = true;
        if index < MAX_AUDIO_DEVICE_CAPABILITIES {
            bounded.values.insert(index, capability);
            bounded.values.pop();
        }
    }
}

fn capability_from_range(
    range: &SupportedStreamConfigRange,
    direction: AudioDeviceDirection,
) -> Option<AudioStreamCapability> {
    let sample_format = AudioSampleFormat::from_cpal(range.sample_format())?;
    if !sample_format.supports_direction(direction) {
        return None;
    }
    let capability = AudioStreamCapability {
        channels: range.channels(),
        sample_format,
        min_sample_rate: range.min_sample_rate(),
        max_sample_rate: range.max_sample_rate(),
        buffer_size: buffer_capability(range.buffer_size()),
    };
    capability.is_valid().then_some(capability)
}

fn buffer_capability(buffer: &SupportedBufferSize) -> AudioBufferCapability {
    match buffer {
        SupportedBufferSize::Range { min, max } => AudioBufferCapability::Range {
            min: *min,
            max: *max,
        },
        SupportedBufferSize::Unknown => AudioBufferCapability::Unknown,
    }
}

fn default_stream_hint(config: &SupportedStreamConfig) -> Option<AudioDefaultStreamHint> {
    (config.channels() != 0 && config.sample_rate() != 0).then(|| AudioDefaultStreamHint {
        channels: config.channels(),
        sample_rate: config.sample_rate(),
        sample_format: AudioSampleFormat::from_cpal(config.sample_format()),
    })
}

fn default_capability(
    config: &SupportedStreamConfig,
    direction: AudioDeviceDirection,
) -> Option<AudioStreamCapability> {
    let hint = default_stream_hint(config)?;
    let sample_format = hint.sample_format?;
    if !sample_format.supports_direction(direction) {
        return None;
    }
    Some(AudioStreamCapability {
        channels: hint.channels,
        sample_format,
        min_sample_rate: hint.sample_rate,
        max_sample_rate: hint.sample_rate,
        buffer_size: buffer_capability(config.buffer_size()),
    })
}

fn effective_default_config(
    config: &SupportedStreamConfig,
    direction: AudioDeviceDirection,
) -> Option<AudioEffectiveStreamConfig> {
    let hint = default_stream_hint(config)?;
    let sample_format = hint.sample_format?;
    if !sample_format.supports_direction(direction) {
        return None;
    }
    Some(AudioEffectiveStreamConfig {
        channels: hint.channels,
        sample_rate: hint.sample_rate,
        sample_format,
        // A supported range describes capability, not the backend's actual
        // callback size. The default stream still requests backend default.
        buffer_size: AudioEffectiveBufferSize::BackendDefault,
    })
}

pub fn resolve_audio_device(
    profile: &AudioDeviceProfile,
) -> Result<ResolvedAudioDevice, AudioDeviceResolveError> {
    let (host, device) = match &profile.selection {
        AudioDeviceSelection::SystemDefault => {
            let host = cpal::default_host();
            let device = match profile.direction {
                AudioDeviceDirection::Input => host.default_input_device(),
                AudioDeviceDirection::Output => host.default_output_device(),
            }
            .ok_or(AudioDeviceResolveError::DefaultUnavailable {
                direction: profile.direction,
            })?;
            (host, device)
        }
        AudioDeviceSelection::Stable(identity) => {
            let id = identity.parse()?;
            let host = cpal::host_from_id(id.host()).map_err(|error| {
                AudioDeviceResolveError::HostOpen {
                    host_id: id.host().to_string(),
                    kind: AudioStreamFaultKind::from_cpal(error.kind()),
                    message: error.to_string(),
                }
            })?;
            let devices =
                host.devices()
                    .map_err(|error| AudioDeviceResolveError::DeviceEnumeration {
                        host_id: id.host().to_string(),
                        kind: AudioStreamFaultKind::from_cpal(error.kind()),
                        message: error.to_string(),
                    })?;
            let mut identity_failure = None;
            let mut selected = None;
            let mut lookup_truncated = false;
            for (index, candidate) in devices.enumerate() {
                if index >= MAX_AUDIO_DEVICES_PER_HOST {
                    lookup_truncated = true;
                    break;
                }
                match candidate.id() {
                    Ok(candidate_id) if candidate_id == id => {
                        selected = Some(candidate);
                        break;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        identity_failure = Some((
                            AudioStreamFaultKind::from_cpal(error.kind()),
                            error.to_string(),
                        ));
                    }
                }
            }
            let device = if let Some(device) = selected {
                device
            } else if lookup_truncated {
                return Err(AudioDeviceResolveError::StableLookupTruncated);
            } else if let Some((kind, message)) = identity_failure {
                return Err(AudioDeviceResolveError::StableLookupIncomplete { kind, message });
            } else {
                return Err(AudioDeviceResolveError::StableDeviceNotFound {
                    direction: profile.direction,
                    device_id: identity.device_id.clone(),
                });
            };
            (host, device)
        }
    };
    let id = device.id().map_err(
        |error| AudioDeviceResolveError::SelectedIdentityUnavailable {
            kind: AudioStreamFaultKind::from_cpal(error.kind()),
            message: error.to_string(),
        },
    )?;
    if id.host() != host.id() {
        return Err(AudioDeviceResolveError::HostIdentityChanged);
    }
    let identity = AudioDeviceIdentity::try_from_cpal(&id)?;
    let name = device
        .description()
        .map(|description| bounded_copy(description.name(), MAX_AUDIO_DEVICE_NAME_BYTES))
        .unwrap_or_else(|_| "Unnamed audio device".to_owned());
    let default_config = match profile.direction {
        AudioDeviceDirection::Input => device.default_input_config(),
        AudioDeviceDirection::Output => device.default_output_config(),
    };
    let default_config = default_config.ok();
    let default_hint = default_config.as_ref().and_then(default_stream_hint);
    let pinned = default_config
        .as_ref()
        .and_then(|config| default_capability(config, profile.direction));
    let capabilities = match profile.direction {
        AudioDeviceDirection::Input => device.supported_input_configs().map(|configs| {
            canonical_capabilities(
                configs,
                profile.direction,
                pinned,
                Some(profile),
                default_hint,
            )
        }),
        AudioDeviceDirection::Output => device.supported_output_configs().map(|configs| {
            canonical_capabilities(
                configs,
                profile.direction,
                pinned,
                Some(profile),
                default_hint,
            )
        }),
    }
    .map_err(|error| AudioDeviceResolveError::CapabilityQuery {
        direction: profile.direction,
        kind: AudioStreamFaultKind::from_cpal(error.kind()),
        message: error.to_string(),
    })?
    .values;
    let negotiated =
        negotiate_stream_config_with_hint(profile, identity, &capabilities, default_hint)?;
    Ok(ResolvedAudioDevice {
        device,
        name,
        negotiated,
    })
}

impl AudioNegotiatedStreamConfig {
    /// Exact, stable profile suitable for validation as a last-known-good request.
    pub fn resolved_profile(&self) -> AudioDeviceProfile {
        AudioDeviceProfile {
            direction: self.requested.direction,
            selection: AudioDeviceSelection::Stable(self.resolved_device.clone()),
            sample_rate: AudioSampleRateRequest::Exact(self.effective.sample_rate),
            channels: AudioChannelRequest::Exact(self.effective.channels),
            sample_format: AudioSampleFormatRequest::Exact(self.effective.sample_format),
            buffer_size: match self.effective.buffer_size {
                AudioEffectiveBufferSize::BackendDefault => AudioBufferSizeRequest::BackendDefault,
                AudioEffectiveBufferSize::Fixed(frames) => AudioBufferSizeRequest::Fixed(frames),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum AudioNegotiationError {
    #[error("resolved audio device does not match the requested stable identity")]
    ResolvedDeviceMismatch,
    #[error("audio device published no renderer-compatible stream configuration")]
    NoCompatibleConfiguration,
    #[error("requested sample rate is zero or unsupported")]
    UnsupportedExactSampleRate,
    #[error("requested channel count is zero or unsupported")]
    UnsupportedExactChannels,
    #[error("requested sample format is unsupported")]
    UnsupportedExactSampleFormat,
    #[error("requested fixed buffer size is zero, outside the device range, or unknown")]
    UnsupportedFixedBufferSize,
}

#[derive(Clone, Copy)]
struct Candidate {
    effective: AudioEffectiveStreamConfig,
    score: (u8, u32, u32, u32, u16, u8, u32, u8, u32),
    fallbacks: [Option<AudioNegotiationFallback>; 5],
}

fn candidate_for_capability(
    requested: &AudioDeviceProfile,
    default: Option<AudioDefaultStreamHint>,
    capability: AudioStreamCapability,
) -> Option<Candidate> {
    let any_device_default_requested =
        matches!(
            requested.sample_format,
            AudioSampleFormatRequest::DeviceDefault
        ) || matches!(requested.channels, AudioChannelRequest::DeviceDefault)
            || matches!(requested.sample_rate, AudioSampleRateRequest::DeviceDefault);
    let default_configuration_fallback = (default.is_none() && any_device_default_requested)
        .then_some(AudioNegotiationFallback::DefaultConfigurationUnavailable);
    let format_fallback = match requested.sample_format {
        AudioSampleFormatRequest::Exact(format) if format != capability.sample_format => {
            return None;
        }
        AudioSampleFormatRequest::Automatic => Some(AudioNegotiationFallback::AutomaticFormat),
        AudioSampleFormatRequest::DeviceDefault
            if default
                .and_then(|value| value.sample_format)
                .is_some_and(|format| format != capability.sample_format)
                || default.is_some_and(|value| value.sample_format.is_none()) =>
        {
            Some(AudioNegotiationFallback::DefaultFormatUnavailable)
        }
        _ => None,
    };

    let channel_target = match requested.channels {
        AudioChannelRequest::Exact(channels) => {
            if channels != capability.channels {
                return None;
            }
            channels
        }
        AudioChannelRequest::Nearest(channels) => channels.max(1),
        AudioChannelRequest::DeviceDefault => default.map_or(2, |value| value.channels.max(1)),
    };
    let channel_fallback = match requested.channels {
        AudioChannelRequest::Nearest(_) if capability.channels != channel_target => {
            Some(AudioNegotiationFallback::NearestChannels)
        }
        AudioChannelRequest::DeviceDefault
            if default.is_some_and(|value| value.channels != capability.channels) =>
        {
            Some(AudioNegotiationFallback::DefaultChannelsUnavailable)
        }
        _ => None,
    };

    let rate_target = match requested.sample_rate {
        AudioSampleRateRequest::Exact(rate) => rate,
        AudioSampleRateRequest::Nearest(rate) => rate.max(1),
        AudioSampleRateRequest::DeviceDefault => {
            default.map_or(48_000, |value| value.sample_rate.max(1))
        }
    };
    let sample_rate = match requested.sample_rate {
        AudioSampleRateRequest::Exact(rate) => {
            if !(capability.min_sample_rate..=capability.max_sample_rate).contains(&rate) {
                return None;
            }
            rate
        }
        AudioSampleRateRequest::Nearest(_) | AudioSampleRateRequest::DeviceDefault => {
            rate_target.clamp(capability.min_sample_rate, capability.max_sample_rate)
        }
    };
    let rate_fallback = match requested.sample_rate {
        AudioSampleRateRequest::Nearest(_) if sample_rate != rate_target => {
            Some(AudioNegotiationFallback::NearestSampleRate)
        }
        AudioSampleRateRequest::DeviceDefault
            if default.is_some_and(|value| value.sample_rate != sample_rate) =>
        {
            Some(AudioNegotiationFallback::DefaultSampleRateUnavailable)
        }
        _ => None,
    };

    let (buffer_size, buffer_fallback, buffer_distance) = match requested.buffer_size {
        AudioBufferSizeRequest::BackendDefault => {
            (AudioEffectiveBufferSize::BackendDefault, None, 0)
        }
        AudioBufferSizeRequest::Fixed(frames) => match capability.buffer_size {
            AudioBufferCapability::Range { min, max } if (min..=max).contains(&frames) => {
                (AudioEffectiveBufferSize::Fixed(frames), None, 0)
            }
            _ => return None,
        },
        AudioBufferSizeRequest::Nearest(frames) => match capability.buffer_size {
            AudioBufferCapability::Range { min, max } => {
                let selected = frames.max(1).clamp(min, max);
                (
                    AudioEffectiveBufferSize::Fixed(selected),
                    (selected != frames).then_some(AudioNegotiationFallback::NearestBufferSize),
                    selected.abs_diff(frames),
                )
            }
            AudioBufferCapability::Unknown => (
                AudioEffectiveBufferSize::BackendDefault,
                Some(AudioNegotiationFallback::BackendDefaultBufferBecauseRangeUnknown),
                u32::MAX,
            ),
        },
    };

    Some(Candidate {
        effective: AudioEffectiveStreamConfig {
            channels: capability.channels,
            sample_rate,
            sample_format: capability.sample_format,
            buffer_size,
        },
        score: (
            u8::from(format_fallback.is_some())
                + u8::from(channel_fallback.is_some())
                + u8::from(rate_fallback.is_some())
                + u8::from(buffer_fallback.is_some()),
            u32::from(capability.channels.abs_diff(channel_target)),
            sample_rate.abs_diff(rate_target),
            buffer_distance,
            capability.channels,
            capability.sample_format.renderer_rank(),
            sample_rate,
            match buffer_size {
                AudioEffectiveBufferSize::BackendDefault => 0,
                AudioEffectiveBufferSize::Fixed(_) => 1,
            },
            match buffer_size {
                AudioEffectiveBufferSize::BackendDefault => 0,
                AudioEffectiveBufferSize::Fixed(frames) => frames,
            },
        ),
        fallbacks: [
            default_configuration_fallback,
            format_fallback,
            channel_fallback,
            rate_fallback,
            buffer_fallback,
        ],
    })
}

/// Resolves one request against a bounded capability snapshot. Exact requests
/// either match exactly or fail; only `Nearest`/`Automatic` may substitute.
pub fn negotiate_stream_config(
    requested: &AudioDeviceProfile,
    resolved_device: AudioDeviceIdentity,
    capabilities: &[AudioStreamCapability],
    device_default: Option<AudioEffectiveStreamConfig>,
) -> Result<AudioNegotiatedStreamConfig, AudioNegotiationError> {
    let default_hint = device_default.map(|config| AudioDefaultStreamHint {
        channels: config.channels,
        sample_rate: config.sample_rate,
        sample_format: Some(config.sample_format),
    });
    negotiate_stream_config_with_hint(requested, resolved_device, capabilities, default_hint)
}

fn negotiate_stream_config_with_hint(
    requested: &AudioDeviceProfile,
    resolved_device: AudioDeviceIdentity,
    capabilities: &[AudioStreamCapability],
    device_default: Option<AudioDefaultStreamHint>,
) -> Result<AudioNegotiatedStreamConfig, AudioNegotiationError> {
    if let AudioDeviceSelection::Stable(expected) = &requested.selection
        && expected != &resolved_device
    {
        return Err(AudioNegotiationError::ResolvedDeviceMismatch);
    }
    if matches!(requested.sample_rate, AudioSampleRateRequest::Exact(0)) {
        return Err(AudioNegotiationError::UnsupportedExactSampleRate);
    }
    if matches!(requested.channels, AudioChannelRequest::Exact(0)) {
        return Err(AudioNegotiationError::UnsupportedExactChannels);
    }
    if matches!(requested.buffer_size, AudioBufferSizeRequest::Fixed(0)) {
        return Err(AudioNegotiationError::UnsupportedFixedBufferSize);
    }
    if matches!(
        requested.sample_format,
        AudioSampleFormatRequest::Exact(format)
            if !format.supports_direction(requested.direction)
    ) {
        return Err(AudioNegotiationError::UnsupportedExactSampleFormat);
    }
    let mut canonical = BoundedCapabilities::default();
    canonical
        .values
        .reserve(capabilities.len().min(MAX_AUDIO_DEVICE_CAPABILITIES));
    let mut request_pinned: Option<(AudioStreamCapability, Candidate)> = None;
    let mut stage_pins = [None; 4];
    for capability in capabilities.iter().copied().filter(|capability| {
        capability.is_valid()
            && capability
                .sample_format
                .supports_direction(requested.direction)
    }) {
        let stage = exact_support_stage(capability, requested);
        for stage_pin in stage_pins.iter_mut().take(stage) {
            if stage_pin.is_none_or(|current| capability < current) {
                *stage_pin = Some(capability);
            }
        }
        if let Some(candidate) = candidate_for_capability(requested, device_default, capability)
            && request_pinned.is_none_or(|(_, current)| candidate.score < current.score)
        {
            request_pinned = Some((capability, candidate));
        }
        bounded_insert_capability(&mut canonical, capability);
    }
    ensure_pinned_capabilities(
        &mut canonical,
        [
            request_pinned.map(|(capability, _)| capability),
            stage_pins[0],
            stage_pins[1],
            stage_pins[2],
            stage_pins[3],
        ],
    );
    let capabilities = canonical.values.as_slice();
    if capabilities.is_empty() {
        return Err(AudioNegotiationError::NoCompatibleConfiguration);
    }
    let valid_capabilities = || capabilities.iter().copied();
    let matches_exact_format = |capability: AudioStreamCapability| {
        !matches!(
            requested.sample_format,
            AudioSampleFormatRequest::Exact(format) if format != capability.sample_format
        )
    };
    let matches_exact_channels = |capability: AudioStreamCapability| {
        !matches!(
            requested.channels,
            AudioChannelRequest::Exact(channels) if channels != capability.channels
        )
    };
    let matches_exact_rate = |capability: AudioStreamCapability| {
        !matches!(
            requested.sample_rate,
            AudioSampleRateRequest::Exact(rate)
                if !(capability.min_sample_rate..=capability.max_sample_rate).contains(&rate)
        )
    };
    if matches!(requested.sample_format, AudioSampleFormatRequest::Exact(_))
        && !valid_capabilities().any(matches_exact_format)
    {
        return Err(AudioNegotiationError::UnsupportedExactSampleFormat);
    }
    if matches!(requested.channels, AudioChannelRequest::Exact(_))
        && !valid_capabilities()
            .filter(|capability| matches_exact_format(*capability))
            .any(matches_exact_channels)
    {
        return Err(AudioNegotiationError::UnsupportedExactChannels);
    }
    if matches!(requested.sample_rate, AudioSampleRateRequest::Exact(_))
        && !valid_capabilities()
            .filter(|capability| {
                matches_exact_format(*capability) && matches_exact_channels(*capability)
            })
            .any(matches_exact_rate)
    {
        return Err(AudioNegotiationError::UnsupportedExactSampleRate);
    }
    if let AudioBufferSizeRequest::Fixed(frames) = requested.buffer_size
        && !valid_capabilities()
            .filter(|capability| {
                matches_exact_format(*capability)
                    && matches_exact_channels(*capability)
                    && matches_exact_rate(*capability)
            })
            .any(|capability| {
                matches!(
                    capability.buffer_size,
                    AudioBufferCapability::Range { min, max } if (min..=max).contains(&frames)
                )
            })
    {
        return Err(AudioNegotiationError::UnsupportedFixedBufferSize);
    }

    let mut best: Option<Candidate> = None;
    for capability in capabilities.iter().copied() {
        let Some(candidate) = candidate_for_capability(requested, device_default, capability)
        else {
            continue;
        };
        if best.is_none_or(|current| candidate.score < current.score) {
            best = Some(candidate);
        }
    }

    let Some(best) = best else {
        return Err(AudioNegotiationError::NoCompatibleConfiguration);
    };
    let mut fallback_reasons = Vec::with_capacity(5);
    fallback_reasons.extend(best.fallbacks.into_iter().flatten());
    Ok(AudioNegotiatedStreamConfig {
        requested: requested.clone(),
        resolved_device,
        effective: best.effective,
        fallback_reasons,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum AudioStreamFaultKind {
    #[default]
    None = 0,
    Xrun = 1,
    DeviceChanged = 2,
    DeviceNotAvailable = 3,
    PermissionDenied = 4,
    StreamInvalidated = 5,
    DeviceBusy = 6,
    HostUnavailable = 7,
    RealtimeDenied = 8,
    ResourceExhausted = 9,
    UnsupportedConfig = 10,
    BackendError = 11,
    Other = 12,
}

impl AudioStreamFaultKind {
    pub fn from_cpal(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::Xrun => Self::Xrun,
            ErrorKind::DeviceChanged => Self::DeviceChanged,
            ErrorKind::DeviceNotAvailable => Self::DeviceNotAvailable,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::StreamInvalidated => Self::StreamInvalidated,
            ErrorKind::DeviceBusy => Self::DeviceBusy,
            ErrorKind::HostUnavailable => Self::HostUnavailable,
            ErrorKind::RealtimeDenied => Self::RealtimeDenied,
            ErrorKind::ResourceExhausted => Self::ResourceExhausted,
            ErrorKind::UnsupportedConfig => Self::UnsupportedConfig,
            ErrorKind::BackendError => Self::BackendError,
            _ => Self::Other,
        }
    }

    fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Xrun,
            2 => Self::DeviceChanged,
            3 => Self::DeviceNotAvailable,
            4 => Self::PermissionDenied,
            5 => Self::StreamInvalidated,
            6 => Self::DeviceBusy,
            7 => Self::HostUnavailable,
            8 => Self::RealtimeDenied,
            9 => Self::ResourceExhausted,
            10 => Self::UnsupportedConfig,
            11 => Self::BackendError,
            12 => Self::Other,
            _ => Self::None,
        }
    }

    /// Whether a device transaction must fail closed. CPAL documents
    /// `RealtimeDenied` as a scheduling-priority warning while audio continues.
    pub const fn invalidates_stream(self) -> bool {
        !matches!(self, Self::None | Self::Xrun | Self::RealtimeDenied)
    }

    pub const fn is_nonfatal_warning(self) -> bool {
        matches!(self, Self::RealtimeDenied)
    }
}

#[derive(Debug, Default)]
pub struct CallbackTelemetry {
    callback_revision: AtomicU64,
    callback_count: AtomicU64,
    total_frames: AtomicU64,
    last_frames: AtomicU32,
    minimum_frames: AtomicU32,
    maximum_frames: AtomicU32,
    size_changes: AtomicU64,
    unaligned_callbacks: AtomicU64,
    xrun_count: AtomicU64,
    stream_error_count: AtomicU64,
    /// High 56 bits are a monotonic receipt; low 8 bits are the fault kind.
    last_error_state: AtomicU64,
    invalidating_error_count: AtomicU64,
    /// Fatal reports are kept separately so a later xrun/warning cannot hide one.
    last_invalidating_error_state: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioStreamTelemetrySnapshot {
    pub callback_count: u64,
    pub total_frames: u64,
    pub last_frames: Option<u32>,
    pub minimum_frames: Option<u32>,
    pub maximum_frames: Option<u32>,
    pub size_changes: u64,
    pub unaligned_callbacks: u64,
    pub xrun_count: u64,
    pub stream_error_count: u64,
    pub last_error_kind: AudioStreamFaultKind,
    pub last_error_revision: u64,
    pub invalidating_error_count: u64,
    pub last_invalidating_error_kind: AudioStreamFaultKind,
    pub last_invalidating_error_revision: u64,
}

impl CallbackTelemetry {
    /// Observes the raw backend callback before the renderer's internal chunking.
    pub fn observe_callback(&self, interleaved_samples: usize, channels: usize) -> Option<u32> {
        let callback_revision = self.begin_callback_write();
        if channels == 0 || !interleaved_samples.is_multiple_of(channels) {
            self.unaligned_callbacks.fetch_add(1, Ordering::Relaxed);
            self.end_callback_write(callback_revision);
            return None;
        }
        let frames = u32::try_from(interleaved_samples / channels).unwrap_or(u32::MAX);
        let previous_count = self.callback_count.fetch_add(1, Ordering::Relaxed);
        let previous = self.last_frames.swap(frames, Ordering::Relaxed);
        if previous_count != 0 && previous != frames {
            self.size_changes.fetch_add(1, Ordering::Relaxed);
        }
        if previous_count == 0 {
            self.minimum_frames.store(frames, Ordering::Relaxed);
            self.maximum_frames.store(frames, Ordering::Relaxed);
        } else {
            self.minimum_frames.fetch_min(frames, Ordering::Relaxed);
            self.maximum_frames.fetch_max(frames, Ordering::Relaxed);
        }
        self.total_frames
            .fetch_add(u64::from(frames), Ordering::Relaxed);
        self.end_callback_write(callback_revision);
        Some(frames)
    }

    pub fn observe_error(&self, kind: ErrorKind) {
        let kind = AudioStreamFaultKind::from_cpal(kind);
        if kind == AudioStreamFaultKind::Xrun {
            self.xrun_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.stream_error_count.fetch_add(1, Ordering::Relaxed);
        }
        // Keep the kind and receipt in one CAS-updated word. This remains
        // coherent even if a backend invokes overlapping error callbacks.
        update_packed_error_state(&self.last_error_state, kind);
        if kind.invalidates_stream() {
            self.invalidating_error_count
                .fetch_add(1, Ordering::Relaxed);
            update_packed_error_state(&self.last_invalidating_error_state, kind);
        }
    }

    pub fn callback_count(&self) -> u64 {
        self.read_callback_stats().0
    }

    pub fn snapshot(&self) -> AudioStreamTelemetrySnapshot {
        let (
            callback_count,
            total_frames,
            last_frames,
            minimum_frames,
            maximum_frames,
            size_changes,
            unaligned_callbacks,
        ) = self.read_callback_stats();
        let last_error_state = self.last_error_state.load(Ordering::Acquire);
        let last_error_kind = AudioStreamFaultKind::from_code((last_error_state & 0xff) as u32);
        let last_error_revision = last_error_state >> 8;
        let last_invalidating_error_state =
            self.last_invalidating_error_state.load(Ordering::Acquire);
        let last_invalidating_error_kind =
            AudioStreamFaultKind::from_code((last_invalidating_error_state & 0xff) as u32);
        let last_invalidating_error_revision = last_invalidating_error_state >> 8;
        AudioStreamTelemetrySnapshot {
            callback_count,
            total_frames,
            last_frames: (callback_count != 0).then_some(last_frames),
            minimum_frames: (callback_count != 0).then_some(minimum_frames),
            maximum_frames: (callback_count != 0).then_some(maximum_frames),
            size_changes,
            unaligned_callbacks,
            xrun_count: self.xrun_count.load(Ordering::Relaxed),
            stream_error_count: self.stream_error_count.load(Ordering::Relaxed),
            last_error_kind,
            last_error_revision,
            invalidating_error_count: self.invalidating_error_count.load(Ordering::Relaxed),
            last_invalidating_error_kind,
            last_invalidating_error_revision,
        }
    }

    fn begin_callback_write(&self) -> u64 {
        let mut revision = self.callback_revision.load(Ordering::Relaxed);
        loop {
            if revision & 1 != 0 {
                std::hint::spin_loop();
                revision = self.callback_revision.load(Ordering::Relaxed);
                continue;
            }
            match self.callback_revision.compare_exchange_weak(
                revision,
                revision.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return revision,
                Err(actual) => revision = actual,
            }
        }
    }

    fn end_callback_write(&self, revision: u64) {
        self.callback_revision
            .store(revision.wrapping_add(2), Ordering::Release);
    }

    #[allow(clippy::type_complexity)]
    fn read_callback_stats(&self) -> (u64, u64, u32, u32, u32, u64, u64) {
        loop {
            let before = self.callback_revision.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let values = (
                self.callback_count.load(Ordering::Relaxed),
                self.total_frames.load(Ordering::Relaxed),
                self.last_frames.load(Ordering::Relaxed),
                self.minimum_frames.load(Ordering::Relaxed),
                self.maximum_frames.load(Ordering::Relaxed),
                self.size_changes.load(Ordering::Relaxed),
                self.unaligned_callbacks.load(Ordering::Relaxed),
            );
            std::sync::atomic::fence(Ordering::Acquire);
            if before == self.callback_revision.load(Ordering::Relaxed) {
                return values;
            }
        }
    }
}

fn update_packed_error_state(state: &AtomicU64, kind: AudioStreamFaultKind) {
    // Keep the same lock-free update semantics without requiring a newer
    // atomic API than the pinned release toolchain.
    let mut current = state.load(Ordering::Relaxed);
    loop {
        let revision = (current >> 8).saturating_add(1).min((1_u64 << 56) - 1);
        let next = (revision << 8) | u64::from(kind as u32);
        match state.compare_exchange_weak(current, next, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> AudioDeviceIdentity {
        let host = cpal::default_host().id();
        AudioDeviceIdentity {
            host_id: host.to_string(),
            device_id: DeviceId::new(host, "test-device").to_string(),
        }
    }

    fn capability(
        channels: u16,
        format: AudioSampleFormat,
        min_rate: u32,
        max_rate: u32,
        buffer: AudioBufferCapability,
    ) -> AudioStreamCapability {
        AudioStreamCapability {
            channels,
            sample_format: format,
            min_sample_rate: min_rate,
            max_sample_rate: max_rate,
            buffer_size: buffer,
        }
    }

    fn range(
        channels: u16,
        format: SampleFormat,
        min_rate: u32,
        max_rate: u32,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            channels,
            min_rate,
            max_rate,
            SupportedBufferSize::Range { min: 64, max: 512 },
            format,
        )
    }

    #[test]
    fn identity_round_trip_rejects_host_mismatch() {
        let canonical = identity();
        assert_eq!(canonical.parse().unwrap().to_string(), canonical.device_id);
        let mut mismatched = canonical;
        mismatched.host_id = "not-the-host".into();
        assert_eq!(
            mismatched.parse(),
            Err(AudioDeviceIdentityError::HostMismatch)
        );

        let mut noncanonical = identity();
        noncanonical.host_id = noncanonical.host_id.to_ascii_uppercase();
        if noncanonical.host_id != identity().host_id {
            assert!(matches!(
                noncanonical.parse(),
                Err(AudioDeviceIdentityError::NonCanonical)
                    | Err(AudioDeviceIdentityError::InvalidDeviceId)
            ));
        }
    }

    #[test]
    fn exact_requests_never_clamp_or_change_format() {
        let capabilities = [capability(
            2,
            AudioSampleFormat::F32,
            44_100,
            96_000,
            AudioBufferCapability::Range { min: 64, max: 512 },
        )];
        let mut request = AudioDeviceProfile::system_default_output();
        request.sample_rate = AudioSampleRateRequest::Exact(48_000);
        request.channels = AudioChannelRequest::Exact(2);
        request.sample_format = AudioSampleFormatRequest::Exact(AudioSampleFormat::F32);
        request.buffer_size = AudioBufferSizeRequest::Fixed(128);
        let negotiated =
            negotiate_stream_config(&request, identity(), &capabilities, None).unwrap();
        assert_eq!(negotiated.effective.sample_rate, 48_000);
        assert_eq!(
            negotiated.effective.buffer_size,
            AudioEffectiveBufferSize::Fixed(128)
        );

        request.sample_rate = AudioSampleRateRequest::Exact(192_000);
        assert_eq!(
            negotiate_stream_config(&request, identity(), &capabilities, None),
            Err(AudioNegotiationError::UnsupportedExactSampleRate)
        );
        request.sample_rate = AudioSampleRateRequest::Exact(48_000);
        request.buffer_size = AudioBufferSizeRequest::Fixed(32);
        assert_eq!(
            negotiate_stream_config(&request, identity(), &capabilities, None),
            Err(AudioNegotiationError::UnsupportedFixedBufferSize)
        );
    }

    #[test]
    fn stable_device_selection_cannot_be_satisfied_by_another_device() {
        let capabilities = [capability(
            2,
            AudioSampleFormat::F32,
            48_000,
            48_000,
            AudioBufferCapability::Unknown,
        )];
        let mut request = AudioDeviceProfile::system_default_output();
        let expected = identity();
        request.selection = AudioDeviceSelection::Stable(expected.clone());
        let mut actual = expected;
        actual.device_id = DeviceId::new(cpal::default_host().id(), "other-device").to_string();
        assert_eq!(
            negotiate_stream_config(&request, actual, &capabilities, None),
            Err(AudioNegotiationError::ResolvedDeviceMismatch)
        );

        request.selection = AudioDeviceSelection::SystemDefault;
        assert!(negotiate_stream_config(&request, identity(), &capabilities, None).is_ok());
    }

    #[test]
    fn input_accepts_all_recording_pcm_formats_but_output_remains_renderer_exact() {
        for format in [
            AudioSampleFormat::I8,
            AudioSampleFormat::I24,
            AudioSampleFormat::I32,
            AudioSampleFormat::I64,
            AudioSampleFormat::U8,
            AudioSampleFormat::U24,
            AudioSampleFormat::U32,
            AudioSampleFormat::U64,
            AudioSampleFormat::F64,
        ] {
            let capabilities = [capability(
                2,
                format,
                48_000,
                48_000,
                AudioBufferCapability::Unknown,
            )];
            let mut request = AudioDeviceProfile::system_default_input();
            request.sample_format = AudioSampleFormatRequest::Exact(format);
            assert!(
                negotiate_stream_config(&request, identity(), &capabilities, None).is_ok(),
                "input format {format} must remain recordable"
            );

            request.direction = AudioDeviceDirection::Output;
            assert_eq!(
                negotiate_stream_config(&request, identity(), &capabilities, None),
                Err(AudioNegotiationError::UnsupportedExactSampleFormat)
            );
        }
    }

    #[test]
    fn unsupported_default_format_preserves_default_rate_and_channels() {
        let capabilities = [
            capability(
                2,
                AudioSampleFormat::F32,
                48_000,
                48_000,
                AudioBufferCapability::Unknown,
            ),
            capability(
                8,
                AudioSampleFormat::F32,
                96_000,
                96_000,
                AudioBufferCapability::Unknown,
            ),
        ];
        let request = AudioDeviceProfile::system_default_output();
        let negotiated = negotiate_stream_config_with_hint(
            &request,
            identity(),
            &capabilities,
            Some(AudioDefaultStreamHint {
                channels: 8,
                sample_rate: 96_000,
                sample_format: None,
            }),
        )
        .unwrap();
        assert_eq!(negotiated.effective.channels, 8);
        assert_eq!(negotiated.effective.sample_rate, 96_000);
        assert_eq!(
            negotiated.fallback_reasons,
            vec![AudioNegotiationFallback::DefaultFormatUnavailable]
        );

        let unavailable =
            negotiate_stream_config_with_hint(&request, identity(), &capabilities, None).unwrap();
        assert!(
            unavailable
                .fallback_reasons
                .contains(&AudioNegotiationFallback::DefaultConfigurationUnavailable)
        );
    }

    #[test]
    fn bounded_capabilities_are_order_independent_and_pin_exact_request() {
        let forward = (1..=513_u16)
            .map(|channels| range(channels, SampleFormat::F32, 48_000, 48_000))
            .collect::<Vec<_>>();
        let reverse = forward.iter().copied().rev().collect::<Vec<_>>();
        let left = canonical_capabilities(
            forward.iter().copied(),
            AudioDeviceDirection::Output,
            None,
            None,
            None,
        );
        let right = canonical_capabilities(
            reverse.into_iter(),
            AudioDeviceDirection::Output,
            None,
            None,
            None,
        );
        assert!(left.truncated && right.truncated);
        assert_eq!(left.values, right.values);
        assert_eq!(left.values.len(), MAX_AUDIO_DEVICE_CAPABILITIES);

        let mut request = AudioDeviceProfile::system_default_output();
        request.channels = AudioChannelRequest::Exact(513);
        request.sample_rate = AudioSampleRateRequest::Exact(48_000);
        request.sample_format = AudioSampleFormatRequest::Exact(AudioSampleFormat::F32);
        let pinned = canonical_capabilities(
            forward.into_iter(),
            AudioDeviceDirection::Output,
            None,
            Some(&request),
            None,
        );
        assert!(pinned.values.iter().any(|value| value.channels == 513));
        assert!(negotiate_stream_config(&request, identity(), &pinned.values, None).is_ok());

        let mut invalid_then_valid = vec![
            capability(
                0,
                AudioSampleFormat::F32,
                48_000,
                48_000,
                AudioBufferCapability::Unknown,
            );
            MAX_AUDIO_DEVICE_CAPABILITIES
        ];
        invalid_then_valid.push(capability(
            2,
            AudioSampleFormat::F32,
            48_000,
            48_000,
            AudioBufferCapability::Unknown,
        ));
        let mut valid_request = AudioDeviceProfile::system_default_output();
        valid_request.channels = AudioChannelRequest::Exact(2);
        assert!(
            negotiate_stream_config(&valid_request, identity(), &invalid_then_valid, None).is_ok()
        );

        let default = capability(
            600,
            AudioSampleFormat::F32,
            48_000,
            48_000,
            AudioBufferCapability::Range { min: 64, max: 512 },
        );
        let mut format_exact = AudioDeviceProfile::system_default_output();
        format_exact.sample_format = AudioSampleFormatRequest::Exact(AudioSampleFormat::F32);
        let default_pinned = canonical_capabilities(
            (1..=513_u16).map(|channels| range(channels, SampleFormat::F32, 48_000, 48_000)),
            AudioDeviceDirection::Output,
            Some(default),
            Some(&format_exact),
            Some(AudioDefaultStreamHint {
                channels: 600,
                sample_rate: 48_000,
                sample_format: Some(AudioSampleFormat::F32),
            }),
        );
        assert!(default_pinned.values.contains(&default));

        let default_buffer = capability(
            2,
            AudioSampleFormat::F32,
            48_000,
            48_000,
            AudioBufferCapability::Range { min: 512, max: 512 },
        );
        let nearest_buffer = AudioStreamCapability {
            buffer_size: AudioBufferCapability::Range { min: 128, max: 128 },
            ..default_buffer
        };
        let mut crowded = (1..=MAX_AUDIO_DEVICE_CAPABILITIES as u32)
            .map(|sample_rate| range(1, SampleFormat::F32, sample_rate, sample_rate))
            .collect::<Vec<_>>();
        crowded.push(SupportedStreamConfigRange::new(
            2,
            48_000,
            48_000,
            SupportedBufferSize::Range { min: 128, max: 128 },
            SampleFormat::F32,
        ));
        let mut nearest_request = AudioDeviceProfile::system_default_output();
        nearest_request.channels = AudioChannelRequest::Exact(2);
        nearest_request.sample_rate = AudioSampleRateRequest::Exact(48_000);
        nearest_request.sample_format = AudioSampleFormatRequest::Exact(AudioSampleFormat::F32);
        nearest_request.buffer_size = AudioBufferSizeRequest::Nearest(128);
        let both_pinned = canonical_capabilities(
            crowded.into_iter(),
            AudioDeviceDirection::Output,
            Some(default_buffer),
            Some(&nearest_request),
            Some(AudioDefaultStreamHint {
                channels: 2,
                sample_rate: 48_000,
                sample_format: Some(AudioSampleFormat::F32),
            }),
        );
        assert_eq!(both_pinned.values.len(), MAX_AUDIO_DEVICE_CAPABILITIES);
        assert!(both_pinned.values.contains(&default_buffer));
        assert!(both_pinned.values.contains(&nearest_buffer));
        let selected = negotiate_stream_config_with_hint(
            &nearest_request,
            identity(),
            &both_pinned.values,
            Some(AudioDefaultStreamHint {
                channels: 2,
                sample_rate: 48_000,
                sample_format: Some(AudioSampleFormat::F32),
            }),
        )
        .unwrap();
        assert_eq!(
            selected.effective.buffer_size,
            AudioEffectiveBufferSize::Fixed(128)
        );

        let default_i16 = capability(
            64,
            AudioSampleFormat::I16,
            192_000,
            192_000,
            AudioBufferCapability::Unknown,
        );
        let preferred_f32 = capability(
            64,
            AudioSampleFormat::F32,
            192_000,
            192_000,
            AudioBufferCapability::Range { min: 64, max: 512 },
        );
        let mut defaults_request = AudioDeviceProfile::system_default_output();
        defaults_request.sample_format = AudioSampleFormatRequest::Exact(AudioSampleFormat::F32);
        let default_hint = AudioDefaultStreamHint {
            channels: 64,
            sample_rate: 192_000,
            sample_format: Some(AudioSampleFormat::I16),
        };
        let mut default_crowded = (1..=MAX_AUDIO_DEVICE_CAPABILITIES as u32)
            .map(|rate| range(1, SampleFormat::F32, rate, rate))
            .collect::<Vec<_>>();
        default_crowded.push(range(2, SampleFormat::F32, 48_000, 48_000));
        default_crowded.push(range(64, SampleFormat::F32, 192_000, 192_000));
        let default_scored = canonical_capabilities(
            default_crowded.into_iter(),
            AudioDeviceDirection::Output,
            Some(default_i16),
            Some(&defaults_request),
            Some(default_hint),
        );
        assert!(default_scored.values.contains(&default_i16));
        assert!(default_scored.values.contains(&preferred_f32));
        let selected = negotiate_stream_config_with_hint(
            &defaults_request,
            identity(),
            &default_scored.values,
            Some(default_hint),
        )
        .unwrap();
        assert_eq!(selected.effective.channels, 64);
        assert_eq!(selected.effective.sample_rate, 192_000);

        let mut staged = (1..=MAX_AUDIO_DEVICE_CAPABILITIES as u32)
            .map(|rate| range(1, SampleFormat::I16, rate, rate))
            .collect::<Vec<_>>();
        staged.push(range(6, SampleFormat::F32, 48_000, 48_000));
        let mut staged_request = AudioDeviceProfile::system_default_output();
        staged_request.sample_format = AudioSampleFormatRequest::Exact(AudioSampleFormat::F32);
        staged_request.channels = AudioChannelRequest::Exact(2);
        let staged = canonical_capabilities(
            staged.into_iter(),
            AudioDeviceDirection::Output,
            None,
            Some(&staged_request),
            None,
        );
        assert!(
            staged.values.iter().any(|value| {
                value.sample_format == AudioSampleFormat::F32 && value.channels == 6
            })
        );
        assert_eq!(
            negotiate_stream_config(&staged_request, identity(), &staged.values, None),
            Err(AudioNegotiationError::UnsupportedExactChannels)
        );
    }

    #[test]
    fn catalog_text_identity_and_diagnostic_storage_are_bounded() {
        let unicode = "音🙂".repeat(400);
        let bounded = bounded_utf8(unicode, MAX_AUDIO_DEVICE_NAME_BYTES);
        assert!(bounded.len() <= MAX_AUDIO_DEVICE_NAME_BYTES);
        assert!(bounded.is_char_boundary(bounded.len()));

        let mut diagnostics = Vec::new();
        for _ in 0..(MAX_AUDIO_DEVICE_CATALOG_DIAGNOSTICS + 20) {
            push_catalog_diagnostic(
                &mut diagnostics,
                "x".repeat(MAX_AUDIO_DEVICE_DIAGNOSTIC_BYTES + 500),
            );
        }
        assert_eq!(diagnostics.len(), MAX_AUDIO_DEVICE_CATALOG_DIAGNOSTICS);
        assert!(
            diagnostics
                .iter()
                .all(|message| message.len() <= MAX_AUDIO_DEVICE_DIAGNOSTIC_BYTES)
        );

        let host = cpal::default_host().id();
        let oversized = AudioDeviceIdentity {
            host_id: host.to_string(),
            device_id: DeviceId::new(host, "x".repeat(MAX_AUDIO_DEVICE_ID_BYTES + 1)).to_string(),
        };
        assert_eq!(oversized.parse(), Err(AudioDeviceIdentityError::TooLong));
    }

    #[test]
    fn nearest_ties_are_independent_of_capability_order() {
        let rate_low = capability(
            2,
            AudioSampleFormat::F32,
            44_000,
            44_000,
            AudioBufferCapability::Range { min: 100, max: 100 },
        );
        let rate_high = capability(
            2,
            AudioSampleFormat::F32,
            52_000,
            52_000,
            AudioBufferCapability::Range { min: 156, max: 156 },
        );
        let mut request = AudioDeviceProfile::system_default_output();
        request.sample_rate = AudioSampleRateRequest::Nearest(48_000);
        request.channels = AudioChannelRequest::Exact(2);
        request.sample_format = AudioSampleFormatRequest::Exact(AudioSampleFormat::F32);
        request.buffer_size = AudioBufferSizeRequest::BackendDefault;
        let forward =
            negotiate_stream_config(&request, identity(), &[rate_low, rate_high], None).unwrap();
        let reverse =
            negotiate_stream_config(&request, identity(), &[rate_high, rate_low], None).unwrap();
        assert_eq!(forward.effective, reverse.effective);
        assert_eq!(forward.fallback_reasons, reverse.fallback_reasons);
        assert_eq!(forward.effective.sample_rate, 44_000);

        request.sample_rate = AudioSampleRateRequest::Exact(48_000);
        request.buffer_size = AudioBufferSizeRequest::Nearest(128);
        let buffer_low = AudioStreamCapability {
            min_sample_rate: 48_000,
            max_sample_rate: 48_000,
            ..rate_low
        };
        let buffer_high = AudioStreamCapability {
            min_sample_rate: 48_000,
            max_sample_rate: 48_000,
            ..rate_high
        };
        let forward =
            negotiate_stream_config(&request, identity(), &[buffer_low, buffer_high], None)
                .unwrap();
        let reverse =
            negotiate_stream_config(&request, identity(), &[buffer_high, buffer_low], None)
                .unwrap();
        assert_eq!(forward.effective, reverse.effective);
        assert_eq!(
            forward.effective.buffer_size,
            AudioEffectiveBufferSize::Fixed(100)
        );
    }

    #[test]
    fn request_validation_precedes_empty_capability_errors() {
        let mut request = AudioDeviceProfile::system_default_output();
        request.sample_rate = AudioSampleRateRequest::Exact(0);
        assert_eq!(
            negotiate_stream_config(&request, identity(), &[], None),
            Err(AudioNegotiationError::UnsupportedExactSampleRate)
        );
        request.sample_rate = AudioSampleRateRequest::DeviceDefault;
        request.sample_format = AudioSampleFormatRequest::Exact(AudioSampleFormat::F64);
        assert_eq!(
            negotiate_stream_config(&request, identity(), &[], None),
            Err(AudioNegotiationError::UnsupportedExactSampleFormat)
        );
    }

    #[test]
    fn automatic_selection_is_deterministic_and_nearest_is_explicit() {
        let capabilities = [
            capability(
                6,
                AudioSampleFormat::I16,
                44_100,
                48_000,
                AudioBufferCapability::Unknown,
            ),
            capability(
                2,
                AudioSampleFormat::F32,
                44_100,
                96_000,
                AudioBufferCapability::Range { min: 64, max: 512 },
            ),
        ];
        let mut request = AudioDeviceProfile::system_default_output();
        request.sample_rate = AudioSampleRateRequest::Nearest(100_000);
        request.channels = AudioChannelRequest::Nearest(2);
        request.sample_format = AudioSampleFormatRequest::Automatic;
        request.buffer_size = AudioBufferSizeRequest::Nearest(32);
        let negotiated =
            negotiate_stream_config(&request, identity(), &capabilities, None).unwrap();
        assert_eq!(negotiated.effective.channels, 2);
        assert_eq!(negotiated.effective.sample_format, AudioSampleFormat::F32);
        assert_eq!(negotiated.effective.sample_rate, 96_000);
        assert_eq!(
            negotiated.effective.buffer_size,
            AudioEffectiveBufferSize::Fixed(64)
        );
        assert!(
            negotiated
                .fallback_reasons
                .contains(&AudioNegotiationFallback::NearestSampleRate)
        );
        assert!(
            negotiated
                .fallback_reasons
                .contains(&AudioNegotiationFallback::NearestBufferSize)
        );
    }

    #[test]
    fn unknown_buffer_never_fabricates_a_fixed_size() {
        let capabilities = [capability(
            2,
            AudioSampleFormat::F32,
            48_000,
            48_000,
            AudioBufferCapability::Unknown,
        )];
        let mut request = AudioDeviceProfile::system_default_output();
        request.buffer_size = AudioBufferSizeRequest::Fixed(256);
        assert_eq!(
            negotiate_stream_config(&request, identity(), &capabilities, None),
            Err(AudioNegotiationError::UnsupportedFixedBufferSize)
        );
        request.buffer_size = AudioBufferSizeRequest::Nearest(256);
        let negotiated =
            negotiate_stream_config(&request, identity(), &capabilities, None).unwrap();
        assert_eq!(
            negotiated.effective.buffer_size,
            AudioEffectiveBufferSize::BackendDefault
        );
    }

    #[test]
    fn callback_telemetry_observes_raw_sizes_and_error_kinds() {
        let telemetry = CallbackTelemetry::default();
        assert_eq!(telemetry.snapshot().last_frames, None);
        assert_eq!(telemetry.observe_callback(128, 2), Some(64));
        assert_eq!(telemetry.observe_callback(256, 2), Some(128));
        assert_eq!(telemetry.observe_callback(128, 2), Some(64));
        assert_eq!(telemetry.observe_callback(3, 2), None);
        telemetry.observe_error(ErrorKind::Xrun);
        telemetry.observe_error(ErrorKind::DeviceNotAvailable);
        assert_eq!(
            telemetry.snapshot(),
            AudioStreamTelemetrySnapshot {
                callback_count: 3,
                total_frames: 256,
                last_frames: Some(64),
                minimum_frames: Some(64),
                maximum_frames: Some(128),
                size_changes: 2,
                unaligned_callbacks: 1,
                xrun_count: 1,
                stream_error_count: 1,
                last_error_kind: AudioStreamFaultKind::DeviceNotAvailable,
                last_error_revision: 2,
                invalidating_error_count: 1,
                last_invalidating_error_kind: AudioStreamFaultKind::DeviceNotAvailable,
                last_invalidating_error_revision: 1,
            }
        );
    }

    #[test]
    fn callback_snapshots_and_concurrent_error_receipts_remain_coherent() {
        use std::{
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
            thread,
        };

        let telemetry = Arc::new(CallbackTelemetry::default());
        let running = Arc::new(AtomicBool::new(true));
        let writer_telemetry = Arc::clone(&telemetry);
        let writer_running = Arc::clone(&running);
        let writer = thread::spawn(move || {
            for index in 0..10_000 {
                let frames = if index & 1 == 0 { 64 } else { 128 };
                writer_telemetry.observe_callback(frames * 2, 2);
            }
            writer_running.store(false, Ordering::Release);
        });
        while running.load(Ordering::Acquire) {
            let snapshot = telemetry.snapshot();
            if snapshot.callback_count != 0 {
                assert!(snapshot.last_frames.is_some_and(|frames| frames != 0));
                assert!(snapshot.minimum_frames.is_some_and(|frames| frames != 0));
                assert!(snapshot.maximum_frames.is_some_and(|frames| frames != 0));
                assert!(snapshot.total_frames >= snapshot.callback_count * 64);
            }
        }
        writer.join().unwrap();
        assert_eq!(telemetry.callback_count(), 10_000);

        let first = Arc::clone(&telemetry);
        let second = Arc::clone(&telemetry);
        let first = thread::spawn(move || {
            for _ in 0..1_000 {
                first.observe_error(ErrorKind::Xrun);
            }
        });
        let second = thread::spawn(move || {
            for _ in 0..1_000 {
                second.observe_error(ErrorKind::DeviceNotAvailable);
            }
        });
        first.join().unwrap();
        second.join().unwrap();
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.last_error_revision, 2_000);
        assert_eq!(snapshot.xrun_count, 1_000);
        assert_eq!(snapshot.stream_error_count, 1_000);
        assert_eq!(snapshot.invalidating_error_count, 1_000);
        assert_eq!(snapshot.last_invalidating_error_revision, 1_000);
        assert_eq!(
            snapshot.last_invalidating_error_kind,
            AudioStreamFaultKind::DeviceNotAvailable
        );
        assert!(matches!(
            snapshot.last_error_kind,
            AudioStreamFaultKind::Xrun | AudioStreamFaultKind::DeviceNotAvailable
        ));

        let concurrent = Arc::new(CallbackTelemetry::default());
        let short = Arc::clone(&concurrent);
        let long = Arc::clone(&concurrent);
        let short = thread::spawn(move || {
            for _ in 0..1_000 {
                short.observe_callback(128, 2);
            }
        });
        let long = thread::spawn(move || {
            for _ in 0..1_000 {
                long.observe_callback(256, 2);
            }
        });
        short.join().unwrap();
        long.join().unwrap();
        let snapshot = concurrent.snapshot();
        assert_eq!(snapshot.callback_count, 2_000);
        assert_eq!(snapshot.total_frames, 192_000);
        assert_eq!(snapshot.minimum_frames, Some(64));
        assert_eq!(snapshot.maximum_frames, Some(128));

        let wrapping = CallbackTelemetry::default();
        wrapping
            .callback_revision
            .store(u64::MAX - 1, Ordering::Relaxed);
        assert_eq!(wrapping.observe_callback(128, 2), Some(64));
        assert_eq!(wrapping.callback_revision.load(Ordering::Relaxed), 0);
        assert_eq!(wrapping.snapshot().last_frames, Some(64));
        wrapping.last_error_state.store(
            (((1_u64 << 56) - 1) << 8) | u64::from(AudioStreamFaultKind::Xrun as u32),
            Ordering::Relaxed,
        );
        wrapping.observe_error(ErrorKind::DeviceNotAvailable);
        let snapshot = wrapping.snapshot();
        assert_eq!(snapshot.last_error_revision, (1_u64 << 56) - 1);
        assert_eq!(
            snapshot.last_error_kind,
            AudioStreamFaultKind::DeviceNotAvailable
        );

        let ordered = CallbackTelemetry::default();
        ordered.observe_error(ErrorKind::DeviceNotAvailable);
        ordered.observe_error(ErrorKind::Xrun);
        let snapshot = ordered.snapshot();
        assert_eq!(snapshot.last_error_kind, AudioStreamFaultKind::Xrun);
        assert_eq!(
            snapshot.last_invalidating_error_kind,
            AudioStreamFaultKind::DeviceNotAvailable
        );
        assert_eq!(snapshot.last_invalidating_error_revision, 1);

        let warning = CallbackTelemetry::default();
        warning.observe_error(ErrorKind::RealtimeDenied);
        warning.observe_error(ErrorKind::Xrun);
        let snapshot = warning.snapshot();
        assert_eq!(snapshot.last_invalidating_error_revision, 0);
        assert_eq!(snapshot.invalidating_error_count, 0);
        assert!(AudioStreamFaultKind::RealtimeDenied.is_nonfatal_warning());
    }
}
