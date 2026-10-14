//! Pure bounded canonical observation codec. No SDK, filesystem or hashing.
//!
//! Baseline references are selected only by exact field equality in the encoder.
//! The decoder resolves the fixed dictionary, reconstructs owned values, and
//! recomputes equality; no transmitted "validated" flag exists. Dictionary/event/
//! raw-file cryptographic binding and all protocol/control semantics belong to
//! the caller. The historical PCM descriptor refers to a separately
//! acknowledged raw archive; the borrowed native lane retains PCM inline.

use std::fmt;

pub(super) const VERSION: u8 = 1;
pub(super) const MAX_ENCODED_BYTES: usize = 64 * 1024;
pub(super) const MAX_TEXT_BYTES: usize = 4_096;
pub(super) const MAX_METADATA_TEXT_BYTES: usize = 4 * MAX_TEXT_BYTES;
// Four individually bounded metadata fields plus one individually bounded
// site. This local pure-codec limit does not alter any default observer cap.
pub(super) const MAX_TOTAL_TEXT_BYTES: usize = MAX_METADATA_TEXT_BYTES + MAX_TEXT_BYTES;
pub(super) const MAX_BUSES: usize = 32;
pub(super) const HEADER_BYTES: usize = 31;

// Stable wire codes. Never cast the application's Rust Stage enum.
pub(super) const STAGE_PREFLIGHT: u8 = 0;
pub(super) const STAGE_INITIAL_SOURCE: u8 = 1;
pub(super) const STAGE_SOURCE_AUDIBILITY: u8 = 2;
pub(super) const STAGE_SOURCE_STOP: u8 = 3;
pub(super) const STAGE_CONVERSION: u8 = 4;
pub(super) const STAGE_FIRST_PLAYBACK: u8 = 5;
pub(super) const STAGE_UNDO: u8 = 6;
pub(super) const STAGE_REDO: u8 = 7;
pub(super) const STAGE_SECOND_PLAYBACK: u8 = 8;
pub(super) const STAGE_WHOLE_EXPORT: u8 = 9;
pub(super) const STAGE_CLEANUP: u8 = 10;
pub(super) const STAGE_FINISHED: u8 = 11;

