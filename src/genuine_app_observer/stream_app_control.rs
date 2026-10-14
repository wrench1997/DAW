//! Borrowed App/control wire v1 for genuine observer kinds 10, 11, 12 and 15.
//! No callback collects here. The parent supplies original global sequence,
//! timestamp, stage and optional instance; this body supplies no SDK authority.
//! Runtime State follows the existing tap: length-only OrdinaryState kind 16,
//! never an invented state payload. See runtime_tap for the explicit routing.
#![cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]

use super::{RecordKind, TimelineAppState, TimelinePhase};
use crate::{
    audio::{
        AudioAssetEvent, AudioAssetOperation, GeneratorEndpointEvent, GeneratorRemovalRejectReason,
        GeneratorRemovalStamp,
    },
    plugins::plugin_runtime::{
        MAX_PLUGIN_PARAMETER_PAGE_ITEMS, PluginParameterCommand, PluginParameterDescriptor,
        RuntimeEvent, StateCaptureRejectReason,
    },
    timeline_runtime::{
        TimelineDiscontinuityActivationError, TimelineDiscontinuityKind, TimelineRejectReason,
        TimelineRuntimeEvent, TimelineTransportActivationRejectReason,
    },
};

pub(super) const MAX_APP_CONTROL_BYTES: usize = 64 * 1024;
const MAX_TEXT: usize = 4096;
const MAGIC: &[u8; 4] = b"NAPP";
const VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppError {
    DestinationNotEmpty,
    DestinationCapacity,
    Bound,
    Truncated,
    TrailingBytes,
    InvalidMagic,
    UnknownVersion,
    UnknownCode,
    InvalidBoolean,
    InvalidOption,
    InvalidUtf8,
    KindMismatch,
    UnsupportedKind,
    OrdinaryStateRequired,
}

pub(super) enum AppView<'a> {
    AudioAsset(&'a AudioAssetEvent),
    GeneratorEndpoint(&'a GeneratorEndpointEvent),
    Timeline {
        event: &'a TimelineRuntimeEvent,
        before: &'a TimelineAppState,
        after: &'a TimelineAppState,
    },
    Runtime {
        endpoint_id: u64,
        project_session: u64,
        instance_id: u64,
        event: &'a RuntimeEvent,
    },
}
impl AppView<'_> {
    pub(super) const fn kind(&self) -> u16 {
        match self {
            Self::AudioAsset(_) => 10,
            Self::GeneratorEndpoint(_) => 11,
            Self::Timeline { .. } => 12,
            Self::Runtime { .. } => 15,
        }
    }
}

/// Literal length-only fields already collected by record_runtime_event.
/// Pass these to the existing typed control View::OrdinaryState (kind 16).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OrdinaryStateLength {
    pub endpoint_id: u64,
    pub project_session: u64,
    pub instance_id: u64,
    pub slot: usize,
    pub request_id: u64,
    pub epoch: Option<u64>,
    pub byte_len: usize,
}
pub(super) enum RuntimeTap<'a> {
    Retained(AppView<'a>),
    OrdinaryState(OrdinaryStateLength),
}
pub(super) fn runtime_tap(
    endpoint_id: u64,
    project_session: u64,
    instance_id: u64,
    event: &RuntimeEvent,
) -> RuntimeTap<'_> {
    if let RuntimeEvent::State {
        slot,
        request_id,
        epoch,
        bytes,
    } = event
    {
        RuntimeTap::OrdinaryState(OrdinaryStateLength {
            endpoint_id,
            project_session,
            instance_id,
            slot: *slot,
            request_id: *request_id,
            epoch: *epoch,
            byte_len: bytes.len(),
        })
    } else {
        RuntimeTap::Retained(AppView::Runtime {
            endpoint_id,
            project_session,
            instance_id,
            event,
        })
    }
}
pub(super) fn view_record(record: &RecordKind) -> Result<AppView<'_>, AppError> {
    match record {
        RecordKind::AppAudioAssetEvent(event) => Ok(AppView::AudioAsset(event)),
        RecordKind::AppGeneratorEndpointEvent(event) => Ok(AppView::GeneratorEndpoint(event)),
        RecordKind::AppTimelineRuntimeEvent {
            event,
            before,
            after,
        } => Ok(AppView::Timeline {
            event,
            before,
            after,
        }),
        RecordKind::RuntimeEvent {
            endpoint_id,
            project_session,
            instance_id,
            event,
        } => match runtime_tap(*endpoint_id, *project_session, *instance_id, event) {
            RuntimeTap::Retained(view) => Ok(view),
            RuntimeTap::OrdinaryState(_) => Err(AppError::OrdinaryStateRequired),
        },
        _ => Err(AppError::UnsupportedKind),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ParameterBits<'a> {
    pub id: u32,
    pub name: &'a str,
    pub unit: &'a str,
    pub current_normalized_bits: u32,
    pub default_normalized_bits: Option<u32>,
    pub step_count: Option<u32>,
    pub automatable: bool,
    pub read_only: bool,
    pub bypass: bool,
}
/// Every item is validated while decoding. The complete encoded page is then
/// borrowed, so no Vec, String or 64-item array is allocated or copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Catalog<'a> {
    count: usize,
    encoded: &'a [u8],
}
impl<'a> Catalog<'a> {
    pub(super) const fn len(&self) -> usize {
        self.count
    }
    pub(super) const fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub(super) fn items(&self) -> CatalogItems<'a> {
        CatalogItems {
            reader: Reader {
                bytes: self.encoded,
                position: 0,
            },
            remaining: self.count,
        }
    }
}
pub(super) struct CatalogItems<'a> {
    reader: Reader<'a>,
    remaining: usize,
}
impl<'a> Iterator for CatalogItems<'a> {
    type Item = Result<ParameterBits<'a>, AppError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        let result = read_parameter(&mut self.reader);
        if result.is_err() {
            self.remaining = 0;
        }
        Some(result)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RuntimeBody<'a> {
    SlotReady {
        slot: usize,
        name: &'a str,
        latency_samples: u32,
        tail_samples: u32,
    },
    SlotFault {
        slot: usize,
        message: &'a str,
    },
    ParameterValue {
        slot: usize,
        request_id: u64,
        id: u32,
        value_bits: u32,
    },
    ParameterCatalogPage {
        slot: usize,
        request_id: u64,
        cursor: u32,
        next_cursor: Option<u32>,
        done: bool,
        catalog_revision: u64,
        items: Catalog<'a>,
    },
    ParameterSetAck {
        slot: usize,
        request_id: u64,
        id: u32,
        value_bits: u32,
    },
    ParameterCommandFailed {
        slot: usize,
        request_id: u64,
        command: PluginParameterCommand,
        id: Option<u32>,
        message: &'a str,
    },
    StateRejected {
        slot: usize,
        request_id: u64,
        epoch: u64,
        reason: StateCaptureRejectReason,
    },
    ShutdownComplete,
}
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // Complete copy-only before/after must not acquire heap owners.
pub(super) enum AppControl<'a> {
    AudioAsset(AudioAssetEvent),
    GeneratorEndpoint(GeneratorEndpointEvent),
    Timeline {
        event: TimelineRuntimeEvent,
        before: TimelineAppState,
        after: TimelineAppState,
    },
    Runtime {
        endpoint_id: u64,
        project_session: u64,
        instance_id: u64,
        event: RuntimeBody<'a>,
    },
}

struct Writer<'a> {
    destination: &'a mut Vec<u8>,
    total_body_budget: usize,
}
impl Writer<'_> {
    fn bytes(&mut self, value: &[u8]) -> Result<(), AppError> {
        let length = self
            .destination
            .len()
            .checked_add(value.len())
            .ok_or(AppError::Bound)?;
        if length > self.total_body_budget {
            return Err(AppError::Bound);
        }
        if length > self.destination.capacity() {
            return Err(AppError::DestinationCapacity);
        }
        self.destination.extend_from_slice(value);
        Ok(())
    }
    fn u8(&mut self, value: u8) -> Result<(), AppError> {
        self.bytes(&[value])
    }
    fn u16(&mut self, value: u16) -> Result<(), AppError> {
        self.bytes(&value.to_le_bytes())
    }
    fn u32(&mut self, value: u32) -> Result<(), AppError> {
        self.bytes(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<(), AppError> {
        self.bytes(&value.to_le_bytes())
    }
    fn usize(&mut self, value: usize) -> Result<(), AppError> {
        self.u64(u64::try_from(value).map_err(|_| AppError::Bound)?)
    }
    fn bool(&mut self, value: bool) -> Result<(), AppError> {
        self.u8(u8::from(value))
    }
    fn option<T>(
        &mut self,
        value: Option<T>,
        encode: impl FnOnce(&mut Self, T) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.bool(value.is_some())?;
        if let Some(value) = value {
            encode(self, value)?;
        }
        Ok(())
    }
    fn text(&mut self, value: &str) -> Result<(), AppError> {
        if value.len() > MAX_TEXT {
            return Err(AppError::Bound);
        }
        self.u32(u32::try_from(value.len()).map_err(|_| AppError::Bound)?)?;
        self.bytes(value.as_bytes())
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], AppError> {
        let end = self.position.checked_add(length).ok_or(AppError::Bound)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(AppError::Truncated)?;
        self.position = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, AppError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, AppError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| AppError::Truncated)?,
        ))
    }
    fn u32(&mut self) -> Result<u32, AppError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| AppError::Truncated)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, AppError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| AppError::Truncated)?,
        ))
    }
    fn usize(&mut self) -> Result<usize, AppError> {
        usize::try_from(self.u64()?).map_err(|_| AppError::Bound)
    }
    fn bool(&mut self) -> Result<bool, AppError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(AppError::InvalidBoolean),
        }
    }
    fn option<T>(
        &mut self,
        decode: impl FnOnce(&mut Self) -> Result<T, AppError>,
    ) -> Result<Option<T>, AppError> {
        match self.u8()? {
            0 => Ok(None),
            1 => decode(self).map(Some),
            _ => Err(AppError::InvalidOption),
        }
    }
    fn text(&mut self) -> Result<&'a str, AppError> {
        let length = usize::try_from(self.u32()?).map_err(|_| AppError::Bound)?;
        if length > MAX_TEXT {
            return Err(AppError::Bound);
        }
        std::str::from_utf8(self.take(length)?).map_err(|_| AppError::InvalidUtf8)
    }
}

/// Requires an empty, caller-reserved Vec with capacity <= 64KiB. No reserve,
/// realloc, String clone or RecordKind construction occurs in this encoder.
/// A failed attempt clears its partial bytes; an occupied Vec is preserved.
pub(super) fn encode(view: AppView<'_>, destination: &mut Vec<u8>) -> Result<u16, AppError> {
    encode_with_limit(view, destination, MAX_APP_CONTROL_BYTES)
}

