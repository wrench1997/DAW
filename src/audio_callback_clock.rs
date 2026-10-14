//! Opt-in observations of the backend's stream clock, independent of rendering.
//!
//! The output callback owns one fixed-size observer. It reads only the CPAL
//! timestamps supplied to that callback: no extra clock calls, locks, IPC,
//! allocation, or logging. Intervals shorter than half or longer than one and
//! a half times the previous raw buffer duration are diagnostic heuristics,
//! not driver qualifications or plugin deadline decisions.

use std::{
    ffi::OsStr,
    sync::atomic::{AtomicU64, Ordering},
};

use cpal::{OutputStreamTimestamp, StreamInstant};

const NANOS_PER_SECOND: u128 = 1_000_000_000;
const SNAPSHOT_READ_ATTEMPTS: usize = 3;
const COMMON_WORDS: usize = 7;
const STAMP_WORDS: usize = 12;
const SNAPSHOT_WORDS: usize = COMMON_WORDS + 2 * STAMP_WORDS;

/// Measurements for either the callback or predicted playback timestamp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallbackClockStampSnapshot {
    /// Numerically representable intervals, including explicitly counted zeroes.
    pub interval_count: u64,
    /// None on the first callback, a backwards stamp, or duration overflow.
    pub last_interval_ns: Option<u64>,
    pub minimum_interval_ns: Option<u64>,
    pub maximum_interval_ns: Option<u64>,
    pub repeated_timestamps: u64,
    pub backwards_timestamps: u64,
    /// Positive intervals below half the previous raw buffer's duration.
    pub short_bursts: u64,
    /// Intervals above one and a half times that duration.
    pub long_intervals: u64,
    /// Elapsed since the first timestamp; invalid after any clock reversal.
    pub elapsed_ns: Option<u64>,
    /// Timestamp elapsed minus the duration of preceding raw frames.
    pub deviation_ns: Option<i64>,
    pub maximum_absolute_deviation_ns: u64,
}

/// Independent clock diagnostics, copied without waiting for a callback writer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallbackClockSnapshot {
    pub enabled: bool,
    /// False when the bounded read overlaps publication. No torn counters are
    /// returned as evidence in that case.
    pub coherent: bool,
    /// Set on incoherence, arithmetic overflow, an invalid rate, or a timestamp
    /// reversal. Ordinary jitter is still reported and is not hidden by this.
    pub inconclusive: bool,
    pub sample_rate: u32,
    pub callback_count: u64,
    pub total_frames: u64,
    pub last_frames: u64,
    /// Based on the previous callback's frames, never the current buffer size.
    pub expected_interval_ns: Option<u64>,
    /// Duration of all frames before the current callback, excluding its buffer.
    pub expected_elapsed_ns: Option<u64>,
    pub callback: CallbackClockStampSnapshot,
    pub playback: CallbackClockStampSnapshot,
    /// A timestamp duration or signed deviation cannot be represented in ns.
    pub duration_overflow: bool,
    pub counter_overflow: bool,
    pub invalid_sample_rate: bool,
}

/// Single-writer, bounded publication. The stream's observer is the only writer;
/// control-thread initialization finishes before CPAL can invoke that observer.
#[derive(Debug)]
pub struct CallbackClockTelemetry {
    revision: AtomicU64,
    words: [AtomicU64; SNAPSHOT_WORDS],
}