pub(super) const METADATA_SITE_ASSEMBLED: &str = "StrictBackend assembled actual metadata";
pub(super) const METADATA_SITE_PREPARE: &str = "StrictBackend prepare result";
pub(super) const METADATA_SITE_RESULT: &str = "StrictBackend metadata result";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Header {
    pub(super) global_seq: u64,
    pub(super) monotonic_ns: u64,
    pub(super) stage: u8,
    /// Run-owned instance ordinal; zero encodes an absent instance.
    pub(super) instance: u64,
    pub(super) tag: u8,
    pub(super) version: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Boundary {
    Attempt,
    Returned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    None,
    Ok,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AudioBus {
    pub(super) channel_count: u32,
    pub(super) is_main: bool,
    pub(super) active: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EventBus {
    pub(super) channel_count: u32,
    pub(super) is_main: bool,
    pub(super) active: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AudioLayout {
    pub(super) inputs: Vec<AudioBus>,
    pub(super) outputs: Vec<AudioBus>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct EventLayout {
    pub(super) inputs: Vec<EventBus>,
    pub(super) outputs: Vec<EventBus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CanonicalMetadata {
    pub(super) class_uid: String,
    pub(super) version: String,
    pub(super) name: String,
    pub(super) category: String,
    pub(super) audio: AudioLayout,
    pub(super) events: EventLayout,
    pub(super) latency_samples: u32,
    pub(super) tail_samples: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MetadataResult {
    Ok(CanonicalMetadata),
    Error(String),
}

/// All floating values retain IEEE bits, including NaN payloads and signed zero.
/// Finiteness and analytical transport equality are the protocol fold's duty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TransportBits {
    pub(super) sample_position: i64,
    pub(super) quarter_note_bits: u64,
    pub(super) tempo_bits: u64,
    pub(super) playing: bool,
    pub(super) time_sig_numerator: i32,
    pub(super) time_sig_denominator: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PcmDescriptor {
    pub(super) frames: u64,
    pub(super) channel_lengths: Vec<u64>,
    pub(super) transport: TransportBits,
    pub(super) transport_applied: bool,
    pub(super) output_events: u64,
    pub(super) output_events_lost: bool,
    pub(super) raw_offset: u64,
    pub(super) byte_length: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Payload {
    Prepare {
        boundary: Boundary,
        outcome: Outcome,
    },
    MetadataCall {
        boundary: Boundary,
        outcome: Outcome,
    },
    MidiCall {
        boundary: Boundary,
        outcome: Outcome,
    },
    AudioDeclaration(AudioLayout),
    EventDeclaration(EventLayout),
    Metadata {
        site: String,
        result: MetadataResult,
    },
    Midi {
        data: [u8; 3],
        offset: u16,
    },
    Process {
        boundary: Boundary,
        frames: u64,
        transport: TransportBits,
        outcome: Outcome,
    },
    Pcm(PcmDescriptor),
    /// Opaque lossless typed-control body. The future adapter/fold must validate
    /// its kind and semantics; this codec never calls it validated authority.
    RetainedControl {
        kind: u16,
        canonical_bytes: Vec<u8>,
    },
}

impl Payload {
    pub(super) fn tag(&self) -> u8 {
        match self {
            Self::Prepare { .. } => 1,
            Self::MetadataCall { .. } => 2,
            Self::AudioDeclaration(_) => 3,
            Self::EventDeclaration(_) => 4,
            Self::Metadata { .. } => 5,
            Self::Midi { .. } => 6,
            Self::Process { .. } => 7,
            Self::Pcm(_) => 8,
            Self::RetainedControl { .. } => 9,
            Self::MidiCall { .. } => 10,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CanonicalEvent {
    pub(super) header: Header,
    pub(super) payload: Payload,
}

impl CanonicalEvent {
    pub(super) fn new(
        global_seq: u64,
        monotonic_ns: u64,
        stage: u8,
        instance: u64,
        payload: Payload,
    ) -> Self {
        Self {
            header: Header {
                global_seq,
                monotonic_ns,
                stage,
                instance,
                tag: payload.tag(),
                version: VERSION,
            },
            payload,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct BaselineDictionary {
    id: u64,
    metadata: CanonicalMetadata,
}

impl BaselineDictionary {
    /// Caller must pin this id to this complete immutable dictionary outside
    /// the codec. Reusing one id for another value violates that input contract.
    pub(super) fn new(id: u64, metadata: CanonicalMetadata) -> Result<Self, CodecError> {
        let mut validator = Writer::new(MAX_ENCODED_BYTES - HEADER_BYTES);
        validator.metadata(&metadata)?;
        Ok(Self { id, metadata })
    }
    pub(super) fn id(&self) -> u64 {
        self.id
    }
    pub(super) fn metadata(&self) -> &CanonicalMetadata {
        &self.metadata
    }
    pub(super) fn audio(&self) -> &AudioLayout {
        &self.metadata.audio
    }
    pub(super) fn events(&self) -> &EventLayout {
        &self.metadata.events
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DecodedEvent {
    pub(super) event: CanonicalEvent,
    /// None for other events; false for changed declarations/metadata or an
    /// actual metadata failure. Computed from reconstructed fields, not a flag.
    pub(super) baseline_match: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CodecError {
    Limit(&'static str),
    Overflow(&'static str),
    Truncated {
        offset: usize,
        needed: usize,
        remaining: usize,
    },
    TrailingBytes {
        offset: usize,
        remaining: usize,
    },
    InvalidUtf8 {
        offset: usize,
    },
    UnsupportedVersion(u8),
    UnknownTag(u8),
    UnknownStage(u8),
    InvalidDiscriminant {
        field: &'static str,
        value: u8,
    },
    HeaderTagMismatch {
        header: u8,
        payload: u8,
    },
    WrongBaseline {
        expected: u64,
        actual: u64,
    },
    PcmByteLengthMismatch {
        declared: u64,
        expected: u64,
    },
    DestinationNotEmpty,
    DestinationCapacity,
    TypedControlRequired,
    UnsupportedNativeOperation,
    InvalidCall,
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for CodecError {}

fn check_stage(stage: u8) -> Result<(), CodecError> {
    match stage {
        STAGE_PREFLIGHT
        | STAGE_INITIAL_SOURCE
        | STAGE_SOURCE_AUDIBILITY
        | STAGE_SOURCE_STOP
        | STAGE_CONVERSION
        | STAGE_FIRST_PLAYBACK
        | STAGE_UNDO
        | STAGE_REDO
        | STAGE_SECOND_PLAYBACK
        | STAGE_WHOLE_EXPORT
        | STAGE_CLEANUP
        | STAGE_FINISHED => Ok(()),
        _ => Err(CodecError::UnknownStage(stage)),
    }
}

fn check_count(count: usize, field: &'static str) -> Result<u8, CodecError> {
    if count > MAX_BUSES {
        return Err(CodecError::Limit(field));
    }
    u8::try_from(count).map_err(|_| CodecError::Overflow(field))
}

fn check_pcm(pcm: &PcmDescriptor) -> Result<(), CodecError> {
    check_count(pcm.channel_lengths.len(), "PCM channel count")?;
    let samples = pcm.channel_lengths.iter().try_fold(0_u64, |sum, &length| {
        sum.checked_add(length)
            .ok_or(CodecError::Overflow("PCM sample count"))
    })?;
    let expected = samples
        .checked_mul(4)
        .ok_or(CodecError::Overflow("PCM byte count"))?;
    if expected != pcm.byte_length {
        return Err(CodecError::PcmByteLengthMismatch {
            declared: pcm.byte_length,
            expected,
        });
    }
    pcm.raw_offset
        .checked_add(pcm.byte_length)
        .ok_or(CodecError::Overflow("PCM raw range"))?;
    Ok(())
}

struct Writer {
    bytes: Vec<u8>,
    limit: usize,
    text_bytes: usize,
}

impl Writer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            text_bytes: 0,
        }
    }
    fn put(&mut self, bytes: &[u8]) -> Result<(), CodecError> {
        let end = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or(CodecError::Overflow("encoded byte count"))?;
        if end > self.limit {
            return Err(CodecError::Limit("encoded event bytes"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn u8(&mut self, value: u8) -> Result<(), CodecError> {
        self.put(&[value])
    }
    fn u16(&mut self, value: u16) -> Result<(), CodecError> {
        self.put(&value.to_le_bytes())
    }
    fn u32(&mut self, value: u32) -> Result<(), CodecError> {
        self.put(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<(), CodecError> {
        self.put(&value.to_le_bytes())
    }
    fn i32(&mut self, value: i32) -> Result<(), CodecError> {
        self.put(&value.to_le_bytes())
    }
    fn i64(&mut self, value: i64) -> Result<(), CodecError> {
        self.put(&value.to_le_bytes())
    }
    fn boolean(&mut self, value: bool) -> Result<(), CodecError> {
        self.u8(u8::from(value))
    }
    fn optional_boolean(&mut self, value: Option<bool>) -> Result<(), CodecError> {
        self.u8(match value {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        })
    }
    fn charge_text(&mut self, value: &str) -> Result<(), CodecError> {
        if value.len() > MAX_TEXT_BYTES {
            return Err(CodecError::Limit("individual text bytes"));
        }
        let total = self
            .text_bytes
            .checked_add(value.len())
            .ok_or(CodecError::Overflow("total text bytes"))?;
        if total > MAX_TOTAL_TEXT_BYTES {
            return Err(CodecError::Limit("total text bytes"));
        }
        self.text_bytes = total;
        Ok(())
    }
    fn charge_metadata_text(&mut self, metadata: &CanonicalMetadata) -> Result<(), CodecError> {
        for text in [
            &metadata.class_uid,
            &metadata.version,
            &metadata.name,
            &metadata.category,
        ] {
            self.charge_text(text)?;
        }
        Ok(())
    }
    fn text(&mut self, value: &str) -> Result<(), CodecError> {
        self.charge_text(value)?;
        self.u32(u32::try_from(value.len()).map_err(|_| CodecError::Overflow("text length"))?)?;
        self.put(value.as_bytes())?;
        Ok(())
    }
    fn boundary(&mut self, boundary: Boundary) -> Result<(), CodecError> {
        self.u8(match boundary {
            Boundary::Attempt => 0,
            Boundary::Returned => 1,
        })
    }
    fn outcome(&mut self, outcome: &Outcome) -> Result<(), CodecError> {
        match outcome {
            Outcome::None => self.u8(0),
            Outcome::Ok => self.u8(1),
            Outcome::Error(reason) => {
                self.u8(2)?;
                self.text(reason)
            }
        }
    }
    fn audio(&mut self, layout: &AudioLayout) -> Result<(), CodecError> {
        for buses in [&layout.inputs, &layout.outputs] {
            self.u8(check_count(buses.len(), "audio bus count")?)?;
            for bus in buses {
                self.u32(bus.channel_count)?;
                self.boolean(bus.is_main)?;
                self.optional_boolean(bus.active)?;
            }
        }
        Ok(())
    }
    fn events(&mut self, layout: &EventLayout) -> Result<(), CodecError> {
        for buses in [&layout.inputs, &layout.outputs] {
            self.u8(check_count(buses.len(), "event bus count")?)?;
            for bus in buses {
                self.u32(bus.channel_count)?;
                self.boolean(bus.is_main)?;
                self.optional_boolean(bus.active)?;
            }
        }
        Ok(())
    }
    fn metadata(&mut self, metadata: &CanonicalMetadata) -> Result<(), CodecError> {
        for text in [
            &metadata.class_uid,
            &metadata.version,
            &metadata.name,
            &metadata.category,
        ] {
            self.text(text)?;
        }
        self.audio(&metadata.audio)?;
        self.events(&metadata.events)?;
        self.u32(metadata.latency_samples)?;
        self.u32(metadata.tail_samples)
    }
    fn transport(&mut self, transport: TransportBits) -> Result<(), CodecError> {
        self.i64(transport.sample_position)?;
        self.u64(transport.quarter_note_bits)?;
        self.u64(transport.tempo_bits)?;
        self.boolean(transport.playing)?;
        self.i32(transport.time_sig_numerator)?;
        self.i32(transport.time_sig_denominator)
    }
    fn reference(&mut self, dictionary: &BaselineDictionary) -> Result<(), CodecError> {
        self.u8(1)?;
        self.u64(dictionary.id)
    }
}

pub(super) fn encode_event(
    event: &CanonicalEvent,
    dictionary: &BaselineDictionary,
) -> Result<Vec<u8>, CodecError> {
    if event.header.version != VERSION {
        return Err(CodecError::UnsupportedVersion(event.header.version));
    }
    check_stage(event.header.stage)?;
    if event.header.tag != event.payload.tag() {
        return Err(CodecError::HeaderTagMismatch {
            header: event.header.tag,
            payload: event.payload.tag(),
        });
    }
    let mut payload = Writer::new(MAX_ENCODED_BYTES - HEADER_BYTES);
    match &event.payload {
        Payload::Prepare { boundary, outcome }
        | Payload::MetadataCall { boundary, outcome }
        | Payload::MidiCall { boundary, outcome } => {
            payload.boundary(*boundary)?;
            payload.outcome(outcome)?;
        }
        Payload::AudioDeclaration(actual) => {
            if actual == dictionary.audio() {
                payload.reference(dictionary)?;
            } else {
                payload.u8(0)?;
                payload.audio(actual)?;
            }
        }
        Payload::EventDeclaration(actual) => {
            if actual == dictionary.events() {
                payload.reference(dictionary)?;
            } else {
                payload.u8(0)?;
                payload.events(actual)?;
            }
        }
        Payload::Metadata { site, result } => {
            payload.text(site)?;
            match result {
                MetadataResult::Ok(actual) => {
                    payload.u8(0)?;
                    if actual == dictionary.metadata() {
                        payload.charge_metadata_text(actual)?;
                        payload.reference(dictionary)?;
                    } else {
                        payload.u8(0)?;
                        payload.metadata(actual)?;
                    }
                }
                MetadataResult::Error(reason) => {
                    payload.u8(1)?;
                    payload.text(reason)?;
                }
            }
        }
        Payload::Midi { data, offset } => {
            payload.put(data)?;
            payload.u16(*offset)?;
        }
        Payload::Process {
            boundary,
            frames,
            transport,
            outcome,
        } => {
            payload.boundary(*boundary)?;
            payload.u64(*frames)?;
            payload.transport(*transport)?;
            payload.outcome(outcome)?;
        }
        Payload::Pcm(pcm) => {
            check_pcm(pcm)?;
            payload.u64(pcm.frames)?;
            payload.u8(check_count(pcm.channel_lengths.len(), "PCM channel count")?)?;
            for &length in &pcm.channel_lengths {
                payload.u64(length)?;
            }
            payload.transport(pcm.transport)?;
            payload.boolean(pcm.transport_applied)?;
            payload.u64(pcm.output_events)?;
            payload.boolean(pcm.output_events_lost)?;
            payload.u64(pcm.raw_offset)?;
            payload.u64(pcm.byte_length)?;
        }
        Payload::RetainedControl {
            kind,
            canonical_bytes,
        } => {
            payload.u16(*kind)?;
            payload.u32(
                u32::try_from(canonical_bytes.len())
                    .map_err(|_| CodecError::Overflow("retained control length"))?,
            )?;
            payload.put(canonical_bytes)?;
        }
    }
    let mut encoded = Writer::new(MAX_ENCODED_BYTES);
    encoded.u64(event.header.global_seq)?;
    encoded.u64(event.header.monotonic_ns)?;
    encoded.u8(event.header.stage)?;
    encoded.u64(event.header.instance)?;
    encoded.u8(event.header.tag)?;
    encoded.u8(event.header.version)?;
    encoded.u32(
        u32::try_from(payload.bytes.len()).map_err(|_| CodecError::Overflow("payload length"))?,
    )?;
    encoded.put(&payload.bytes)?;
    Ok(encoded.bytes)
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
    text_bytes: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            cursor: 0,
            text_bytes: 0,
        }
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8], CodecError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(CodecError::Overflow("decoded byte range"))?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(CodecError::Truncated {
                offset: self.cursor,
                needed: length,
                remaining: self.bytes.len().saturating_sub(self.cursor),
            })?;
        self.cursor = end;
        Ok(value)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        let mut array = [0_u8; N];
        array.copy_from_slice(self.take(N)?);
        Ok(array)
    }
    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn i32(&mut self) -> Result<i32, CodecError> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    fn i64(&mut self) -> Result<i64, CodecError> {
        Ok(i64::from_le_bytes(self.array()?))
    }
    fn discriminant_error(field: &'static str, value: u8) -> CodecError {
        CodecError::InvalidDiscriminant { field, value }
    }
    fn boolean(&mut self) -> Result<bool, CodecError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(Self::discriminant_error("boolean", value)),
        }
    }
    fn optional_boolean(&mut self) -> Result<Option<bool>, CodecError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(false)),
            2 => Ok(Some(true)),
            value => Err(Self::discriminant_error("optional boolean", value)),
        }
    }
    fn count(&mut self, field: &'static str) -> Result<usize, CodecError> {
        let count = usize::from(self.u8()?);
        check_count(count, field)?;
        Ok(count)
    }
    fn text(&mut self) -> Result<String, CodecError> {
        let length =
            usize::try_from(self.u32()?).map_err(|_| CodecError::Overflow("text length"))?;
        if length > MAX_TEXT_BYTES {
            return Err(CodecError::Limit("individual text bytes"));
        }
        let total = self
            .text_bytes
            .checked_add(length)
            .ok_or(CodecError::Overflow("total text bytes"))?;
        if total > MAX_TOTAL_TEXT_BYTES {
            return Err(CodecError::Limit("total text bytes"));
        }
        let offset = self.cursor;
        let value = std::str::from_utf8(self.take(length)?)
            .map_err(|_| CodecError::InvalidUtf8 { offset })?
            .to_owned();
        self.text_bytes = total;
        Ok(value)
    }
    fn charge_metadata_text(&mut self, metadata: &CanonicalMetadata) -> Result<(), CodecError> {
        for text in [
            &metadata.class_uid,
            &metadata.version,
            &metadata.name,
            &metadata.category,
        ] {
            if text.len() > MAX_TEXT_BYTES {
                return Err(CodecError::Limit("individual text bytes"));
            }
            self.text_bytes = self
                .text_bytes
                .checked_add(text.len())
                .ok_or(CodecError::Overflow("total text bytes"))?;
            if self.text_bytes > MAX_TOTAL_TEXT_BYTES {
                return Err(CodecError::Limit("total text bytes"));
            }
        }
        Ok(())
    }
    fn boundary(&mut self) -> Result<Boundary, CodecError> {
        match self.u8()? {
            0 => Ok(Boundary::Attempt),
            1 => Ok(Boundary::Returned),
            value => Err(Self::discriminant_error("boundary", value)),
        }
    }
    fn outcome(&mut self) -> Result<Outcome, CodecError> {
        match self.u8()? {
            0 => Ok(Outcome::None),
            1 => Ok(Outcome::Ok),
            2 => Ok(Outcome::Error(self.text()?)),
            value => Err(Self::discriminant_error("outcome", value)),
        }
    }
    fn audio(&mut self) -> Result<AudioLayout, CodecError> {
        let mut groups = [Vec::new(), Vec::new()];
        for buses in &mut groups {
            let count = self.count("audio bus count")?;
            buses.reserve_exact(count);
            for _ in 0..count {
                buses.push(AudioBus {
                    channel_count: self.u32()?,
                    is_main: self.boolean()?,
                    active: self.optional_boolean()?,
                });
            }
        }
        let [inputs, outputs] = groups;
        Ok(AudioLayout { inputs, outputs })
    }
    fn events(&mut self) -> Result<EventLayout, CodecError> {
        let mut groups = [Vec::new(), Vec::new()];
        for buses in &mut groups {
            let count = self.count("event bus count")?;
            buses.reserve_exact(count);
            for _ in 0..count {
                buses.push(EventBus {
                    channel_count: self.u32()?,
                    is_main: self.boolean()?,
                    active: self.optional_boolean()?,
                });
            }
        }
        let [inputs, outputs] = groups;
        Ok(EventLayout { inputs, outputs })
    }
    fn metadata(&mut self) -> Result<CanonicalMetadata, CodecError> {
        Ok(CanonicalMetadata {
            class_uid: self.text()?,
            version: self.text()?,
            name: self.text()?,
            category: self.text()?,
            audio: self.audio()?,
            events: self.events()?,
            latency_samples: self.u32()?,
            tail_samples: self.u32()?,
        })
    }
    fn transport(&mut self) -> Result<TransportBits, CodecError> {
        Ok(TransportBits {
            sample_position: self.i64()?,
            quarter_note_bits: self.u64()?,
            tempo_bits: self.u64()?,
            playing: self.boolean()?,
            time_sig_numerator: self.i32()?,
            time_sig_denominator: self.i32()?,
        })
    }
    fn reference(&mut self, dictionary: &BaselineDictionary) -> Result<(), CodecError> {
        let actual = self.u64()?;
        if actual != dictionary.id {
            return Err(CodecError::WrongBaseline {
                expected: dictionary.id,
                actual,
            });
        }
        Ok(())
    }
    fn mode(&mut self) -> Result<bool, CodecError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(Self::discriminant_error("baseline mode", value)),
        }
    }
    fn finish(&self) -> Result<(), CodecError> {
        if self.cursor != self.bytes.len() {
            return Err(CodecError::TrailingBytes {
                offset: self.cursor,
                remaining: self.bytes.len() - self.cursor,
            });
        }
        Ok(())
    }
}

pub(super) fn decode_event(
    bytes: &[u8],
    dictionary: &BaselineDictionary,
) -> Result<DecodedEvent, CodecError> {
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(CodecError::Limit("encoded event bytes"));
    }
    let mut reader = Reader::new(bytes);
    let header = Header {
        global_seq: reader.u64()?,
        monotonic_ns: reader.u64()?,
        stage: reader.u8()?,
        instance: reader.u64()?,
        tag: reader.u8()?,
        version: reader.u8()?,
    };
    if header.version != VERSION {
        return Err(CodecError::UnsupportedVersion(header.version));
    }
    check_stage(header.stage)?;
    let payload_length =
        usize::try_from(reader.u32()?).map_err(|_| CodecError::Overflow("payload length"))?;
    let payload_bytes = reader.take(payload_length)?;
    reader.finish()?;
    let mut payload = Reader::new(payload_bytes);
    let mut baseline_match = None;
    let value = match header.tag {
        1 => Payload::Prepare {
            boundary: payload.boundary()?,
            outcome: payload.outcome()?,
        },
        2 => Payload::MetadataCall {
            boundary: payload.boundary()?,
            outcome: payload.outcome()?,
        },
        3 => {
            let actual = if payload.mode()? {
                payload.reference(dictionary)?;
                dictionary.audio().clone()
            } else {
                payload.audio()?
            };
            baseline_match = Some(&actual == dictionary.audio());
            Payload::AudioDeclaration(actual)
        }
        4 => {
            let actual = if payload.mode()? {
                payload.reference(dictionary)?;
                dictionary.events().clone()
            } else {
                payload.events()?
            };
            baseline_match = Some(&actual == dictionary.events());
            Payload::EventDeclaration(actual)
        }
        5 => {
            let site = payload.text()?;
            let result = match payload.u8()? {
                0 => {
                    let actual = if payload.mode()? {
                        payload.reference(dictionary)?;
                        payload.charge_metadata_text(dictionary.metadata())?;
                        dictionary.metadata().clone()
                    } else {
                        payload.metadata()?
                    };
                    baseline_match = Some(&actual == dictionary.metadata());
                    MetadataResult::Ok(actual)
                }
                1 => {
                    baseline_match = Some(false);
                    MetadataResult::Error(payload.text()?)
                }
                value => return Err(Reader::discriminant_error("metadata result", value)),
            };
            Payload::Metadata { site, result }
        }
        6 => Payload::Midi {
            data: payload.array()?,
            offset: payload.u16()?,
        },
        7 => Payload::Process {
            boundary: payload.boundary()?,
            frames: payload.u64()?,
            transport: payload.transport()?,
            outcome: payload.outcome()?,
        },
        8 => {
            let frames = payload.u64()?;
            let count = payload.count("PCM channel count")?;
            let mut channel_lengths = Vec::with_capacity(count);
            for _ in 0..count {
                channel_lengths.push(payload.u64()?);
            }
            let pcm = PcmDescriptor {
                frames,
                channel_lengths,
                transport: payload.transport()?,
                transport_applied: payload.boolean()?,
                output_events: payload.u64()?,
                output_events_lost: payload.boolean()?,
                raw_offset: payload.u64()?,
                byte_length: payload.u64()?,
            };
            check_pcm(&pcm)?;
            Payload::Pcm(pcm)
        }
        9 => {
            let kind = payload.u16()?;
            let length = usize::try_from(payload.u32()?)
                .map_err(|_| CodecError::Overflow("retained control length"))?;
            Payload::RetainedControl {
                kind,
                canonical_bytes: payload.take(length)?.to_vec(),
            }
        }
        10 => Payload::MidiCall {
            boundary: payload.boundary()?,
            outcome: payload.outcome()?,
        },
        tag => return Err(CodecError::UnknownTag(tag)),
    };
    payload.finish()?;
    Ok(DecodedEvent {
        event: CanonicalEvent {
            header,
            payload: value,
        },
        baseline_match,
    })
}

// The native lane borrows the actual tap operands. It does not construct an
// owned CanonicalEvent, clone bus arrays/text/PCM, or allocate a payload Vec.
// Its two new tags cannot be mistaken for the old external-raw PCM descriptor.
pub(super) const NATIVE_CALL_TAG: u8 = 11;
pub(super) const NATIVE_INLINE_PCM_TAG: u8 = 12;
pub(super) const DEFAULT_NATIVE_BODY_LIMIT: usize = MAX_ENCODED_BYTES - HEADER_BYTES - 4;

use crate::{
    instrument_bounce::{BounceAudioBus, BouncePluginMetadata},
    plugins::plugin_runtime::PluginTransport,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NativeOperation {
    Prepare,
    Metadata,
    Midi,
    Process,
}

#[cfg(feature = "vst3")]
impl From<super::Boundary> for Boundary {
    fn from(value: super::Boundary) -> Self {
        match value {
            super::Boundary::Attempt => Self::Attempt,
            super::Boundary::Returned => Self::Returned,
        }
    }
}

#[cfg(feature = "vst3")]
impl TryFrom<super::Operation> for NativeOperation {
    type Error = CodecError;
    fn try_from(value: super::Operation) -> Result<Self, Self::Error> {
        match value {
            super::Operation::StrictPrepare => Ok(Self::Prepare),
            super::Operation::StrictMetadata => Ok(Self::Metadata),
            super::Operation::StrictMidi => Ok(Self::Midi),
            super::Operation::StrictProcess => Ok(Self::Process),
            _ => Err(CodecError::UnsupportedNativeOperation),
        }
    }
}

pub(super) enum NativeView<'a> {
    AudioDeclaration {
        inputs: &'a [BounceAudioBus],
        outputs: &'a [BounceAudioBus],
    },
    EventDeclaration {
        inputs: &'a [(u32, bool, Option<bool>)],
        outputs: &'a [(u32, bool, Option<bool>)],
    },
    Metadata {
        site: &'a str,
        result: Result<&'a BouncePluginMetadata, &'a str>,
    },
    Midi {
        data: [u8; 3],
        offset: u16,
    },
    Call {
        operation: NativeOperation,
        boundary: Boundary,
        frames: Option<usize>,
        transport: Option<&'a PluginTransport>,
        outcome: Option<&'a super::CallOutcome>,
    },
    StrictPcm {
        frames: usize,
        transport: &'a PluginTransport,
        channels: &'a [Vec<f32>],
        transport_applied: bool,
        output_events: usize,
        output_events_lost: bool,
    },
}

impl NativeView<'_> {
    pub(super) fn kind(&self) -> u16 {
        match self {
            Self::Call { .. } => 1,
            Self::Metadata { .. } => 4,
            Self::AudioDeclaration { .. } => 5,
            Self::EventDeclaration { .. } => 6,
            Self::StrictPcm { .. } => 7,
            Self::Midi { .. } => 8,
        }
    }
    pub(super) fn tag(&self) -> u8 {
        match self {
            Self::AudioDeclaration { .. } => 3,
            Self::EventDeclaration { .. } => 4,
            Self::Metadata { .. } => 5,
            Self::Midi { .. } => 6,
            Self::Call { .. } => NATIVE_CALL_TAG,
            Self::StrictPcm { .. } => NATIVE_INLINE_PCM_TAG,
        }
    }
}

struct NativeWriter<'a> {
    bytes: &'a mut Vec<u8>,
    limit: usize,
    text_bytes: usize,
}
impl NativeWriter<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), CodecError> {
        let end = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or(CodecError::Overflow("native body bytes"))?;
        if end > self.limit {
            return Err(CodecError::Limit("native body bytes"));
        }
        if end > self.bytes.capacity() {
            return Err(CodecError::DestinationCapacity);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn u8(&mut self, value: u8) -> Result<(), CodecError> {
        self.put(&[value])
    }
    fn u16(&mut self, value: u16) -> Result<(), CodecError> {
        self.put(&value.to_le_bytes())
    }
    fn u32(&mut self, value: u32) -> Result<(), CodecError> {
        self.put(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<(), CodecError> {
        self.put(&value.to_le_bytes())
    }
    fn boolean(&mut self, value: bool) -> Result<(), CodecError> {
        self.u8(u8::from(value))
    }
    fn optional_boolean(&mut self, value: Option<bool>) -> Result<(), CodecError> {
        self.u8(match value {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        })
    }
    fn charge_text(&mut self, value: &str) -> Result<(), CodecError> {
        if value.len() > MAX_TEXT_BYTES {
            return Err(CodecError::Limit("individual text bytes"));
        }
        self.text_bytes = self
            .text_bytes
            .checked_add(value.len())
            .ok_or(CodecError::Overflow("total text bytes"))?;
        if self.text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(CodecError::Limit("total text bytes"));
        }
        Ok(())
    }
    fn text(&mut self, value: &str) -> Result<(), CodecError> {
        self.charge_text(value)?;
        self.u32(u32::try_from(value.len()).map_err(|_| CodecError::Overflow("text length"))?)?;
        self.put(value.as_bytes())
    }
    fn reference(&mut self, dictionary: &BaselineDictionary) -> Result<(), CodecError> {
        self.u8(1)?;
        self.u64(dictionary.id())
    }
    fn audio(
        &mut self,
        inputs: &[BounceAudioBus],
        outputs: &[BounceAudioBus],
    ) -> Result<(), CodecError> {
        for buses in [inputs, outputs] {
            self.u8(check_count(buses.len(), "audio bus count")?)?;
            for bus in buses {
                self.u32(
                    u32::try_from(bus.channel_count)
                        .map_err(|_| CodecError::Overflow("audio channel count"))?,
                )?;
                self.boolean(bus.is_main)?;
                self.optional_boolean(bus.active)?;
            }
        }
        Ok(())
    }
    fn events(
        &mut self,
        inputs: &[(u32, bool, Option<bool>)],
        outputs: &[(u32, bool, Option<bool>)],
    ) -> Result<(), CodecError> {
        for buses in [inputs, outputs] {
            self.u8(check_count(buses.len(), "event bus count")?)?;
            for &(count, main, active) in buses {
                self.u32(count)?;
                self.boolean(main)?;
                self.optional_boolean(active)?;
            }
        }
        Ok(())
    }
    fn metadata(&mut self, value: &BouncePluginMetadata) -> Result<(), CodecError> {
        for text in [
            &value.class_uid,
            &value.version,
            &value.name,
            &value.category,
        ] {
            self.text(text)?;
        }
        self.audio(&value.input_buses, &value.output_buses)?;
        self.events(&value.event_input_buses, &value.event_output_buses)?;
        self.u32(value.latency_samples)?;
        self.u32(value.tail_samples)
    }
    fn transport(&mut self, value: &PluginTransport) -> Result<(), CodecError> {
        self.put(&value.sample_position.to_le_bytes())?;
        self.u64(value.quarter_note_position.to_bits())?;
        self.u64(value.tempo.to_bits())?;
        self.boolean(value.playing)?;
        self.put(&value.time_sig_numerator.to_le_bytes())?;
        self.put(&value.time_sig_denominator.to_le_bytes())
    }
}

fn native_audio_equal(
    inputs: &[BounceAudioBus],
    outputs: &[BounceAudioBus],
    baseline: &AudioLayout,
) -> bool {
    [inputs, outputs]
        .into_iter()
        .zip([&baseline.inputs, &baseline.outputs])
        .all(|(actual, expected)| {
            actual.len() == expected.len()
                && actual.iter().zip(expected).all(|(a, b)| {
                    u32::try_from(a.channel_count).ok() == Some(b.channel_count)
                        && a.is_main == b.is_main
                        && a.active == b.active
                })
        })
}
fn native_events_equal(
    inputs: &[(u32, bool, Option<bool>)],
    outputs: &[(u32, bool, Option<bool>)],
    baseline: &EventLayout,
) -> bool {
    [inputs, outputs]
        .into_iter()
        .zip([&baseline.inputs, &baseline.outputs])
        .all(|(actual, expected)| {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(&(count, main, active), bus)| {
                        count == bus.channel_count && main == bus.is_main && active == bus.active
                    })
        })
}
fn native_metadata_equal(actual: &BouncePluginMetadata, expected: &CanonicalMetadata) -> bool {
    actual.class_uid == expected.class_uid
        && actual.version == expected.version
        && actual.name == expected.name
        && actual.category == expected.category
        && actual.latency_samples == expected.latency_samples
        && actual.tail_samples == expected.tail_samples
        && native_audio_equal(&actual.input_buses, &actual.output_buses, &expected.audio)
        && native_events_equal(
            &actual.event_input_buses,
            &actual.event_output_buses,
            &expected.events,
        )
}
fn check_native_call(boundary: Boundary, has_outcome: bool) -> Result<(), CodecError> {
    match (boundary, has_outcome) {
        (Boundary::Attempt, false) | (Boundary::Returned, true) => Ok(()),
        _ => Err(CodecError::InvalidCall),
    }
}

/// The default reserves the historical 31-byte header and four framing bytes.
/// The live NOBS caller must use the explicit limit for its complete header.
pub(super) fn encode_native_body(
    view: NativeView<'_>,
    dictionary: &BaselineDictionary,
    destination: &mut Vec<u8>,
) -> Result<u8, CodecError> {
    encode_native_body_with_limit(view, dictionary, destination, DEFAULT_NATIVE_BODY_LIMIT)
}

/// No allocation/reservation/growth is performed. `limit` is the remaining
/// body budget after the caller's *complete* header and framing have been
/// deducted from 64 KiB. Failure clears partial bytes; nonempty input is kept.
/// Typed failures must be routed to stream_control::View::Call by the parent.
pub(super) fn encode_native_body_with_limit(
    view: NativeView<'_>,
    dictionary: &BaselineDictionary,
    destination: &mut Vec<u8>,
    limit: usize,
) -> Result<u8, CodecError> {
    if !destination.is_empty() {
        return Err(CodecError::DestinationNotEmpty);
    }
    if destination.capacity() > MAX_ENCODED_BYTES || limit > MAX_ENCODED_BYTES {
        return Err(CodecError::DestinationCapacity);
    }
    let tag = view.tag();
    let result = write_native_body(
        view,
        dictionary,
        &mut NativeWriter {
            bytes: destination,
            limit,
            text_bytes: 0,
        },
    );
    if result.is_err() {
        destination.clear();
    }
    result.map(|()| tag)
}

fn write_native_body(
    view: NativeView<'_>,
    dictionary: &BaselineDictionary,
    writer: &mut NativeWriter<'_>,
) -> Result<(), CodecError> {
    match view {
        NativeView::AudioDeclaration { inputs, outputs } => {
            if native_audio_equal(inputs, outputs, dictionary.audio()) {
                writer.reference(dictionary)
            } else {
                writer.u8(0)?;
                writer.audio(inputs, outputs)
            }
        }
        NativeView::EventDeclaration { inputs, outputs } => {
            if native_events_equal(inputs, outputs, dictionary.events()) {
                writer.reference(dictionary)
            } else {
                writer.u8(0)?;
                writer.events(inputs, outputs)
            }
        }
        NativeView::Metadata { site, result } => {
            writer.text(site)?;
            match result {
                Ok(actual) => {
                    writer.u8(0)?;
                    if native_metadata_equal(actual, dictionary.metadata()) {
                        for text in [
                            &actual.class_uid,
                            &actual.version,
                            &actual.name,
                            &actual.category,
                        ] {
                            writer.charge_text(text)?;
                        }
                        writer.reference(dictionary)
                    } else {
                        writer.u8(0)?;
                        writer.metadata(actual)
                    }
                }
                Err(error) => {
                    writer.u8(1)?;
                    writer.text(error)
                }
            }
        }
        NativeView::Midi { data, offset } => {
            writer.put(&data)?;
            writer.u16(offset)
        }
        NativeView::Call {
            operation,
            boundary,
            frames,
            transport,
            outcome,
        } => {
            check_native_call(boundary, outcome.is_some())?;
            if matches!(outcome, Some(super::CallOutcome::StrictError(_))) {
                return Err(CodecError::TypedControlRequired);
            }
            writer.u8(match operation {
                NativeOperation::Prepare => 1,
                NativeOperation::Metadata => 2,
                NativeOperation::Midi => 3,
                NativeOperation::Process => 4,
            })?;
            writer.u8(match boundary {
                Boundary::Attempt => 0,
                Boundary::Returned => 1,
            })?;
            match frames {
                None => writer.u8(0)?,
                Some(value) => {
                    writer.u8(1)?;
                    writer.u64(
                        u64::try_from(value).map_err(|_| CodecError::Overflow("frame count"))?,
                    )?;
                }
            }
            match transport {
                None => writer.u8(0)?,
                Some(value) => {
                    writer.u8(1)?;
                    writer.transport(value)?;
                }
            }
            match outcome {
                None => writer.u8(0),
                Some(super::CallOutcome::Ok) => writer.u8(1),
                Some(super::CallOutcome::Error(error)) => {
                    writer.u8(2)?;
                    writer.text(error)
                }
                Some(super::CallOutcome::StrictError(_)) => Err(CodecError::TypedControlRequired),
            }
        }
        NativeView::StrictPcm {
            frames,
            transport,
            channels,
            transport_applied,
            output_events,
            output_events_lost,
        } => {
            writer.u64(u64::try_from(frames).map_err(|_| CodecError::Overflow("frame count"))?)?;
            writer.u8(check_count(channels.len(), "PCM channel count")?)?;
            for channel in channels {
                writer.u64(
                    u64::try_from(channel.len())
                        .map_err(|_| CodecError::Overflow("PCM channel length"))?,
                )?;
                for sample in channel {
                    writer.u32(sample.to_bits())?;
                }
            }
            writer.transport(transport)?;
            writer.boolean(transport_applied)?;
            writer.u64(
                u64::try_from(output_events)
                    .map_err(|_| CodecError::Overflow("output event count"))?,
            )?;
            writer.boolean(output_events_lost)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BorrowedBus {
    pub(super) channel_count: u32,
    pub(super) is_main: bool,
    pub(super) active: Option<bool>,
}
#[derive(Clone, Copy, Debug)]
pub(super) enum BusList<'a> {
    Audio(&'a [AudioBus]),
    Event(&'a [EventBus]),
    Wire(&'a [u8]),
}
impl BusList<'_> {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Audio(buses) => buses.len(),
            Self::Event(buses) => buses.len(),
            Self::Wire(bytes) => bytes.len() / 6,
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(super) fn iter(&self) -> BusIter<'_> {
        BusIter {
            buses: *self,
            next: 0,
        }
    }
}
pub(super) struct BusIter<'a> {
    buses: BusList<'a>,
    next: usize,
}
impl Iterator for BusIter<'_> {
    type Item = BorrowedBus;
    fn next(&mut self) -> Option<Self::Item> {
        let next = self.next;
        let value = match self.buses {
            BusList::Audio(buses) => {
                let bus = buses.get(next)?;
                BorrowedBus {
                    channel_count: bus.channel_count,
                    is_main: bus.is_main,
                    active: bus.active,
                }
            }
            BusList::Event(buses) => {
                let bus = buses.get(next)?;
                BorrowedBus {
                    channel_count: bus.channel_count,
                    is_main: bus.is_main,
                    active: bus.active,
                }
            }
            BusList::Wire(bytes) => {
                let bus = bytes.get(next.checked_mul(6)?..next.checked_mul(6)?.checked_add(6)?)?;
                BorrowedBus {
                    channel_count: u32::from_le_bytes([bus[0], bus[1], bus[2], bus[3]]),
                    is_main: bus[4] == 1,
                    active: match bus[5] {
                        0 => None,
                        1 => Some(false),
                        2 => Some(true),
                        _ => return None,
                    },
                }
            }
        };
        self.next += 1;
        Some(value)
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct BorrowedMetadata<'a> {
    pub(super) class_uid: &'a str,
    pub(super) version: &'a str,
    pub(super) name: &'a str,
    pub(super) category: &'a str,
    pub(super) audio_inputs: BusList<'a>,
    pub(super) audio_outputs: BusList<'a>,
    pub(super) event_inputs: BusList<'a>,
    pub(super) event_outputs: BusList<'a>,
    pub(super) latency_samples: u32,
    pub(super) tail_samples: u32,
}
impl<'a> BorrowedMetadata<'a> {
    fn baseline(value: &'a CanonicalMetadata) -> Self {
        Self {
            class_uid: &value.class_uid,
            version: &value.version,
            name: &value.name,
            category: &value.category,
            audio_inputs: BusList::Audio(&value.audio.inputs),
            audio_outputs: BusList::Audio(&value.audio.outputs),
            event_inputs: BusList::Event(&value.events.inputs),
            event_outputs: BusList::Event(&value.events.outputs),
            latency_samples: value.latency_samples,
            tail_samples: value.tail_samples,
        }
    }
    pub(super) fn equals_baseline(&self, value: &CanonicalMetadata) -> bool {
        self.class_uid == value.class_uid
            && self.version == value.version
            && self.name == value.name
            && self.category == value.category
            && self.latency_samples == value.latency_samples
            && self.tail_samples == value.tail_samples
            && bus_list_audio_equal(self.audio_inputs, &value.audio.inputs)
            && bus_list_audio_equal(self.audio_outputs, &value.audio.outputs)
            && bus_list_event_equal(self.event_inputs, &value.events.inputs)
            && bus_list_event_equal(self.event_outputs, &value.events.outputs)
    }
}
fn bus_list_audio_equal(actual: BusList<'_>, expected: &[AudioBus]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(a, b)| {
            a.channel_count == b.channel_count && a.is_main == b.is_main && a.active == b.active
        })
}
fn bus_list_event_equal(actual: BusList<'_>, expected: &[EventBus]) -> bool {
    actual.len() == expected.len()
        && actual.iter().zip(expected).all(|(a, b)| {
            a.channel_count == b.channel_count && a.is_main == b.is_main && a.active == b.active
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BorrowedOutcome<'a> {
    None,
    Ok,
    Error(&'a str),
}
#[derive(Clone, Copy, Debug)]
pub(super) struct InlinePcm<'a> {
    pub(super) frames: u64,
    channels: &'a [u8],
    channel_count: usize,
    pub(super) transport: TransportBits,
    pub(super) transport_applied: bool,
    pub(super) output_events: u64,
    pub(super) output_events_lost: bool,
}
impl InlinePcm<'_> {
    pub(super) fn channel_count(&self) -> usize {
        self.channel_count
    }
    pub(super) fn channels(&self) -> PcmChannelIter<'_> {
        PcmChannelIter {
            remaining: self.channels,
            count: self.channel_count,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) struct InlineChannel<'a> {
    pub(super) sample_bytes: &'a [u8],
}
impl InlineChannel<'_> {
    pub(super) fn len(&self) -> usize {
        self.sample_bytes.len() / 4
    }
    pub(super) fn is_empty(&self) -> bool {
        self.sample_bytes.is_empty()
    }
    pub(super) fn sample_bits(&self) -> impl Iterator<Item = u32> + '_ {
        self.sample_bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_le_bytes(*bytes))
    }
}
pub(super) struct PcmChannelIter<'a> {
    remaining: &'a [u8],
    count: usize,
}
impl<'a> Iterator for PcmChannelIter<'a> {
    type Item = InlineChannel<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.count == 0 {
            return None;
        }
        let length = u64::from_le_bytes(self.remaining.get(..8)?.try_into().ok()?);
        let bytes = usize::try_from(length).ok()?.checked_mul(4)?;
        let end = 8_usize.checked_add(bytes)?;
        let sample_bytes = self.remaining.get(8..end)?;
        self.remaining = self.remaining.get(end..)?;
        self.count -= 1;
        Some(InlineChannel { sample_bytes })
    }
}
#[derive(Clone, Copy, Debug)]
pub(super) enum NativeBody<'a> {
    AudioDeclaration {
        inputs: BusList<'a>,
        outputs: BusList<'a>,
        baseline_match: bool,
    },
    EventDeclaration {
        inputs: BusList<'a>,
        outputs: BusList<'a>,
        baseline_match: bool,
    },
    Metadata {
        site: &'a str,
        result: Result<BorrowedMetadata<'a>, &'a str>,
        baseline_match: bool,
    },
    Midi {
        data: [u8; 3],
        offset: u16,
    },
    Call {
        operation: NativeOperation,
        boundary: Boundary,
        frames: Option<u64>,
        transport: Option<TransportBits>,
        outcome: BorrowedOutcome<'a>,
    },
    StrictPcm(InlinePcm<'a>),
}
impl NativeBody<'_> {
    pub(super) fn kind(&self) -> u16 {
        match self {
            Self::Call { .. } => 1,
            Self::Metadata { .. } => 4,
            Self::AudioDeclaration { .. } => 5,
            Self::EventDeclaration { .. } => 6,
            Self::StrictPcm(_) => 7,
            Self::Midi { .. } => 8,
        }
    }
}

impl<'a> Reader<'a> {
    fn borrowed_text(&mut self) -> Result<&'a str, CodecError> {
        let length =
            usize::try_from(self.u32()?).map_err(|_| CodecError::Overflow("text length"))?;
        if length > MAX_TEXT_BYTES {
            return Err(CodecError::Limit("individual text bytes"));
        }
        self.text_bytes = self
            .text_bytes
            .checked_add(length)
            .ok_or(CodecError::Overflow("total text bytes"))?;
        if self.text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(CodecError::Limit("total text bytes"));
        }
        let offset = self.cursor;
        std::str::from_utf8(self.take(length)?).map_err(|_| CodecError::InvalidUtf8 { offset })
    }
    fn wire_buses(&mut self, name: &'static str) -> Result<BusList<'a>, CodecError> {
        let count = self.count(name)?;
        let begin = self.cursor;
        for _ in 0..count {
            self.u32()?;
            self.boolean()?;
            self.optional_boolean()?;
        }
        Ok(BusList::Wire(&self.bytes[begin..self.cursor]))
    }
    fn borrowed_metadata(&mut self) -> Result<BorrowedMetadata<'a>, CodecError> {
        Ok(BorrowedMetadata {
            class_uid: self.borrowed_text()?,
            version: self.borrowed_text()?,
            name: self.borrowed_text()?,
            category: self.borrowed_text()?,
            audio_inputs: self.wire_buses("audio bus count")?,
            audio_outputs: self.wire_buses("audio bus count")?,
            event_inputs: self.wire_buses("event bus count")?,
            event_outputs: self.wire_buses("event bus count")?,
            latency_samples: self.u32()?,
            tail_samples: self.u32()?,
        })
    }
}

/// Validates the entire body before returning borrowed views. Native tags 11
/// and 12 are distinct from legacy calls and external raw descriptor tag 8.
/// Caller validates its full NOBS header, instance dictionary and file bounds.
pub(super) fn decode_native_body<'a>(
    tag: u8,
    bytes: &'a [u8],
    dictionary: &'a BaselineDictionary,
) -> Result<NativeBody<'a>, CodecError> {
    decode_native_body_with_limit(tag, bytes, dictionary, DEFAULT_NATIVE_BODY_LIMIT)
}
pub(super) fn decode_native_body_with_limit<'a>(
    tag: u8,
    bytes: &'a [u8],
    dictionary: &'a BaselineDictionary,
    limit: usize,
) -> Result<NativeBody<'a>, CodecError> {
    if limit > MAX_ENCODED_BYTES || bytes.len() > limit {
        return Err(CodecError::Limit("native body bytes"));
    }
    let mut r = Reader::new(bytes);
    let result = match tag {
        3 => {
            let (inputs, outputs) = if r.mode()? {
                r.reference(dictionary)?;
                (
                    BusList::Audio(&dictionary.audio().inputs),
                    BusList::Audio(&dictionary.audio().outputs),
                )
            } else {
                (
                    r.wire_buses("audio bus count")?,
                    r.wire_buses("audio bus count")?,
                )
            };
            NativeBody::AudioDeclaration {
                inputs,
                outputs,
                baseline_match: bus_list_audio_equal(inputs, &dictionary.audio().inputs)
                    && bus_list_audio_equal(outputs, &dictionary.audio().outputs),
            }
        }
        4 => {
            let (inputs, outputs) = if r.mode()? {
                r.reference(dictionary)?;
                (
                    BusList::Event(&dictionary.events().inputs),
                    BusList::Event(&dictionary.events().outputs),
                )
            } else {
                (
                    r.wire_buses("event bus count")?,
                    r.wire_buses("event bus count")?,
                )
            };
            NativeBody::EventDeclaration {
                inputs,
                outputs,
                baseline_match: bus_list_event_equal(inputs, &dictionary.events().inputs)
                    && bus_list_event_equal(outputs, &dictionary.events().outputs),
            }
        }
        5 => {
            let site = r.borrowed_text()?;
            let result = match r.u8()? {
                0 => {
                    let value = if r.mode()? {
                        r.reference(dictionary)?;
                        r.charge_metadata_text(dictionary.metadata())?;
                        BorrowedMetadata::baseline(dictionary.metadata())
                    } else {
                        r.borrowed_metadata()?
                    };
                    Ok(value)
                }
                1 => Err(r.borrowed_text()?),
                value => return Err(Reader::discriminant_error("metadata result", value)),
            };
            let baseline_match = result
                .as_ref()
                .is_ok_and(|value| value.equals_baseline(dictionary.metadata()));
            NativeBody::Metadata {
                site,
                result,
                baseline_match,
            }
        }
        6 => NativeBody::Midi {
            data: r.array()?,
            offset: r.u16()?,
        },
        NATIVE_CALL_TAG => {
            let operation = match r.u8()? {
                1 => NativeOperation::Prepare,
                2 => NativeOperation::Metadata,
                3 => NativeOperation::Midi,
                4 => NativeOperation::Process,
                value => return Err(Reader::discriminant_error("native operation", value)),
            };
            let boundary = r.boundary()?;
            let frames = match r.u8()? {
                0 => None,
                1 => Some(r.u64()?),
                value => return Err(Reader::discriminant_error("optional frames", value)),
            };
            let transport = match r.u8()? {
                0 => None,
                1 => Some(r.transport()?),
                value => return Err(Reader::discriminant_error("optional transport", value)),
            };
            let outcome = match r.u8()? {
                0 => BorrowedOutcome::None,
                1 => BorrowedOutcome::Ok,
                2 => BorrowedOutcome::Error(r.borrowed_text()?),
                value => return Err(Reader::discriminant_error("native outcome", value)),
            };
            check_native_call(boundary, !matches!(outcome, BorrowedOutcome::None))?;
            NativeBody::Call {
                operation,
                boundary,
                frames,
                transport,
                outcome,
            }
        }
        NATIVE_INLINE_PCM_TAG => {
            let frames = r.u64()?;
            let channel_count = r.count("PCM channel count")?;
            let begin = r.cursor;
            for _ in 0..channel_count {
                let samples = usize::try_from(r.u64()?)
                    .map_err(|_| CodecError::Overflow("PCM channel length"))?;
                let byte_count = samples
                    .checked_mul(4)
                    .ok_or(CodecError::Overflow("PCM channel bytes"))?;
                r.take(byte_count)?;
            }
            let channels = &r.bytes[begin..r.cursor];
            NativeBody::StrictPcm(InlinePcm {
                frames,
                channels,
                channel_count,
                transport: r.transport()?,
                transport_applied: r.boolean()?,
                output_events: r.u64()?,
                output_events_lost: r.boolean()?,
            })
        }
        value => return Err(CodecError::UnknownTag(value)),
    };
    r.finish()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> CanonicalMetadata {
        CanonicalMetadata {
            class_uid: "5653544E694B386B6F6E74616B742038".into(),
            version: "8.4.1".into(),
            name: "NI · 保全".into(),
            category: "Instrument".into(),
            audio: AudioLayout {
                inputs: vec![AudioBus {
                    channel_count: 0,
                    is_main: true,
                    active: None,
                }],
                outputs: (0..32)
                    .map(|index| AudioBus {
                        channel_count: 2,
                        is_main: index == 0,
                        active: Some(index == 0),
                    })
                    .collect(),
            },
            events: EventLayout {
                inputs: vec![EventBus {
                    channel_count: 16,
                    is_main: true,
                    active: Some(true),
                }],
                outputs: vec![
                    EventBus {
                        channel_count: 16,
                        is_main: false,
                        active: Some(false),
                    },
                    EventBus {
                        channel_count: 1,
                        is_main: false,
                        active: None,
                    },
                ],
            },
            latency_samples: 257,
            tail_samples: u32::MAX,
        }
    }
    fn dictionary() -> BaselineDictionary {
        BaselineDictionary::new(73, metadata()).unwrap()
    }
    fn transport() -> TransportBits {
        TransportBits {
            sample_position: -128,
            quarter_note_bits: 0x8000_0000_0000_0000,
            tempo_bits: 0x7ff8_0000_0000_0051,
            playing: true,
            time_sig_numerator: i32::MIN,
            time_sig_denominator: i32::MAX,
        }
    }
    fn event(payload: Payload) -> CanonicalEvent {
        CanonicalEvent::new(
            u64::MAX,
            u64::MAX - 1,
            STAGE_WHOLE_EXPORT,
            u64::MAX - 2,
            payload,
        )
    }
    fn roundtrip(value: &CanonicalEvent, dictionary: &BaselineDictionary) -> DecodedEvent {
        let bytes = encode_event(value, dictionary).unwrap();
        let decoded = decode_event(&bytes, dictionary).unwrap();
        assert_eq!(&decoded.event, value);
        assert_eq!(encode_event(&decoded.event, dictionary).unwrap(), bytes);
        decoded
    }
    fn pcm() -> PcmDescriptor {
        PcmDescriptor {
            frames: 128,
            channel_lengths: vec![128, 128],
            transport: transport(),
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
            raw_offset: 1_048_576,
            byte_length: 1_024,
        }
    }

    #[test]
    fn every_payload_boundary_outcome_and_complete_header_roundtrip_losslessly() {
        let dictionary = dictionary();
        let mut values = vec![
            Payload::AudioDeclaration(dictionary.audio().clone()),
            Payload::EventDeclaration(dictionary.events().clone()),
            Payload::Metadata {
                site: METADATA_SITE_ASSEMBLED.into(),
                result: MetadataResult::Ok(metadata()),
            },
            Payload::Metadata {
                site: METADATA_SITE_PREPARE.into(),
                result: MetadataResult::Ok(metadata()),
            },
            Payload::Metadata {
                site: METADATA_SITE_RESULT.into(),
                result: MetadataResult::Error("SDK错误\nwhole text preserved".into()),
            },
            Payload::Midi {
                data: [0x90, 60, 25],
                offset: 127,
            },
            Payload::Pcm(pcm()),
            Payload::RetainedControl {
                kind: u16::MAX,
                canonical_bytes: (0..=255).collect(),
            },
        ];
        for boundary in [Boundary::Attempt, Boundary::Returned] {
            for outcome in [
                Outcome::None,
                Outcome::Ok,
                Outcome::Error("actual failure\0without truncation".into()),
            ] {
                values.push(Payload::Prepare {
                    boundary,
                    outcome: outcome.clone(),
                });
                values.push(Payload::MetadataCall {
                    boundary,
                    outcome: outcome.clone(),
                });
                values.push(Payload::MidiCall {
                    boundary,
                    outcome: outcome.clone(),
                });
                values.push(Payload::Process {
                    boundary,
                    frames: u64::MAX,
                    transport: transport(),
                    outcome,
                });
            }
        }
        for value in values {
            roundtrip(&event(value), &dictionary);
        }
        for stage in 0..=11 {
            let value = CanonicalEvent::new(
                0,
                0,
                stage,
                0,
                Payload::Midi {
                    data: [0, 0, 0],
                    offset: 0,
                },
            );
            roundtrip(&value, &dictionary);
        }
    }

    #[test]
    fn changed_last_bus_latency_tail_or_text_is_inline_and_decoder_rechecks_equality() {
        let dictionary = dictionary();
        for change in 0..6 {
            let mut changed = metadata();
            match change {
                0 => changed.audio.outputs.last_mut().unwrap().channel_count += 1,
                1 => changed.audio.outputs.last_mut().unwrap().active = None,
                2 => changed.events.outputs.last_mut().unwrap().is_main = true,
                3 => changed.latency_samples += 1,
                4 => changed.tail_samples -= 1,
                _ => changed.name.push('!'),
            }
            let decoded = roundtrip(
                &event(Payload::Metadata {
                    site: METADATA_SITE_RESULT.into(),
                    result: MetadataResult::Ok(changed),
                }),
                &dictionary,
            );
            assert_eq!(decoded.baseline_match, Some(false));
        }
        let good = event(Payload::Metadata {
            site: METADATA_SITE_RESULT.into(),
            result: MetadataResult::Ok(metadata()),
        });
        assert_eq!(roundtrip(&good, &dictionary).baseline_match, Some(true));
        let mut audio = dictionary.audio().clone();
        audio.outputs.last_mut().unwrap().is_main = true;
        assert_eq!(
            roundtrip(&event(Payload::AudioDeclaration(audio)), &dictionary).baseline_match,
            Some(false)
        );
        let mut events = dictionary.events().clone();
        events.outputs.last_mut().unwrap().channel_count += 1;
        assert_eq!(
            roundtrip(&event(Payload::EventDeclaration(events)), &dictionary).baseline_match,
            Some(false)
        );
        let bytes = encode_event(&good, &dictionary).unwrap();
        let other = BaselineDictionary::new(dictionary.id() + 1, metadata()).unwrap();
        assert!(matches!(
            decode_event(&bytes, &other),
            Err(CodecError::WrongBaseline { .. })
        ));
    }

    #[test]
    fn every_truncation_append_unknown_header_and_discriminant_is_rejected() {
        let dictionary = dictionary();
        let bytes = encode_event(&event(Payload::Pcm(pcm())), &dictionary).unwrap();
        for length in 0..bytes.len() {
            assert!(
                decode_event(&bytes[..length], &dictionary).is_err(),
                "accepted truncation {length}"
            );
        }
        let mut appended = bytes.clone();
        appended.push(0);
        assert!(matches!(
            decode_event(&appended, &dictionary),
            Err(CodecError::TrailingBytes { .. })
        ));
        for (offset, value) in [(16, 255), (25, 255), (26, 255)] {
            let mut bad = bytes.clone();
            bad[offset] = value;
            assert!(decode_event(&bad, &dictionary).is_err());
        }
        let process = encode_event(
            &event(Payload::Process {
                boundary: Boundary::Attempt,
                frames: 128,
                transport: transport(),
                outcome: Outcome::None,
            }),
            &dictionary,
        )
        .unwrap();
        for offset in [
            HEADER_BYTES,
            HEADER_BYTES + 1 + 8 + 8 + 8 + 8,
            process.len() - 1,
        ] {
            let mut bad = process.clone();
            bad[offset] = 255;
            assert!(matches!(
                decode_event(&bad, &dictionary),
                Err(CodecError::InvalidDiscriminant { .. })
            ));
        }
        let mut audio = dictionary.audio().clone();
        audio.inputs[0].channel_count = 1; // Force canonical inline declaration.
        let audio = encode_event(&event(Payload::AudioDeclaration(audio)), &dictionary).unwrap();
        for (offset, value) in [
            (HEADER_BYTES, 2),
            (HEADER_BYTES + 1, 33),
            (HEADER_BYTES + 2 + 4, 2),
            (HEADER_BYTES + 2 + 5, 3),
        ] {
            let mut bad = audio.clone();
            bad[offset] = value;
            assert!(decode_event(&bad, &dictionary).is_err());
        }
    }

    #[test]
    fn bounded_text_bus_control_and_pcm_range_fail_without_truncating_actual_values() {
        let dictionary = dictionary();
        let reason = "x".repeat(MAX_TEXT_BYTES);
        roundtrip(
            &event(Payload::Prepare {
                boundary: Boundary::Returned,
                outcome: Outcome::Error(reason.clone()),
            }),
            &dictionary,
        );
        assert!(
            encode_event(
                &event(Payload::Prepare {
                    boundary: Boundary::Returned,
                    outcome: Outcome::Error(format!("{reason}!"))
                }),
                &dictionary
            )
            .is_err()
        );
        let mut too_many = dictionary.audio().clone();
        too_many.outputs.push(too_many.outputs[0].clone());
        assert!(encode_event(&event(Payload::AudioDeclaration(too_many)), &dictionary).is_err());
        let maximum_body = MAX_ENCODED_BYTES - HEADER_BYTES - 2 - 4;
        let control = event(Payload::RetainedControl {
            kind: 1,
            canonical_bytes: vec![0xa5; maximum_body],
        });
        let bytes = encode_event(&control, &dictionary).unwrap();
        assert_eq!(bytes.len(), MAX_ENCODED_BYTES);
        assert_eq!(decode_event(&bytes, &dictionary).unwrap().event, control);
        assert!(
            encode_event(
                &event(Payload::RetainedControl {
                    kind: 1,
                    canonical_bytes: vec![0xa5; maximum_body + 1]
                }),
                &dictionary
            )
            .is_err()
        );
        let mut mismatch = pcm();
        mismatch.byte_length -= 4;
        assert!(matches!(
            encode_event(&event(Payload::Pcm(mismatch)), &dictionary),
            Err(CodecError::PcmByteLengthMismatch { .. })
        ));
        let mut overflow = pcm();
        overflow.raw_offset = u64::MAX;
        assert!(matches!(
            encode_event(&event(Payload::Pcm(overflow)), &dictionary),
            Err(CodecError::Overflow("PCM raw range"))
        ));
        let mut sample_overflow = pcm();
        sample_overflow.channel_lengths = vec![u64::MAX, 1];
        assert!(encode_event(&event(Payload::Pcm(sample_overflow)), &dictionary).is_err());
    }

    #[test]
    fn ieee_payloads_signed_zero_and_opaque_raw_control_bits_are_never_sanitized() {
        let dictionary = dictionary();
        let encoded = event(Payload::Process {
            boundary: Boundary::Returned,
            frames: 128,
            transport: transport(),
            outcome: Outcome::Ok,
        });
        let decoded = roundtrip(&encoded, &dictionary);
        let Payload::Process { transport, .. } = decoded.event.payload else {
            panic!("wrong payload");
        };
        assert_eq!(transport.quarter_note_bits, (-0.0_f64).to_bits());
        assert_eq!(transport.tempo_bits, 0x7ff8_0000_0000_0051);
        let bits = [0x8000_0000_u32, 0x7fc0_0051, 0x7f80_0000, 0xff80_0000, 1];
        let body = bits.iter().flat_map(|bits| bits.to_le_bytes()).collect();
        let raw = event(Payload::RetainedControl {
            kind: 73,
            canonical_bytes: body,
        });
        assert_eq!(roundtrip(&raw, &dictionary).event, raw);
    }

    #[test]
    fn corrupted_internal_lengths_utf8_and_noncanonical_flags_do_not_decode() {
        let dictionary = dictionary();
        let failure = event(Payload::Prepare {
            boundary: Boundary::Returned,
            outcome: Outcome::Error("a".into()),
        });
        let encoded = encode_event(&failure, &dictionary).unwrap();
        let mut bad_utf8 = encoded.clone();
        *bad_utf8.last_mut().unwrap() = 0xff;
        assert!(matches!(
            decode_event(&bad_utf8, &dictionary),
            Err(CodecError::InvalidUtf8 { .. })
        ));
        let mut text_too_large = encoded.clone();
        text_too_large[HEADER_BYTES + 2..HEADER_BYTES + 6]
            .copy_from_slice(&((MAX_TEXT_BYTES + 1) as u32).to_le_bytes());
        assert!(matches!(
            decode_event(&text_too_large, &dictionary),
            Err(CodecError::Limit("individual text bytes"))
        ));
        let mut short_payload = encoded.clone();
        short_payload[27..31].copy_from_slice(&0_u32.to_le_bytes());
        assert!(matches!(
            decode_event(&short_payload, &dictionary),
            Err(CodecError::TrailingBytes { .. })
        ));
        let mut long_payload = encoded.clone();
        long_payload[27..31].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_event(&long_payload, &dictionary).is_err());
        let mut header_mismatch = failure;
        header_mismatch.header.tag = 6;
        assert!(matches!(
            encode_event(&header_mismatch, &dictionary),
            Err(CodecError::HeaderTagMismatch { .. })
        ));
    }

    #[test]
    fn complete_metadata_and_site_boundaries_are_independent_of_reference_encoding() {
        let mut large = metadata();
        large.class_uid = "u".repeat(MAX_TEXT_BYTES);
        large.version = "v".repeat(MAX_TEXT_BYTES);
        large.name = "n".repeat(MAX_TEXT_BYTES);
        large.category = "c".repeat(MAX_TEXT_BYTES);
        let dictionary = BaselineDictionary::new(73, large.clone()).unwrap();
        let site = "s".repeat(MAX_TEXT_BYTES);
        let accepted = event(Payload::Metadata {
            site: site.clone(),
            result: MetadataResult::Ok(large.clone()),
        });
        let decoded = roundtrip(&accepted, &dictionary);
        assert_eq!(decoded.baseline_match, Some(true));
        assert_eq!(
            large.class_uid.len()
                + large.version.len()
                + large.name.len()
                + large.category.len()
                + site.len(),
            MAX_TOTAL_TEXT_BYTES
        );
        let mut changed = large.clone();
        changed.tail_samples -= 1;
        let inline = event(Payload::Metadata {
            site: site.clone(),
            result: MetadataResult::Ok(changed),
        });
        assert_eq!(roundtrip(&inline, &dictionary).baseline_match, Some(false));
        let rejected = event(Payload::Metadata {
            site: format!("{site}x"),
            result: MetadataResult::Ok(large),
        });
        assert_eq!(
            encode_event(&rejected, &dictionary),
            Err(CodecError::Limit("individual text bytes"))
        );
        let mut forged = encode_event(&accepted, &dictionary).unwrap();
        forged.insert(HEADER_BYTES + 4, b'x');
        forged[HEADER_BYTES..HEADER_BYTES + 4]
            .copy_from_slice(&((MAX_TEXT_BYTES + 1) as u32).to_le_bytes());
        let payload_length = (forged.len() - HEADER_BYTES) as u32;
        forged[27..31].copy_from_slice(&payload_length.to_le_bytes());
        assert_eq!(
            decode_event(&forged, &dictionary),
            Err(CodecError::Limit("individual text bytes"))
        );
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;

    fn fixture() -> (BaselineDictionary, BouncePluginMetadata) {
        let actual = BouncePluginMetadata {
            class_uid: "5653544E694B386B6F6E74616B742038".into(),
            version: "8.4.1".into(),
            name: "NOIRE · 完整".into(),
            category: "Instrument|Piano".into(),
            input_buses: vec![BounceAudioBus {
                channel_count: 1,
                is_main: true,
                active: None,
            }],
            output_buses: (0..32)
                .map(|index| BounceAudioBus {
                    channel_count: 2,
                    is_main: index == 0,
                    active: Some(index == 0),
                })
                .collect(),
            event_input_buses: vec![(16, true, Some(true)), (1, false, None)],
            event_output_buses: vec![(0, false, Some(false))],
            latency_samples: 257,
            tail_samples: u32::MAX,
        };
        let dictionary = BaselineDictionary::new(
            73,
            CanonicalMetadata {
                class_uid: actual.class_uid.clone(),
                version: actual.version.clone(),
                name: actual.name.clone(),
                category: actual.category.clone(),
                audio: AudioLayout {
                    inputs: actual
                        .input_buses
                        .iter()
                        .map(|b| AudioBus {
                            channel_count: u32::try_from(b.channel_count).unwrap(),
                            is_main: b.is_main,
                            active: b.active,
                        })
                        .collect(),
                    outputs: actual
                        .output_buses
                        .iter()
                        .map(|b| AudioBus {
                            channel_count: u32::try_from(b.channel_count).unwrap(),
                            is_main: b.is_main,
                            active: b.active,
                        })
                        .collect(),
                },
                events: EventLayout {
                    inputs: actual
                        .event_input_buses
                        .iter()
                        .map(|&(channel_count, is_main, active)| EventBus {
                            channel_count,
                            is_main,
                            active,
                        })
                        .collect(),
                    outputs: actual
                        .event_output_buses
                        .iter()
                        .map(|&(channel_count, is_main, active)| EventBus {
                            channel_count,
                            is_main,
                            active,
                        })
                        .collect(),
                },
                latency_samples: actual.latency_samples,
                tail_samples: actual.tail_samples,
            },
        )
        .unwrap();
        (dictionary, actual)
    }
    fn transport() -> PluginTransport {
        PluginTransport {
            sample_position: i64::MIN,
            quarter_note_position: -0.0,
            tempo: f64::from_bits(0x7ff8_0000_0000_0051),
            playing: true,
            time_sig_numerator: i32::MIN,
            time_sig_denominator: i32::MAX,
        }
    }
    fn bytes(view: NativeView<'_>, dictionary: &BaselineDictionary) -> (u8, Vec<u8>) {
        let mut bytes = Vec::with_capacity(MAX_ENCODED_BYTES);
        let pointer = bytes.as_ptr();
        let capacity = bytes.capacity();
        let tag = encode_native_body(view, dictionary, &mut bytes).unwrap();
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes.capacity(), capacity);
        (tag, bytes)
    }
    fn pcm_view<'a>(
        frames: usize,
        transport: &'a PluginTransport,
        channels: &'a [Vec<f32>],
    ) -> NativeView<'a> {
        NativeView::StrictPcm {
            frames,
            transport,
            channels,
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        }
    }
    fn assert_bus_lists_match(decoded: BorrowedMetadata<'_>, actual: &BouncePluginMetadata) {
        for (retained, original) in [
            (decoded.audio_inputs, &actual.input_buses),
            (decoded.audio_outputs, &actual.output_buses),
        ] {
            assert_eq!(retained.len(), original.len());
            assert_eq!(retained.is_empty(), original.is_empty());
            for (a, b) in retained.iter().zip(original) {
                assert_eq!(a.channel_count as usize, b.channel_count);
                assert_eq!(a.is_main, b.is_main);
                assert_eq!(a.active, b.active);
            }
        }
        for (retained, original) in [
            (decoded.event_inputs, &actual.event_input_buses),
            (decoded.event_outputs, &actual.event_output_buses),
        ] {
            assert_eq!(retained.len(), original.len());
            assert_eq!(retained.is_empty(), original.is_empty());
            for (a, &(count, main, active)) in retained.iter().zip(original) {
                assert_eq!(
                    (a.channel_count, a.is_main, a.active),
                    (count, main, active)
                );
            }
        }
    }

    #[test]
    fn native_midi_and_absent_call_operands_have_independent_exact_wire_bytes() {
        let (dictionary, _) = fixture();
        let (tag, midi) = bytes(
            NativeView::Midi {
                data: [0x90, 60, 127],
                offset: 0x1234,
            },
            &dictionary,
        );
        assert_eq!(tag, 6);
        assert_eq!(midi, [0x90, 60, 127, 0x34, 0x12]);
        let NativeBody::Midi { data, offset } =
            decode_native_body(tag, &midi, &dictionary).unwrap()
        else {
            panic!("MIDI tag lost");
        };
        assert_eq!(data, [0x90, 60, 127]);
        assert_eq!(offset, 0x1234);
        for (operation, wire) in [
            (NativeOperation::Prepare, 1),
            (NativeOperation::Metadata, 2),
            (NativeOperation::Midi, 3),
            (NativeOperation::Process, 4),
        ] {
            let (tag, body) = bytes(
                NativeView::Call {
                    operation,
                    boundary: Boundary::Attempt,
                    frames: None,
                    transport: None,
                    outcome: None,
                },
                &dictionary,
            );
            assert_eq!(tag, NATIVE_CALL_TAG);
            assert_eq!(body, [wire, 0, 0, 0, 0]);
            let NativeBody::Call {
                operation: recovered,
                frames,
                transport,
                outcome,
                ..
            } = decode_native_body(tag, &body, &dictionary).unwrap()
            else {
                panic!("call tag lost");
            };
            assert_eq!(recovered, operation);
            assert_eq!(frames, None);
            assert_eq!(transport, None);
            assert_eq!(outcome, BorrowedOutcome::None);
        }
    }

    #[test]
    fn native_optional_call_fields_and_ieee_transport_are_never_defaulted() {
        let (dictionary, _) = fixture();
        let transport = transport();
        let ok = super::super::CallOutcome::Ok;
        let error = super::super::CallOutcome::Error("实际错误\0完整文字".into());
        for operation in [
            NativeOperation::Prepare,
            NativeOperation::Metadata,
            NativeOperation::Midi,
            NativeOperation::Process,
        ] {
            for frames in [None, Some(0), Some(usize::MAX)] {
                for musical_context in [None, Some(&transport)] {
                    for outcome in [&ok, &error] {
                        let (tag, body) = bytes(
                            NativeView::Call {
                                operation,
                                boundary: Boundary::Returned,
                                frames,
                                transport: musical_context,
                                outcome: Some(outcome),
                            },
                            &dictionary,
                        );
                        let NativeBody::Call {
                            operation: actual,
                            boundary,
                            frames: actual_frames,
                            transport: actual_transport,
                            outcome: result,
                        } = decode_native_body(tag, &body, &dictionary).unwrap()
                        else {
                            panic!("call lost");
                        };
                        assert_eq!(actual, operation);
                        assert_eq!(boundary, Boundary::Returned);
                        assert_eq!(actual_frames, frames.map(|f| f as u64));
                        if let Some(value) = actual_transport {
                            assert!(musical_context.is_some());
                            assert_eq!(value.sample_position, i64::MIN);
                            assert_eq!(value.quarter_note_bits, (-0.0_f64).to_bits());
                            assert_eq!(value.tempo_bits, 0x7ff8_0000_0000_0051);
                            assert!(value.playing);
                            assert_eq!(value.time_sig_numerator, i32::MIN);
                            assert_eq!(value.time_sig_denominator, i32::MAX);
                        } else {
                            assert!(musical_context.is_none());
                        }
                        match outcome {
                            super::super::CallOutcome::Ok => {
                                assert_eq!(result, BorrowedOutcome::Ok)
                            }
                            super::super::CallOutcome::Error(text) => {
                                assert_eq!(result, BorrowedOutcome::Error(text))
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn native_typed_failure_and_invalid_boundary_cannot_enter_compact_success_lane() {
        use crate::instrument_bounce::render::{
            BackendDelivery, BackendFailure, BackendFailureKind,
        };
        let (dictionary, _) = fixture();
        let strict = super::super::CallOutcome::StrictError(
            BackendFailure::new(
                BackendFailureKind::Deadline,
                "typed failure remains complete",
            )
            .with_delivery(BackendDelivery::MayHaveApplied),
        );
        let ok = super::super::CallOutcome::Ok;
        let mut destination = Vec::with_capacity(MAX_ENCODED_BYTES);
        for operation in [
            NativeOperation::Prepare,
            NativeOperation::Metadata,
            NativeOperation::Midi,
            NativeOperation::Process,
        ] {
            assert_eq!(
                encode_native_body(
                    NativeView::Call {
                        operation,
                        boundary: Boundary::Returned,
                        frames: None,
                        transport: None,
                        outcome: Some(&strict)
                    },
                    &dictionary,
                    &mut destination
                ),
                Err(CodecError::TypedControlRequired)
            );
            assert!(destination.is_empty());
        }
        for (boundary, outcome) in [(Boundary::Attempt, Some(&ok)), (Boundary::Returned, None)] {
            assert_eq!(
                encode_native_body(
                    NativeView::Call {
                        operation: NativeOperation::Prepare,
                        boundary,
                        frames: None,
                        transport: None,
                        outcome
                    },
                    &dictionary,
                    &mut destination
                ),
                Err(CodecError::InvalidCall)
            );
            assert!(destination.is_empty());
        }
        let mut invalid = [1, 0, 0, 0, 1];
        assert!(matches!(
            decode_native_body(NATIVE_CALL_TAG, &invalid, &dictionary),
            Err(CodecError::InvalidCall)
        ));
        invalid[1] = 1;
        invalid[4] = 0;
        assert!(matches!(
            decode_native_body(NATIVE_CALL_TAG, &invalid, &dictionary),
            Err(CodecError::InvalidCall)
        ));
        invalid[4] = 3;
        assert!(matches!(
            decode_native_body(NATIVE_CALL_TAG, &invalid, &dictionary),
            Err(CodecError::InvalidDiscriminant {
                field: "native outcome",
                value: 3
            })
        ));
    }

    #[test]
    fn native_audio_event_metadata_bodies_keep_legacy_wire_exactly() {
        let (dictionary, actual) = fixture();
        let cases = [
            (
                NativeView::AudioDeclaration {
                    inputs: &actual.input_buses,
                    outputs: &actual.output_buses,
                },
                Payload::AudioDeclaration(dictionary.audio().clone()),
            ),
            (
                NativeView::EventDeclaration {
                    inputs: &actual.event_input_buses,
                    outputs: &actual.event_output_buses,
                },
                Payload::EventDeclaration(dictionary.events().clone()),
            ),
            (
                NativeView::Metadata {
                    site: "actual metadata site",
                    result: Ok(&actual),
                },
                Payload::Metadata {
                    site: "actual metadata site".into(),
                    result: MetadataResult::Ok(dictionary.metadata().clone()),
                },
            ),
            (
                NativeView::Metadata {
                    site: "actual failure site",
                    result: Err("SDK failure\0whole error"),
                },
                Payload::Metadata {
                    site: "actual failure site".into(),
                    result: MetadataResult::Error("SDK failure\0whole error".into()),
                },
            ),
        ];
        for (view, payload) in cases {
            let (tag, body) = bytes(view, &dictionary);
            let old = encode_event(
                &CanonicalEvent::new(1, 2, STAGE_WHOLE_EXPORT, 3, payload),
                &dictionary,
            )
            .unwrap();
            assert_eq!(tag, old[25]);
            assert_eq!(body, old[HEADER_BYTES..]);
            decode_native_body(tag, &body, &dictionary).unwrap();
        }
    }

    #[test]
    fn native_standalone_audio_and_event_declarations_decode_all_fields_in_both_modes() {
        let (dictionary, actual) = fixture();
        for changed in [false, true] {
            let mut actual = actual.clone();
            if changed {
                actual.input_buses[0] = BounceAudioBus {
                    channel_count: 3,
                    is_main: false,
                    active: Some(false),
                };
                actual.output_buses[31] = BounceAudioBus {
                    channel_count: 5,
                    is_main: true,
                    active: None,
                };
                actual.event_input_buses[1] = (31, true, Some(false));
                actual.event_output_buses[0] = (64, true, None);
            }
            let (tag, body) = bytes(
                NativeView::AudioDeclaration {
                    inputs: &actual.input_buses,
                    outputs: &actual.output_buses,
                },
                &dictionary,
            );
            let NativeBody::AudioDeclaration {
                inputs,
                outputs,
                baseline_match,
            } = decode_native_body(tag, &body, &dictionary).unwrap()
            else {
                panic!("audio declaration tag lost");
            };
            assert_eq!(baseline_match, !changed);
            for (retained, original) in [
                (inputs, &actual.input_buses),
                (outputs, &actual.output_buses),
            ] {
                assert_eq!(retained.len(), original.len());
                for (retained, original) in retained.iter().zip(original) {
                    assert_eq!(retained.channel_count as usize, original.channel_count);
                    assert_eq!(retained.is_main, original.is_main);
                    assert_eq!(retained.active, original.active);
                }
            }
            let (tag, body) = bytes(
                NativeView::EventDeclaration {
                    inputs: &actual.event_input_buses,
                    outputs: &actual.event_output_buses,
                },
                &dictionary,
            );
            let NativeBody::EventDeclaration {
                inputs,
                outputs,
                baseline_match,
            } = decode_native_body(tag, &body, &dictionary).unwrap()
            else {
                panic!("event declaration tag lost");
            };
            assert_eq!(baseline_match, !changed);
            for (retained, original) in [
                (inputs, &actual.event_input_buses),
                (outputs, &actual.event_output_buses),
            ] {
                assert_eq!(retained.len(), original.len());
                for (retained, &(channel_count, is_main, active)) in retained.iter().zip(original) {
                    assert_eq!(retained.channel_count, channel_count);
                    assert_eq!(retained.is_main, is_main);
                    assert_eq!(retained.active, active);
                }
            }
        }
    }

    #[test]
    fn native_reference_requires_every_text_bus_flag_latency_tail_and_site_survives() {
        let (dictionary, actual) = fixture();
        let (tag, body) = bytes(
            NativeView::Metadata {
                site: "borrowed site",
                result: Ok(&actual),
            },
            &dictionary,
        );
        let NativeBody::Metadata {
            site,
            result: Ok(decoded),
            baseline_match,
        } = decode_native_body(tag, &body, &dictionary).unwrap()
        else {
            panic!("metadata lost");
        };
        assert_eq!(site, "borrowed site");
        assert!(baseline_match);
        assert_eq!(decoded.name.as_ptr(), dictionary.metadata().name.as_ptr());
        assert_bus_lists_match(decoded, &actual);
        for change in 0..18 {
            let mut changed = actual.clone();
            match change {
                0 => changed.class_uid.push('!'),
                1 => changed.version.push('!'),
                2 => changed.name.push('!'),
                3 => changed.category.push('!'),
                4 => changed.input_buses[0].channel_count += 1,
                5 => changed.input_buses[0].is_main = false,
                6 => changed.input_buses[0].active = Some(false),
                7 => changed.output_buses[31].channel_count += 1,
                8 => changed.output_buses[31].is_main = true,
                9 => changed.output_buses[31].active = None,
                10 => changed.event_input_buses[1].0 += 1,
                11 => changed.event_input_buses[1].1 = true,
                12 => changed.event_input_buses[1].2 = Some(false),
                13 => changed.event_output_buses[0].0 += 1,
                14 => changed.event_output_buses[0].1 = true,
                15 => changed.event_output_buses[0].2 = None,
                16 => changed.latency_samples += 1,
                _ => changed.tail_samples -= 1,
            }
            let (tag, body) = bytes(
                NativeView::Metadata {
                    site: "borrowed site",
                    result: Ok(&changed),
                },
                &dictionary,
            );
            let NativeBody::Metadata {
                result: Ok(decoded),
                baseline_match,
                ..
            } = decode_native_body(tag, &body, &dictionary).unwrap()
            else {
                panic!("literal metadata lost");
            };
            assert!(!baseline_match, "field {change} was silently referenced");
            assert_eq!(decoded.class_uid, changed.class_uid);
            assert_eq!(decoded.version, changed.version);
            assert_eq!(decoded.name, changed.name);
            assert_eq!(decoded.category, changed.category);
            assert_eq!(decoded.latency_samples, changed.latency_samples);
            assert_eq!(decoded.tail_samples, changed.tail_samples);
            let begin = body.as_ptr() as usize;
            let end = begin + body.len();
            let name_pointer = decoded.name.as_ptr() as usize;
            assert!(name_pointer >= begin && name_pointer + decoded.name.len() <= end);
            assert_bus_lists_match(decoded, &changed);
        }
    }

    #[test]
    fn native_pcm_inline_has_exact_sample_bits_channel_lengths_and_reply_context() {
        let (dictionary, _) = fixture();
        let transport = transport();
        let bits = [0x8000_0000, 0x7fc0_0051, 0x7f80_0000, 0xff80_0000, 1_u32];
        let channels = vec![
            bits.iter().map(|&b| f32::from_bits(b)).collect::<Vec<_>>(),
            vec![],
            vec![f32::from_bits(0xffc0_0073)],
        ];
        let (tag, body) = bytes(
            NativeView::StrictPcm {
                frames: usize::MAX,
                transport: &transport,
                channels: &channels,
                transport_applied: false,
                output_events: usize::MAX,
                output_events_lost: true,
            },
            &dictionary,
        );
        assert_eq!(tag, 12);
        assert_ne!(tag, 8);
        let mut expected = Vec::new();
        expected.extend_from_slice(&(usize::MAX as u64).to_le_bytes());
        expected.push(3);
        expected.extend_from_slice(&5_u64.to_le_bytes());
        for bit in bits {
            expected.extend_from_slice(&bit.to_le_bytes());
        }
        expected.extend_from_slice(&0_u64.to_le_bytes());
        expected.extend_from_slice(&1_u64.to_le_bytes());
        expected.extend_from_slice(&0xffc0_0073_u32.to_le_bytes());
        expected.extend_from_slice(&i64::MIN.to_le_bytes());
        expected.extend_from_slice(&(-0.0_f64).to_bits().to_le_bytes());
        expected.extend_from_slice(&0x7ff8_0000_0000_0051_u64.to_le_bytes());
        expected.push(1);
        expected.extend_from_slice(&i32::MIN.to_le_bytes());
        expected.extend_from_slice(&i32::MAX.to_le_bytes());
        expected.push(0);
        expected.extend_from_slice(&(usize::MAX as u64).to_le_bytes());
        expected.push(1);
        assert_eq!(body, expected);
        let NativeBody::StrictPcm(pcm) = decode_native_body(tag, &body, &dictionary).unwrap()
        else {
            panic!("PCM lost");
        };
        assert_eq!(pcm.frames, usize::MAX as u64);
        assert_eq!(pcm.channel_count(), 3);
        assert!(!pcm.transport_applied);
        assert_eq!(pcm.output_events, usize::MAX as u64);
        assert!(pcm.output_events_lost);
        assert_eq!(pcm.transport.quarter_note_bits, (-0.0_f64).to_bits());
        assert_eq!(pcm.transport.tempo_bits, 0x7ff8_0000_0000_0051);
        let recovered = pcm.channels().collect::<Vec<_>>();
        assert_eq!(recovered[0].sample_bits().collect::<Vec<_>>(), bits);
        assert_eq!(recovered[0].len(), 5);
        assert!(recovered[1].is_empty());
        assert_eq!(
            recovered[2].sample_bits().collect::<Vec<_>>(),
            [0xffc0_0073]
        );
        assert!(matches!(
            decode_native_body(8, &body, &dictionary),
            Err(CodecError::UnknownTag(8))
        ));
    }

    #[test]
    fn native_decoder_rejects_each_pcm_truncation_appended_bytes_and_noncanonical_flags() {
        let (dictionary, _) = fixture();
        let transport = transport();
        let channels = vec![vec![0.0; 128], vec![0.0; 128]];
        let (tag, body) = bytes(
            NativeView::StrictPcm {
                frames: 128,
                transport: &transport,
                channels: &channels,
                transport_applied: true,
                output_events: 0,
                output_events_lost: false,
            },
            &dictionary,
        );
        for length in 0..body.len() {
            assert!(
                decode_native_body(tag, &body[..length], &dictionary).is_err(),
                "accepted PCM truncation {length}"
            );
        }
        let mut corrupt = body.clone();
        corrupt.push(0);
        assert!(matches!(
            decode_native_body(tag, &corrupt, &dictionary),
            Err(CodecError::TrailingBytes { .. })
        ));
        for offset in [body.len() - 1, body.len() - 10, body.len() - 19] {
            let mut corrupt = body.clone();
            corrupt[offset] = 2;
            assert!(matches!(
                decode_native_body(tag, &corrupt, &dictionary),
                Err(CodecError::InvalidDiscriminant { .. })
            ));
        }
        let mut corrupt = body.clone();
        corrupt[8] = 33;
        assert!(matches!(
            decode_native_body(tag, &corrupt, &dictionary),
            Err(CodecError::Limit("PCM channel count"))
        ));
        let mut corrupt = body.clone();
        corrupt[9..17].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(decode_native_body(tag, &corrupt, &dictionary).is_err());
    }

    #[test]
    fn native_caller_limit_reserves_complete_header_and_preserves_allocation_on_failure() {
        let (dictionary, _) = fixture();
        let transport = transport();
        let frames = (MAX_ENCODED_BYTES - 62 - 60) / 4;
        let channels = vec![vec![1.0; frames]];
        let mut body = Vec::with_capacity(MAX_ENCODED_BYTES);
        let pointer = body.as_ptr();
        let capacity = body.capacity();
        let tag = encode_native_body_with_limit(
            pcm_view(frames, &transport, &channels),
            &dictionary,
            &mut body,
            MAX_ENCODED_BYTES - 62,
        )
        .unwrap();
        assert!(body.len() + 62 <= MAX_ENCODED_BYTES);
        let exact = body.len();
        decode_native_body_with_limit(tag, &body, &dictionary, MAX_ENCODED_BYTES - 62).unwrap();
        assert!(decode_native_body_with_limit(tag, &body, &dictionary, exact - 1).is_err());
        body.clear();
        assert_eq!(
            encode_native_body_with_limit(
                pcm_view(frames, &transport, &channels),
                &dictionary,
                &mut body,
                exact - 1
            ),
            Err(CodecError::Limit("native body bytes"))
        );
        assert!(body.is_empty());
        assert_eq!(body.as_ptr(), pointer);
        assert_eq!(body.capacity(), capacity);
        let one_more = vec![vec![1.0; frames + 1]];
        assert_eq!(
            encode_native_body_with_limit(
                pcm_view(frames + 1, &transport, &one_more),
                &dictionary,
                &mut body,
                MAX_ENCODED_BYTES - 62
            ),
            Err(CodecError::Limit("native body bytes"))
        );
        assert!(body.is_empty());
        let mut small = Vec::with_capacity(4);
        let pointer = small.as_ptr();
        assert_eq!(
            encode_native_body(
                NativeView::Midi {
                    data: [1, 2, 3],
                    offset: 4
                },
                &dictionary,
                &mut small
            ),
            Err(CodecError::DestinationCapacity)
        );
        assert!(small.is_empty());
        assert_eq!(small.as_ptr(), pointer);
        assert_eq!(small.capacity(), 4);
        small.extend_from_slice(&[1, 2]);
        assert_eq!(
            encode_native_body(
                NativeView::Midi {
                    data: [1, 2, 3],
                    offset: 4
                },
                &dictionary,
                &mut small
            ),
            Err(CodecError::DestinationNotEmpty)
        );
        assert_eq!(small, [1, 2]);
        let mut excessive = Vec::with_capacity(MAX_ENCODED_BYTES + 1);
        assert_eq!(
            encode_native_body(
                NativeView::Midi {
                    data: [1, 2, 3],
                    offset: 4
                },
                &dictionary,
                &mut excessive
            ),
            Err(CodecError::DestinationCapacity)
        );
        assert_eq!(
            encode_native_body_with_limit(
                NativeView::Midi {
                    data: [1, 2, 3],
                    offset: 4
                },
                &dictionary,
                &mut body,
                MAX_ENCODED_BYTES + 1
            ),
            Err(CodecError::DestinationCapacity)
        );
    }

    #[test]
    fn native_reference_still_enforces_full_metadata_and_site_text_and_bus_bounds() {
        let (dictionary, mut actual) = fixture();
        let mut body = Vec::with_capacity(MAX_ENCODED_BYTES);
        actual.output_buses.push(actual.output_buses[0]);
        assert_eq!(
            encode_native_body(
                NativeView::AudioDeclaration {
                    inputs: &actual.input_buses,
                    outputs: &actual.output_buses
                },
                &dictionary,
                &mut body
            ),
            Err(CodecError::Limit("audio bus count"))
        );
        assert!(body.is_empty());
        let (dictionary, mut actual) = fixture();
        actual.event_output_buses = vec![(1, true, None); 33];
        assert_eq!(
            encode_native_body(
                NativeView::EventDeclaration {
                    inputs: &actual.event_input_buses,
                    outputs: &actual.event_output_buses
                },
                &dictionary,
                &mut body
            ),
            Err(CodecError::Limit("event bus count"))
        );
        assert!(body.is_empty());
        let (dictionary, actual) = fixture();
        let too_long = "s".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(
            encode_native_body(
                NativeView::Metadata {
                    site: &too_long,
                    result: Ok(&actual)
                },
                &dictionary,
                &mut body
            ),
            Err(CodecError::Limit("individual text bytes"))
        );
        assert!(body.is_empty());
        let (tag, reference) = bytes(
            NativeView::AudioDeclaration {
                inputs: &actual.input_buses,
                outputs: &actual.output_buses,
            },
            &dictionary,
        );
        let other = BaselineDictionary::new(74, dictionary.metadata().clone()).unwrap();
        assert!(matches!(
            decode_native_body(tag, &reference, &other),
            Err(CodecError::WrongBaseline { .. })
        ));
    }

    #[test]
    fn native_maximum_metadata_reference_and_literal_charge_identical_complete_text() {
        let (mut dictionary, mut actual) = fixture();
        actual.class_uid = "u".repeat(MAX_TEXT_BYTES);
        actual.version = "v".repeat(MAX_TEXT_BYTES);
        actual.name = "n".repeat(MAX_TEXT_BYTES);
        actual.category = "c".repeat(MAX_TEXT_BYTES);
        let mut baseline = dictionary.metadata().clone();
        baseline.class_uid = actual.class_uid.clone();
        baseline.version = actual.version.clone();
        baseline.name = actual.name.clone();
        baseline.category = actual.category.clone();
        dictionary = BaselineDictionary::new(73, baseline).unwrap();
        let site = "s".repeat(MAX_TEXT_BYTES);
        let (tag, reference) = bytes(
            NativeView::Metadata {
                site: &site,
                result: Ok(&actual),
            },
            &dictionary,
        );
        let NativeBody::Metadata {
            baseline_match: true,
            result: Ok(decoded),
            ..
        } = decode_native_body(tag, &reference, &dictionary).unwrap()
        else {
            panic!("maximum exact metadata reference lost");
        };
        assert_eq!(decoded.category.len(), MAX_TEXT_BYTES);
        actual.tail_samples -= 1;
        let (tag, literal) = bytes(
            NativeView::Metadata {
                site: &site,
                result: Ok(&actual),
            },
            &dictionary,
        );
        assert!(literal.len() > MAX_TOTAL_TEXT_BYTES);
        let NativeBody::Metadata {
            baseline_match: false,
            result: Ok(decoded),
            ..
        } = decode_native_body(tag, &literal, &dictionary).unwrap()
        else {
            panic!("maximum literal metadata lost");
        };
        assert_eq!(decoded.tail_samples, actual.tail_samples);
        actual.category.push('!');
        let mut destination = Vec::with_capacity(MAX_ENCODED_BYTES);
        assert_eq!(
            encode_native_body(
                NativeView::Metadata {
                    site: &site,
                    result: Ok(&actual)
                },
                &dictionary,
                &mut destination
            ),
            Err(CodecError::Limit("individual text bytes"))
        );
        assert!(destination.is_empty());
    }

    #[test]
    fn native_body_kind_matches_original_observer_kind_across_each_native_tag() {
        let (dictionary, actual) = fixture();
        let transport = transport();
        let channels = vec![vec![0.0; 128], vec![0.0; 128]];
        let views = [
            (
                1,
                NativeView::Call {
                    operation: NativeOperation::Prepare,
                    boundary: Boundary::Attempt,
                    frames: None,
                    transport: None,
                    outcome: None,
                },
            ),
            (
                4,
                NativeView::Metadata {
                    site: "site",
                    result: Ok(&actual),
                },
            ),
            (
                5,
                NativeView::AudioDeclaration {
                    inputs: &actual.input_buses,
                    outputs: &actual.output_buses,
                },
            ),
            (
                6,
                NativeView::EventDeclaration {
                    inputs: &actual.event_input_buses,
                    outputs: &actual.event_output_buses,
                },
            ),
            (7, pcm_view(128, &transport, &channels)),
            (
                8,
                NativeView::Midi {
                    data: [0x90, 60, 100],
                    offset: 127,
                },
            ),
        ];
        for (kind, view) in views {
            assert_eq!(view.kind(), kind);
            let (tag, body) = bytes(view, &dictionary);
            assert_eq!(
                decode_native_body(tag, &body, &dictionary).unwrap().kind(),
                kind
            );
        }
    }

    #[cfg(feature = "vst3")]
    #[test]
    fn native_operation_conversion_cannot_accept_ordinary_source_or_disposal_call() {
        use super::super::Operation;
        for operation in [
            Operation::SourceLoad,
            Operation::OrdinaryProcess,
            Operation::StrictDispose,
        ] {
            assert_eq!(
                NativeOperation::try_from(operation),
                Err(CodecError::UnsupportedNativeOperation)
            );
        }
        assert_eq!(
            NativeOperation::try_from(Operation::StrictPrepare),
            Ok(NativeOperation::Prepare)
        );
        assert_eq!(
            NativeOperation::try_from(Operation::StrictMetadata),
            Ok(NativeOperation::Metadata)
        );
        assert_eq!(
            NativeOperation::try_from(Operation::StrictMidi),
            Ok(NativeOperation::Midi)
        );
        assert_eq!(
            NativeOperation::try_from(Operation::StrictProcess),
            Ok(NativeOperation::Process)
        );
    }
}