/// The limit covers the complete typed body, including MAGIC/version/kind. The
/// caller subtracts its outer envelope from the operand limit before calling.
/// Nothing is omitted to meet that limit, and a budget above 64KiB is an error.
pub(super) fn encode_with_limit(
    view: AppView<'_>,
    destination: &mut Vec<u8>,
    total_body_budget: usize,
) -> Result<u16, AppError> {
    if !destination.is_empty() {
        return Err(AppError::DestinationNotEmpty);
    }
    if total_body_budget > MAX_APP_CONTROL_BYTES {
        return Err(AppError::Bound);
    }
    if destination.capacity() > MAX_APP_CONTROL_BYTES {
        return Err(AppError::DestinationCapacity);
    }
    let kind = view.kind();
    let result = (|| {
        let mut w = Writer {
            destination,
            total_body_budget,
        };
        w.bytes(MAGIC)?;
        w.u16(VERSION)?;
        w.u16(kind)?;
        match view {
            AppView::AudioAsset(event) => write_asset(&mut w, *event),
            AppView::GeneratorEndpoint(event) => write_generator(&mut w, *event),
            AppView::Timeline {
                event,
                before,
                after,
            } => {
                write_timeline(&mut w, *event)?;
                write_app_state(&mut w, before)?;
                write_app_state(&mut w, after)
            }
            AppView::Runtime {
                endpoint_id,
                project_session,
                instance_id,
                event,
            } => {
                w.u64(endpoint_id)?;
                w.u64(project_session)?;
                w.u64(instance_id)?;
                write_runtime(&mut w, event)
            }
        }
    })();
    if result.is_err() {
        destination.clear();
    }
    result.map(|()| kind)
}
pub(super) fn decode(bytes: &[u8], expected_kind: u16) -> Result<AppControl<'_>, AppError> {
    if bytes.len() > MAX_APP_CONTROL_BYTES {
        return Err(AppError::Bound);
    }
    let mut r = Reader { bytes, position: 0 };
    if r.take(4)? != MAGIC {
        return Err(AppError::InvalidMagic);
    }
    if r.u16()? != VERSION {
        return Err(AppError::UnknownVersion);
    }
    let kind = r.u16()?;
    if kind != expected_kind {
        return Err(AppError::KindMismatch);
    }
    let value = match kind {
        10 => AppControl::AudioAsset(read_asset(&mut r)?),
        11 => AppControl::GeneratorEndpoint(read_generator(&mut r)?),
        12 => AppControl::Timeline {
            event: read_timeline(&mut r)?,
            before: read_app_state(&mut r)?,
            after: read_app_state(&mut r)?,
        },
        15 => AppControl::Runtime {
            endpoint_id: r.u64()?,
            project_session: r.u64()?,
            instance_id: r.u64()?,
            event: read_runtime(&mut r)?,
        },
        _ => return Err(AppError::UnknownCode),
    };
    if r.position != bytes.len() {
        return Err(AppError::TrailingBytes);
    }
    Ok(value)
}

fn write_asset_operation(
    w: &mut Writer<'_>,
    operation: AudioAssetOperation,
) -> Result<(), AppError> {
    let AudioAssetOperation {
        engine_session,
        generation,
        operation_id,
    } = operation;
    w.u64(engine_session)?;
    w.u64(generation)?;
    w.u64(operation_id)
}
fn read_asset_operation(r: &mut Reader<'_>) -> Result<AudioAssetOperation, AppError> {
    Ok(AudioAssetOperation {
        engine_session: r.u64()?,
        generation: r.u64()?,
        operation_id: r.u64()?,
    })
}
fn write_asset(w: &mut Writer<'_>, event: AudioAssetEvent) -> Result<(), AppError> {
    match event {
        AudioAssetEvent::Registered {
            operation,
            id,
            success,
        } => {
            w.u8(1)?;
            write_asset_operation(w, operation)?;
            w.u64(id)?;
            w.bool(success)
        }
        AudioAssetEvent::Unregistered {
            operation,
            id,
            removed,
        } => {
            w.u8(2)?;
            write_asset_operation(w, operation)?;
            w.u64(id)?;
            w.bool(removed)
        }
        AudioAssetEvent::Cleared { operation, removed } => {
            w.u8(3)?;
            write_asset_operation(w, operation)?;
            w.usize(removed)
        }
    }
}
fn read_asset(r: &mut Reader<'_>) -> Result<AudioAssetEvent, AppError> {
    Ok(match r.u8()? {
        1 => AudioAssetEvent::Registered {
            operation: read_asset_operation(r)?,
            id: r.u64()?,
            success: r.bool()?,
        },
        2 => AudioAssetEvent::Unregistered {
            operation: read_asset_operation(r)?,
            id: r.u64()?,
            removed: r.bool()?,
        },
        3 => AudioAssetEvent::Cleared {
            operation: read_asset_operation(r)?,
            removed: r.usize()?,
        },
        _ => return Err(AppError::UnknownCode),
    })
}
fn write_stamp(w: &mut Writer<'_>, stamp: GeneratorRemovalStamp) -> Result<(), AppError> {
    let GeneratorRemovalStamp {
        operation_id,
        engine_session,
        project_session,
        channel_id,
        endpoint_id,
        plugin_instance_id,
        stopped_epoch,
    } = stamp;
    w.u64(operation_id)?;
    w.u64(engine_session)?;
    w.u64(project_session)?;
    w.u32(channel_id)?;
    w.u64(endpoint_id)?;
    w.u64(plugin_instance_id)?;
    w.u64(stopped_epoch)
}
fn read_stamp(r: &mut Reader<'_>) -> Result<GeneratorRemovalStamp, AppError> {
    Ok(GeneratorRemovalStamp {
        operation_id: r.u64()?,
        engine_session: r.u64()?,
        project_session: r.u64()?,
        channel_id: r.u32()?,
        endpoint_id: r.u64()?,
        plugin_instance_id: r.u64()?,
        stopped_epoch: r.u64()?,
    })
}
fn removal_code(reason: GeneratorRemovalRejectReason) -> u8 {
    match reason {
        GeneratorRemovalRejectReason::InvalidStamp => 1,
        GeneratorRemovalRejectReason::EngineSessionChanged => 2,
        GeneratorRemovalRejectReason::ProjectSessionChanged => 3,
        GeneratorRemovalRejectReason::TransportRunning => 4,
        GeneratorRemovalRejectReason::TransportRequestPending => 5,
        GeneratorRemovalRejectReason::TimelineTransitionPending => 6,
        GeneratorRemovalRejectReason::StoppedEpochChanged => 7,
        GeneratorRemovalRejectReason::MissingEndpoint => 8,
        GeneratorRemovalRejectReason::EndpointChanged => 9,
        GeneratorRemovalRejectReason::PluginInstanceChanged => 10,
        GeneratorRemovalRejectReason::EndpointProjectChanged => 11,
        GeneratorRemovalRejectReason::EndpointEpochChanged => 12,
        GeneratorRemovalRejectReason::EndpointManifestChanged => 13,
    }
}
fn read_removal(code: u8) -> Result<GeneratorRemovalRejectReason, AppError> {
    Ok(match code {
        1 => GeneratorRemovalRejectReason::InvalidStamp,
        2 => GeneratorRemovalRejectReason::EngineSessionChanged,
        3 => GeneratorRemovalRejectReason::ProjectSessionChanged,
        4 => GeneratorRemovalRejectReason::TransportRunning,
        5 => GeneratorRemovalRejectReason::TransportRequestPending,
        6 => GeneratorRemovalRejectReason::TimelineTransitionPending,
        7 => GeneratorRemovalRejectReason::StoppedEpochChanged,
        8 => GeneratorRemovalRejectReason::MissingEndpoint,
        9 => GeneratorRemovalRejectReason::EndpointChanged,
        10 => GeneratorRemovalRejectReason::PluginInstanceChanged,
        11 => GeneratorRemovalRejectReason::EndpointProjectChanged,
        12 => GeneratorRemovalRejectReason::EndpointEpochChanged,
        13 => GeneratorRemovalRejectReason::EndpointManifestChanged,
        _ => return Err(AppError::UnknownCode),
    })
}
fn write_generator(w: &mut Writer<'_>, event: GeneratorEndpointEvent) -> Result<(), AppError> {
    match event {
        GeneratorEndpointEvent::Installed {
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            replaced_endpoint_id,
            replaced_plugin_instance_id,
            success,
        } => {
            w.u8(1)?;
            w.u32(channel_id)?;
            w.u64(endpoint_id)?;
            w.u64(plugin_instance_id)?;
            w.usize(mixer_track)?;
            w.option(replaced_endpoint_id, Writer::u64)?;
            w.option(replaced_plugin_instance_id, Writer::u64)?;
            w.bool(success)
        }
        GeneratorEndpointEvent::Removed {
            channel_id,
            endpoint_id,
            plugin_instance_id,
        } => {
            w.u8(2)?;
            w.u32(channel_id)?;
            w.option(endpoint_id, Writer::u64)?;
            w.option(plugin_instance_id, Writer::u64)
        }
        GeneratorEndpointEvent::StampedRemoved { stamp, result } => {
            w.u8(3)?;
            write_stamp(w, stamp)?;
            match result {
                Ok(()) => w.u8(1),
                Err(reason) => {
                    w.u8(2)?;
                    w.u8(removal_code(reason))
                }
            }
        }
        GeneratorEndpointEvent::Cleared {
            request_id,
            removed,
        } => {
            w.u8(4)?;
            w.u64(request_id)?;
            w.usize(removed)
        }
        GeneratorEndpointEvent::RouteSet {
            channel_id,
            endpoint_id,
            plugin_instance_id,
            mixer_track,
            success,
        } => {
            w.u8(5)?;
            w.u32(channel_id)?;
            w.option(endpoint_id, Writer::u64)?;
            w.option(plugin_instance_id, Writer::u64)?;
            w.usize(mixer_track)?;
            w.bool(success)
        }
    }
}
fn read_generator(r: &mut Reader<'_>) -> Result<GeneratorEndpointEvent, AppError> {
    Ok(match r.u8()? {
        1 => GeneratorEndpointEvent::Installed {
            channel_id: r.u32()?,
            endpoint_id: r.u64()?,
            plugin_instance_id: r.u64()?,
            mixer_track: r.usize()?,
            replaced_endpoint_id: r.option(Reader::u64)?,
            replaced_plugin_instance_id: r.option(Reader::u64)?,
            success: r.bool()?,
        },
        2 => GeneratorEndpointEvent::Removed {
            channel_id: r.u32()?,
            endpoint_id: r.option(Reader::u64)?,
            plugin_instance_id: r.option(Reader::u64)?,
        },
        3 => {
            let stamp = read_stamp(r)?;
            let result = match r.u8()? {
                1 => Ok(()),
                2 => Err(read_removal(r.u8()?)?),
                _ => return Err(AppError::UnknownCode),
            };
            GeneratorEndpointEvent::StampedRemoved { stamp, result }
        }
        4 => GeneratorEndpointEvent::Cleared {
            request_id: r.u64()?,
            removed: r.usize()?,
        },
        5 => GeneratorEndpointEvent::RouteSet {
            channel_id: r.u32()?,
            endpoint_id: r.option(Reader::u64)?,
            plugin_instance_id: r.option(Reader::u64)?,
            mixer_track: r.usize()?,
            success: r.bool()?,
        },
        _ => return Err(AppError::UnknownCode),
    })
}

