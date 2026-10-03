//! Allocation-free primitives shared by live MIDI input backends and the audio callback.
//!
//! This module deliberately has no dependency on a MIDI backend. Backend callbacks validate and
//! copy their bytes into [`LiveMidiEvent`]; callback/audio hand-off code can then use the clock,
//! overload and note-pairing primitives without parsing untrusted variable-length messages.

/// Maximum number of events emitted by one overload-coalescing pass.
pub const MIDI_SCRATCH_CAPACITY: usize = 16;
/// Maximum number of simultaneously open notes tracked by [`NoteFifoPairer`].
pub const NOTE_PAIR_CAPACITY: usize = 128;

const MICROS_PER_SECOND: u128 = 1_000_000;

/// A validated MIDI 1.0 channel-voice message and its backend timing metadata.
///
/// `data[len..]` is always zero. The representation is fixed-size and `Copy`, so moving it across
/// an SPSC queue requires neither allocation nor ownership bookkeeping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiveMidiEvent {
    pub connection_epoch: u64,
    pub sequence: u64,
    pub timestamp_us: u64,
    pub arrival_mono_ns: u64,
    pub len: u8,
    pub data: [u8; 3],
}

impl LiveMidiEvent {
    /// Validates and copies one complete MIDI 1.0 channel-voice message.
    ///
    /// Running status is intentionally not accepted. Note-on with velocity zero is normalized to
    /// note-off (with release velocity zero). System common, SysEx and realtime messages are
    /// filtered before entering the trusted realtime path.
    pub fn from_message(
        connection_epoch: u64,
        sequence: u64,
        timestamp_us: u64,
        arrival_mono_ns: u64,
        message: &[u8],
    ) -> Result<Self, MidiMessageError> {
        let Some(&status) = message.first() else {
            return Err(MidiMessageError::InvalidLength {
                expected: 1,
                actual: 0,
            });
        };
        if status >= 0xf8 {
            return Err(MidiMessageError::FilteredSystemRealtime { status });
        }
        if status >= 0xf0 {
            return Err(MidiMessageError::FilteredSystemMessage { status });
        }
        if status < 0x80 {
            return Err(MidiMessageError::RunningStatusUnsupported { first_byte: status });
        }

        let expected = match status & 0xf0 {
            0xc0 | 0xd0 => 2,
            0x80 | 0x90 | 0xa0 | 0xb0 | 0xe0 => 3,
            _ => unreachable!("all channel-voice status nibbles are covered"),
        };
        if message.len() != expected {
            return Err(MidiMessageError::InvalidLength {
                expected: expected as u8,
                actual: message.len().min(u8::MAX as usize) as u8,
            });
        }
        for (index, &byte) in message[1..].iter().enumerate() {
            if byte >= 0xf8 {
                return Err(MidiMessageError::FilteredSystemRealtime { status: byte });
            }
            if byte >= 0xf0 {
                return Err(MidiMessageError::FilteredSystemMessage { status: byte });
            }
            if byte & 0x80 != 0 {
                return Err(MidiMessageError::InvalidDataByte {
                    index: (index + 1) as u8,
                    value: byte,
                });
            }
        }

        let mut data = [0; 3];
        data[..expected].copy_from_slice(message);
        if status & 0xf0 == 0x90 && data[2] == 0 {
            data[0] = 0x80 | (status & 0x0f);
        }
        Ok(Self {
            connection_epoch,
            sequence,
            timestamp_us,
            arrival_mono_ns,
            len: expected as u8,
            data,
        })
    }

    pub const fn status(self) -> u8 {
        self.data[0]
    }

    pub const fn channel(self) -> u8 {
        self.data[0] & 0x0f
    }

    pub const fn is_note_on(self) -> bool {
        self.data[0] & 0xf0 == 0x90
    }

