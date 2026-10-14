//! Canonical framing for original observer identities and borrowed typed bodies.
//! Parsing this header proves framing only, never body semantics or SDK ownership.
use super::{InstanceIdentity, InstancePhase, Stage};

pub(super) const MAX_OPERAND: usize = 65_536;
pub(super) const MAX_PREFIX: usize = 62;
pub(super) const MAX_BODY: usize = MAX_OPERAND - MAX_PREFIX;
const MAGIC: &[u8; 4] = b"NOBS";
const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lane {
    Control,
    App,
    Native(u8),
    State,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Header {
    pub sequence: u64,
    pub monotonic_ns: u64,
    pub stage: Stage,
    pub instance: Option<InstanceIdentity>,
    pub kind: u16,
    pub lane: Lane,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Bound,
    Capacity,
    Header,
    Truncated,
    Framing,
    UnknownVersion,
    InvalidFlag,
    UnknownStage,
    UnknownPhase,
}

impl Header {
    fn validate(self) -> Result<(), Error> {
        if self.sequence == 0 || self.instance.is_some_and(|i| i.ordinal == 0) {
            return Err(Error::Header);
        }
        let valid = match self.lane {
            Lane::Control => matches!(self.kind, 1 | 3 | 13 | 14 | 16..=23),
            Lane::App => matches!(self.kind, 10..=12 | 15),
            Lane::State => matches!(self.kind, 2 | 9),
            Lane::Native(tag) => matches!(
                (self.kind, tag),
                (1, 1 | 2 | 7 | 10 | 11) | (4, 5) | (5, 3) | (6, 4) | (7, 8 | 12) | (8, 6)
            ),
        };
        if valid { Ok(()) } else { Err(Error::Header) }
    }
}

/// Add framing in the same pre-reserved allocation after a borrowed body codec.
/// An invalid header, bound or capacity leaves the caller's body unchanged.
pub(super) fn wrap(header: Header, body: &mut Vec<u8>) -> Result<(), Error> {
    header.validate()?;
    if body.len() > MAX_BODY || body.capacity() > MAX_OPERAND {
        return Err(Error::Bound);
    }
    let mut prefix = Prefix::new();
    prefix.put(&[0; 4]);
    prefix.put(MAGIC);
    prefix.put(&[VERSION]);
    let (lane, tag) = match header.lane {
        Lane::Control => (1, 0),
        Lane::App => (2, 0),
        Lane::Native(tag) => (3, tag),
        Lane::State => (4, 0),
    };
    prefix.put(&[lane, tag]);
    prefix.put(&header.kind.to_le_bytes());
    prefix.put(&header.sequence.to_le_bytes());
    prefix.put(&header.monotonic_ns.to_le_bytes());
    prefix.put(&[stage_code(header.stage)]);
    prefix.put(&[u8::from(header.instance.is_some())]);
    if let Some(instance) = header.instance {
        prefix.put(&instance.ordinal.to_le_bytes());
        prefix.put(&[match instance.phase {
            InstancePhase::InitialSource => 1,
            InstancePhase::StrictClone => 2,
            InstancePhase::UndoSource => 3,
        }]);
        prefix.optional(instance.worker_id.map(|v| v as u64));
        prefix.optional(instance.source_instance_id);
    }
    prefix.put(&(body.len() as u32).to_le_bytes());
    let total = body.len().checked_add(prefix.len).ok_or(Error::Bound)?;
    if total > MAX_OPERAND || total > body.capacity() {
        return Err(Error::Capacity);
    }
    prefix.bytes[..4].copy_from_slice(&((total - 4) as u32).to_le_bytes());
    let original_len = body.len();
    body.resize(total, 0);
    body.copy_within(..original_len, prefix.len);
    body[..prefix.len].copy_from_slice(&prefix.bytes[..prefix.len]);
    Ok(())
}

struct Prefix {
    bytes: [u8; MAX_PREFIX],
    len: usize,
}
impl Prefix {
    fn new() -> Self {
        Self {
            bytes: [0; MAX_PREFIX],
            len: 0,
        }
    }
    fn put(&mut self, bytes: &[u8]) {
        // Only fixed-width internal fields are written, at most MAX_PREFIX.
        self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }
    fn optional(&mut self, value: Option<u64>) {
        self.put(&[u8::from(value.is_some())]);
        if let Some(value) = value {
            self.put(&value.to_le_bytes());
        }
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<(Header, &[u8]), Error> {
    if bytes.len() > MAX_OPERAND {
        return Err(Error::Bound);
    }
    let mut reader = Reader { bytes, offset: 0 };
    let framed_length = reader.u32()? as usize;
    if framed_length.checked_add(4) != Some(bytes.len()) {
        return Err(Error::Framing);
    }
    if reader.take(4)? != MAGIC {
        return Err(Error::Header);
    }
    if reader.byte()? != VERSION {
        return Err(Error::UnknownVersion);
    }
    let lane = reader.byte()?;
    let tag = reader.byte()?;
    let kind = reader.u16()?;
    let sequence = reader.u64()?;
    let monotonic_ns = reader.u64()?;
    let stage = stage_from_code(reader.byte()?)?;
    let instance = if reader.flag()? {
        let ordinal = reader.u64()?;
        let phase = match reader.byte()? {
            1 => InstancePhase::InitialSource,
            2 => InstancePhase::StrictClone,
            3 => InstancePhase::UndoSource,
            _ => return Err(Error::UnknownPhase),
        };
        let worker_id = reader
            .optional()?
            .map(|v| usize::try_from(v).map_err(|_| Error::Header))
            .transpose()?;
        let source_instance_id = reader.optional()?;
        Some(InstanceIdentity {
            ordinal,
            phase,
            worker_id,
            source_instance_id,
        })
    } else {
        None
    };
    let lane = match (lane, tag) {
        (1, 0) => Lane::Control,
        (2, 0) => Lane::App,
        (3, tag) => Lane::Native(tag),
        (4, 0) => Lane::State,
        _ => return Err(Error::Header),
    };
    let body_len = reader.u32()? as usize;
    if body_len > MAX_BODY {
        return Err(Error::Bound);
    }
    let body = reader.take(body_len)?;
    if reader.offset != bytes.len() {
        return Err(Error::Framing);
    }
    let header = Header {
        sequence,
        monotonic_ns,
        stage,
        instance,
        kind,
        lane,
    };
    header.validate()?;
    Ok((header, body))
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8], Error> {
        let end = self.offset.checked_add(size).ok_or(Error::Bound)?;
        let value = self.bytes.get(self.offset..end).ok_or(Error::Truncated)?;
        self.offset = end;
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn flag(&mut self) -> Result<bool, Error> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::InvalidFlag),
        }
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().map_err(|_| Error::Truncated)?,
        ))
    }
    fn optional(&mut self) -> Result<Option<u64>, Error> {
        if self.flag()? {
            Ok(Some(self.u64()?))
        } else {
            Ok(None)
        }
    }
}
fn stage_code(stage: Stage) -> u8 {
    match stage {
        Stage::Preflight => 0,
        Stage::InitialSource => 1,
        Stage::SourceAudibility => 2,
        Stage::SourceStop => 3,
        Stage::Conversion => 4,
        Stage::FirstPlayback => 5,
        Stage::Undo => 6,
        Stage::Redo => 7,
        Stage::SecondPlayback => 8,
        Stage::WholeExport => 9,
        Stage::Cleanup => 10,
        Stage::Finished => 11,
    }
}
fn stage_from_code(code: u8) -> Result<Stage, Error> {
    Ok(match code {
        0 => Stage::Preflight,
        1 => Stage::InitialSource,
        2 => Stage::SourceAudibility,
        3 => Stage::SourceStop,
        4 => Stage::Conversion,
        5 => Stage::FirstPlayback,
        6 => Stage::Undo,
        7 => Stage::Redo,
        8 => Stage::SecondPlayback,
        9 => Stage::WholeExport,
        10 => Stage::Cleanup,
        11 => Stage::Finished,
        _ => return Err(Error::UnknownStage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn header() -> Header {
        Header {
            sequence: 1,
            monotonic_ns: 23,
            stage: Stage::WholeExport,
            instance: Some(InstanceIdentity {
                ordinal: 2,
                phase: InstancePhase::StrictClone,
                worker_id: Some(0),
                source_instance_id: Some(0),
            }),
            kind: 23,
            lane: Lane::Control,
        }
    }
    fn body() -> Vec<u8> {
        let mut value = Vec::with_capacity(MAX_OPERAND);
        value.extend_from_slice(b"body");
        value
    }
    #[test]
    fn full_optional_zero_identity_roundtrips_without_collapsing_none() {
        let original = header();
        let mut value = body();
        let pointer = value.as_ptr();
        wrap(original, &mut value).unwrap();
        assert_eq!(value.as_ptr(), pointer);
        assert_eq!(decode(&value).unwrap(), (original, b"body".as_slice()));
        assert_eq!(value.len(), MAX_PREFIX + 4);
        for identity in [
            None,
            Some(InstanceIdentity {
                worker_id: None,
                source_instance_id: None,
                ..original.instance.unwrap()
            }),
        ] {
            let changed = Header {
                instance: identity,
                ..original
            };
            let mut other = body();
            wrap(changed, &mut other).unwrap();
            assert_ne!(other, value);
            assert_eq!(decode(&other).unwrap().0, changed);
        }
    }
    #[test]
    fn framing_has_independently_specified_magic_kind_clock_and_stage_bytes() {
        let mut value = body();
        wrap(header(), &mut value).unwrap();
        assert_eq!(&value[4..13], b"NOBS\x01\x01\x00\x17\x00");
        assert_eq!(&value[13..21], &1_u64.to_le_bytes());
        assert_eq!(&value[21..29], &23_u64.to_le_bytes());
        assert_eq!(value[29], 9);
        assert_eq!(&value[MAX_PREFIX..], b"body");
    }
    #[test]
    fn exact_total_operand_bound_never_reallocates() {
        let mut value = Vec::with_capacity(MAX_OPERAND);
        value.resize(MAX_BODY, 7);
        let pointer = value.as_ptr();
        wrap(header(), &mut value).unwrap();
        assert_eq!(value.len(), MAX_OPERAND);
        assert_eq!(value.as_ptr(), pointer);
        assert_eq!(decode(&value).unwrap().1.len(), MAX_BODY);
    }
    #[test]
    fn refused_header_bound_and_capacity_preserve_original_body() {
        for wrong in [
            Header {
                sequence: 0,
                ..header()
            },
            Header {
                kind: 9,
                ..header()
            },
            Header {
                lane: Lane::App,
                ..header()
            },
        ] {
            let mut value = body();
            let before = value.clone();
            assert_eq!(wrap(wrong, &mut value), Err(Error::Header));
            assert_eq!(value, before);
        }
        let mut oversized = Vec::with_capacity(MAX_OPERAND);
        oversized.resize(MAX_BODY + 1, 9);
        assert_eq!(wrap(header(), &mut oversized), Err(Error::Bound));
        assert_eq!(oversized.len(), MAX_BODY + 1);
        let mut tiny = Vec::with_capacity(4);
        tiny.extend_from_slice(b"body");
        assert_eq!(wrap(header(), &mut tiny), Err(Error::Capacity));
        assert_eq!(tiny, b"body");
    }
    #[test]
    fn every_truncation_extra_bytes_and_invalid_header_refuse() {
        let mut valid = body();
        wrap(header(), &mut valid).unwrap();
        for end in 0..valid.len() {
            assert!(decode(&valid[..end]).is_err());
        }
        let mut extra = valid.clone();
        extra.push(0);
        assert_eq!(decode(&extra), Err(Error::Framing));
        for (offset, bad) in [
            (8, 2),
            (9, 9),
            (10, 1),
            (29, 12),
            (30, 2),
            (39, 4),
            (40, 2),
            (49, 2),
        ] {
            let mut changed = valid.clone();
            changed[offset] = bad;
            assert!(decode(&changed).is_err(), "offset {offset}");
        }
    }
    #[test]
    fn original_state_header_remains_unassigned_and_native_tag_is_closed() {
        let original = Header {
            instance: None,
            kind: 9,
            lane: Lane::State,
            ..header()
        };
        let mut value = body();
        wrap(original, &mut value).unwrap();
        assert_eq!(decode(&value).unwrap().0.instance, None);
        for (kind, tag) in [(1, 11), (4, 5), (5, 3), (6, 4), (7, 12), (8, 6)] {
            let original = Header {
                kind,
                lane: Lane::Native(tag),
                ..header()
            };
            let mut value = body();
            wrap(original, &mut value).unwrap();
            assert_eq!(decode(&value).unwrap().0, original);
        }
        let mut value = body();
        assert_eq!(
            wrap(
                Header {
                    kind: 7,
                    lane: Lane::Native(5),
                    ..header()
                },
                &mut value
            ),
            Err(Error::Header)
        );
    }
}