fn write_discontinuity_kind(
    w: &mut Writer<'_>,
    kind: TimelineDiscontinuityKind,
) -> Result<(), AppError> {
    match kind {
        TimelineDiscontinuityKind::OneShot => w.u8(1),
        TimelineDiscontinuityKind::Loop { token } => {
            w.u8(2)?;
            w.u64(token)
        }
    }
}
fn read_discontinuity_kind(r: &mut Reader<'_>) -> Result<TimelineDiscontinuityKind, AppError> {
    Ok(match r.u8()? {
        1 => TimelineDiscontinuityKind::OneShot,
        2 => TimelineDiscontinuityKind::Loop { token: r.u64()? },
        _ => return Err(AppError::UnknownCode),
    })
}
fn write_activation_error(
    w: &mut Writer<'_>,
    error: TimelineDiscontinuityActivationError,
) -> Result<(), AppError> {
    use TimelineDiscontinuityActivationError as E;
    match error {
        E::MissingTimeline => w.u8(1),
        E::Shutdown => w.u8(2),
        E::InvalidEpoch => w.u8(3),
        E::NeedsResync => w.u8(4),
        E::RevisionMismatch { active, requested } => {
            w.u8(5)?;
            w.u64(active)?;
            w.u64(requested)
        }
        E::MissingPrepared { kind } => {
            w.u8(6)?;
            write_discontinuity_kind(w, kind)
        }
        E::OneShotEpochMismatch {
            expected,
            requested,
        } => {
            w.u8(7)?;
            w.u64(expected)?;
            w.u64(requested)
        }
        E::StaleEpoch { latest, requested } => {
            w.u8(8)?;
            w.u64(latest)?;
            w.u64(requested)
        }
        E::FrameMismatch {
            expected,
            requested,
        } => {
            w.u8(9)?;
            w.u64(expected)?;
            w.u64(requested)
        }
        E::OneShotConsumed => w.u8(10),
        E::LoopTokenMismatch {
            expected,
            requested,
        } => {
            w.u8(11)?;
            w.u64(expected)?;
            w.u64(requested)
        }
        E::ChunkInProgress => w.u8(12),
        E::IncompleteCandidateBundle => w.u8(13),
    }
}
fn read_activation_error(
    r: &mut Reader<'_>,
) -> Result<TimelineDiscontinuityActivationError, AppError> {
    use TimelineDiscontinuityActivationError as E;
    Ok(match r.u8()? {
        1 => E::MissingTimeline,
        2 => E::Shutdown,
        3 => E::InvalidEpoch,
        4 => E::NeedsResync,
        5 => E::RevisionMismatch {
            active: r.u64()?,
            requested: r.u64()?,
        },
        6 => E::MissingPrepared {
            kind: read_discontinuity_kind(r)?,
        },
        7 => E::OneShotEpochMismatch {
            expected: r.u64()?,
            requested: r.u64()?,
        },
        8 => E::StaleEpoch {
            latest: r.u64()?,
            requested: r.u64()?,
        },
        9 => E::FrameMismatch {
            expected: r.u64()?,
            requested: r.u64()?,
        },
        10 => E::OneShotConsumed,
        11 => E::LoopTokenMismatch {
            expected: r.u64()?,
            requested: r.u64()?,
        },
        12 => E::ChunkInProgress,
        13 => E::IncompleteCandidateBundle,
        _ => return Err(AppError::UnknownCode),
    })
}
fn write_activation_reject(
    w: &mut Writer<'_>,
    reason: TimelineTransportActivationRejectReason,
) -> Result<(), AppError> {
    use TimelineTransportActivationRejectReason as E;
    match reason {
        E::PlaybackPreparationRequired => w.u8(1),
        E::Runtime(error) => {
            w.u8(2)?;
            write_activation_error(w, error)
        }
        E::MissingAudioAsset { asset_id } => {
            w.u8(3)?;
            w.u64(asset_id)
        }
        E::ChannelBaseTable => w.u8(4),
        E::GeneratorRouteBinding => w.u8(5),
        E::MixerGraphPlan => w.u8(6),
        E::GraphPdcPlan => w.u8(7),
        E::MixerDelayBank => w.u8(8),
        E::PluginAutomationBinding => w.u8(9),
        E::ExecutorState => w.u8(10),
        E::RenderPlanCapacity => w.u8(11),
        E::EndpointEventCapacity => w.u8(12),
    }
}
fn read_activation_reject(
    r: &mut Reader<'_>,
) -> Result<TimelineTransportActivationRejectReason, AppError> {
    use TimelineTransportActivationRejectReason as E;
    Ok(match r.u8()? {
        1 => E::PlaybackPreparationRequired,
        2 => E::Runtime(read_activation_error(r)?),
        3 => E::MissingAudioAsset { asset_id: r.u64()? },
        4 => E::ChannelBaseTable,
        5 => E::GeneratorRouteBinding,
        6 => E::MixerGraphPlan,
        7 => E::GraphPdcPlan,
        8 => E::MixerDelayBank,
        9 => E::PluginAutomationBinding,
        10 => E::ExecutorState,
        11 => E::RenderPlanCapacity,
        12 => E::EndpointEventCapacity,
        _ => return Err(AppError::UnknownCode),
    })
}
fn write_timeline_reject(w: &mut Writer<'_>, reason: TimelineRejectReason) -> Result<(), AppError> {
    match reason {
        TimelineRejectReason::InvalidRevision => w.u8(1),
        TimelineRejectReason::StaleRevision { latest_revision } => {
            w.u8(2)?;
            w.u64(latest_revision)
        }
        TimelineRejectReason::RevisionMismatch { active_revision } => {
            w.u8(3)?;
            w.option(active_revision, Writer::u64)
        }
        TimelineRejectReason::StaleEpoch { latest_epoch } => {
            w.u8(4)?;
            w.u64(latest_epoch)
        }
        TimelineRejectReason::InvalidChaseFrame => w.u8(5),
        TimelineRejectReason::ChasePendingDelivery => w.u8(6),
        TimelineRejectReason::MixerDelayBankReuseUnavailable { fingerprint } => {
            w.u8(7)?;
            w.u64(fingerprint)
        }
    }
}
fn read_timeline_reject(r: &mut Reader<'_>) -> Result<TimelineRejectReason, AppError> {
    Ok(match r.u8()? {
        1 => TimelineRejectReason::InvalidRevision,
        2 => TimelineRejectReason::StaleRevision {
            latest_revision: r.u64()?,
        },
        3 => TimelineRejectReason::RevisionMismatch {
            active_revision: r.option(Reader::u64)?,
        },
        4 => TimelineRejectReason::StaleEpoch {
            latest_epoch: r.u64()?,
        },
        5 => TimelineRejectReason::InvalidChaseFrame,
        6 => TimelineRejectReason::ChasePendingDelivery,
        7 => TimelineRejectReason::MixerDelayBankReuseUnavailable {
            fingerprint: r.u64()?,
        },
        _ => return Err(AppError::UnknownCode),
    })
}
fn write_timeline(w: &mut Writer<'_>, event: TimelineRuntimeEvent) -> Result<(), AppError> {
    let code = match event {
        TimelineRuntimeEvent::Installed { .. } => 1,
        TimelineRuntimeEvent::ChaseInstalled { .. } => 2,
        TimelineRuntimeEvent::LoopChaseInstalled { .. } => 3,
        TimelineRuntimeEvent::TransportActivationApplied { .. } => 4,
        TimelineRuntimeEvent::TransportActivationRejected { .. } => 5,
        TimelineRuntimeEvent::Cleared { .. } => 6,
        TimelineRuntimeEvent::Rejected { .. } => 7,
        TimelineRuntimeEvent::ShutdownComplete { .. } => 8,
    };
    w.u8(code)?;
    w.u64(event.request_id())?;
    w.u64(event.revision())?;
    match event {
        TimelineRuntimeEvent::Installed { .. }
        | TimelineRuntimeEvent::Cleared { .. }
        | TimelineRuntimeEvent::ShutdownComplete { .. } => Ok(()),
        TimelineRuntimeEvent::ChaseInstalled { epoch, frame, .. }
        | TimelineRuntimeEvent::TransportActivationApplied { epoch, frame, .. } => {
            w.u64(epoch)?;
            w.u64(frame)
        }
        TimelineRuntimeEvent::LoopChaseInstalled { frame, token, .. } => {
            w.u64(frame)?;
            w.u64(token)
        }
        TimelineRuntimeEvent::TransportActivationRejected { reason, .. } => {
            write_activation_reject(w, reason)
        }
        TimelineRuntimeEvent::Rejected { reason, .. } => write_timeline_reject(w, reason),
    }
}
fn read_timeline(r: &mut Reader<'_>) -> Result<TimelineRuntimeEvent, AppError> {
    let code = r.u8()?;
    if !(1..=8).contains(&code) {
        return Err(AppError::UnknownCode);
    }
    let request_id = r.u64()?;
    let revision = r.u64()?;
    Ok(match code {
        1 => TimelineRuntimeEvent::Installed {
            request_id,
            revision,
        },
        2 => TimelineRuntimeEvent::ChaseInstalled {
            request_id,
            revision,
            epoch: r.u64()?,
            frame: r.u64()?,
        },
        3 => TimelineRuntimeEvent::LoopChaseInstalled {
            request_id,
            revision,
            frame: r.u64()?,
            token: r.u64()?,
        },
        4 => TimelineRuntimeEvent::TransportActivationApplied {
            request_id,
            revision,
            epoch: r.u64()?,
            frame: r.u64()?,
        },
        5 => TimelineRuntimeEvent::TransportActivationRejected {
            request_id,
            revision,
            reason: read_activation_reject(r)?,
        },
        6 => TimelineRuntimeEvent::Cleared {
            request_id,
            revision,
        },
        7 => TimelineRuntimeEvent::Rejected {
            request_id,
            revision,
            reason: read_timeline_reject(r)?,
        },
        8 => TimelineRuntimeEvent::ShutdownComplete {
            request_id,
            revision,
        },
        _ => return Err(AppError::UnknownCode),
    })
}
fn write_phase(w: &mut Writer<'_>, phase: TimelinePhase) -> Result<(), AppError> {
    match phase {
        TimelinePhase::Unavailable => w.u8(1),
        TimelinePhase::InstallReady => w.u8(2),
        TimelinePhase::InstallQueued {
            request_id,
            generation,
        } => {
            w.u8(3)?;
            w.u64(request_id)?;
            w.u64(generation)
        }
        TimelinePhase::LoopReady => w.u8(4),
        TimelinePhase::LoopQueued {
            request_id,
            generation,
            frame,
        } => {
            w.u8(5)?;
            w.u64(request_id)?;
            w.u64(generation)?;
            w.u64(frame)
        }
        TimelinePhase::SeekReady => w.u8(6),
        TimelinePhase::SeekQueued {
            request_id,
            generation,
            seek_serial,
            epoch,
            frame,
        } => {
            w.u8(7)?;
            w.u64(request_id)?;
            w.u64(generation)?;
            w.u64(seek_serial)?;
            w.u64(epoch)?;
            w.u64(frame)
        }
        TimelinePhase::AwaitingActivation {
            generation,
            seek_serial,
            epoch,
            frame,
            activation_request_id,
        } => {
            w.u8(8)?;
            w.u64(generation)?;
            w.u64(seek_serial)?;
            w.u64(epoch)?;
            w.u64(frame)?;
            w.u64(activation_request_id)
        }
        TimelinePhase::Active { generation, epoch } => {
            w.u8(9)?;
            w.u64(generation)?;
            w.u64(epoch)
        }
        TimelinePhase::AssetHandoffClearReady { revision } => {
            w.u8(10)?;
            w.u64(revision)
        }
        TimelinePhase::AssetHandoffClearQueued {
            request_id,
            revision,
        } => {
            w.u8(11)?;
            w.u64(request_id)?;
            w.u64(revision)
        }
        TimelinePhase::FaultClearReady { revision } => {
            w.u8(12)?;
            w.u64(revision)
        }
        TimelinePhase::FaultClearQueued {
            request_id,
            revision,
        } => {
            w.u8(13)?;
            w.u64(request_id)?;
            w.u64(revision)
        }
        TimelinePhase::Faulted => w.u8(14),
    }
}
fn read_phase(r: &mut Reader<'_>) -> Result<TimelinePhase, AppError> {
    Ok(match r.u8()? {
        1 => TimelinePhase::Unavailable,
        2 => TimelinePhase::InstallReady,
        3 => TimelinePhase::InstallQueued {
            request_id: r.u64()?,
            generation: r.u64()?,
        },
        4 => TimelinePhase::LoopReady,
        5 => TimelinePhase::LoopQueued {
            request_id: r.u64()?,
            generation: r.u64()?,
            frame: r.u64()?,
        },
        6 => TimelinePhase::SeekReady,
        7 => TimelinePhase::SeekQueued {
            request_id: r.u64()?,
            generation: r.u64()?,
            seek_serial: r.u64()?,
            epoch: r.u64()?,
            frame: r.u64()?,
        },
        8 => TimelinePhase::AwaitingActivation {
            generation: r.u64()?,
            seek_serial: r.u64()?,
            epoch: r.u64()?,
            frame: r.u64()?,
            activation_request_id: r.u64()?,
        },
        9 => TimelinePhase::Active {
            generation: r.u64()?,
            epoch: r.u64()?,
        },
        10 => TimelinePhase::AssetHandoffClearReady { revision: r.u64()? },
        11 => TimelinePhase::AssetHandoffClearQueued {
            request_id: r.u64()?,
            revision: r.u64()?,
        },
        12 => TimelinePhase::FaultClearReady { revision: r.u64()? },
        13 => TimelinePhase::FaultClearQueued {
            request_id: r.u64()?,
            revision: r.u64()?,
        },
        14 => TimelinePhase::Faulted,
        _ => return Err(AppError::UnknownCode),
    })
}
fn write_pair(w: &mut Writer<'_>, value: (u64, u64)) -> Result<(), AppError> {
    w.u64(value.0)?;
    w.u64(value.1)
}
fn read_pair(r: &mut Reader<'_>) -> Result<(u64, u64), AppError> {
    Ok((r.u64()?, r.u64()?))
}
fn write_app_state(w: &mut Writer<'_>, value: &TimelineAppState) -> Result<(), AppError> {
    // No '..': adding a true App field requires an explicit wire audit.
    let TimelineAppState {
        phase,
        project_session,
        engine_session,
        sync_session,
        desired_generation,
        observed_fingerprint,
        compiled_generation,
        compiled_fingerprint,
        source_generation,
        source_fingerprint,
        sync_source_generation,
        desired_seek_serial,
        desired_seek_frame,
        loop_token,
        loop_disable_request,
        retry_count,
        stale_confirmations,
        confirmed_identity,
        retry_required,
        retry_pending,
    } = *value;
    write_phase(w, phase)?;
    w.u64(project_session)?;
    w.option(engine_session, Writer::u64)?;
    w.u64(sync_session)?;
    w.u64(desired_generation)?;
    w.u64(observed_fingerprint)?;
    w.option(compiled_generation, Writer::u64)?;
    w.option(compiled_fingerprint, Writer::u64)?;
    w.option(source_generation, Writer::u64)?;
    w.option(source_fingerprint, Writer::u64)?;
    w.option(sync_source_generation, Writer::u64)?;
    w.u64(desired_seek_serial)?;
    w.u64(desired_seek_frame)?;
    w.option(loop_token, Writer::u64)?;
    w.option(loop_disable_request, Writer::u64)?;
    w.u64(retry_count)?;
    w.u64(stale_confirmations)?;
    w.option(confirmed_identity, write_pair)?;
    w.bool(retry_required)?;
    w.option(retry_pending, write_pair)
}
fn read_app_state(r: &mut Reader<'_>) -> Result<TimelineAppState, AppError> {
    Ok(TimelineAppState {
        phase: read_phase(r)?,
        project_session: r.u64()?,
        engine_session: r.option(Reader::u64)?,
        sync_session: r.u64()?,
        desired_generation: r.u64()?,
        observed_fingerprint: r.u64()?,
        compiled_generation: r.option(Reader::u64)?,
        compiled_fingerprint: r.option(Reader::u64)?,
        source_generation: r.option(Reader::u64)?,
        source_fingerprint: r.option(Reader::u64)?,
        sync_source_generation: r.option(Reader::u64)?,
        desired_seek_serial: r.u64()?,
        desired_seek_frame: r.u64()?,
        loop_token: r.option(Reader::u64)?,
        loop_disable_request: r.option(Reader::u64)?,
        retry_count: r.u64()?,
        stale_confirmations: r.u64()?,
        confirmed_identity: r.option(read_pair)?,
        retry_required: r.bool()?,
        retry_pending: r.option(read_pair)?,
    })
}

