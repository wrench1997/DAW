//! Exact, bounded hand-off for callback-confirmed live MIDI recording.
//!
//! [`PreparedMidiRecordEndpoint`] is the only half used by the audio callback. It owns the sole
//! producer of a preallocated SPSC and performs only fixed-size validation, atomics and bounded
//! pushes. [`MidiRecordControl`] owns the consumer and all allocation-heavy note collection.

use std::{
    error::Error,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::{
    audio::MidiGeneratorRouteStamp,
    midi_runtime::{LiveMidiEvent, NoteFifoPairer, NotePairResult, PairedMidiNote},
};

pub const DEFAULT_MIDI_RECORD_PACKET_CAPACITY: usize = 8_192;
pub const MAX_MIDI_RECORD_PACKET_CAPACITY: usize = 262_144;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiRecordRealtimeStamp {
    pub session_id: u64,
    pub project_session: u64,
    pub timeline_revision: u64,
    pub route_id: u64,
    pub connection_epoch: u64,
    pub generator: MidiGeneratorRouteStamp,
}

impl MidiRecordRealtimeStamp {
    pub const fn is_valid(self) -> bool {
        self.session_id != 0
            && self.project_session != 0
            && self.timeline_revision != 0
            && self.route_id != 0
            && self.connection_epoch != 0
            && self.generator.project_session == self.project_session
            && self.generator.endpoint_id != 0
            && self.generator.plugin_instance_id != 0
            && matches!(self.generator.slot, None | Some(0))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiRecordClockAnchor {
    pub device_frame: u64,
    pub timeline_frame: u64,
    pub transport_epoch: u64,
    pub loop_count: u64,
}

impl MidiRecordClockAnchor {
    pub const fn is_valid(self) -> bool {
        self.transport_epoch != 0
    }
}

/// One NoteOn/NoteOff that was accepted by the same Generator Live batch used for audition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiRecordPacket {
    pub session_id: u64,
    pub route_id: u64,
    pub connection_epoch: u64,
    pub transport_epoch: u64,
    pub scheduled_device_frame: u64,
    pub timeline_frame: u64,
    pub event: LiveMidiEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MidiTakeInvalidReason {
    MirrorRingFull = 1,
    InputMustPreserveOverflow = 2,
    InputEventDrop = 3,
    InputMessageRejected = 4,
    LiveScratchOverflow = 5,
    LiveBatchRejected = 6,
    RouteIdentityChanged = 7,
    ConnectionEpochChanged = 8,
    TransportEpochChanged = 9,
    TimelineRevisionChanged = 10,
    TimestampRegression = 11,
    NotePairCapacityExceeded = 12,
    TakeEventLimitReached = 13,
    PatternCycleCrossed = 14,
    ProjectTargetChanged = 15,
    TransportNotPlaying = 16,
    TimelineNotReady = 17,
    InvalidPacket = 18,
}

impl MidiTakeInvalidReason {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::MirrorRingFull,
            2 => Self::InputMustPreserveOverflow,
            3 => Self::InputEventDrop,
            4 => Self::InputMessageRejected,
            5 => Self::LiveScratchOverflow,
            6 => Self::LiveBatchRejected,
            7 => Self::RouteIdentityChanged,
            8 => Self::ConnectionEpochChanged,
            9 => Self::TransportEpochChanged,
            10 => Self::TimelineRevisionChanged,
            11 => Self::TimestampRegression,
            12 => Self::NotePairCapacityExceeded,
            13 => Self::TakeEventLimitReached,
            14 => Self::PatternCycleCrossed,
            15 => Self::ProjectTargetChanged,
            16 => Self::TransportNotPlaying,
            17 => Self::TimelineNotReady,
            18 => Self::InvalidPacket,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiRecordEndpointStatus {
    pub accepted_packets: u64,
    pub sealed: bool,
    pub invalid_reason: Option<MidiTakeInvalidReason>,
}

#[derive(Default)]
struct MidiRecordStats {
    accepted_packets: AtomicU64,
    sealed: AtomicBool,
    invalid_reason: AtomicU8,
}

impl MidiRecordStats {
    fn status(&self) -> MidiRecordEndpointStatus {
        MidiRecordEndpointStatus {
            accepted_packets: self.accepted_packets.load(Ordering::Acquire),
            sealed: self.sealed.load(Ordering::Acquire),
            invalid_reason: MidiTakeInvalidReason::from_u8(
                self.invalid_reason.load(Ordering::Acquire),
            ),
        }
    }

    fn invalidate(&self, reason: MidiTakeInvalidReason) {
        let _ = self.invalid_reason.compare_exchange(
            0,
            reason as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.sealed.store(true, Ordering::Release);
    }

    fn seal(&self) {
        self.sealed.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiRecordMirrorResult {
    Mirrored(usize),
    Sealed,
    Invalid(MidiTakeInvalidReason),
}

/// Callback-owned producer. It is neither cloneable nor shareable and must be returned in an
/// exact audio-thread lifecycle receipt before the control side is finalized.
pub struct PreparedMidiRecordEndpoint {
    stamp: MidiRecordRealtimeStamp,
    producer: Producer<MidiRecordPacket>,
    stats: Arc<MidiRecordStats>,
    start: Option<MidiRecordClockAnchor>,
}

impl PreparedMidiRecordEndpoint {
    pub const fn stamp(&self) -> MidiRecordRealtimeStamp {
        self.stamp
    }

    pub fn status(&self) -> MidiRecordEndpointStatus {
        self.stats.status()
    }

    pub(crate) fn activate(&mut self, start: MidiRecordClockAnchor) -> bool {
        if !start.is_valid() || self.start.is_some() || self.status().sealed {
            self.invalidate(MidiTakeInvalidReason::InvalidPacket);
            return false;
        }
        self.start = Some(start);
        true
    }

    pub(crate) fn start_anchor(&self) -> Option<MidiRecordClockAnchor> {
        self.start
    }

    pub(crate) fn invalidate(&mut self, reason: MidiTakeInvalidReason) {
        self.stats.invalidate(reason);
    }

    pub(crate) fn seal(&mut self) {
        self.stats.seal();
    }

    /// Mirrors one callback batch atomically with respect to capacity. A preflight failure writes
    /// zero packets, records the first invalid reason, and seals the take.
    pub(crate) fn mirror_batch(&mut self, packets: &[MidiRecordPacket]) -> MidiRecordMirrorResult {
        if self.status().sealed {
            return MidiRecordMirrorResult::Sealed;
        }
        let Some(start) = self.start else {
            self.invalidate(MidiTakeInvalidReason::InvalidPacket);
            return MidiRecordMirrorResult::Invalid(MidiTakeInvalidReason::InvalidPacket);
        };
        for packet in packets {
            let frame_delta = packet
                .scheduled_device_frame
                .checked_sub(start.device_frame);
            let expected_timeline =
                frame_delta.and_then(|delta| start.timeline_frame.checked_add(delta));
            if packet.session_id != self.stamp.session_id
                || packet.route_id != self.stamp.route_id
                || packet.connection_epoch != self.stamp.connection_epoch
                || packet.event.connection_epoch != self.stamp.connection_epoch
                || packet.transport_epoch != start.transport_epoch
                || expected_timeline != Some(packet.timeline_frame)
                || (!packet.event.is_note_on() && !packet.event.is_note_off())
            {
                self.invalidate(MidiTakeInvalidReason::InvalidPacket);
                return MidiRecordMirrorResult::Invalid(MidiTakeInvalidReason::InvalidPacket);
            }
        }
        if self.producer.slots() < packets.len() {
            self.invalidate(MidiTakeInvalidReason::MirrorRingFull);
            return MidiRecordMirrorResult::Invalid(MidiTakeInvalidReason::MirrorRingFull);
        }
        for &packet in packets {
            if let Err(PushError::Full(_)) = self.producer.push(packet) {
                self.invalidate(MidiTakeInvalidReason::MirrorRingFull);
                return MidiRecordMirrorResult::Invalid(MidiTakeInvalidReason::MirrorRingFull);
            }
        }
        self.stats
            .accepted_packets
            .fetch_add(packets.len() as u64, Ordering::Release);
        MidiRecordMirrorResult::Mirrored(packets.len())
    }
}

impl fmt::Debug for PreparedMidiRecordEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedMidiRecordEndpoint")
            .field("stamp", &self.stamp)
            .field("start", &self.start)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiRecordDrainReport {
    pub drained_packets: usize,
    pub completed_notes: usize,
    pub orphan_note_offs: usize,
    pub remaining_open_notes: usize,
    pub invalid_reason: Option<MidiTakeInvalidReason>,
}

/// Control-thread half of one take. Raw edges are paired immediately while draining, so memory
/// remains bounded by the caller-selected take limit.
pub struct MidiRecordControl {
    stamp: MidiRecordRealtimeStamp,
    consumer: Consumer<MidiRecordPacket>,
    stats: Arc<MidiRecordStats>,
    start: Option<MidiRecordClockAnchor>,
    pairer: NoteFifoPairer,
    completed: Vec<PairedMidiNote>,
    maximum_notes: usize,
    orphan_note_offs: usize,
}

impl MidiRecordControl {
    pub const fn stamp(&self) -> MidiRecordRealtimeStamp {
        self.stamp
    }

    pub fn status(&self) -> MidiRecordEndpointStatus {
        self.stats.status()
    }

    pub fn set_start_anchor(&mut self, start: MidiRecordClockAnchor) -> bool {
        if !start.is_valid() || self.start.is_some() {
            self.stats.invalidate(MidiTakeInvalidReason::InvalidPacket);
            return false;
        }
        self.start = Some(start);
        true
    }

    pub fn drain(&mut self, maximum_packets: usize) -> MidiRecordDrainReport {
        let completed_before = self.completed.len();
        let orphan_before = self.orphan_note_offs;
        let mut drained = 0;
        while drained < maximum_packets {
            let Ok(packet) = self.consumer.pop() else {
                break;
            };
            drained += 1;
            self.consume_packet(packet);
        }
        MidiRecordDrainReport {
            drained_packets: drained,
            completed_notes: self.completed.len() - completed_before,
            orphan_note_offs: self.orphan_note_offs - orphan_before,
            remaining_open_notes: self.pairer.open_note_count(),
            invalid_reason: self.status().invalid_reason,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.consumer.is_empty()
    }

    pub fn completed_notes(&self) -> &[PairedMidiNote] {
        &self.completed
    }

    pub fn orphan_note_offs(&self) -> usize {
        self.orphan_note_offs
    }

    /// Final control-thread drain after the endpoint has been returned by a Stop/Clear receipt.
    /// All still-open notes close at that exact receipt anchor.
    pub fn finish(
        &mut self,
        endpoint: &PreparedMidiRecordEndpoint,
        stop: MidiRecordClockAnchor,
    ) -> MidiRecordDrainReport {
        if endpoint.stamp() != self.stamp
            || endpoint.start_anchor() != self.start
            || !stop.is_valid()
        {
            self.stats.invalidate(MidiTakeInvalidReason::InvalidPacket);
        }
        if let Some(start) = self.start {
            if start.transport_epoch != stop.transport_epoch || start.loop_count != stop.loop_count
            {
                self.stats
                    .invalidate(MidiTakeInvalidReason::TransportEpochChanged);
            }
            let expected_timeline = stop
                .device_frame
                .checked_sub(start.device_frame)
                .and_then(|delta| start.timeline_frame.checked_add(delta));
            if expected_timeline != Some(stop.timeline_frame) {
                self.stats.invalidate(MidiTakeInvalidReason::InvalidPacket);
            }
        }
        let mut aggregate = self.drain(usize::MAX);
        while let Some(note) = self
            .pairer
            .close_one_at_frames(stop.device_frame, stop.timeline_frame)
        {
            self.push_completed(note);
        }
        aggregate.completed_notes = self.completed.len();
        aggregate.orphan_note_offs = self.orphan_note_offs;
        aggregate.remaining_open_notes = self.pairer.open_note_count();
        aggregate.invalid_reason = self.status().invalid_reason;
        aggregate
    }

    pub fn into_completed_notes(self) -> Vec<PairedMidiNote> {
        self.completed
    }

    fn consume_packet(&mut self, packet: MidiRecordPacket) {
        let Some(start) = self.start else {
            self.stats.invalidate(MidiTakeInvalidReason::InvalidPacket);
            return;
        };
        if packet.session_id != self.stamp.session_id
            || packet.route_id != self.stamp.route_id
            || packet.connection_epoch != self.stamp.connection_epoch
            || packet.event.connection_epoch != self.stamp.connection_epoch
            || packet.transport_epoch != start.transport_epoch
            || packet
                .scheduled_device_frame
                .checked_sub(start.device_frame)
                .and_then(|delta| start.timeline_frame.checked_add(delta))
                != Some(packet.timeline_frame)
            || (!packet.event.is_note_on() && !packet.event.is_note_off())
        {
            self.stats.invalidate(MidiTakeInvalidReason::InvalidPacket);
            return;
        }
        match self.pairer.take_at_frames(
            packet.event,
            packet.transport_epoch,
            packet.scheduled_device_frame,
            packet.timeline_frame,
        ) {
            NotePairResult::Ignored | NotePairResult::NoteStarted => {}
            NotePairResult::NoteCompleted(note) => self.push_completed(note),
            NotePairResult::OrphanNoteOff => {
                self.orphan_note_offs = self.orphan_note_offs.saturating_add(1);
            }
            NotePairResult::CapacityExceeded => {
                self.stats
                    .invalidate(MidiTakeInvalidReason::NotePairCapacityExceeded);
            }
            NotePairResult::TakeInvalid { .. } => {
                self.stats
                    .invalidate(MidiTakeInvalidReason::TransportEpochChanged);
            }
        }
    }

    fn push_completed(&mut self, note: PairedMidiNote) {
        if self.completed.len() == self.maximum_notes {
            self.stats
                .invalidate(MidiTakeInvalidReason::TakeEventLimitReached);
            return;
        }
        self.completed.push(note);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiRecordPrepareError {
    InvalidStamp,
    InvalidCapacity,
}

impl fmt::Display for MidiRecordPrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStamp => formatter.write_str("MIDI recording stamp is invalid"),
            Self::InvalidCapacity => formatter.write_str("MIDI recording capacity is invalid"),
        }
    }
}

impl Error for MidiRecordPrepareError {}

pub fn prepare_midi_recording(
    stamp: MidiRecordRealtimeStamp,
    capacity: usize,
) -> Result<(PreparedMidiRecordEndpoint, MidiRecordControl), MidiRecordPrepareError> {
    if !stamp.is_valid() {
        return Err(MidiRecordPrepareError::InvalidStamp);
    }
    if !(2..=MAX_MIDI_RECORD_PACKET_CAPACITY).contains(&capacity) {
        return Err(MidiRecordPrepareError::InvalidCapacity);
    }
    let (producer, consumer) = RingBuffer::new(capacity);
    let stats = Arc::new(MidiRecordStats::default());
    Ok((
        PreparedMidiRecordEndpoint {
            stamp,
            producer,
            stats: Arc::clone(&stats),
            start: None,
        },
        MidiRecordControl {
            stamp,
            consumer,
            stats,
            start: None,
            pairer: NoteFifoPairer::new(),
            completed: Vec::with_capacity(capacity),
            maximum_notes: capacity,
            orphan_note_offs: 0,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> MidiRecordRealtimeStamp {
        MidiRecordRealtimeStamp {
            session_id: 1,
            project_session: 2,
            timeline_revision: 3,
            route_id: 4,
            connection_epoch: 5,
            generator: MidiGeneratorRouteStamp {
                project_session: 2,
                channel_id: 6,
                endpoint_id: 7,
                plugin_instance_id: 8,
                slot: None,
            },
        }
    }

    fn event(sequence: u64, data: [u8; 3]) -> LiveMidiEvent {
        LiveMidiEvent {
            connection_epoch: 5,
            sequence,
            timestamp_us: sequence,
            arrival_mono_ns: sequence,
            len: 3,
            data,
        }
    }

    fn packet(sequence: u64, frame: u64, data: [u8; 3]) -> MidiRecordPacket {
        MidiRecordPacket {
            session_id: 1,
            route_id: 4,
            connection_epoch: 5,
            transport_epoch: 9,
            scheduled_device_frame: frame,
            timeline_frame: frame + 100,
            event: event(sequence, data),
        }
    }

    #[test]
    fn batch_capacity_failure_is_zero_partial_and_sticky() {
        let (mut endpoint, mut control) = prepare_midi_recording(stamp(), 2).unwrap();
        let start = MidiRecordClockAnchor {
            device_frame: 10,
            timeline_frame: 110,
            transport_epoch: 9,
            loop_count: 0,
        };
        assert!(endpoint.activate(start));
        assert!(control.set_start_anchor(start));
        assert_eq!(
            endpoint.mirror_batch(&[
                packet(1, 10, [0x90, 60, 100]),
                packet(2, 11, [0x80, 60, 0]),
                packet(3, 12, [0x90, 61, 100]),
            ]),
            MidiRecordMirrorResult::Invalid(MidiTakeInvalidReason::MirrorRingFull)
        );
        assert_eq!(control.drain(8).drained_packets, 0);
        assert_eq!(
            endpoint.status().invalid_reason,
            Some(MidiTakeInvalidReason::MirrorRingFull)
        );
    }

    #[test]
    fn control_pairs_overlap_fifo_with_exact_timeline_frames() {
        let (mut endpoint, mut control) = prepare_midi_recording(stamp(), 8).unwrap();
        let start = MidiRecordClockAnchor {
            device_frame: 10,
            timeline_frame: 110,
            transport_epoch: 9,
            loop_count: 0,
        };
        assert!(endpoint.activate(start));
        assert!(control.set_start_anchor(start));
        let packets = [
            packet(1, 10, [0x90, 60, 100]),
            packet(2, 11, [0x90, 60, 80]),
            packet(3, 15, [0x80, 60, 0]),
            packet(4, 20, [0x80, 60, 0]),
        ];
        assert_eq!(
            endpoint.mirror_batch(&packets),
            MidiRecordMirrorResult::Mirrored(4)
        );
        assert_eq!(control.drain(8).completed_notes, 2);
        assert_eq!(control.completed_notes()[0].velocity, 100);
        assert_eq!(control.completed_notes()[0].start_timeline_frame, 110);
        assert_eq!(control.completed_notes()[0].end_timeline_frame, 115);
        assert_eq!(control.completed_notes()[1].velocity, 80);
        assert_eq!(control.completed_notes()[1].end_timeline_frame, 120);
    }

    #[test]
    fn returned_endpoint_allows_exact_stop_closure() {
        let (mut endpoint, mut control) = prepare_midi_recording(stamp(), 4).unwrap();
        let start = MidiRecordClockAnchor {
            device_frame: 10,
            timeline_frame: 110,
            transport_epoch: 9,
            loop_count: 0,
        };
        assert!(endpoint.activate(start));
        assert!(control.set_start_anchor(start));
        assert_eq!(
            endpoint.mirror_batch(&[packet(1, 12, [0x90, 64, 127])]),
            MidiRecordMirrorResult::Mirrored(1)
        );
        endpoint.seal();
        let stop = MidiRecordClockAnchor {
            device_frame: 30,
            timeline_frame: 130,
            transport_epoch: 9,
            loop_count: 0,
        };
        let report = control.finish(&endpoint, stop);
        assert_eq!(report.completed_notes, 1);
        assert_eq!(control.completed_notes()[0].end_device_frame, 30);
        assert_eq!(control.completed_notes()[0].end_timeline_frame, 130);
        assert_eq!(control.status().invalid_reason, None);
    }

    #[test]
    fn preparation_rejects_nonzero_generator_slot() {
        let mut invalid = stamp();
        invalid.generator.slot = Some(1);
        assert!(matches!(
            prepare_midi_recording(invalid, 8),
            Err(MidiRecordPrepareError::InvalidStamp)
        ));
    }
}