    pub const fn is_note_off(self) -> bool {
        self.data[0] & 0xf0 == 0x80
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMessageError {
    InvalidLength { expected: u8, actual: u8 },
    RunningStatusUnsupported { first_byte: u8 },
    InvalidDataByte { index: u8, value: u8 },
    FilteredSystemMessage { status: u8 },
    FilteredSystemRealtime { status: u8 },
}

/// Correlates one backend timestamp with the audio device's absolute frame clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiClockAnchor {
    pub connection_epoch: u64,
    pub timestamp_us: u64,
    pub device_frame: u64,
    pub sample_rate_hz: u32,
}

impl MidiClockAnchor {
    pub const fn new(
        connection_epoch: u64,
        timestamp_us: u64,
        device_frame: u64,
        sample_rate_hz: u32,
    ) -> Result<Self, MidiClockError> {
        if sample_rate_hz == 0 {
            return Err(MidiClockError::ZeroSampleRate);
        }
        Ok(Self {
            connection_epoch,
            timestamp_us,
            device_frame,
            sample_rate_hz,
        })
    }

    /// Maps a timestamp with saturating integer arithmetic.
    ///
    /// Positive deltas use `u128`; negative deltas use `i128`. Fractional frames are truncated
    /// toward the anchor. Results outside the `u64` device timeline saturate at its endpoints.
    pub fn device_frame_at(self, timestamp_us: u64) -> u64 {
        if timestamp_us >= self.timestamp_us {
            let delta_us = u128::from(timestamp_us - self.timestamp_us);
            let frame_delta =
                delta_us.saturating_mul(u128::from(self.sample_rate_hz)) / MICROS_PER_SECOND;
            let mapped = u128::from(self.device_frame).saturating_add(frame_delta);
            mapped.min(u128::from(u64::MAX)) as u64
        } else {
            let delta_us = i128::from(self.timestamp_us - timestamp_us);
            let frame_delta = delta_us.saturating_mul(i128::from(self.sample_rate_hz))
                / MICROS_PER_SECOND as i128;
            let mapped = i128::from(self.device_frame).saturating_sub(frame_delta);
            mapped.clamp(0, i128::from(u64::MAX)) as u64
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiClockError {
    ZeroSampleRate,
    ConnectionEpochMismatch { expected: u64, actual: u64 },
    InvalidScheduleWindow { start: u64, end: u64 },
}

/// The audio scheduling window for one callback; `end_frame` is exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiScheduleWindow {
    pub start_frame: u64,
    pub end_frame: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiScheduleDecision {
    /// The timestamp mapped behind the callback and was clamped to `start_frame`.
    LateClamped,
    /// The event belongs in the current callback window.
    Current,
    /// The event maps at or beyond `end_frame` and must remain queued for a later callback.
    FutureRetained,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiTimestampMapping {
    pub device_frame: u64,
    pub decision: MidiScheduleDecision,
    /// True when this connection's raw timestamp moved backwards. In that case the mapper uses
    /// the preceding timestamp so target frames never regress because of a backend clock glitch.
    pub timestamp_regressed: bool,
    pub effective_timestamp_us: u64,
}

/// Stateful timestamp mapper belonging to exactly one MIDI input connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiTimestampMapper {
    anchor: MidiClockAnchor,
    last_timestamp_us: u64,
    has_timestamp: bool,
}

impl MidiTimestampMapper {
    pub const fn new(anchor: MidiClockAnchor) -> Self {
        Self {
            anchor,
            last_timestamp_us: anchor.timestamp_us,
            has_timestamp: false,
        }
    }

    pub const fn anchor(&self) -> MidiClockAnchor {
        self.anchor
    }

    /// Starts a new connection-specific mapping history.
    pub fn reset(&mut self, anchor: MidiClockAnchor) {
        *self = Self::new(anchor);
    }

    pub fn map_event(
        &mut self,
        event: LiveMidiEvent,
        window: MidiScheduleWindow,
    ) -> Result<MidiTimestampMapping, MidiClockError> {
        self.map_timestamp(event.connection_epoch, event.timestamp_us, window)
    }

    pub fn map_timestamp(
        &mut self,
        connection_epoch: u64,
        timestamp_us: u64,
        window: MidiScheduleWindow,
    ) -> Result<MidiTimestampMapping, MidiClockError> {
        if connection_epoch != self.anchor.connection_epoch {
            return Err(MidiClockError::ConnectionEpochMismatch {
                expected: self.anchor.connection_epoch,
                actual: connection_epoch,
            });
        }
        if window.end_frame < window.start_frame {
            return Err(MidiClockError::InvalidScheduleWindow {
                start: window.start_frame,
                end: window.end_frame,
            });
        }

        let timestamp_regressed = self.has_timestamp && timestamp_us < self.last_timestamp_us;
        let effective_timestamp_us = if timestamp_regressed {
            self.last_timestamp_us
        } else {
            timestamp_us
        };
        self.last_timestamp_us = effective_timestamp_us;
        self.has_timestamp = true;

        let mapped = self.anchor.device_frame_at(effective_timestamp_us);
        let (device_frame, decision) = if mapped < window.start_frame {
            (window.start_frame, MidiScheduleDecision::LateClamped)
        } else if mapped >= window.end_frame {
            (mapped, MidiScheduleDecision::FutureRetained)
        } else {
            (mapped, MidiScheduleDecision::Current)
        };
        Ok(MidiTimestampMapping {
            device_frame,
            decision,
            timestamp_regressed,
            effective_timestamp_us,
        })
    }
}

/// Overload importance. A must-preserve event may never be silently discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MidiEventPriority {
    Continuous,
    Discrete,
    MustPreserve,
}

/// Notes and state-reset/sustain edges must survive overload. Ordinary CC, pitch bend and channel
/// pressure are continuous controls and can use last-value-wins coalescing.
pub const fn classify_event_priority(event: LiveMidiEvent) -> MidiEventPriority {
    match event.status() & 0xf0 {
        0x80 | 0x90 => MidiEventPriority::MustPreserve,
        0xb0 if event.data[1] == 64 || event.data[1] == 120 || event.data[1] == 123 => {
            MidiEventPriority::MustPreserve
        }
        0xb0 | 0xd0 | 0xe0 => MidiEventPriority::Continuous,
        _ => MidiEventPriority::Discrete,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiScratchPush {
    Stored,
    Coalesced,
    Dropped,
    /// All 16 slots contain must-preserve edges. The caller must trigger its MIDI panic/recovery
    /// path instead of pretending the seventeenth edge was delivered.
    PanicRequired,
}

/// Fixed callback scratch implementing priority-aware last-value-wins overload handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEventScratch {
    events: [LiveMidiEvent; MIDI_SCRATCH_CAPACITY],
    len: u8,
}

impl Default for MidiEventScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl MidiEventScratch {
    pub const fn new() -> Self {
        Self {
            events: [LiveMidiEvent {
                connection_epoch: 0,
                sequence: 0,
                timestamp_us: 0,
                arrival_mono_ns: 0,
                len: 0,
                data: [0; 3],
            }; MIDI_SCRATCH_CAPACITY],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn events(&self) -> &[LiveMidiEvent] {
        &self.events[..self.len()]
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn push(&mut self, event: LiveMidiEvent) -> MidiScratchPush {
        let priority = classify_event_priority(event);
        if self.len() < MIDI_SCRATCH_CAPACITY {
            self.append(event);
            return MidiScratchPush::Stored;
        }
        // Preserve sample order until the scratch is actually overloaded. Coalescing an early
        // control across an intervening note would change the state observed by that note.
        if priority == MidiEventPriority::Continuous
            && let Some(index) = self.find_continuous_key(event)
        {
            self.remove(index);
            self.append(event);
            return MidiScratchPush::Coalesced;
        }

        let eviction = match priority {
            MidiEventPriority::MustPreserve => self
                .find_priority(MidiEventPriority::Continuous)
                .or_else(|| self.find_priority(MidiEventPriority::Discrete)),
            MidiEventPriority::Discrete => self.find_priority(MidiEventPriority::Continuous),
            MidiEventPriority::Continuous => self.find_priority(MidiEventPriority::Continuous),
        };
        if let Some(index) = eviction {
            self.remove(index);
            self.append(event);
            MidiScratchPush::Stored
        } else if priority == MidiEventPriority::MustPreserve {
            MidiScratchPush::PanicRequired
        } else {
            MidiScratchPush::Dropped
        }
    }

    fn append(&mut self, event: LiveMidiEvent) {
        debug_assert!(self.len() < MIDI_SCRATCH_CAPACITY);
        self.events[self.len()] = event;
        self.len += 1;
    }

    fn remove(&mut self, index: usize) {
        let old_len = self.len();
        self.events.copy_within(index + 1..old_len, index);
        self.len -= 1;
    }

    fn find_priority(&self, priority: MidiEventPriority) -> Option<usize> {
        self.events()
            .iter()
            .position(|&event| classify_event_priority(event) == priority)
    }

    fn find_continuous_key(&self, incoming: LiveMidiEvent) -> Option<usize> {
        let incoming_kind = incoming.status() & 0xf0;
        self.events().iter().position(|&existing| {
            if classify_event_priority(existing) != MidiEventPriority::Continuous
                || existing.connection_epoch != incoming.connection_epoch
                || existing.channel() != incoming.channel()
                || existing.status() & 0xf0 != incoming_kind
            {
                return false;
            }
            incoming_kind != 0xb0 || existing.data[1] == incoming.data[1]
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairedNoteEnd {
    NoteOff,
    TransportStop,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ActiveMidiNote {
    connection_epoch: u64,
    transport_epoch: u64,
    channel: u8,
    key: u8,
    velocity: u8,
    start_device_frame: u64,
    start_timeline_frame: u64,
    start_timestamp_us: u64,
    start_sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairedMidiNote {
    pub connection_epoch: u64,
    pub transport_epoch: u64,
    pub channel: u8,
    pub key: u8,
    pub velocity: u8,
    pub release_velocity: u8,
    pub start_device_frame: u64,
    pub end_device_frame: u64,
    pub start_timeline_frame: u64,
    pub end_timeline_frame: u64,
    pub start_timestamp_us: u64,
    pub end_timestamp_us: u64,
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub end: PairedNoteEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePairResult {
    Ignored,
    NoteStarted,
    NoteCompleted(PairedMidiNote),
    OrphanNoteOff,
    CapacityExceeded,
    /// A take cannot span a connection or transport epoch boundary. All open notes were cleared;
    /// the boundary event itself was intentionally not consumed.
    TakeInvalid {
        discarded_open_notes: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteEpochSync {
    Initialized,
    Unchanged,
    TakeInvalid { discarded_open_notes: u8 },
}

/// Allocation-free FIFO note-on/note-off pairing.
///
/// Overlapping note-ons for the same channel/key are paired with note-offs in arrival order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteFifoPairer {
    active: [ActiveMidiNote; NOTE_PAIR_CAPACITY],
    len: u8,
    connection_epoch: u64,
    transport_epoch: u64,
    has_epochs: bool,
}

impl Default for NoteFifoPairer {
    fn default() -> Self {
        Self::new()
    }
}

impl NoteFifoPairer {
    pub const fn new() -> Self {
        Self {
            active: [ActiveMidiNote {
                connection_epoch: 0,
                transport_epoch: 0,
                channel: 0,
                key: 0,
                velocity: 0,
                start_device_frame: 0,
                start_timeline_frame: 0,
                start_timestamp_us: 0,
                start_sequence: 0,
            }; NOTE_PAIR_CAPACITY],
            len: 0,
            connection_epoch: 0,
            transport_epoch: 0,
            has_epochs: false,
        }
    }

    pub const fn open_note_count(&self) -> usize {
        self.len as usize
    }

    pub fn reset(&mut self) -> u8 {
        let discarded = self.len;
        self.len = 0;
        self.has_epochs = false;
        discarded
    }

    pub fn sync_epochs(&mut self, connection_epoch: u64, transport_epoch: u64) -> NoteEpochSync {
        if !self.has_epochs {
            self.connection_epoch = connection_epoch;
            self.transport_epoch = transport_epoch;
            self.has_epochs = true;
            return NoteEpochSync::Initialized;
        }
        if self.connection_epoch == connection_epoch && self.transport_epoch == transport_epoch {
            return NoteEpochSync::Unchanged;
        }
        let discarded_open_notes = self.len;
        self.len = 0;
        self.connection_epoch = connection_epoch;
        self.transport_epoch = transport_epoch;
        NoteEpochSync::TakeInvalid {
            discarded_open_notes,
        }
    }

    /// Consumes a note edge at its already-mapped absolute device frame.
    pub fn take(
        &mut self,
        event: LiveMidiEvent,
        transport_epoch: u64,
        device_frame: u64,
    ) -> NotePairResult {
        self.take_at_frames(event, transport_epoch, device_frame, device_frame)
    }

    /// Consumes a note edge at its already-mapped absolute device and timeline frames.
    ///
    /// The two clocks are carried independently so a control-thread recorder can retain the
    /// callback's exact schedule instead of reconstructing it from floating-point tempo state.
    pub fn take_at_frames(
        &mut self,
        event: LiveMidiEvent,
        transport_epoch: u64,
        device_frame: u64,
        timeline_frame: u64,
    ) -> NotePairResult {
        if let NoteEpochSync::TakeInvalid {
            discarded_open_notes,
        } = self.sync_epochs(event.connection_epoch, transport_epoch)
        {
            return NotePairResult::TakeInvalid {
                discarded_open_notes,
            };
        }
        if event.is_note_on() {
            if self.open_note_count() == NOTE_PAIR_CAPACITY {
                return NotePairResult::CapacityExceeded;
            }
            self.active[self.open_note_count()] = ActiveMidiNote {
                connection_epoch: event.connection_epoch,
                transport_epoch,
                channel: event.channel(),
                key: event.data[1],
                velocity: event.data[2],
                start_device_frame: device_frame,
                start_timeline_frame: timeline_frame,
                start_timestamp_us: event.timestamp_us,
                start_sequence: event.sequence,
            };
            self.len += 1;
            return NotePairResult::NoteStarted;
        }
        if !event.is_note_off() {
            return NotePairResult::Ignored;
        }
        let Some(index) = self.active[..self.open_note_count()]
            .iter()
            .position(|note| note.channel == event.channel() && note.key == event.data[1])
        else {
            return NotePairResult::OrphanNoteOff;
        };
        let active = self.remove(index);
        NotePairResult::NoteCompleted(PairedMidiNote {
            connection_epoch: active.connection_epoch,
            transport_epoch: active.transport_epoch,
            channel: active.channel,
            key: active.key,
            velocity: active.velocity,
            release_velocity: event.data[2],
            start_device_frame: active.start_device_frame,
            end_device_frame: device_frame.max(active.start_device_frame),
            start_timeline_frame: active.start_timeline_frame,
            end_timeline_frame: timeline_frame.max(active.start_timeline_frame),
            start_timestamp_us: active.start_timestamp_us,
            end_timestamp_us: event.timestamp_us,
            start_sequence: active.start_sequence,
            end_sequence: event.sequence,
            end: PairedNoteEnd::NoteOff,
        })
    }

    /// Closes the oldest open note at transport stop. Call until it returns `None`.
    pub fn close_one_at(&mut self, end_device_frame: u64) -> Option<PairedMidiNote> {
        self.close_one_at_frames(end_device_frame, end_device_frame)
    }

    /// Closes the oldest open note at exact device and timeline stop frames.
    pub fn close_one_at_frames(
        &mut self,
        end_device_frame: u64,
        end_timeline_frame: u64,
    ) -> Option<PairedMidiNote> {
        if self.len == 0 {
            return None;
        }
        let active = self.remove(0);
        Some(PairedMidiNote {
            connection_epoch: active.connection_epoch,
            transport_epoch: active.transport_epoch,
            channel: active.channel,
            key: active.key,
            velocity: active.velocity,
            release_velocity: 0,
            start_device_frame: active.start_device_frame,
            end_device_frame: end_device_frame.max(active.start_device_frame),
            start_timeline_frame: active.start_timeline_frame,
            end_timeline_frame: end_timeline_frame.max(active.start_timeline_frame),
            start_timestamp_us: active.start_timestamp_us,
            end_timestamp_us: active.start_timestamp_us,
            start_sequence: active.start_sequence,
            end_sequence: active.start_sequence,
            end: PairedNoteEnd::TransportStop,
        })
    }

    /// Fills caller-owned storage with stop closures, returning the number written.
    pub fn close_all_at(&mut self, end_device_frame: u64, output: &mut [PairedMidiNote]) -> usize {
        let mut written = 0;
        while written < output.len() {
            let Some(note) = self.close_one_at(end_device_frame) else {
                break;
            };
            output[written] = note;
            written += 1;
        }
        written
    }

    /// Fills caller-owned storage with exact-clock stop closures.
    pub fn close_all_at_frames(
        &mut self,
        end_device_frame: u64,
        end_timeline_frame: u64,
        output: &mut [PairedMidiNote],
    ) -> usize {
        let mut written = 0;
        while written < output.len() {
            let Some(note) = self.close_one_at_frames(end_device_frame, end_timeline_frame) else {
                break;
            };
            output[written] = note;
            written += 1;
        }
        written
    }

    fn remove(&mut self, index: usize) -> ActiveMidiNote {
        let old_len = self.open_note_count();
        let removed = self.active[index];
        self.active.copy_within(index + 1..old_len, index);
        self.len -= 1;
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(sequence: u64, timestamp_us: u64, bytes: &[u8]) -> LiveMidiEvent {
        LiveMidiEvent::from_message(7, sequence, timestamp_us, timestamp_us * 1_000, bytes).unwrap()
    }

    #[test]
    fn validates_lengths_data_and_normalizes_zero_velocity() {
        assert_eq!(
            LiveMidiEvent::from_message(1, 2, 3, 4, &[0xc2, 9])
                .unwrap()
                .len,
            2
        );
        assert!(matches!(
            LiveMidiEvent::from_message(1, 2, 3, 4, &[0x90, 60]),
            Err(MidiMessageError::InvalidLength {
                expected: 3,
                actual: 2
            })
        ));
        assert!(matches!(
            LiveMidiEvent::from_message(1, 2, 3, 4, &[60, 100]),
            Err(MidiMessageError::RunningStatusUnsupported { .. })
        ));
        assert!(matches!(
            LiveMidiEvent::from_message(1, 2, 3, 4, &[0x90, 0x80, 1]),
            Err(MidiMessageError::InvalidDataByte { index: 1, .. })
        ));
        let normalized = LiveMidiEvent::from_message(1, 2, 3, 4, &[0x95, 60, 0]).unwrap();
        assert_eq!(normalized.data, [0x85, 60, 0]);
        assert!(normalized.is_note_off());
    }

    #[test]
    fn filters_sysex_system_common_and_realtime() {
        assert!(matches!(
            LiveMidiEvent::from_message(1, 2, 3, 4, &[0xf0, 1, 0xf7]),
            Err(MidiMessageError::FilteredSystemMessage { status: 0xf0 })
        ));
        assert!(matches!(
            LiveMidiEvent::from_message(1, 2, 3, 4, &[0xf2, 1, 2]),
            Err(MidiMessageError::FilteredSystemMessage { status: 0xf2 })
        ));
        assert!(matches!(
            LiveMidiEvent::from_message(1, 2, 3, 4, &[0xf8]),
            Err(MidiMessageError::FilteredSystemRealtime { status: 0xf8 })
        ));
        assert!(matches!(
            LiveMidiEvent::from_message(1, 2, 3, 4, &[0x90, 60, 0xf8]),
            Err(MidiMessageError::FilteredSystemRealtime { status: 0xf8 })
        ));
    }

    #[test]
    fn timestamp_mapping_classifies_late_current_future_and_regression() {
        let anchor = MidiClockAnchor::new(7, 1_000_000, 48_000, 48_000).unwrap();
        let mut mapper = MidiTimestampMapper::new(anchor);
        let window = MidiScheduleWindow {
            start_frame: 48_480,
            end_frame: 48_960,
        };
        let late = mapper.map_timestamp(7, 1_005_000, window).unwrap();
        assert_eq!(late.device_frame, 48_480);
        assert_eq!(late.decision, MidiScheduleDecision::LateClamped);

        let current = mapper.map_timestamp(7, 1_015_000, window).unwrap();
        assert_eq!(current.device_frame, 48_720);
        assert_eq!(current.decision, MidiScheduleDecision::Current);

        let future = mapper.map_timestamp(7, 1_025_000, window).unwrap();
        assert_eq!(future.device_frame, 49_200);
        assert_eq!(future.decision, MidiScheduleDecision::FutureRetained);

        let regressed = mapper.map_timestamp(7, 1_020_000, window).unwrap();
        assert!(regressed.timestamp_regressed);
        assert_eq!(regressed.effective_timestamp_us, 1_025_000);
        assert_eq!(regressed.device_frame, future.device_frame);
        assert!(matches!(
            mapper.map_timestamp(8, 1_030_000, window),
            Err(MidiClockError::ConnectionEpochMismatch { .. })
        ));
    }

    #[test]
    fn timestamp_mapping_saturates_without_float_edge_cases() {
        let high = MidiClockAnchor::new(1, 0, u64::MAX - 1, u32::MAX).unwrap();
        assert_eq!(high.device_frame_at(u64::MAX), u64::MAX);
        let low = MidiClockAnchor::new(1, u64::MAX, 1, u32::MAX).unwrap();
        assert_eq!(low.device_frame_at(0), 0);
        assert_eq!(
            MidiClockAnchor::new(1, 0, 0, 0),
            Err(MidiClockError::ZeroSampleRate)
        );
    }

    #[test]
    fn priorities_preserve_barriers_until_overload_then_coalesce() {
        assert_eq!(
            classify_event_priority(event(1, 1, &[0x90, 60, 100])),
            MidiEventPriority::MustPreserve
        );
        for cc in [64, 120, 123] {
            assert_eq!(
                classify_event_priority(event(1, 1, &[0xb0, cc, 1])),
                MidiEventPriority::MustPreserve
            );
        }
        assert_eq!(
            classify_event_priority(event(1, 1, &[0xb0, 1, 2])),
            MidiEventPriority::Continuous
        );
        assert_eq!(
            classify_event_priority(event(1, 1, &[0xe0, 1, 2])),
            MidiEventPriority::Continuous
        );
        assert_eq!(
            classify_event_priority(event(1, 1, &[0xd0, 2])),
            MidiEventPriority::Continuous
        );

        let mut scratch = MidiEventScratch::new();
        assert_eq!(
            scratch.push(event(1, 1, &[0xb0, 7, 10])),
            MidiScratchPush::Stored
        );
        scratch.push(event(2, 2, &[0x90, 60, 100]));
        assert_eq!(
            scratch.push(event(3, 3, &[0xb0, 7, 99])),
            MidiScratchPush::Stored
        );
        assert_eq!(scratch.len(), 3);
        assert_eq!(scratch.events()[0].data, [0xb0, 7, 10]);
        assert_eq!(scratch.events()[1].sequence, 2);
        assert_eq!(scratch.events()[2].data, [0xb0, 7, 99]);

        for sequence in 4..=16 {
            assert_eq!(
                scratch.push(event(sequence, sequence, &[0x90, sequence as u8, 100])),
                MidiScratchPush::Stored
            );
        }
        assert_eq!(scratch.len(), MIDI_SCRATCH_CAPACITY);
        assert_eq!(
            scratch.push(event(17, 17, &[0xb0, 7, 120])),
            MidiScratchPush::Coalesced
        );
        assert_eq!(scratch.events()[0].sequence, 2);
        assert_eq!(scratch.events()[1].sequence, 3);
        assert_eq!(scratch.events().last().unwrap().data, [0xb0, 7, 120]);
    }

    #[test]
    fn seventeenth_must_preserve_edge_requires_panic() {
        let mut scratch = MidiEventScratch::new();
        for sequence in 0..MIDI_SCRATCH_CAPACITY as u64 {
            assert_eq!(
                scratch.push(event(sequence, sequence, &[0x90, sequence as u8, 100])),
                MidiScratchPush::Stored
            );
        }
        assert_eq!(
            scratch.push(event(16, 16, &[0x80, 0, 0])),
            MidiScratchPush::PanicRequired
        );
        assert_eq!(scratch.len(), MIDI_SCRATCH_CAPACITY);
    }

    #[test]
    fn must_preserve_edge_evicts_continuous_control() {
        let mut scratch = MidiEventScratch::new();
        scratch.push(event(0, 0, &[0xb0, 1, 10]));
        for sequence in 1..MIDI_SCRATCH_CAPACITY as u64 {
            scratch.push(event(sequence, sequence, &[0x90, sequence as u8, 100]));
        }
        assert_eq!(
            scratch.push(event(17, 17, &[0x80, 90, 0])),
            MidiScratchPush::Stored
        );
        assert!(
            scratch
                .events()
                .iter()
                .all(|event| classify_event_priority(*event) == MidiEventPriority::MustPreserve)
        );
    }

    #[test]
    fn epoch_change_invalidates_take_and_clears_open_notes() {
        let mut pairer = NoteFifoPairer::new();
        assert_eq!(
            pairer.take(event(1, 10, &[0x90, 60, 100]), 4, 100),
            NotePairResult::NoteStarted
        );
        let mut changed = event(2, 20, &[0x80, 60, 0]);
        changed.connection_epoch = 8;
        assert_eq!(
            pairer.take(changed, 4, 200),
            NotePairResult::TakeInvalid {
                discarded_open_notes: 1
            }
        );
        assert_eq!(pairer.open_note_count(), 0);
        assert_eq!(
            pairer.sync_epochs(8, 5),
            NoteEpochSync::TakeInvalid {
                discarded_open_notes: 0
            }
        );
    }

    #[test]
    fn pairer_accepts_128_open_notes_and_rejects_the_129th_explicitly() {
        let mut pairer = NoteFifoPairer::new();
        for index in 0..NOTE_PAIR_CAPACITY {
            let channel = (index / 128) as u8;
            let key = (index % 128) as u8;
            assert_eq!(
                pairer.take(
                    event(index as u64, index as u64, &[0x90 | channel, key, 100]),
                    4,
                    index as u64,
                ),
                NotePairResult::NoteStarted
            );
        }
        assert_eq!(pairer.open_note_count(), 128);
        assert_eq!(
            pairer.take(event(129, 129, &[0x91, 0, 100]), 4, 129),
            NotePairResult::CapacityExceeded
        );
        assert_eq!(pairer.open_note_count(), 128);
    }

    #[test]
    fn overlapping_notes_pair_fifo_and_orphans_are_reported() {
        let mut pairer = NoteFifoPairer::new();
        pairer.take(event(1, 10, &[0x91, 60, 100]), 4, 100);
        pairer.take(event(2, 20, &[0x91, 60, 110]), 4, 200);
        let first = pairer.take(event(3, 30, &[0x81, 60, 7]), 4, 300);
        let NotePairResult::NoteCompleted(first) = first else {
            panic!("first note was not paired");
        };
        assert_eq!(first.velocity, 100);
        assert_eq!(first.start_device_frame, 100);
        assert_eq!(pairer.open_note_count(), 1);

        let second = pairer.take(event(4, 40, &[0x81, 60, 8]), 4, 400);
        let NotePairResult::NoteCompleted(second) = second else {
            panic!("second note was not paired");
        };
        assert_eq!(second.velocity, 110);
        assert_eq!(second.start_device_frame, 200);
        assert_eq!(
            pairer.take(event(5, 50, &[0x81, 60, 0]), 4, 500),
            NotePairResult::OrphanNoteOff
        );
    }

    #[test]
    fn stop_closes_open_notes_without_allocating() {
        let mut pairer = NoteFifoPairer::new();
        pairer.take(event(1, 10, &[0x90, 60, 100]), 4, 100);
        pairer.take(event(2, 20, &[0x90, 64, 110]), 4, 200);
        let placeholder = PairedMidiNote {
            connection_epoch: 0,
            transport_epoch: 0,
            channel: 0,
            key: 0,
            velocity: 0,
            release_velocity: 0,
            start_device_frame: 0,
            end_device_frame: 0,
            start_timeline_frame: 0,
            end_timeline_frame: 0,
            start_timestamp_us: 0,
            end_timestamp_us: 0,
            start_sequence: 0,
            end_sequence: 0,
            end: PairedNoteEnd::TransportStop,
        };
        let mut closed = [placeholder; 2];
        assert_eq!(pairer.close_all_at(500, &mut closed), 2);
        assert_eq!((closed[0].key, closed[1].key), (60, 64));
        assert!(
            closed.iter().all(
                |note| note.end == PairedNoteEnd::TransportStop && note.end_device_frame == 500
            )
        );
        assert_eq!(pairer.open_note_count(), 0);
    }

    #[test]
    fn core_transport_types_are_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<LiveMidiEvent>();
        assert_copy::<MidiClockAnchor>();
        assert_copy::<MidiTimestampMapper>();
        assert_copy::<MidiEventScratch>();
        assert_copy::<NoteFifoPairer>();
    }
}