fn write_parameter(
    w: &mut Writer<'_>,
    parameter: &PluginParameterDescriptor,
) -> Result<(), AppError> {
    let PluginParameterDescriptor {
        id,
        name,
        unit,
        current_normalized,
        default_normalized,
        step_count,
        automatable,
        read_only,
        bypass,
    } = parameter;
    w.u32(*id)?;
    w.text(name)?;
    w.text(unit)?;
    w.u32(current_normalized.to_bits())?;
    w.option(default_normalized.map(f32::to_bits), Writer::u32)?;
    w.option(*step_count, Writer::u32)?;
    w.bool(*automatable)?;
    w.bool(*read_only)?;
    w.bool(*bypass)
}
fn read_parameter<'a>(r: &mut Reader<'a>) -> Result<ParameterBits<'a>, AppError> {
    Ok(ParameterBits {
        id: r.u32()?,
        name: r.text()?,
        unit: r.text()?,
        current_normalized_bits: r.u32()?,
        default_normalized_bits: r.option(Reader::u32)?,
        step_count: r.option(Reader::u32)?,
        automatable: r.bool()?,
        read_only: r.bool()?,
        bypass: r.bool()?,
    })
}
fn write_runtime(w: &mut Writer<'_>, event: &RuntimeEvent) -> Result<(), AppError> {
    match event {
        RuntimeEvent::SlotReady {
            slot,
            name,
            latency_samples,
            tail_samples,
        } => {
            w.u8(1)?;
            w.usize(*slot)?;
            w.text(name)?;
            w.u32(*latency_samples)?;
            w.u32(*tail_samples)
        }
        RuntimeEvent::SlotFault { slot, message } => {
            w.u8(2)?;
            w.usize(*slot)?;
            w.text(message)
        }
        RuntimeEvent::ParameterValue {
            slot,
            request_id,
            id,
            value,
        } => {
            w.u8(3)?;
            w.usize(*slot)?;
            w.u64(*request_id)?;
            w.u32(*id)?;
            w.u32(value.to_bits())
        }
        RuntimeEvent::ParameterCatalogPage {
            slot,
            request_id,
            cursor,
            next_cursor,
            done,
            catalog_revision,
            items,
        } => {
            if items.len() > MAX_PLUGIN_PARAMETER_PAGE_ITEMS {
                return Err(AppError::Bound);
            }
            w.u8(4)?;
            w.usize(*slot)?;
            w.u64(*request_id)?;
            w.u32(*cursor)?;
            w.option(*next_cursor, Writer::u32)?;
            w.bool(*done)?;
            w.u64(*catalog_revision)?;
            w.usize(items.len())?;
            for item in items {
                write_parameter(w, item)?;
            }
            Ok(())
        }
        RuntimeEvent::ParameterSetAck {
            slot,
            request_id,
            id,
            value,
        } => {
            w.u8(5)?;
            w.usize(*slot)?;
            w.u64(*request_id)?;
            w.u32(*id)?;
            w.u32(value.to_bits())
        }
        RuntimeEvent::ParameterCommandFailed {
            slot,
            request_id,
            command,
            id,
            message,
        } => {
            w.u8(6)?;
            w.usize(*slot)?;
            w.u64(*request_id)?;
            w.u8(match command {
                PluginParameterCommand::Catalog => 1,
                PluginParameterCommand::Set => 2,
                PluginParameterCommand::Query => 3,
            })?;
            w.option(*id, Writer::u32)?;
            w.text(message)
        }
        RuntimeEvent::State { .. } => Err(AppError::OrdinaryStateRequired),
        RuntimeEvent::StateRejected {
            slot,
            request_id,
            epoch,
            reason,
        } => {
            w.u8(8)?;
            w.usize(*slot)?;
            w.u64(*request_id)?;
            w.u64(*epoch)?;
            w.u8(match reason {
                StateCaptureRejectReason::EpochChanged => 1,
                StateCaptureRejectReason::BackendFailure => 2,
            })
        }
        RuntimeEvent::ShutdownComplete => w.u8(9),
    }
}
fn read_runtime<'a>(r: &mut Reader<'a>) -> Result<RuntimeBody<'a>, AppError> {
    Ok(match r.u8()? {
        1 => RuntimeBody::SlotReady {
            slot: r.usize()?,
            name: r.text()?,
            latency_samples: r.u32()?,
            tail_samples: r.u32()?,
        },
        2 => RuntimeBody::SlotFault {
            slot: r.usize()?,
            message: r.text()?,
        },
        3 => RuntimeBody::ParameterValue {
            slot: r.usize()?,
            request_id: r.u64()?,
            id: r.u32()?,
            value_bits: r.u32()?,
        },
        4 => {
            let slot = r.usize()?;
            let request_id = r.u64()?;
            let cursor = r.u32()?;
            let next_cursor = r.option(Reader::u32)?;
            let done = r.bool()?;
            let catalog_revision = r.u64()?;
            let count = r.usize()?;
            if count > MAX_PLUGIN_PARAMETER_PAGE_ITEMS {
                return Err(AppError::Bound);
            }
            let start = r.position;
            for _ in 0..count {
                read_parameter(r)?;
            }
            let encoded = &r.bytes[start..r.position];
            RuntimeBody::ParameterCatalogPage {
                slot,
                request_id,
                cursor,
                next_cursor,
                done,
                catalog_revision,
                items: Catalog { count, encoded },
            }
        }
        5 => RuntimeBody::ParameterSetAck {
            slot: r.usize()?,
            request_id: r.u64()?,
            id: r.u32()?,
            value_bits: r.u32()?,
        },
        6 => RuntimeBody::ParameterCommandFailed {
            slot: r.usize()?,
            request_id: r.u64()?,
            command: match r.u8()? {
                1 => PluginParameterCommand::Catalog,
                2 => PluginParameterCommand::Set,
                3 => PluginParameterCommand::Query,
                _ => return Err(AppError::UnknownCode),
            },
            id: r.option(Reader::u32)?,
            message: r.text()?,
        },
        7 => return Err(AppError::OrdinaryStateRequired),
        8 => RuntimeBody::StateRejected {
            slot: r.usize()?,
            request_id: r.u64()?,
            epoch: r.u64()?,
            reason: match r.u8()? {
                1 => StateCaptureRejectReason::EpochChanged,
                2 => StateCaptureRejectReason::BackendFailure,
                _ => return Err(AppError::UnknownCode),
            },
        },
        9 => RuntimeBody::ShutdownComplete,
        _ => return Err(AppError::UnknownCode),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_bytes(view: AppView<'_>) -> (u16, Vec<u8>) {
        let mut bytes = Vec::with_capacity(MAX_APP_CONTROL_BYTES);
        let pointer = bytes.as_ptr();
        let capacity = bytes.capacity();
        let kind = encode(view, &mut bytes).unwrap();
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes.capacity(), capacity);
        (kind, bytes)
    }
    fn runtime(event: &RuntimeEvent) -> AppView<'_> {
        AppView::Runtime {
            endpoint_id: 41,
            project_session: 43,
            instance_id: 47,
            event,
        }
    }
    fn runtime_body<'a>(bytes: &'a [u8]) -> RuntimeBody<'a> {
        let AppControl::Runtime {
            endpoint_id,
            project_session,
            instance_id,
            event,
        } = decode(bytes, 15).unwrap()
        else {
            panic!("runtime body absent")
        };
        assert_eq!((endpoint_id, project_session, instance_id), (41, 43, 47));
        event
    }
    fn state(phase: TimelinePhase) -> TimelineAppState {
        TimelineAppState {
            phase,
            project_session: 11,
            engine_session: Some(0),
            sync_session: 13,
            desired_generation: 17,
            observed_fingerprint: 19,
            compiled_generation: Some(23),
            compiled_fingerprint: None,
            source_generation: Some(29),
            source_fingerprint: Some(31),
            sync_source_generation: Some(37),
            desired_seek_serial: 41,
            desired_seek_frame: 43,
            loop_token: Some(0),
            loop_disable_request: Some(47),
            retry_count: 53,
            stale_confirmations: 59,
            confirmed_identity: Some((61, 0)),
            retry_required: true,
            retry_pending: Some((0, 67)),
        }
    }
    fn timeline_roundtrip(
        event: TimelineRuntimeEvent,
        before: TimelineAppState,
        after: TimelineAppState,
    ) {
        let (kind, bytes) = encode_bytes(AppView::Timeline {
            event: &event,
            before: &before,
            after: &after,
        });
        assert_eq!(kind, 12);
        assert_eq!(
            decode(&bytes, kind).unwrap(),
            AppControl::Timeline {
                event,
                before,
                after
            }
        );
    }
    fn stamp() -> GeneratorRemovalStamp {
        GeneratorRemovalStamp {
            operation_id: 3,
            engine_session: 5,
            project_session: 7,
            channel_id: 11,
            endpoint_id: 13,
            plugin_instance_id: 17,
            stopped_epoch: 19,
        }
    }
    fn parameter(index: u32) -> PluginParameterDescriptor {
        PluginParameterDescriptor {
            id: index,
            name: format!("参数{index}\0"),
            unit: "dB\n".into(),
            current_normalized: f32::from_bits(0x7fc0_0137 + index),
            default_normalized: Some(-0.0),
            step_count: Some(0),
            automatable: true,
            read_only: false,
            bypass: true,
        }
    }
    fn catalog(items: Vec<PluginParameterDescriptor>) -> RuntimeEvent {
        RuntimeEvent::ParameterCatalogPage {
            slot: 53,
            request_id: 59,
            cursor: 61,
            next_cursor: Some(0),
            done: false,
            catalog_revision: 67,
            items,
        }
    }
    fn assert_parameter(actual: ParameterBits<'_>, expected: &PluginParameterDescriptor) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.name, expected.name);
        assert_eq!(actual.unit, expected.unit);
        assert_eq!(
            actual.current_normalized_bits,
            expected.current_normalized.to_bits()
        );
        assert_eq!(
            actual.default_normalized_bits,
            expected.default_normalized.map(f32::to_bits)
        );
        assert_eq!(actual.step_count, expected.step_count);
        assert_eq!(actual.automatable, expected.automatable);
        assert_eq!(actual.read_only, expected.read_only);
        assert_eq!(actual.bypass, expected.bypass);
    }

    #[test]
    fn app_control_asset_wire_golden_uses_original_identity_fields_and_explicit_codes() {
        let event = AudioAssetEvent::Registered {
            operation: AudioAssetOperation {
                engine_session: 0x0102,
                generation: 0x0304,
                operation_id: 0x0506,
            },
            id: 0x0708,
            success: false,
        };
        let (kind, bytes) = encode_bytes(AppView::AudioAsset(&event));
        let expected = [
            b'N', b'A', b'P', b'P', 1, 0, 10, 0, 1, 2, 1, 0, 0, 0, 0, 0, 0, 4, 3, 0, 0, 0, 0, 0, 0,
            6, 5, 0, 0, 0, 0, 0, 0, 8, 7, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(bytes, expected);
        assert_eq!(kind, 10);
        assert_eq!(
            decode(&expected, 10).unwrap(),
            AppControl::AudioAsset(event)
        );
    }
    #[test]
    fn app_control_all_asset_variants_preserve_removed_count_and_false_receipts() {
        let operation = AudioAssetOperation {
            engine_session: 0,
            generation: u64::MAX,
            operation_id: 71,
        };
        for event in [
            AudioAssetEvent::Registered {
                operation,
                id: 73,
                success: true,
            },
            AudioAssetEvent::Unregistered {
                operation,
                id: 79,
                removed: false,
            },
            AudioAssetEvent::Cleared {
                operation,
                removed: usize::MAX,
            },
        ] {
            let (kind, bytes) = encode_bytes(AppView::AudioAsset(&event));
            assert_eq!(decode(&bytes, kind).unwrap(), AppControl::AudioAsset(event));
        }
    }
    #[test]
    fn app_control_all_generator_variants_and_optional_zero_ids_are_distinct() {
        for optional in [None, Some(0), Some(u64::MAX)] {
            for event in [
                GeneratorEndpointEvent::Installed {
                    channel_id: 83,
                    endpoint_id: 89,
                    plugin_instance_id: 97,
                    mixer_track: usize::MAX,
                    replaced_endpoint_id: optional,
                    replaced_plugin_instance_id: optional,
                    success: false,
                },
                GeneratorEndpointEvent::Removed {
                    channel_id: 101,
                    endpoint_id: optional,
                    plugin_instance_id: optional,
                },
                GeneratorEndpointEvent::StampedRemoved {
                    stamp: stamp(),
                    result: Ok(()),
                },
                GeneratorEndpointEvent::Cleared {
                    request_id: 103,
                    removed: usize::MAX,
                },
                GeneratorEndpointEvent::RouteSet {
                    channel_id: 107,
                    endpoint_id: optional,
                    plugin_instance_id: optional,
                    mixer_track: 109,
                    success: true,
                },
            ] {
                let (kind, bytes) = encode_bytes(AppView::GeneratorEndpoint(&event));
                assert_eq!(kind, 11);
                assert_eq!(
                    decode(&bytes, kind).unwrap(),
                    AppControl::GeneratorEndpoint(event)
                );
            }
        }
    }
    #[test]
    fn app_control_all_thirteen_removal_reasons_preserve_full_stamp_and_refusal() {
        for reason in [
            GeneratorRemovalRejectReason::InvalidStamp,
            GeneratorRemovalRejectReason::EngineSessionChanged,
            GeneratorRemovalRejectReason::ProjectSessionChanged,
            GeneratorRemovalRejectReason::TransportRunning,
            GeneratorRemovalRejectReason::TransportRequestPending,
            GeneratorRemovalRejectReason::TimelineTransitionPending,
            GeneratorRemovalRejectReason::StoppedEpochChanged,
            GeneratorRemovalRejectReason::MissingEndpoint,
            GeneratorRemovalRejectReason::EndpointChanged,
            GeneratorRemovalRejectReason::PluginInstanceChanged,
            GeneratorRemovalRejectReason::EndpointProjectChanged,
            GeneratorRemovalRejectReason::EndpointEpochChanged,
            GeneratorRemovalRejectReason::EndpointManifestChanged,
        ] {
            let event = GeneratorEndpointEvent::StampedRemoved {
                stamp: stamp(),
                result: Err(reason),
            };
            let (kind, bytes) = encode_bytes(AppView::GeneratorEndpoint(&event));
            assert_eq!(
                decode(&bytes, kind).unwrap(),
                AppControl::GeneratorEndpoint(event)
            );
        }
        for code in [0, 14, 255] {
            assert_eq!(read_removal(code), Err(AppError::UnknownCode));
        }
    }
    #[test]
    fn app_control_all_timeline_events_preserve_request_revision_and_payload_domains() {
        for event in [
            TimelineRuntimeEvent::Installed {
                request_id: 2,
                revision: 3,
            },
            TimelineRuntimeEvent::ChaseInstalled {
                request_id: 5,
                revision: 7,
                epoch: 11,
                frame: 13,
            },
            TimelineRuntimeEvent::LoopChaseInstalled {
                request_id: 17,
                revision: 19,
                frame: 23,
                token: 0,
            },
            TimelineRuntimeEvent::TransportActivationApplied {
                request_id: 29,
                revision: 31,
                epoch: 37,
                frame: 41,
            },
            TimelineRuntimeEvent::TransportActivationRejected {
                request_id: 43,
                revision: 47,
                reason: TimelineTransportActivationRejectReason::MissingAudioAsset { asset_id: 53 },
            },
            TimelineRuntimeEvent::Cleared {
                request_id: 59,
                revision: 61,
            },
            TimelineRuntimeEvent::Rejected {
                request_id: 67,
                revision: 71,
                reason: TimelineRejectReason::InvalidRevision,
            },
            TimelineRuntimeEvent::ShutdownComplete {
                request_id: 73,
                revision: u64::MAX,
            },
        ] {
            timeline_roundtrip(
                event,
                state(TimelinePhase::SeekReady),
                state(TimelinePhase::Faulted),
            );
        }
    }
    #[test]
    fn app_control_every_timeline_refusal_keeps_optional_revision_zero_and_fingerprint() {
        for reason in [
            TimelineRejectReason::InvalidRevision,
            TimelineRejectReason::StaleRevision {
                latest_revision: u64::MAX,
            },
            TimelineRejectReason::RevisionMismatch {
                active_revision: None,
            },
            TimelineRejectReason::RevisionMismatch {
                active_revision: Some(0),
            },
            TimelineRejectReason::StaleEpoch { latest_epoch: 0 },
            TimelineRejectReason::InvalidChaseFrame,
            TimelineRejectReason::ChasePendingDelivery,
            TimelineRejectReason::MixerDelayBankReuseUnavailable { fingerprint: 89 },
        ] {
            timeline_roundtrip(
                TimelineRuntimeEvent::Rejected {
                    request_id: 97,
                    revision: 101,
                    reason,
                },
                state(TimelinePhase::InstallReady),
                state(TimelinePhase::Faulted),
            );
        }
    }
    #[test]
    fn app_control_all_activation_refusals_and_nested_runtime_errors_are_exact() {
        use TimelineDiscontinuityActivationError as E;
        let nested = [
            E::MissingTimeline,
            E::Shutdown,
            E::InvalidEpoch,
            E::NeedsResync,
            E::RevisionMismatch {
                active: 2,
                requested: 3,
            },
            E::MissingPrepared {
                kind: TimelineDiscontinuityKind::OneShot,
            },
            E::MissingPrepared {
                kind: TimelineDiscontinuityKind::Loop { token: 0 },
            },
            E::OneShotEpochMismatch {
                expected: 5,
                requested: 7,
            },
            E::StaleEpoch {
                latest: 11,
                requested: 13,
            },
            E::FrameMismatch {
                expected: 17,
                requested: 19,
            },
            E::OneShotConsumed,
            E::LoopTokenMismatch {
                expected: 23,
                requested: 29,
            },
            E::ChunkInProgress,
            E::IncompleteCandidateBundle,
        ];
        for error in nested {
            timeline_roundtrip(
                TimelineRuntimeEvent::TransportActivationRejected {
                    request_id: 31,
                    revision: 37,
                    reason: TimelineTransportActivationRejectReason::Runtime(error),
                },
                state(TimelinePhase::Active {
                    generation: 41,
                    epoch: 43,
                }),
                state(TimelinePhase::Faulted),
            );
        }
        use TimelineTransportActivationRejectReason as R;
        for reason in [
            R::PlaybackPreparationRequired,
            R::MissingAudioAsset { asset_id: u64::MAX },
            R::ChannelBaseTable,
            R::GeneratorRouteBinding,
            R::MixerGraphPlan,
            R::GraphPdcPlan,
            R::MixerDelayBank,
            R::PluginAutomationBinding,
            R::ExecutorState,
            R::RenderPlanCapacity,
            R::EndpointEventCapacity,
        ] {
            timeline_roundtrip(
                TimelineRuntimeEvent::TransportActivationRejected {
                    request_id: 47,
                    revision: 53,
                    reason,
                },
                state(TimelinePhase::SeekReady),
                state(TimelinePhase::Faulted),
            );
        }
    }
    #[test]
    fn app_control_all_fourteen_phases_keep_distinct_operands_and_complete_before_after() {
        let phases = [
            TimelinePhase::Unavailable,
            TimelinePhase::InstallReady,
            TimelinePhase::InstallQueued {
                request_id: 2,
                generation: 3,
            },
            TimelinePhase::LoopReady,
            TimelinePhase::LoopQueued {
                request_id: 5,
                generation: 7,
                frame: 11,
            },
            TimelinePhase::SeekReady,
            TimelinePhase::SeekQueued {
                request_id: 13,
                generation: 17,
                seek_serial: 19,
                epoch: 23,
                frame: 29,
            },
            TimelinePhase::AwaitingActivation {
                generation: 31,
                seek_serial: 37,
                epoch: 41,
                frame: 43,
                activation_request_id: 47,
            },
            TimelinePhase::Active {
                generation: 53,
                epoch: 59,
            },
            TimelinePhase::AssetHandoffClearReady { revision: 61 },
            TimelinePhase::AssetHandoffClearQueued {
                request_id: 67,
                revision: 71,
            },
            TimelinePhase::FaultClearReady { revision: 73 },
            TimelinePhase::FaultClearQueued {
                request_id: 79,
                revision: 83,
            },
            TimelinePhase::Faulted,
        ];
        for phase in phases {
            let before = state(phase);
            let mut after = state(TimelinePhase::Unavailable);
            after.project_session = u64::MAX;
            after.engine_session = None;
            after.sync_session = 101;
            after.desired_generation = 103;
            after.observed_fingerprint = 107;
            after.compiled_generation = None;
            after.compiled_fingerprint = Some(0);
            after.source_generation = None;
            after.source_fingerprint = None;
            after.sync_source_generation = None;
            after.desired_seek_serial = 109;
            after.desired_seek_frame = 113;
            after.loop_token = None;
            after.loop_disable_request = None;
            after.retry_count = 127;
            after.stale_confirmations = 131;
            after.confirmed_identity = None;
            after.retry_required = false;
            after.retry_pending = None;
            timeline_roundtrip(
                TimelineRuntimeEvent::Installed {
                    request_id: 137,
                    revision: 139,
                },
                before,
                after,
            );
        }
    }
    #[test]
    fn app_control_runtime_slot_names_faults_and_latency_unknown_tail_are_borrowed() {
        let ready = RuntimeEvent::SlotReady {
            slot: usize::MAX,
            name: "Kontakt 8\0声音".into(),
            latency_samples: 0,
            tail_samples: u32::MAX,
        };
        let (_, bytes) = encode_bytes(runtime(&ready));
        let RuntimeBody::SlotReady {
            slot,
            name,
            latency_samples,
            tail_samples,
        } = runtime_body(&bytes)
        else {
            panic!("ready absent")
        };
        assert_eq!(slot, usize::MAX);
        assert_eq!(name, "Kontakt 8\0声音");
        assert_eq!(latency_samples, 0);
        assert_eq!(tail_samples, u32::MAX);
        let pointer = name.as_ptr() as usize;
        assert!(
            pointer >= bytes.as_ptr() as usize
                && pointer + name.len() <= bytes.as_ptr() as usize + bytes.len()
        );
        let fault = RuntimeEvent::SlotFault {
            slot: 149,
            message: "SDK failed\nwithout rollback".into(),
        };
        let (_, bytes) = encode_bytes(runtime(&fault));
        assert_eq!(
            runtime_body(&bytes),
            RuntimeBody::SlotFault {
                slot: 149,
                message: "SDK failed\nwithout rollback"
            }
        );
    }
    #[test]
    fn app_control_parameter_value_and_set_ack_preserve_ieee_bits_without_sanitizing() {
        for bits in [0, 0x8000_0000, 0x7fc0_0137, 0x7f80_0000, 0xff80_0000] {
            let value = f32::from_bits(bits);
            for event in [
                RuntimeEvent::ParameterValue {
                    slot: 2,
                    request_id: 3,
                    id: 5,
                    value,
                },
                RuntimeEvent::ParameterSetAck {
                    slot: 7,
                    request_id: 11,
                    id: 13,
                    value,
                },
            ] {
                let (_, bytes) = encode_bytes(runtime(&event));
                match runtime_body(&bytes) {
                    RuntimeBody::ParameterValue {
                        slot,
                        request_id,
                        id,
                        value_bits,
                    } => {
                        assert_eq!((slot, request_id, id), (2, 3, 5));
                        assert_eq!(value_bits, bits);
                    }
                    RuntimeBody::ParameterSetAck {
                        slot,
                        request_id,
                        id,
                        value_bits,
                    } => {
                        assert_eq!((slot, request_id, id), (7, 11, 13));
                        assert_eq!(value_bits, bits);
                    }
                    _ => panic!("parameter receipt kind changed"),
                }
            }
        }
    }
    #[test]
    fn app_control_catalog_preserves_every_item_field_and_complete_page_identity() {
        let mut second = parameter(173);
        second.default_normalized = None;
        second.step_count = None;
        second.automatable = false;
        second.read_only = true;
        second.bypass = false;
        let expected = vec![parameter(167), second];
        let event = catalog(expected.clone());
        let (_, bytes) = encode_bytes(runtime(&event));
        let RuntimeBody::ParameterCatalogPage {
            slot,
            request_id,
            cursor,
            next_cursor,
            done,
            catalog_revision,
            items,
        } = runtime_body(&bytes)
        else {
            panic!("catalog absent")
        };
        assert_eq!(
            (
                slot,
                request_id,
                cursor,
                next_cursor,
                done,
                catalog_revision
            ),
            (53, 59, 61, Some(0), false, 67)
        );
        assert_eq!(items.len(), 2);
        assert!(!items.is_empty());
        let mut count = 0;
        for (actual, expected) in items.items().zip(&expected) {
            assert_parameter(actual.unwrap(), expected);
            count += 1;
        }
        assert_eq!(count, 2);
        for actual in items.items() {
            let actual = actual.unwrap();
            for text in [actual.name, actual.unit] {
                let pointer = text.as_ptr() as usize;
                assert!(
                    pointer >= bytes.as_ptr() as usize
                        && pointer + text.len() <= bytes.as_ptr() as usize + bytes.len()
                );
            }
        }
    }
    #[test]
    fn app_control_catalog_empty_maximum_count_and_none_cursor_remain_distinct() {
        for count in [0, MAX_PLUGIN_PARAMETER_PAGE_ITEMS] {
            let expected: Vec<_> = (0..count).map(|index| parameter(index as u32)).collect();
            let mut event = catalog(expected.clone());
            let RuntimeEvent::ParameterCatalogPage {
                next_cursor, done, ..
            } = &mut event
            else {
                panic!("catalog absent")
            };
            *next_cursor = None;
            *done = true;
            let (_, bytes) = encode_bytes(runtime(&event));
            let RuntimeBody::ParameterCatalogPage {
                next_cursor,
                done,
                items,
                ..
            } = runtime_body(&bytes)
            else {
                panic!("catalog absent")
            };
            assert_eq!(next_cursor, None);
            assert!(done);
            assert_eq!(items.len(), count);
            assert_eq!(items.is_empty(), count == 0);
            assert_eq!(items.items().count(), count);
            for (actual, expected) in items.items().zip(&expected) {
                assert_parameter(actual.unwrap(), expected);
            }
        }
    }
    #[test]
    fn app_control_catalog_default_float_and_step_options_preserve_all_original_bits() {
        for default_bits in [
            None,
            Some(0_u32),
            Some(0x8000_0000),
            Some(0x7fc0_0137),
            Some(0x7f80_0000),
        ] {
            for step_count in [None, Some(0), Some(u32::MAX)] {
                let mut expected = parameter(177);
                expected.default_normalized = default_bits.map(f32::from_bits);
                expected.step_count = step_count;
                let event = catalog(vec![expected.clone()]);
                let (_, bytes) = encode_bytes(runtime(&event));
                let RuntimeBody::ParameterCatalogPage { items, .. } = runtime_body(&bytes) else {
                    panic!("catalog absent")
                };
                let actual = items.items().next().unwrap().unwrap();
                assert_parameter(actual, &expected);
                assert_eq!(actual.default_normalized_bits, default_bits);
                assert_eq!(actual.step_count, step_count);
            }
        }
    }
    #[test]
    fn app_control_all_parameter_commands_preserve_optional_id_zero_and_message() {
        for command in [
            PluginParameterCommand::Catalog,
            PluginParameterCommand::Set,
            PluginParameterCommand::Query,
        ] {
            for id in [None, Some(0), Some(u32::MAX)] {
                let event = RuntimeEvent::ParameterCommandFailed {
                    slot: 179,
                    request_id: 181,
                    command,
                    id,
                    message: "preserved\0failure".into(),
                };
                let (_, bytes) = encode_bytes(runtime(&event));
                assert_eq!(
                    runtime_body(&bytes),
                    RuntimeBody::ParameterCommandFailed {
                        slot: 179,
                        request_id: 181,
                        command,
                        id,
                        message: "preserved\0failure"
                    }
                );
            }
        }
    }
    #[test]
    fn app_control_state_rejections_and_shutdown_keep_actual_refusal_variants() {
        for reason in [
            StateCaptureRejectReason::EpochChanged,
            StateCaptureRejectReason::BackendFailure,
        ] {
            let event = RuntimeEvent::StateRejected {
                slot: 191,
                request_id: 193,
                epoch: 197,
                reason,
            };
            let (_, bytes) = encode_bytes(runtime(&event));
            assert_eq!(
                runtime_body(&bytes),
                RuntimeBody::StateRejected {
                    slot: 191,
                    request_id: 193,
                    epoch: 197,
                    reason
                }
            );
        }
        let (_, bytes) = encode_bytes(runtime(&RuntimeEvent::ShutdownComplete));
        assert_eq!(runtime_body(&bytes), RuntimeBody::ShutdownComplete);
        assert_eq!(
            bytes,
            [
                b'N', b'A', b'P', b'P', 1, 0, 15, 0, 41, 0, 0, 0, 0, 0, 0, 0, 43, 0, 0, 0, 0, 0, 0,
                0, 47, 0, 0, 0, 0, 0, 0, 0, 9
            ]
        );
    }
    #[test]
    fn app_control_runtime_state_follows_original_length_only_tap_without_copying_bytes() {
        for epoch in [None, Some(0), Some(u64::MAX)] {
            let event = RuntimeEvent::State {
                slot: 199,
                request_id: 211,
                epoch,
                bytes: vec![0xaa; 591772],
            };
            let RuntimeTap::OrdinaryState(observed) = runtime_tap(223, 227, 229, &event) else {
                panic!("ordinary length absent")
            };
            assert_eq!(
                observed,
                OrdinaryStateLength {
                    endpoint_id: 223,
                    project_session: 227,
                    instance_id: 229,
                    slot: 199,
                    request_id: 211,
                    epoch,
                    byte_len: 591772
                }
            );
            let mut destination = Vec::with_capacity(MAX_APP_CONTROL_BYTES);
            assert_eq!(
                encode(runtime(&event), &mut destination),
                Err(AppError::OrdinaryStateRequired)
            );
            assert!(destination.is_empty());
        }
        let event = RuntimeEvent::ShutdownComplete;
        assert!(matches!(
            runtime_tap(0, 0, 0, &event),
            RuntimeTap::Retained(_)
        ));
    }
    #[test]
    fn app_control_retained_record_adapter_accepts_only_real_app_kinds() {
        let record = RecordKind::AppAudioAssetEvent(AudioAssetEvent::Cleared {
            operation: AudioAssetOperation {
                engine_session: 2,
                generation: 3,
                operation_id: 5,
            },
            removed: 7,
        });
        assert_eq!(view_record(&record).unwrap().kind(), 10);
        let record =
            RecordKind::AppGeneratorEndpointEvent(GeneratorEndpointEvent::StampedRemoved {
                stamp: stamp(),
                result: Err(GeneratorRemovalRejectReason::TransportRunning),
            });
        assert_eq!(view_record(&record).unwrap().kind(), 11);
        let record = RecordKind::AppTimelineRuntimeEvent {
            event: TimelineRuntimeEvent::Installed {
                request_id: 11,
                revision: 13,
            },
            before: state(TimelinePhase::InstallReady),
            after: state(TimelinePhase::Active {
                generation: 17,
                epoch: 19,
            }),
        };
        assert_eq!(view_record(&record).unwrap().kind(), 12);
        let record = RecordKind::RuntimeEvent {
            endpoint_id: 23,
            project_session: 29,
            instance_id: 31,
            event: RuntimeEvent::State {
                slot: 0,
                request_id: 37,
                epoch: None,
                bytes: vec![0x41],
            },
        };
        assert!(matches!(
            view_record(&record),
            Err(AppError::OrdinaryStateRequired)
        ));
        assert!(matches!(
            view_record(&RecordKind::Lifecycle("real lifecycle")),
            Err(AppError::UnsupportedKind)
        ));
    }
    #[test]
    fn app_control_every_truncated_body_and_trailing_byte_refuse() {
        let generator = GeneratorEndpointEvent::Installed {
            channel_id: 2,
            endpoint_id: 3,
            plugin_instance_id: 5,
            mixer_track: 7,
            replaced_endpoint_id: Some(11),
            replaced_plugin_instance_id: Some(0),
            success: false,
        };
        let timeline = TimelineRuntimeEvent::TransportActivationRejected {
            request_id: 13,
            revision: 17,
            reason: TimelineTransportActivationRejectReason::Runtime(
                TimelineDiscontinuityActivationError::MissingPrepared {
                    kind: TimelineDiscontinuityKind::Loop { token: 19 },
                },
            ),
        };
        let before = state(TimelinePhase::AwaitingActivation {
            generation: 23,
            seek_serial: 29,
            epoch: 31,
            frame: 37,
            activation_request_id: 41,
        });
        let after = state(TimelinePhase::Faulted);
        let event = catalog(vec![parameter(43), parameter(47)]);
        for (kind, bytes) in [
            encode_bytes(AppView::GeneratorEndpoint(&generator)),
            encode_bytes(AppView::Timeline {
                event: &timeline,
                before: &before,
                after: &after,
            }),
            encode_bytes(runtime(&event)),
        ] {
            for end in 0..bytes.len() {
                assert!(
                    decode(&bytes[..end], kind).is_err(),
                    "kind {kind}, prefix {end} accepted"
                );
            }
            let mut extra = bytes;
            extra.push(0);
            assert_eq!(decode(&extra, kind), Err(AppError::TrailingBytes));
        }
    }
    #[test]
    fn app_control_unknown_magic_version_kind_event_and_bool_are_closed_refusals() {
        let event = AudioAssetEvent::Registered {
            operation: AudioAssetOperation {
                engine_session: 2,
                generation: 3,
                operation_id: 5,
            },
            id: 7,
            success: false,
        };
        let (kind, bytes) = encode_bytes(AppView::AudioAsset(&event));
        for (offset, value, expected) in [
            (0, 0, AppError::InvalidMagic),
            (4, 2, AppError::UnknownVersion),
            (8, 4, AppError::UnknownCode),
            (41, 2, AppError::InvalidBoolean),
        ] {
            let mut changed = bytes.clone();
            changed[offset] = value;
            assert_eq!(decode(&changed, kind), Err(expected));
        }
        assert_eq!(decode(&bytes, kind + 1), Err(AppError::KindMismatch));
        let mut changed = bytes;
        changed[6..8].copy_from_slice(&65535_u16.to_le_bytes());
        assert_eq!(decode(&changed, 65535), Err(AppError::UnknownCode));
    }
    #[test]
    fn app_control_unknown_nested_reason_phase_discontinuity_command_and_state_code_reject() {
        fn reject(wire: &[u8], read: impl FnOnce(&mut Reader<'_>) -> Result<(), AppError>) {
            assert_eq!(
                read(&mut Reader {
                    bytes: wire,
                    position: 0
                }),
                Err(AppError::UnknownCode)
            );
        }
        for code in [0, 255] {
            reject(&[code], |r| read_generator(r).map(|_| ()));
            reject(&[code], |r| read_timeline(r).map(|_| ()));
            reject(&[code], |r| read_timeline_reject(r).map(|_| ()));
            reject(&[code], |r| read_activation_reject(r).map(|_| ()));
            reject(&[code], |r| read_activation_error(r).map(|_| ()));
            reject(&[code], |r| read_discontinuity_kind(r).map(|_| ()));
            reject(&[code], |r| read_phase(r).map(|_| ()));
            reject(&[code], |r| read_runtime(r).map(|_| ()));
        }
        assert_eq!(
            read_runtime(&mut Reader {
                bytes: &[7],
                position: 0
            }),
            Err(AppError::OrdinaryStateRequired)
        );
        let command = RuntimeEvent::ParameterCommandFailed {
            slot: 2,
            request_id: 3,
            command: PluginParameterCommand::Set,
            id: None,
            message: String::new(),
        };
        let (kind, mut bytes) = encode_bytes(runtime(&command));
        bytes[49] = 4;
        assert_eq!(decode(&bytes, kind), Err(AppError::UnknownCode));
        let rejected = RuntimeEvent::StateRejected {
            slot: 5,
            request_id: 7,
            epoch: 11,
            reason: StateCaptureRejectReason::EpochChanged,
        };
        let (kind, mut bytes) = encode_bytes(runtime(&rejected));
        bytes[57] = 3;
        assert_eq!(decode(&bytes, kind), Err(AppError::UnknownCode));
    }
    #[test]
    fn app_control_invalid_option_flags_and_utf8_fail_before_publication() {
        let event = GeneratorEndpointEvent::Removed {
            channel_id: 2,
            endpoint_id: None,
            plugin_instance_id: None,
        };
        let (kind, mut bytes) = encode_bytes(AppView::GeneratorEndpoint(&event));
        bytes[13] = 2;
        assert_eq!(decode(&bytes, kind), Err(AppError::InvalidOption));
        let event = RuntimeEvent::SlotFault {
            slot: 3,
            message: "x".into(),
        };
        let (kind, mut bytes) = encode_bytes(runtime(&event));
        bytes[45] = 0xff;
        assert_eq!(decode(&bytes, kind), Err(AppError::InvalidUtf8));
        bytes[41..45].copy_from_slice(&((MAX_TEXT + 1) as u32).to_le_bytes());
        assert_eq!(decode(&bytes, kind), Err(AppError::Bound));
        let event = catalog(Vec::new());
        let (kind, mut bytes) = encode_bytes(runtime(&event));
        bytes[53] = 2;
        assert_eq!(decode(&bytes, kind), Err(AppError::InvalidOption));
    }
    #[test]
    fn app_control_catalog_limit_and_malformed_final_item_are_not_partial_success() {
        let event = catalog(
            (0..=MAX_PLUGIN_PARAMETER_PAGE_ITEMS)
                .map(|index| parameter(index as u32))
                .collect(),
        );
        let mut destination = Vec::with_capacity(MAX_APP_CONTROL_BYTES);
        assert_eq!(
            encode(runtime(&event), &mut destination),
            Err(AppError::Bound)
        );
        assert!(destination.is_empty());
        let event = catalog(vec![parameter(2)]);
        let (kind, mut bytes) = encode_bytes(runtime(&event));
        // Some(next_cursor) adds four bytes to the page prefix: count is 67..75.
        bytes[67..75]
            .copy_from_slice(&((MAX_PLUGIN_PARAMETER_PAGE_ITEMS + 1) as u64).to_le_bytes());
        assert_eq!(decode(&bytes, kind), Err(AppError::Bound));
        let event = catalog(vec![parameter(3)]);
        let (kind, mut bytes) = encode_bytes(runtime(&event));
        let last = bytes.len() - 1;
        bytes[last] = 2;
        assert_eq!(decode(&bytes, kind), Err(AppError::InvalidBoolean));
    }
    #[test]
    fn app_control_exact_64k_body_and_one_extra_byte_enforce_total_budget() {
        let mut items = Vec::new();
        for index in 0..8 {
            items.push(PluginParameterDescriptor {
                id: index,
                name: "n".repeat(MAX_TEXT),
                unit: "u".repeat(MAX_TEXT),
                current_normalized: 0.0,
                default_normalized: None,
                step_count: None,
                automatable: false,
                read_only: false,
                bypass: false,
            });
        }
        // Header8 + outer24 + page39 + items8*(21+8192) = 65775.
        // Remove239 bytes from the final unit to produce exactly65536 bytes.
        items[7].unit.truncate(MAX_TEXT - 239);
        let mut event = RuntimeEvent::ParameterCatalogPage {
            slot: 0,
            request_id: 0,
            cursor: 0,
            next_cursor: None,
            done: false,
            catalog_revision: 0,
            items,
        };
        let (_, bytes) = encode_bytes(runtime(&event));
        assert_eq!(bytes.len(), MAX_APP_CONTROL_BYTES);
        let RuntimeBody::ParameterCatalogPage { items, .. } = runtime_body(&bytes) else {
            panic!("page absent")
        };
        assert_eq!(items.items().count(), 8);
        // A complete 64KiB typed body cannot share a 64KiB operand with any
        // nonempty envelope. The parent must pass the remaining body budget.
        let mut limited = Vec::with_capacity(MAX_APP_CONTROL_BYTES);
        let pointer = limited.as_ptr();
        let capacity = limited.capacity();
        for reserved_header_bytes in [1, 64] {
            assert_eq!(
                encode_with_limit(
                    runtime(&event),
                    &mut limited,
                    MAX_APP_CONTROL_BYTES - reserved_header_bytes,
                ),
                Err(AppError::Bound)
            );
            assert!(limited.is_empty());
            assert_eq!(limited.as_ptr(), pointer);
            assert_eq!(limited.capacity(), capacity);
        }
        let RuntimeEvent::ParameterCatalogPage { items, .. } = &mut event else {
            panic!("page absent")
        };
        items[7].unit.push('u');
        let mut destination = Vec::with_capacity(MAX_APP_CONTROL_BYTES);
        let pointer = destination.as_ptr();
        let capacity = destination.capacity();
        assert_eq!(
            encode(runtime(&event), &mut destination),
            Err(AppError::Bound)
        );
        assert!(destination.is_empty());
        assert_eq!(destination.as_ptr(), pointer);
        assert_eq!(destination.capacity(), capacity);
        assert_eq!(
            decode(&vec![0; MAX_APP_CONTROL_BYTES + 1], 15),
            Err(AppError::Bound)
        );
    }
    #[test]
    fn app_control_individual_text_limits_count_utf8_bytes_and_retain_exact_boundary() {
        let event = RuntimeEvent::SlotFault {
            slot: 0,
            message: "é".repeat(MAX_TEXT / 2),
        };
        let (_, bytes) = encode_bytes(runtime(&event));
        let RuntimeBody::SlotFault { message, .. } = runtime_body(&bytes) else {
            panic!("fault absent")
        };
        assert_eq!(message.len(), MAX_TEXT);
        let event = RuntimeEvent::SlotFault {
            slot: 0,
            message: "é".repeat(MAX_TEXT / 2 + 1),
        };
        let mut destination = Vec::with_capacity(MAX_APP_CONTROL_BYTES);
        assert_eq!(
            encode(runtime(&event), &mut destination),
            Err(AppError::Bound)
        );
        assert!(destination.is_empty());
    }
    #[test]
    fn app_control_destination_rejection_preserves_occupied_bytes_and_never_reallocates() {
        let event = RuntimeEvent::ShutdownComplete;
        for capacity in [0, 8, 32] {
            let mut destination = Vec::with_capacity(capacity);
            let pointer = destination.as_ptr();
            let actual_capacity = destination.capacity();
            assert_eq!(
                encode(runtime(&event), &mut destination),
                Err(AppError::DestinationCapacity)
            );
            assert!(destination.is_empty());
            assert_eq!(destination.as_ptr(), pointer);
            assert_eq!(destination.capacity(), actual_capacity);
        }
        let mut excessive = Vec::with_capacity(MAX_APP_CONTROL_BYTES + 1);
        assert_eq!(
            encode(runtime(&event), &mut excessive),
            Err(AppError::DestinationCapacity)
        );
        let mut occupied = vec![0x11, 0x13];
        let pointer = occupied.as_ptr();
        assert_eq!(
            encode(runtime(&event), &mut occupied),
            Err(AppError::DestinationNotEmpty)
        );
        assert_eq!(occupied, [0x11, 0x13]);
        assert_eq!(occupied.as_ptr(), pointer);
    }

    #[test]
    fn app_control_caller_budget_keeps_all_catalog_fields_bits_and_some_zero() {
        let event = catalog(vec![parameter(7)]);
        let (kind, original) = encode_bytes(runtime(&event));
        let mut limited = Vec::with_capacity(MAX_APP_CONTROL_BYTES);
        let pointer = limited.as_ptr();
        let capacity = limited.capacity();
        assert_eq!(
            encode_with_limit(runtime(&event), &mut limited, original.len()),
            Ok(kind)
        );
        assert_eq!(limited, original);
        assert_eq!(runtime_body(&limited), runtime_body(&original));
        let RuntimeBody::ParameterCatalogPage {
            next_cursor, items, ..
        } = runtime_body(&limited)
        else {
            panic!("bounded catalog lost");
        };
        assert_eq!(next_cursor, Some(0));
        assert_parameter(items.items().next().unwrap().unwrap(), &parameter(7));
        assert_eq!(limited.as_ptr(), pointer);
        assert_eq!(limited.capacity(), capacity);
        limited.clear();
        assert_eq!(
            encode_with_limit(runtime(&event), &mut limited, original.len() - 1),
            Err(AppError::Bound)
        );
        assert!(limited.is_empty());
        assert_eq!(limited.as_ptr(), pointer);
        assert_eq!(limited.capacity(), capacity);
    }

    #[test]
    fn app_control_budget_rejects_invalid_or_header_only_limits_without_allocation() {
        let event = RuntimeEvent::ShutdownComplete;
        let mut destination = Vec::with_capacity(MAX_APP_CONTROL_BYTES);
        let pointer = destination.as_ptr();
        let capacity = destination.capacity();
        for budget in [0, 7, MAX_APP_CONTROL_BYTES + 1, usize::MAX] {
            assert_eq!(
                encode_with_limit(runtime(&event), &mut destination, budget),
                Err(AppError::Bound)
            );
            assert!(destination.is_empty());
            assert_eq!(destination.as_ptr(), pointer);
            assert_eq!(destination.capacity(), capacity);
        }
        destination.extend_from_slice(&[0x11, 0x13]);
        assert_eq!(
            encode_with_limit(runtime(&event), &mut destination, usize::MAX),
            Err(AppError::DestinationNotEmpty)
        );
        assert_eq!(destination, [0x11, 0x13]);
    }
}
