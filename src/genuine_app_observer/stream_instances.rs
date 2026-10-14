//! Immutable dictionary of supplied, complete genuine observer identities.
//! Capture attribution is separate from the original Capture header's instance=None.
//! Construction headers retain supplied sequence/time. The complete original
//! transcript header, including stage and optional instance, remains in its
//! independent stream envelope; this dictionary cannot rewrite that header.
//! No SDK source authority or bind_instance hook is inferred here.

pub(super) use super::{CaptureLabel, InstanceIdentity, InstancePhase};
pub(super) use crate::instrument_bounce::BounceSourceIdentity as SourceIdentity;

pub(super) const MAX_ENTRIES: usize = 4;
pub(super) const MAX_WIRE_BYTES: usize = 488;
const HEADER_BYTES: usize = 32;
const ENTRY_BYTES: usize = 64;
const CAPTURE_BYTES: usize = 96;
const CONSUMPTION_BYTES: usize = 104;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RecordHeader {
    pub(super) global_sequence: u64,
    pub(super) monotonic_elapsed_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) identity: InstanceIdentity,
    pub(super) construction: RecordHeader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct InitialCaptureLink {
    pub(super) header: RecordHeader,
    /// Separate caller-supplied known-owner attribution; NOT the Capture record.instance.
    pub(super) known_owner_source_ordinal: u64,
    pub(super) receipt_identity: usize,
    pub(super) source: SourceIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ConsumptionLink {
    pub(super) header: RecordHeader,
    /// Dictionary reference to the exact supplied StrictCaptureIdentity record.instance.
    pub(super) clone_ordinal: u64,
    pub(super) known_owner_source_ordinal: u64,
    pub(super) captured_record_sequence: u64,
    pub(super) source: SourceIdentity,
    // StrictCaptureIdentity has no receipt field. None is invented here.
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Empty,
    Full,
    ZeroOrdinal,
    ZeroSequence,
    SequenceOrder,
    TimestampOrder,
    DuplicateOrdinal,
    ChangedTuple,
    MultipleStrictClone,
    WorkerOverflow,
    ReceiptOverflow,
    ChannelOverflow,
    SourceStamp,
    UnknownReference,
    WrongSourcePhase,
    WrongClonePhase,
    SourceInstanceMismatch,
    DuplicateCapture,
    DuplicateConsumption,
    MissingCapture,
    WrongCaptureLabel,
    UnexpectedCaptureInstance,
    MissingStrictInstance,
    StampMismatch,
    CaptureReferenceMismatch,
    CaptureNotEarlier,
    EntryNotEarlier,
    BadMagic,
    BadVersion,
    BadHeader,
    Reserved,
    OptionalId,
    UnknownPhase,
    Truncated,
    ExtraBytes,
    WireBound,
}

fn checked_header(header: RecordHeader) -> Result<(), Error> {
    if header.global_sequence == 0 {
        return Err(Error::ZeroSequence);
    }
    Ok(())
}

fn later(previous: RecordHeader, next: RecordHeader) -> Result<(), Error> {
    checked_header(next)?;
    if next.global_sequence <= previous.global_sequence {
        return Err(Error::SequenceOrder);
    }
    if next.monotonic_elapsed_ns < previous.monotonic_elapsed_ns {
        return Err(Error::TimestampOrder);
    }
    Ok(())
}

fn checked_identity(identity: InstanceIdentity) -> Result<(), Error> {
    if identity.ordinal == 0 {
        return Err(Error::ZeroOrdinal);
    }
    if let Some(worker) = identity.worker_id {
        u64::try_from(worker).map_err(|_| Error::WorkerOverflow)?;
    }
    Ok(())
}

fn checked_source(source: SourceIdentity) -> Result<(), Error> {
    if source.project_session == 0
        || source.timeline_revision == 0
        || source.epoch == 0
        || source.endpoint_id == 0
        || source.channel_id == 0
        || source.instance_id == 0
    {
        return Err(Error::SourceStamp);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Dictionary {
    entries: [Option<Entry>; MAX_ENTRIES],
    count: usize,
    capture: Option<InitialCaptureLink>,
    consumption: Option<ConsumptionLink>,
}

impl Dictionary {
    pub(super) fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries[..self.count].iter().flatten()
    }
    pub(super) fn entry(&self, ordinal: u64) -> Result<Entry, Error> {
        self.entries()
            .find(|entry| entry.identity.ordinal == ordinal)
            .copied()
            .ok_or(Error::UnknownReference)
    }
    pub(super) fn lookup_exact(&self, supplied: InstanceIdentity) -> Result<Entry, Error> {
        checked_identity(supplied)?;
        let stored = self.entry(supplied.ordinal)?;
        if stored.identity != supplied {
            return Err(Error::ChangedTuple);
        }
        Ok(stored)
    }
    pub(super) fn capture(&self) -> Option<InitialCaptureLink> {
        self.capture
    }
    pub(super) fn consumption(&self) -> Option<ConsumptionLink> {
        self.consumption
    }
    pub(super) fn links_complete(&self) -> bool {
        self.capture.is_some() && self.consumption.is_some()
    }
    pub(super) const fn sdk_authority(&self) -> bool {
        false
    }

    fn validate(&self) -> Result<(), Error> {
        if self.count == 0 {
            return Err(Error::Empty);
        }
        if self.count > MAX_ENTRIES {
            return Err(Error::Full);
        }
        let mut headers = [None; MAX_ENTRIES + 2];
        let mut strict_count = 0;
        for (index, entry) in self.entries().enumerate() {
            checked_identity(entry.identity)?;
            checked_header(entry.construction)?;
            if entry.identity.phase == InstancePhase::StrictClone {
                strict_count += 1;
            }
            for earlier in self.entries().take(index) {
                if earlier.identity.ordinal == entry.identity.ordinal {
                    return Err(Error::DuplicateOrdinal);
                }
            }
            if index > 0 {
                later(
                    self.entries[index - 1]
                        .ok_or(Error::BadHeader)?
                        .construction,
                    entry.construction,
                )?;
            }
            headers[index] = Some(entry.construction);
        }
        if strict_count > 1 {
            return Err(Error::MultipleStrictClone);
        }
        if let Some(capture) = self.capture {
            checked_header(capture.header)?;
            checked_source(capture.source)?;
            u64::try_from(capture.receipt_identity).map_err(|_| Error::ReceiptOverflow)?;
            let source = self.entry(capture.known_owner_source_ordinal)?;
            if source.identity.phase != InstancePhase::InitialSource {
                return Err(Error::WrongSourcePhase);
            }
            if source.identity.source_instance_id != Some(capture.source.instance_id) {
                return Err(Error::SourceInstanceMismatch);
            }
            if source.construction.global_sequence >= capture.header.global_sequence {
                return Err(Error::EntryNotEarlier);
            }
            headers[MAX_ENTRIES] = Some(capture.header);
        }
        if let Some(consumption) = self.consumption {
            let capture = self.capture.ok_or(Error::MissingCapture)?;
            checked_header(consumption.header)?;
            checked_source(consumption.source)?;
            let clone = self.entry(consumption.clone_ordinal)?;
            if clone.identity.phase != InstancePhase::StrictClone {
                return Err(Error::WrongClonePhase);
            }
            if clone.construction.global_sequence >= consumption.header.global_sequence {
                return Err(Error::EntryNotEarlier);
            }
            if consumption.known_owner_source_ordinal != capture.known_owner_source_ordinal
                || consumption.captured_record_sequence != capture.header.global_sequence
            {
                return Err(Error::CaptureReferenceMismatch);
            }
            if consumption.source != capture.source {
                return Err(Error::StampMismatch);
            }
            if capture.header.global_sequence >= consumption.header.global_sequence {
                return Err(Error::CaptureNotEarlier);
            }
            headers[MAX_ENTRIES + 1] = Some(consumption.header);
        }
        // File sections are not chronological event order. Check all supplied headers pairwise;
        // never sort, fill gaps, invent a record, or change the original transcript's header.
        for (index, header) in headers.iter().enumerate() {
            if let Some(header) = header {
                for other in headers.iter().skip(index + 1).flatten() {
                    if header.global_sequence == other.global_sequence {
                        return Err(Error::SequenceOrder);
                    }
                    if header.global_sequence < other.global_sequence {
                        later(*header, *other)?;
                    } else {
                        later(*other, *header)?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Builder {
    dictionary: Dictionary,
    last: Option<RecordHeader>,
}

impl Builder {
    pub(super) fn new() -> Self {
        Self {
            dictionary: Dictionary {
                entries: [None; MAX_ENTRIES],
                count: 0,
                capture: None,
                consumption: None,
            },
            last: None,
        }
    }
    fn accept_next(&self, header: RecordHeader) -> Result<(), Error> {
        checked_header(header)?;
        if let Some(previous) = self.last {
            later(previous, header)?;
        }
        Ok(())
    }
    pub(super) fn add_entry(
        &mut self,
        construction: RecordHeader,
        supplied: InstanceIdentity,
    ) -> Result<(), Error> {
        checked_identity(supplied)?;
        if let Ok(existing) = self.dictionary.entry(supplied.ordinal) {
            return Err(if existing.identity == supplied {
                Error::DuplicateOrdinal
            } else {
                Error::ChangedTuple
            });
        }
        if self.dictionary.count == MAX_ENTRIES {
            return Err(Error::Full);
        }
        if supplied.phase == InstancePhase::StrictClone
            && self
                .dictionary
                .entries()
                .any(|entry| entry.identity.phase == InstancePhase::StrictClone)
        {
            return Err(Error::MultipleStrictClone);
        }
        self.accept_next(construction)?;
        self.dictionary.entries[self.dictionary.count] = Some(Entry {
            identity: supplied,
            construction,
        });
        self.dictionary.count += 1;
        self.last = Some(construction);
        Ok(())
    }
    pub(super) fn record_initial_capture(
        &mut self,
        header: RecordHeader,
        actual_label: CaptureLabel,
        actual_record_instance: Option<InstanceIdentity>,
        known_owner_source_ordinal: u64,
        receipt_identity: usize,
        source: SourceIdentity,
    ) -> Result<(), Error> {
        if actual_label != CaptureLabel::Initial {
            return Err(Error::WrongCaptureLabel);
        }
        if actual_record_instance.is_some() {
            return Err(Error::UnexpectedCaptureInstance);
        }
        if self.dictionary.capture.is_some() {
            return Err(Error::DuplicateCapture);
        }
        self.accept_next(header)?;
        let mut candidate = self.dictionary.clone();
        candidate.capture = Some(InitialCaptureLink {
            header,
            known_owner_source_ordinal,
            receipt_identity,
            source,
        });
        candidate.validate()?;
        self.dictionary = candidate;
        self.last = Some(header);
        Ok(())
    }
    pub(super) fn record_strict_consumption(
        &mut self,
        header: RecordHeader,
        actual_record_instance: Option<InstanceIdentity>,
        known_owner_source_ordinal: u64,
        captured_record_sequence: u64,
        source: SourceIdentity,
    ) -> Result<(), Error> {
        if self.dictionary.consumption.is_some() {
            return Err(Error::DuplicateConsumption);
        }
        let actual = actual_record_instance.ok_or(Error::MissingStrictInstance)?;
        let clone = self.dictionary.lookup_exact(actual)?;
        if clone.identity.phase != InstancePhase::StrictClone {
            return Err(Error::WrongClonePhase);
        }
        self.accept_next(header)?;
        let mut candidate = self.dictionary.clone();
        candidate.consumption = Some(ConsumptionLink {
            header,
            clone_ordinal: actual.ordinal,
            known_owner_source_ordinal,
            captured_record_sequence,
            source,
        });
        candidate.validate()?;
        self.dictionary = candidate;
        self.last = Some(header);
        Ok(())
    }
    /// Freezes an immutable supplied dictionary. Incomplete links are allowed as failure evidence.
    pub(super) fn freeze(self) -> Result<Dictionary, Error> {
        self.dictionary.validate()?;
        Ok(self.dictionary)
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) struct EncodedDictionary {
    bytes: [u8; MAX_WIRE_BYTES],
    length: usize,
}
impl EncodedDictionary {
    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

struct Encoder {
    bytes: [u8; MAX_WIRE_BYTES],
    cursor: usize,
}
impl Encoder {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let end = self
            .cursor
            .checked_add(bytes.len())
            .ok_or(Error::WireBound)?;
        self.bytes
            .get_mut(self.cursor..end)
            .ok_or(Error::WireBound)?
            .copy_from_slice(bytes);
        self.cursor = end;
        Ok(())
    }
    fn u32(&mut self, value: u32) -> Result<(), Error> {
        self.write(&value.to_le_bytes())
    }
    fn u64(&mut self, value: u64) -> Result<(), Error> {
        self.write(&value.to_le_bytes())
    }
    fn header(&mut self, value: RecordHeader) -> Result<(), Error> {
        self.u64(value.global_sequence)?;
        self.u64(value.monotonic_elapsed_ns)
    }
    fn source(&mut self, value: SourceIdentity) -> Result<(), Error> {
        for word in [
            value.project_session,
            value.project_fingerprint,
            value.timeline_revision,
            value.epoch,
            value.endpoint_id,
            u64::from(value.channel_id),
            value.instance_id,
        ] {
            self.u64(word)?;
        }
        Ok(())
    }
}

pub(super) fn encode(dictionary: &Dictionary) -> Result<EncodedDictionary, Error> {
    dictionary.validate()?;
    let flags = u32::from(dictionary.capture.is_some())
        | (u32::from(dictionary.consumption.is_some()) << 1);
    let length = HEADER_BYTES
        + dictionary.count * ENTRY_BYTES
        + if dictionary.capture.is_some() {
            CAPTURE_BYTES
        } else {
            0
        }
        + if dictionary.consumption.is_some() {
            CONSUMPTION_BYTES
        } else {
            0
        };
    let mut out = Encoder {
        bytes: [0; MAX_WIRE_BYTES],
        cursor: 0,
    };
    out.write(b"NIDICT01")?;
    for word in [1, dictionary.count as u32, ENTRY_BYTES as u32, flags] {
        out.u32(word)?;
    }
    out.u64(length as u64)?;
    for entry in dictionary.entries() {
        out.u64(entry.identity.ordinal)?;
        out.u32(match entry.identity.phase {
            InstancePhase::InitialSource => 1,
            InstancePhase::StrictClone => 2,
            InstancePhase::UndoSource => 3,
        })?;
        out.u32(0)?;
        out.u64(u64::from(entry.identity.worker_id.is_some()))?;
        out.u64(match entry.identity.worker_id {
            Some(value) => u64::try_from(value).map_err(|_| Error::WorkerOverflow)?,
            None => 0,
        })?;
        out.u64(u64::from(entry.identity.source_instance_id.is_some()))?;
        out.u64(entry.identity.source_instance_id.unwrap_or(0))?;
        out.header(entry.construction)?;
    }
    if let Some(capture) = dictionary.capture {
        out.u32(1)?;
        out.u32(0)?;
        out.header(capture.header)?;
        out.u64(capture.known_owner_source_ordinal)?;
        out.u64(u64::try_from(capture.receipt_identity).map_err(|_| Error::ReceiptOverflow)?)?;
        out.source(capture.source)?;
    }
    if let Some(consumption) = dictionary.consumption {
        out.u32(2)?;
        out.u32(0)?;
        out.header(consumption.header)?;
        out.u64(consumption.clone_ordinal)?;
        out.u64(consumption.known_owner_source_ordinal)?;
        out.u64(consumption.captured_record_sequence)?;
        out.source(consumption.source)?;
    }
    if out.cursor != length {
        return Err(Error::WireBound);
    }
    Ok(EncodedDictionary {
        bytes: out.bytes,
        length,
    })
}

struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
    usize_limit: u64,
}
impl<'a> Decoder<'a> {
    fn read(&mut self, size: usize) -> Result<&'a [u8], Error> {
        let end = self.cursor.checked_add(size).ok_or(Error::WireBound)?;
        let bytes = self.bytes.get(self.cursor..end).ok_or(Error::Truncated)?;
        self.cursor = end;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(
            self.read(4)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.read(8)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }
    fn platform_usize(&mut self, receipt: bool) -> Result<usize, Error> {
        let word = self.u64()?;
        let error = if receipt {
            Error::ReceiptOverflow
        } else {
            Error::WorkerOverflow
        };
        if word > self.usize_limit {
            return Err(error);
        }
        usize::try_from(word).map_err(|_| error)
    }
    fn header(&mut self) -> Result<RecordHeader, Error> {
        Ok(RecordHeader {
            global_sequence: self.u64()?,
            monotonic_elapsed_ns: self.u64()?,
        })
    }
    fn source(&mut self) -> Result<SourceIdentity, Error> {
        let project_session = self.u64()?;
        let project_fingerprint = self.u64()?;
        let timeline_revision = self.u64()?;
        let epoch = self.u64()?;
        let endpoint_id = self.u64()?;
        let channel_id = u32::try_from(self.u64()?).map_err(|_| Error::ChannelOverflow)?;
        let instance_id = self.u64()?;
        let value = SourceIdentity {
            project_session,
            project_fingerprint,
            timeline_revision,
            epoch,
            endpoint_id,
            channel_id,
            instance_id,
        };
        checked_source(value)?;
        Ok(value)
    }
    fn optional_u64(&mut self) -> Result<Option<u64>, Error> {
        let present = self.u64()?;
        let value = self.u64()?;
        match (present, value) {
            (0, 0) => Ok(None),
            (1, value) => Ok(Some(value)),
            _ => Err(Error::OptionalId),
        }
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<Dictionary, Error> {
    decode_with_limit(
        bytes,
        u64::try_from(usize::MAX).map_err(|_| Error::WorkerOverflow)?,
    )
}

fn decode_with_limit(bytes: &[u8], usize_limit: u64) -> Result<Dictionary, Error> {
    if bytes.len() > MAX_WIRE_BYTES {
        return Err(Error::ExtraBytes);
    }
    let mut input = Decoder {
        bytes,
        cursor: 0,
        usize_limit,
    };
    if input.read(8)? != b"NIDICT01" {
        return Err(Error::BadMagic);
    }
    if input.u32()? != 1 {
        return Err(Error::BadVersion);
    }
    let count = usize::try_from(input.u32()?).map_err(|_| Error::Full)?;
    if count == 0 {
        return Err(Error::Empty);
    }
    if count > MAX_ENTRIES {
        return Err(Error::Full);
    }
    if input.u32()? != ENTRY_BYTES as u32 {
        return Err(Error::BadHeader);
    }
    let flags = input.u32()?;
    if flags != 0 && flags != 1 && flags != 3 {
        return Err(Error::BadHeader);
    }
    let length = usize::try_from(input.u64()?).map_err(|_| Error::WireBound)?;
    let expected = HEADER_BYTES
        + count * ENTRY_BYTES
        + if flags & 1 != 0 { CAPTURE_BYTES } else { 0 }
        + if flags & 2 != 0 { CONSUMPTION_BYTES } else { 0 };
    if length != expected {
        return Err(Error::BadHeader);
    }
    if bytes.len() < expected {
        return Err(Error::Truncated);
    }
    if bytes.len() > expected {
        return Err(Error::ExtraBytes);
    }
    let mut entries = [None; MAX_ENTRIES];
    for slot in entries.iter_mut().take(count) {
        let ordinal = input.u64()?;
        let phase = match input.u32()? {
            1 => InstancePhase::InitialSource,
            2 => InstancePhase::StrictClone,
            3 => InstancePhase::UndoSource,
            _ => return Err(Error::UnknownPhase),
        };
        if input.u32()? != 0 {
            return Err(Error::Reserved);
        }
        let present = input.u64()?;
        let worker_id = match present {
            0 => {
                if input.u64()? != 0 {
                    return Err(Error::OptionalId);
                }
                None
            }
            1 => Some(input.platform_usize(false)?),
            _ => return Err(Error::OptionalId),
        };
        let source_instance_id = input.optional_u64()?;
        let construction = input.header()?;
        *slot = Some(Entry {
            identity: InstanceIdentity {
                ordinal,
                phase,
                worker_id,
                source_instance_id,
            },
            construction,
        });
    }
    let capture = if flags & 1 != 0 {
        if input.u32()? != 1 {
            return Err(Error::BadHeader);
        }
        if input.u32()? != 0 {
            return Err(Error::Reserved);
        }
        Some(InitialCaptureLink {
            header: input.header()?,
            known_owner_source_ordinal: input.u64()?,
            receipt_identity: input.platform_usize(true)?,
            source: input.source()?,
        })
    } else {
        None
    };
    let consumption = if flags & 2 != 0 {
        if input.u32()? != 2 {
            return Err(Error::BadHeader);
        }
        if input.u32()? != 0 {
            return Err(Error::Reserved);
        }
        Some(ConsumptionLink {
            header: input.header()?,
            clone_ordinal: input.u64()?,
            known_owner_source_ordinal: input.u64()?,
            captured_record_sequence: input.u64()?,
            source: input.source()?,
        })
    } else {
        None
    };
    if input.cursor != bytes.len() {
        return Err(Error::ExtraBytes);
    }
    let dictionary = Dictionary {
        entries,
        count,
        capture,
        consumption,
    };
    dictionary.validate()?;
    Ok(dictionary)
}

#[cfg(test)]
pub(super) fn decode_for_32_bit_fixture(bytes: &[u8]) -> Result<Dictionary, Error> {
    decode_with_limit(bytes, u64::from(u32::MAX))
}
