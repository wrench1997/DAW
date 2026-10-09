//! Measured stereo mixer-bus sample peaks. No meter value belongs to Project data.
//!
//! The callback reduces each completed post-fader graph bus locally, then hands
//! fixed-size snapshots to a bounded SPSC. A full queue coalesces peaks locally,
//! so a short transient is retained without blocking, allocating or doing an
//! atomic operation per sample. Identity changes discard old-generation peaks.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::mixer_graph::{MIXER_GRAPH_MAX_NODES, MixerTrackId};

const METER_QUEUE_CAPACITY: usize = 4;
pub const METER_STALE_AFTER: Duration = Duration::from_millis(250);
const PEAK_HOLD: Duration = Duration::from_secs(1);
pub const METER_FLOOR_DB: f32 = -60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeterIdentity {
    pub revision: u64,
    pub epoch: u64,
    pub graph_fingerprint: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackPeak {
    pub id: MixerTrackId,
    pub peak: [f32; 2],
    pub invalid: bool,
    reset_generation: u64,
}

impl TrackPeak {
    pub fn measure(id: MixerTrackId, samples: &[[f32; 2]]) -> Self {
        let mut result = Self {
            id,
            ..Self::default()
        };
        for frame in samples {
            for (channel, sample) in frame.iter().copied().enumerate() {
                if sample.is_finite() {
                    result.peak[channel] = result.peak[channel].max(sample.abs());
                } else {
                    result.invalid = true;
                }
            }
        }
        result
    }

    fn merge(&mut self, newer: Self) {
        if self.id != newer.id || self.reset_generation != newer.reset_generation {
            *self = newer;
            return;
        }
        for channel in 0..2 {
            self.peak[channel] = self.peak[channel].max(newer.peak[channel]);
        }
        self.invalid |= newer.invalid;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterFrame {
    pub identity: Option<MeterIdentity>,
    pub end_device_frame: u64,
    pub tracks: [TrackPeak; MIXER_GRAPH_MAX_NODES],
}

impl MeterFrame {
    fn merge(&mut self, newer: Self) {
        if self.identity != newer.identity {
            *self = newer;
            return;
        }
        self.end_device_frame = newer.end_device_frame;
        for (old, new) in self.tracks.iter_mut().zip(newer.tracks) {
            old.merge(new);
        }
    }
}

struct MeterResetRequests {
    revision: AtomicU64,
    tracks: [AtomicU64; MIXER_GRAPH_MAX_NODES],
}

pub struct MeterPublisher {
    queue: Producer<MeterFrame>,
    pending: Option<MeterFrame>,
    resets: Arc<MeterResetRequests>,
    reset_revision: u64,
    reset_generations: [u64; MIXER_GRAPH_MAX_NODES],
}

impl MeterPublisher {
    /// One atomic read per render segment; the fixed reset table is read only
    /// when a UI reset was requested. Called BEFORE sampling this block, so a
    /// mid-block click cannot relabel pre-reset audio as a new measurement.
    pub fn begin_block(&mut self) {
        let revision = self.resets.revision.load(Ordering::Acquire);
        if revision != self.reset_revision {
            for (generation, requested) in
                self.reset_generations.iter_mut().zip(&self.resets.tracks)
            {
                *generation = requested.load(Ordering::Relaxed);
            }
            self.reset_revision = revision;
        }
    }

    /// Callback-only; one bounded push and no retry, even when the UI is stalled.
    pub fn publish(&mut self, mut frame: MeterFrame) {
        for (track, generation) in frame.tracks.iter_mut().zip(self.reset_generations) {
            track.reset_generation = generation;
        }
        let mut pending = self.pending.take().unwrap_or(frame);
        pending.merge(frame);
        if let Err(rtrb::PushError::Full(frame)) = self.queue.push(pending) {
            self.pending = Some(frame);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterReading {
    pub available: bool,
    pub peak: [f32; 2],
    pub held_peak: [f32; 2],
    pub clipped: bool,
    pub invalid: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MeterReadings {
    tracks: [MeterReading; MIXER_GRAPH_MAX_NODES],
    ids: [MixerTrackId; MIXER_GRAPH_MAX_NODES],
}

impl MeterReadings {
    /// Never interpret display order, a recycled runtime slot, or ID zero as an identity.
    pub fn track(&self, runtime_slot: u8, id: MixerTrackId) -> MeterReading {
        let slot = usize::from(runtime_slot);
        if id != 0 && self.ids.get(slot) == Some(&id) {
            self.tracks[slot]
        } else {
            MeterReading::default()
        }
    }
}

pub struct MeterReader {
    queue: Consumer<MeterFrame>,
    identity: Option<MeterIdentity>,
    readings: MeterReadings,
    last_observed: Option<Instant>,
    last_device_frame: u64,
    hold_until: [[Option<Instant>; 2]; MIXER_GRAPH_MAX_NODES],
    resets: Arc<MeterResetRequests>,
    reset_generations: [u64; MIXER_GRAPH_MAX_NODES],
}

pub fn meter_channel() -> (MeterPublisher, MeterReader) {
    let (producer, consumer) = RingBuffer::new(METER_QUEUE_CAPACITY);
    let resets = Arc::new(MeterResetRequests {
        revision: AtomicU64::new(0),
        tracks: std::array::from_fn(|_| AtomicU64::new(0)),
    });
    (
        MeterPublisher {
            queue: producer,
            pending: None,
            resets: Arc::clone(&resets),
            reset_revision: 0,
            reset_generations: [0; MIXER_GRAPH_MAX_NODES],
        },
        MeterReader {
            queue: consumer,
            identity: None,
            readings: MeterReadings::default(),
            last_observed: None,
            last_device_frame: 0,
            hold_until: [[None; 2]; MIXER_GRAPH_MAX_NODES],
            resets,
            reset_generations: [0; MIXER_GRAPH_MAX_NODES],
        },
    )
}

impl MeterReader {
    fn reset_history(&mut self) {
        for (generation, request) in self.reset_generations.iter_mut().zip(&self.resets.tracks) {
            *generation = generation.wrapping_add(1);
            request.store(*generation, Ordering::Relaxed);
        }
        self.resets.revision.fetch_add(1, Ordering::Release);
    }

    fn clear(&mut self, identity: Option<MeterIdentity>) {
        self.identity = identity;
        self.readings = MeterReadings::default();
        self.last_observed = None;
        self.last_device_frame = 0;
        self.hold_until.fill([None; 2]);
    }

    /// UI-only. The exact active generation is supplied by the engine/app;
    /// stopped transport is deliberately not a condition for hiding live input.
    pub fn poll(
        &mut self,
        now: Instant,
        expected: Option<MeterIdentity>,
        device_frame: u64,
        sample_rate: u32,
    ) -> MeterReadings {
        let stale = self
            .last_observed
            .is_some_and(|last| now.saturating_duration_since(last) >= METER_STALE_AFTER);
        if self.identity != expected || stale {
            // Also discard callback-local coalesced history; clearing only the
            // UI would replay an old overload as soon as the next block arrived.
            self.reset_history();
            self.clear(expected);
            self.last_device_frame = device_frame;
        } else if expected.is_none() {
            self.clear(expected);
            self.last_device_frame = device_frame;
        }
        let mut observed: Option<MeterFrame> = None;
        // Snapshot the readable count: bound UI work even if the producer runs
        // concurrently. Older queued generations can never overwrite the active one.
        for _ in 0..self.queue.slots().min(METER_QUEUE_CAPACITY) {
            let Ok(frame) = self.queue.pop() else { break };
            if expected.is_none() || frame.identity != expected {
                continue;
            }
            if let Some(accumulated) = observed.as_mut() {
                accumulated.merge(frame);
            } else {
                observed = Some(frame);
            }
        }
        // Don't replay an old full queue after the device has continued without
        // UI service. The producer's coalesced, current frame arrives next block.
        let max_age_frames = u64::from(sample_rate.max(1)) / 4;
        if let Some(frame) = observed.filter(|frame| {
            frame.end_device_frame > self.last_device_frame
                && device_frame.saturating_sub(frame.end_device_frame) <= max_age_frames
        }) {
            self.last_observed = Some(now);
            self.last_device_frame = frame.end_device_frame;
            for (slot, track) in frame.tracks.into_iter().enumerate() {
                if track.reset_generation != self.reset_generations[slot] {
                    continue;
                }
                if track.id != self.readings.ids[slot] {
                    self.readings.tracks[slot] = MeterReading::default();
                    self.hold_until[slot] = [None; 2];
                }
                self.readings.ids[slot] = track.id;
                let reading = &mut self.readings.tracks[slot];
                reading.available = track.id != 0;
                reading.peak = track.peak;
                reading.invalid = track.invalid;
                reading.clipped |= track.peak.iter().any(|peak| *peak >= 1.0);
                for channel in 0..2 {
                    if track.peak[channel] >= reading.held_peak[channel] {
                        reading.held_peak[channel] = track.peak[channel];
                        self.hold_until[slot][channel] = Some(now + PEAK_HOLD);
                    }
                }
            }
        }
        if self
            .last_observed
            .is_none_or(|last| now.saturating_duration_since(last) >= METER_STALE_AFTER)
        {
            self.clear(expected);
            self.last_device_frame = device_frame;
        } else {
            for (slot, reading) in self.readings.tracks.iter_mut().enumerate() {
                for channel in 0..2 {
                    if self.hold_until[slot][channel].is_none_or(|end| now >= end) {
                        reading.held_peak[channel] = reading.peak[channel];
                    }
                }
            }
        }
        self.readings
    }

    /// Resets the displayed hold and clip latch only, without touching DSP or Project.
    pub fn reset_track(&mut self, runtime_slot: u8, id: MixerTrackId) {
        let slot = usize::from(runtime_slot);
        if id != 0 && self.readings.ids.get(slot) == Some(&id) {
            let reading = &mut self.readings.tracks[slot];
            reading.held_peak = reading.peak;
            reading.clipped = false;
            self.hold_until[slot] = [None; 2];
            self.reset_generations[slot] = self.reset_generations[slot].wrapping_add(1);
            self.resets.tracks[slot].store(self.reset_generations[slot], Ordering::Relaxed);
            self.resets.revision.fetch_add(1, Ordering::Release);
        }
    }
}

/// None denotes exact silence (negative infinity), never an arbitrary dB floor.
pub fn peak_dbfs(peak: f32) -> Option<f32> {
    (peak.is_finite() && peak > 0.0).then(|| 20.0 * peak.log10())
}

pub fn meter_fraction(peak: f32) -> f32 {
    peak_dbfs(peak).map_or(0.0, |db| {
        ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(revision: u64) -> MeterIdentity {
        MeterIdentity {
            revision,
            epoch: 2,
            graph_fingerprint: revision * 101,
        }
    }

    fn publish_block(publisher: &mut MeterPublisher, frame: MeterFrame) {
        publisher.begin_block();
        publisher.publish(frame);
    }

    fn frame(revision: u64, end: u64, id: u64, peak: [f32; 2]) -> MeterFrame {
        let mut frame = MeterFrame {
            identity: Some(identity(revision)),
            end_device_frame: end,
            ..MeterFrame::default()
        };
        frame.tracks[1] = TrackPeak {
            id,
            peak,
            invalid: false,
            ..TrackPeak::default()
        };
        frame
    }

    #[test]
    fn stereo_reduction_dbfs_and_invalid_samples_are_truthful_and_non_mutating() {
        let samples = [[0.25, -0.5], [-0.125, 0.125], [0.0, -0.0]];
        let before = samples;
        let measured = TrackPeak::measure(77, &samples);
        assert_eq!(samples, before);
        assert_eq!(measured.peak, [0.25, 0.5]);
        assert!(!measured.invalid);
        assert!((peak_dbfs(0.5).unwrap() + 6.020_6).abs() < 0.000_1);
        assert_eq!(peak_dbfs(1.0), Some(0.0));
        assert_eq!(peak_dbfs(0.0), None);
        assert_eq!(meter_fraction(0.0), 0.0);
        assert_eq!(meter_fraction(2.0), 1.0);
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let measured = TrackPeak::measure(77, &[[value, 0.25]]);
            assert!(measured.invalid);
            assert_eq!(measured.peak, [0.0, 0.25]);
            assert_eq!(peak_dbfs(value), None);
            assert_eq!(meter_fraction(value), 0.0);
        }
        assert!(peak_dbfs(f32::MAX).unwrap().is_finite());
        assert_eq!(TrackPeak::measure(77, &[]).peak, [0.0; 2]);
    }

    #[test]
    fn full_queue_retains_a_one_sample_overload_without_waiting() {
        let (mut publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity(1)), 0, 48_000);
        for end in 1..=METER_QUEUE_CAPACITY as u64 {
            publish_block(&mut publisher, frame(1, end, 77, [0.0; 2]));
        }
        publish_block(&mut publisher, frame(1, 5, 77, [1.25, 0.25]));
        publish_block(&mut publisher, frame(1, 6, 77, [0.0; 2]));
        assert_eq!(
            reader
                .poll(now, Some(identity(1)), 6, 48_000)
                .track(1, 77)
                .peak,
            [0.0; 2]
        );
        publish_block(&mut publisher, frame(1, 7, 77, [0.0; 2]));
        let reading = reader.poll(now, Some(identity(1)), 7, 48_000).track(1, 77);
        assert_eq!(reading.peak, [1.25, 0.25]);
        assert!(reading.clipped);
        publish_block(&mut publisher, frame(1, 8, 77, [0.0; 2]));
        let reading = reader.poll(now, Some(identity(1)), 8, 48_000).track(1, 77);
        assert_eq!(reading.peak, [0.0; 2]);
        assert!(reading.clipped);
    }

    #[test]
    fn reset_cannot_relatch_queued_pending_or_mid_block_pre_reset_overloads() {
        let (mut publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity(1)), 0, 48_000);
        publisher.begin_block();
        publisher.publish(frame(1, 1, 77, [1.25; 2]));
        assert!(
            reader
                .poll(now, Some(identity(1)), 1, 48_000)
                .track(1, 77)
                .clipped
        );
        for end in 2..=6 {
            publisher.publish(frame(1, end, 77, [2.0; 2]));
        }
        reader.reset_track(1, 77);
        assert!(
            !reader
                .poll(now, Some(identity(1)), 6, 48_000)
                .track(1, 77)
                .clipped
        );
        // A callback that started before the click still owns the old reset tag.
        publisher.publish(frame(1, 7, 77, [3.0; 2]));
        assert!(
            !reader
                .poll(now, Some(identity(1)), 7, 48_000)
                .track(1, 77)
                .clipped
        );
        publisher.begin_block();
        publisher.publish(frame(1, 8, 77, [0.0; 2]));
        let reading = reader.poll(now, Some(identity(1)), 8, 48_000).track(1, 77);
        assert_eq!(reading.peak, [0.0; 2]);
        assert!(!reading.clipped);
        publisher.publish(frame(1, 9, 77, [1.5; 2]));
        assert!(
            reader
                .poll(now, Some(identity(1)), 9, 48_000)
                .track(1, 77)
                .clipped
        );
        // Reset while both queued and pending old overloads remain; only newly
        // observed samples may set the next latch, not the pending maximum.
        for end in 10..=14 {
            publisher.publish(frame(1, end, 77, [4.0; 2]));
        }
        reader.reset_track(1, 77);
        reader.poll(now, Some(identity(1)), 14, 48_000);
        publisher.begin_block();
        publisher.publish(frame(1, 15, 77, [0.25; 2]));
        let reading = reader.poll(now, Some(identity(1)), 15, 48_000).track(1, 77);
        assert_eq!(reading.peak, [0.25; 2]);
        assert!(!reading.clipped);
    }

    #[test]
    fn identity_revision_epoch_and_recycled_slot_never_inherit_peaks() {
        let (mut publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity(1)), 0, 48_000);
        publish_block(&mut publisher, frame(1, 1, 77, [2.0; 2]));
        let readings = reader.poll(now, Some(identity(1)), 1, 48_000);
        assert!(readings.track(1, 77).clipped);
        assert!(!readings.track(1, 88).available);
        assert!(!readings.track(2, 77).available);
        publish_block(&mut publisher, frame(1, 2, 88, [0.25; 2]));
        let readings = reader.poll(now, Some(identity(1)), 2, 48_000);
        assert!(!readings.track(1, 77).available);
        assert!(!readings.track(1, 88).clipped);
        reader.poll(now, Some(identity(2)), 2, 48_000);
        publish_block(&mut publisher, frame(1, 3, 77, [3.0; 2]));
        publish_block(&mut publisher, frame(2, 4, 77, [0.125; 2]));
        let reading = reader.poll(now, Some(identity(2)), 4, 48_000).track(1, 77);
        assert_eq!(reading.peak, [0.125; 2]);
        assert!(!reading.clipped);
        let changed_epoch = MeterIdentity {
            epoch: 3,
            ..identity(2)
        };
        assert!(
            !reader
                .poll(now, Some(changed_epoch), 4, 48_000)
                .track(1, 77)
                .available
        );
    }

    #[test]
    fn full_pending_queue_discards_previous_graph_peak_on_replacement() {
        let (mut publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity(1)), 0, 48_000);
        for end in 1..=5 {
            publish_block(&mut publisher, frame(1, end, 77, [4.0; 2]));
        }
        publish_block(&mut publisher, frame(2, 6, 88, [0.0; 2]));
        reader.poll(now, Some(identity(2)), 6, 48_000);
        publish_block(&mut publisher, frame(2, 7, 88, [0.25; 2]));
        let reading = reader.poll(now, Some(identity(2)), 7, 48_000).track(1, 88);
        assert_eq!(reading.peak, [0.25; 2]);
        assert!(!reading.clipped);
    }

    #[test]
    fn holds_expire_on_live_silence_and_click_resets_only_the_selected_latch() {
        let (mut publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity(1)), 0, 48_000);
        publish_block(&mut publisher, frame(1, 1, 77, [1.0, 0.5]));
        reader.poll(now, Some(identity(1)), 1, 48_000);
        for tick in 1..=11 {
            publish_block(&mut publisher, frame(1, tick + 1, 77, [0.0; 2]));
            let reading = reader
                .poll(
                    now + Duration::from_millis(tick * 100),
                    Some(identity(1)),
                    tick + 1,
                    48_000,
                )
                .track(1, 77);
            assert_eq!(reading.peak, [0.0; 2]);
            assert!(reading.clipped);
            assert_eq!(
                reading.held_peak,
                if tick < 10 { [1.0, 0.5] } else { [0.0; 2] }
            );
        }
        reader.reset_track(1, 88);
        assert!(reader.readings.track(1, 77).clipped);
        reader.reset_track(1, 77);
        assert!(!reader.readings.track(1, 77).clipped);
        assert_eq!(reader.readings.track(1, 77).held_peak, [0.0; 2]);
    }

    #[test]
    fn stale_or_no_device_clears_reading_and_backlog_needs_new_callback_evidence() {
        let (mut publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity(1)), 0, 48_000);
        publish_block(&mut publisher, frame(1, 1, 77, [2.0; 2]));
        assert!(
            reader
                .poll(now, Some(identity(1)), 1, 48_000)
                .track(1, 77)
                .available
        );
        publish_block(&mut publisher, frame(1, 2, 77, [3.0; 2]));
        let later = now + METER_STALE_AFTER;
        assert!(
            !reader
                .poll(later, Some(identity(1)), 2, 48_000)
                .track(1, 77)
                .available
        );
        publish_block(&mut publisher, frame(1, 3, 77, [0.25; 2]));
        let reading = reader
            .poll(later, Some(identity(1)), 3, 48_000)
            .track(1, 77);
        assert!(reading.available);
        assert!(!reading.clipped);
        assert!(!reader.poll(later, None, 3, 48_000).track(1, 77).available);
        let (_, mut replacement) = meter_channel();
        assert!(
            !replacement
                .poll(later, Some(identity(1)), 0, 48_000)
                .track(1, 77)
                .available
        );
    }

    #[test]
    fn stale_reset_also_discards_callback_local_pending_peak_history() {
        let (mut publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity(1)), 0, 48_000);
        publish_block(&mut publisher, frame(1, 1, 77, [0.25; 2]));
        reader.poll(now, Some(identity(1)), 1, 48_000);
        for end in 2..=6 {
            publish_block(&mut publisher, frame(1, end, 77, [3.0; 2]));
        }
        let later = now + METER_STALE_AFTER;
        assert!(
            !reader
                .poll(later, Some(identity(1)), 6, 48_000)
                .track(1, 77)
                .available
        );
        publish_block(&mut publisher, frame(1, 7, 77, [0.0; 2]));
        let reading = reader
            .poll(later, Some(identity(1)), 7, 48_000)
            .track(1, 77);
        assert!(reading.available);
        assert_eq!(reading.peak, [0.0; 2]);
        assert!(!reading.clipped);
    }

    #[test]
    fn late_frames_and_nonfinite_faults_are_not_reported_as_current_valid_audio() {
        let (mut publisher, mut reader) = meter_channel();
        let now = Instant::now();
        reader.poll(now, Some(identity(1)), 0, 48_000);
        publish_block(&mut publisher, frame(1, 1, 77, [2.0; 2]));
        assert!(
            !reader
                .poll(now, Some(identity(1)), 48_000, 48_000)
                .track(1, 77)
                .available
        );
        let mut invalid = frame(1, 48_001, 77, [0.0; 2]);
        invalid.tracks[1] = TrackPeak::measure(77, &[[f32::NAN, f32::INFINITY]]);
        publish_block(&mut publisher, invalid);
        let reading = reader
            .poll(now, Some(identity(1)), 48_001, 48_000)
            .track(1, 77);
        assert!(reading.invalid);
        assert_eq!(reading.peak, [0.0; 2]);
    }
}