impl Default for CallbackClockTelemetry {
    fn default() -> Self {
        Self {
            revision: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl CallbackClockTelemetry {
    fn publish(&self, snapshot: CallbackClockSnapshot) {
        // No compare-exchange loop: the unique stream observer serializes writes.
        // Sequentially consistent atomic words also keep the reader's coherence
        // proof independent of platform/compiler seqlock reordering details.
        let revision = self.revision.load(Ordering::SeqCst);
        self.revision
            .store(revision.wrapping_add(1), Ordering::SeqCst);
        for (word, value) in self.words.iter().zip(snapshot.to_words()) {
            word.store(value, Ordering::SeqCst);
        }
        self.revision
            .store(revision.wrapping_add(2), Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> CallbackClockSnapshot {
        for _ in 0..SNAPSHOT_READ_ATTEMPTS {
            let before = self.revision.load(Ordering::SeqCst);
            if before & 1 != 0 {
                continue;
            }
            let words = std::array::from_fn(|index| self.words[index].load(Ordering::SeqCst));
            let after = self.revision.load(Ordering::SeqCst);
            if before == after && after & 1 == 0 {
                return CallbackClockSnapshot::from_words(words);
            }
        }
        CallbackClockSnapshot {
            // A single enable flag remains meaningful even when the counters
            // cannot be read together. Every measurement is discarded here.
            enabled: self.words[0].load(Ordering::SeqCst) & 1 != 0,
            coherent: false,
            inconclusive: true,
            ..CallbackClockSnapshot::default()
        }
    }
}

/// Fixed-size history owned by the actual raw CPAL output callback.
#[derive(Debug)]
pub struct CallbackClockObserver {
    snapshot: CallbackClockSnapshot,
    first: Option<OutputStreamTimestamp>,
    previous: Option<OutputStreamTimestamp>,
    previous_frames: u64,
}

impl CallbackClockObserver {
    /// Control-thread operation. Only the literal value `1` enables observation.
    pub fn from_environment(sample_rate: u32, telemetry: &CallbackClockTelemetry) -> Self {
        let flag = std::env::var_os("CITRUS_STUDIO_PLUGIN_DIAGNOSTICS");
        let enabled = diagnostics_enabled(flag.as_deref());
        Self::new(enabled, sample_rate, telemetry)
    }

    pub fn new(enabled: bool, sample_rate: u32, telemetry: &CallbackClockTelemetry) -> Self {
        let snapshot = CallbackClockSnapshot {
            enabled,
            coherent: true,
            sample_rate,
            invalid_sample_rate: enabled && sample_rate == 0,
            inconclusive: enabled && sample_rate == 0,
            ..CallbackClockSnapshot::default()
        };
        telemetry.publish(snapshot);
        Self {
            snapshot,
            first: None,
            previous: None,
            previous_frames: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.snapshot.enabled
    }

    pub fn observe(
        &mut self,
        timestamp: OutputStreamTimestamp,
        raw_frames: usize,
        telemetry: &CallbackClockTelemetry,
    ) {
        if !self.is_enabled() {
            return;
        }
        let mut counter_overflow = self.snapshot.counter_overflow;
        let mut duration_overflow = self.snapshot.duration_overflow;
        let preceding_frames = self.snapshot.total_frames;
        let frames = match u64::try_from(raw_frames) {
            Ok(frames) => frames,
            Err(_) => {
                counter_overflow = true;
                u64::MAX
            }
        };
        checked_increment(&mut self.snapshot.callback_count, &mut counter_overflow);
        self.snapshot.total_frames = match preceding_frames.checked_add(frames) {
            Some(total) => total,
            None => {
                counter_overflow = true;
                u64::MAX
            }
        };
        self.snapshot.last_frames = frames;
        self.snapshot.expected_interval_ns = self.previous.and_then(|_| {
            frames_duration_ns(
                self.previous_frames,
                self.snapshot.sample_rate,
                &mut duration_overflow,
            )
        });
        self.snapshot.expected_elapsed_ns = if counter_overflow {
            None
        } else {
            frames_duration_ns(
                preceding_frames,
                self.snapshot.sample_rate,
                &mut duration_overflow,
            )
        };
        let first = *self.first.get_or_insert(timestamp);
        self.snapshot.callback.observe(
            timestamp.callback,
            self.previous.map(|previous| previous.callback),
            first.callback,
            self.snapshot.expected_interval_ns,
            self.snapshot.expected_elapsed_ns,
            &mut duration_overflow,
            &mut counter_overflow,
        );
        self.snapshot.playback.observe(
            timestamp.playback,
            self.previous.map(|previous| previous.playback),
            first.playback,
            self.snapshot.expected_interval_ns,
            self.snapshot.expected_elapsed_ns,
            &mut duration_overflow,
            &mut counter_overflow,
        );
        self.previous = Some(timestamp);
        self.previous_frames = frames;
        self.snapshot.duration_overflow = duration_overflow;
        self.snapshot.counter_overflow = counter_overflow;
        self.snapshot.inconclusive = duration_overflow
            || counter_overflow
            || self.snapshot.invalid_sample_rate
            || self.snapshot.callback.backwards_timestamps != 0
            || self.snapshot.playback.backwards_timestamps != 0;
        telemetry.publish(self.snapshot);
    }
}

impl CallbackClockStampSnapshot {
    #[allow(clippy::too_many_arguments)]
    fn observe(
        &mut self,
        current: StreamInstant,
        previous: Option<StreamInstant>,
        first: StreamInstant,
        expected_interval_ns: Option<u64>,
        expected_elapsed_ns: Option<u64>,
        duration_overflow: &mut bool,
        counter_overflow: &mut bool,
    ) {
        self.last_interval_ns = None;
        if let Some(previous) = previous {
            match current.checked_duration_since(previous) {
                None => checked_increment(&mut self.backwards_timestamps, counter_overflow),
                Some(duration) => {
                    if duration.is_zero() {
                        checked_increment(&mut self.repeated_timestamps, counter_overflow);
                    }
                    if let Some(interval) = checked_ns(duration.as_nanos(), duration_overflow) {
                        checked_increment(&mut self.interval_count, counter_overflow);
                        self.last_interval_ns = Some(interval);
                        self.minimum_interval_ns = Some(
                            self.minimum_interval_ns
                                .map_or(interval, |old| old.min(interval)),
                        );
                        self.maximum_interval_ns = Some(
                            self.maximum_interval_ns
                                .map_or(interval, |old| old.max(interval)),
                        );
                        if let Some(expected) = expected_interval_ns.filter(|value| *value != 0) {
                            if interval != 0 && u128::from(interval) * 2 < u128::from(expected) {
                                checked_increment(&mut self.short_bursts, counter_overflow);
                            }
                            if u128::from(interval) * 2 > u128::from(expected) * 3 {
                                checked_increment(&mut self.long_intervals, counter_overflow);
                            }
                        }
                    }
                }
            }
        }
        self.elapsed_ns = if self.backwards_timestamps == 0 {
            current
                .checked_duration_since(first)
                .and_then(|duration| checked_ns(duration.as_nanos(), duration_overflow))
        } else {
            None
        };
        self.deviation_ns = match (self.elapsed_ns, expected_elapsed_ns) {
            (Some(actual), Some(expected)) => {
                match i64::try_from(i128::from(actual) - i128::from(expected)) {
                    Ok(deviation) => {
                        self.maximum_absolute_deviation_ns = self
                            .maximum_absolute_deviation_ns
                            .max(deviation.unsigned_abs());
                        Some(deviation)
                    }
                    Err(_) => {
                        *duration_overflow = true;
                        None
                    }
                }
            }
            _ => None,
        };
    }

    fn to_words(self) -> [u64; STAMP_WORDS] {
        let present = u64::from(self.last_interval_ns.is_some())
            | (u64::from(self.minimum_interval_ns.is_some()) << 1)
            | (u64::from(self.maximum_interval_ns.is_some()) << 2)
            | (u64::from(self.elapsed_ns.is_some()) << 3)
            | (u64::from(self.deviation_ns.is_some()) << 4);
        [
            present,
            self.interval_count,
            self.last_interval_ns.unwrap_or(0),
            self.minimum_interval_ns.unwrap_or(0),
            self.maximum_interval_ns.unwrap_or(0),
            self.repeated_timestamps,
            self.backwards_timestamps,
            self.short_bursts,
            self.long_intervals,
            self.elapsed_ns.unwrap_or(0),
            u64::from_ne_bytes(self.deviation_ns.unwrap_or(0).to_ne_bytes()),
            self.maximum_absolute_deviation_ns,
        ]
    }

    fn from_words(words: &[u64]) -> Self {
        Self {
            interval_count: words[1],
            last_interval_ns: (words[0] & 1 != 0).then_some(words[2]),
            minimum_interval_ns: (words[0] & 2 != 0).then_some(words[3]),
            maximum_interval_ns: (words[0] & 4 != 0).then_some(words[4]),
            repeated_timestamps: words[5],
            backwards_timestamps: words[6],
            short_bursts: words[7],
            long_intervals: words[8],
            elapsed_ns: (words[0] & 8 != 0).then_some(words[9]),
            deviation_ns: (words[0] & 16 != 0)
                .then_some(i64::from_ne_bytes(words[10].to_ne_bytes())),
            maximum_absolute_deviation_ns: words[11],
        }
    }
}

impl CallbackClockSnapshot {
    fn to_words(self) -> [u64; SNAPSHOT_WORDS] {
        let flags = u64::from(self.enabled)
            | (u64::from(self.inconclusive) << 1)
            | (u64::from(self.duration_overflow) << 2)
            | (u64::from(self.counter_overflow) << 3)
            | (u64::from(self.expected_interval_ns.is_some()) << 4)
            | (u64::from(self.expected_elapsed_ns.is_some()) << 5)
            | (u64::from(self.invalid_sample_rate) << 6);
        let mut words = [0; SNAPSHOT_WORDS];
        words[..COMMON_WORDS].copy_from_slice(&[
            flags,
            u64::from(self.sample_rate),
            self.callback_count,
            self.total_frames,
            self.last_frames,
            self.expected_interval_ns.unwrap_or(0),
            self.expected_elapsed_ns.unwrap_or(0),
        ]);
        words[COMMON_WORDS..COMMON_WORDS + STAMP_WORDS].copy_from_slice(&self.callback.to_words());
        words[COMMON_WORDS + STAMP_WORDS..].copy_from_slice(&self.playback.to_words());
        words
    }

    fn from_words(words: [u64; SNAPSHOT_WORDS]) -> Self {
        Self {
            enabled: words[0] & 1 != 0,
            coherent: true,
            inconclusive: words[0] & 2 != 0,
            duration_overflow: words[0] & 4 != 0,
            counter_overflow: words[0] & 8 != 0,
            expected_interval_ns: (words[0] & 16 != 0).then_some(words[5]),
            expected_elapsed_ns: (words[0] & 32 != 0).then_some(words[6]),
            invalid_sample_rate: words[0] & 64 != 0,
            sample_rate: words[1] as u32,
            callback_count: words[2],
            total_frames: words[3],
            last_frames: words[4],
            callback: CallbackClockStampSnapshot::from_words(
                &words[COMMON_WORDS..COMMON_WORDS + STAMP_WORDS],
            ),
            playback: CallbackClockStampSnapshot::from_words(&words[COMMON_WORDS + STAMP_WORDS..]),
        }
    }
}

fn checked_increment(counter: &mut u64, overflow: &mut bool) {
    match counter.checked_add(1) {
        Some(value) => *counter = value,
        None => *overflow = true,
    }
}

fn diagnostics_enabled(flag: Option<&OsStr>) -> bool {
    flag.is_some_and(|value| value == "1")
}

fn checked_ns(nanoseconds: u128, overflow: &mut bool) -> Option<u64> {
    match u64::try_from(nanoseconds) {
        Ok(value) => Some(value),
        Err(_) => {
            *overflow = true;
            None
        }
    }
}

fn frames_duration_ns(frames: u64, sample_rate: u32, overflow: &mut bool) -> Option<u64> {
    if sample_rate == 0 {
        return None;
    }
    checked_ns(
        u128::from(frames) * NANOS_PER_SECOND / u128::from(sample_rate),
        overflow,
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread, time::Duration};

    use super::*;

    fn stamp(callback_ms: u64, playback_ms: u64) -> OutputStreamTimestamp {
        OutputStreamTimestamp {
            callback: StreamInstant::ZERO
                .checked_add(Duration::from_millis(callback_ms))
                .unwrap(),
            playback: StreamInstant::ZERO
                .checked_add(Duration::from_millis(playback_ms))
                .unwrap(),
        }
    }

    #[test]
    fn only_the_literal_environment_flag_enables_clock_diagnostics() {
        assert!(!diagnostics_enabled(None));
        for flag in ["", "0", "true", "yes", " 1", "1 ", "01"] {
            assert!(!diagnostics_enabled(Some(OsStr::new(flag))));
        }
        assert!(diagnostics_enabled(Some(OsStr::new("1"))));
    }

    #[test]
    fn raw_480_frames_at_48khz_have_ten_ms_intervals_and_zero_drift() {
        let telemetry = CallbackClockTelemetry::default();
        let mut observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        for index in 0..5 {
            observer.observe(stamp(index * 10, 30 + index * 10), 480, &telemetry);
        }
        let snapshot = telemetry.snapshot();
        assert!(snapshot.enabled && snapshot.coherent && !snapshot.inconclusive);
        assert_eq!(snapshot.callback_count, 5);
        assert_eq!(snapshot.total_frames, 2_400);
        assert_eq!(snapshot.expected_interval_ns, Some(10_000_000));
        assert_eq!(snapshot.expected_elapsed_ns, Some(40_000_000));
        for clock in [snapshot.callback, snapshot.playback] {
            assert_eq!(clock.interval_count, 4);
            assert_eq!(clock.last_interval_ns, Some(10_000_000));
            assert_eq!(clock.minimum_interval_ns, Some(10_000_000));
            assert_eq!(clock.maximum_interval_ns, Some(10_000_000));
            assert_eq!(clock.elapsed_ns, Some(40_000_000));
            assert_eq!(clock.deviation_ns, Some(0));
            assert_eq!(clock.maximum_absolute_deviation_ns, 0);
            assert_eq!(clock.short_bursts + clock.long_intervals, 0);
            assert_eq!(clock.backwards_timestamps + clock.repeated_timestamps, 0);
        }
    }

    #[test]
    fn a_long_gap_then_two_short_bursts_preserves_raw_timing_evidence() {
        let telemetry = CallbackClockTelemetry::default();
        let mut observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        for callback_ms in [0, 50, 52, 54] {
            observer.observe(stamp(callback_ms, callback_ms + 30), 480, &telemetry);
        }
        let snapshot = telemetry.snapshot();
        for clock in [snapshot.callback, snapshot.playback] {
            assert_eq!(clock.interval_count, 3);
            assert_eq!(clock.minimum_interval_ns, Some(2_000_000));
            assert_eq!(clock.maximum_interval_ns, Some(50_000_000));
            assert_eq!(clock.long_intervals, 1);
            assert_eq!(clock.short_bursts, 2);
            assert_eq!(clock.elapsed_ns, Some(54_000_000));
            assert_eq!(clock.deviation_ns, Some(24_000_000));
            assert_eq!(clock.maximum_absolute_deviation_ns, 40_000_000);
        }
        assert_eq!(snapshot.expected_elapsed_ns, Some(30_000_000));
    }

    #[test]
    fn variable_raw_buffers_compare_against_previous_frames() {
        let telemetry = CallbackClockTelemetry::default();
        let mut observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        observer.observe(stamp(0, 30), 480, &telemetry);
        observer.observe(stamp(10, 40), 960, &telemetry);
        assert_eq!(telemetry.snapshot().expected_interval_ns, Some(10_000_000));
        observer.observe(stamp(30, 60), 480, &telemetry);
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.expected_interval_ns, Some(20_000_000));
        assert_eq!(snapshot.expected_elapsed_ns, Some(30_000_000));
        assert_eq!(snapshot.total_frames, 1_920);
        assert_eq!(snapshot.callback.minimum_interval_ns, Some(10_000_000));
        assert_eq!(snapshot.callback.maximum_interval_ns, Some(20_000_000));
        assert_eq!(snapshot.callback.deviation_ns, Some(0));
        assert_eq!(snapshot.callback.long_intervals, 0);
        assert_eq!(snapshot.callback.short_bursts, 0);
    }

    #[test]
    fn repeated_and_backwards_stamps_are_distinct_and_never_saturated() {
        let telemetry = CallbackClockTelemetry::default();
        let mut observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        observer.observe(stamp(100, 130), 480, &telemetry);
        observer.observe(stamp(100, 140), 480, &telemetry);
        let repeated = telemetry.snapshot();
        assert_eq!(repeated.callback.last_interval_ns, Some(0));
        assert_eq!(repeated.callback.repeated_timestamps, 1);
        assert_eq!(repeated.callback.backwards_timestamps, 0);
        assert_eq!(repeated.callback.deviation_ns, Some(-10_000_000));
        observer.observe(stamp(90, 140), 480, &telemetry);
        let reversed = telemetry.snapshot();
        assert!(reversed.inconclusive && reversed.coherent);
        assert_eq!(reversed.callback.last_interval_ns, None);
        assert_eq!(reversed.callback.backwards_timestamps, 1);
        assert_eq!(reversed.callback.repeated_timestamps, 1);
        assert_eq!(reversed.callback.elapsed_ns, None);
        assert_eq!(reversed.callback.deviation_ns, None);
        assert_eq!(reversed.playback.repeated_timestamps, 1);
        observer.observe(stamp(110, 120), 480, &telemetry);
        let playback_reversed = telemetry.snapshot();
        assert_eq!(playback_reversed.playback.backwards_timestamps, 1);
        assert_eq!(playback_reversed.playback.last_interval_ns, None);
        assert_eq!(playback_reversed.playback.elapsed_ns, None);
        assert_eq!(playback_reversed.playback.deviation_ns, None);
        assert_eq!(playback_reversed.callback.elapsed_ns, None);
    }

    #[test]
    fn disabled_clock_observation_keeps_existing_raw_size_telemetry() {
        let telemetry = super::super::CallbackTelemetry::default();
        let mut observer = CallbackClockObserver::new(false, 48_000, &telemetry.clock);
        for (frames, callback_ms) in [(480, 100), (960, 0)] {
            telemetry.observe_callback(frames * 2, 2);
            telemetry.observe_clock_callback(&mut observer, stamp(callback_ms, 0), frames);
        }
        let sizes = telemetry.snapshot();
        assert_eq!(sizes.callback_count, 2);
        assert_eq!(sizes.total_frames, 1_440);
        assert_eq!(sizes.minimum_frames, Some(480));
        assert_eq!(sizes.maximum_frames, Some(960));
        assert_eq!(sizes.size_changes, 1);
        let clock = telemetry.clock_snapshot();
        assert!(!clock.enabled && clock.coherent && !clock.inconclusive);
        assert_eq!(clock.callback_count, 0);
        assert_eq!(clock.total_frames, 0);
        assert_eq!(clock.callback.last_interval_ns, None);
    }

    #[test]
    fn huge_durations_and_invalid_rates_are_explicitly_inconclusive() {
        let telemetry = CallbackClockTelemetry::default();
        let mut observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        observer.observe(stamp(0, 0), 480, &telemetry);
        let huge = StreamInstant::ZERO
            .checked_add(Duration::from_secs(u64::MAX))
            .unwrap();
        observer.observe(
            OutputStreamTimestamp {
                callback: huge,
                playback: huge,
            },
            480,
            &telemetry,
        );
        let snapshot = telemetry.snapshot();
        assert!(snapshot.duration_overflow && snapshot.inconclusive);
        assert_eq!(snapshot.callback.last_interval_ns, None);
        assert_eq!(snapshot.callback.elapsed_ns, None);
        assert_eq!(snapshot.playback.elapsed_ns, None);
        let invalid_telemetry = CallbackClockTelemetry::default();
        let mut invalid = CallbackClockObserver::new(true, 0, &invalid_telemetry);
        invalid.observe(stamp(0, 0), 480, &invalid_telemetry);
        invalid.observe(stamp(10, 10), 480, &invalid_telemetry);
        let invalid = invalid_telemetry.snapshot();
        assert!(invalid.invalid_sample_rate && invalid.inconclusive);
        assert_eq!(invalid.expected_interval_ns, None);
        assert_eq!(invalid.expected_elapsed_ns, None);
        assert_eq!(invalid.callback.last_interval_ns, Some(10_000_000));
    }

    #[test]
    fn counter_overflow_retains_explicit_failure_instead_of_wrapping() {
        let telemetry = CallbackClockTelemetry::default();
        let mut observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        observer.snapshot.callback_count = u64::MAX;
        observer.snapshot.total_frames = u64::MAX - 4;
        observer.observe(stamp(0, 0), 480, &telemetry);
        let snapshot = telemetry.snapshot();
        assert!(snapshot.counter_overflow && snapshot.inconclusive);
        assert_eq!(snapshot.callback_count, u64::MAX);
        assert_eq!(snapshot.total_frames, u64::MAX);
        assert_eq!(snapshot.expected_elapsed_ns, None);
        assert_eq!(snapshot.callback.deviation_ns, None);
    }

    #[test]
    fn snapshot_discards_torn_measurements_with_a_bounded_read() {
        let telemetry = CallbackClockTelemetry::default();
        let _observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        telemetry.words[2].store(123, Ordering::SeqCst);
        telemetry.revision.store(3, Ordering::SeqCst);
        let snapshot = telemetry.snapshot();
        assert!(snapshot.enabled && !snapshot.coherent && snapshot.inconclusive);
        assert_eq!(snapshot.callback_count, 0);
        assert_eq!(snapshot.total_frames, 0);
        assert_eq!(snapshot.callback.elapsed_ns, None);
    }

    #[test]
    fn clock_publication_revision_wrap_keeps_even_receipts_coherent() {
        let telemetry = CallbackClockTelemetry::default();
        let mut observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        telemetry.revision.store(u64::MAX - 1, Ordering::SeqCst);
        observer.observe(stamp(0, 30), 480, &telemetry);
        assert_eq!(telemetry.revision.load(Ordering::SeqCst), 0);
        let snapshot = telemetry.snapshot();
        assert!(snapshot.enabled && snapshot.coherent && !snapshot.inconclusive);
        assert_eq!(snapshot.callback_count, 1);
        assert_eq!(snapshot.total_frames, 480);
    }

    #[test]
    fn concurrent_clock_publication_returns_coherent_or_explicitly_unreadable_data() {
        let telemetry = Arc::new(CallbackClockTelemetry::default());
        let mut observer = CallbackClockObserver::new(true, 48_000, &telemetry);
        let writer_telemetry = Arc::clone(&telemetry);
        let writer = thread::spawn(move || {
            for index in 0..2_000 {
                observer.observe(stamp(index * 10, 30 + index * 10), 480, &writer_telemetry);
            }
        });
        for _ in 0..2_000 {
            let snapshot = telemetry.snapshot();
            if snapshot.coherent {
                assert_eq!(snapshot.total_frames, snapshot.callback_count * 480);
                assert!(!snapshot.inconclusive);
                if snapshot.callback_count != 0 {
                    assert_eq!(snapshot.callback.deviation_ns, Some(0));
                    assert_eq!(snapshot.playback.deviation_ns, Some(0));
                }
            } else {
                assert!(snapshot.inconclusive);
                assert_eq!(snapshot.total_frames, 0);
            }
        }
        writer.join().unwrap();
        let complete = telemetry.snapshot();
        assert!(complete.coherent && !complete.inconclusive);
        assert_eq!(complete.callback_count, 2_000);
        assert_eq!(complete.total_frames, 960_000);
    }
}
