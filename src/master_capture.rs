//! Real-time-safe capture of the rendered stereo master bus.
//!
//! [`MasterCaptureEndpoint`] is the only value intended for an audio callback. Its submit
//! methods use a preallocated SPSC ring and atomics only: they do not allocate, lock, wait, or
//! perform I/O. [`MasterCaptureControl`] and [`PendingMasterCapture`] belong on a control thread.
//! The collector writes a same-directory temporary file and publishes it without overwriting an
//! existing target only after the PCM24 WAV has been flushed and synced.
//!
//! Every submitted sample carries its absolute output-device frame. The endpoint converts that
//! clock into a capture-relative sequence. Forward jumps and bounded-ring overflows therefore
//! leave explicit holes rather than shortening the recording. The collector fills every hole
//! with silence and marks the resulting metadata discontinuous and invalid.

use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Seek, SeekFrom, Write},
    mem::size_of,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

pub const MASTER_CAPTURE_CHANNELS: u16 = 2;
pub const MASTER_CAPTURE_BITS_PER_SAMPLE: u16 = 24;
pub const DEFAULT_MAX_MASTER_WAV_FILE_BYTES: u64 = 1_073_741_824;

const PCM24_BYTES_PER_SAMPLE: u64 = 3;
const PCM_WAV_HEADER_BYTES: u64 = 44;
const STREAMING_ENCODE_BUFFER_BYTES: usize = 64 * 1024;
const MAX_RING_BUFFER_BYTES: usize = 32 * 1024 * 1024;
const MIN_RING_BUFFER_FRAMES: usize = 64;
const MAX_CAPTURE_SAMPLE_RATE: u32 = 768_000;
const COLLECTOR_RUNNING: u8 = 0;
const COLLECTOR_COMMIT: u8 = 1;
const COLLECTOR_ABORT: u8 = 2;
static SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Allocation and file-size policy for a capture session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MasterCaptureOptions {
    /// Amount of rendered stereo audio that may wait for the disk thread.
    pub ring_buffer_duration: Duration,
    /// Maximum final RIFF/WAV size, including header and an optional pad byte.
    pub maximum_file_bytes: u64,
}

impl Default for MasterCaptureOptions {
    fn default() -> Self {
        Self {
            ring_buffer_duration: Duration::from_secs(2),
            maximum_file_bytes: DEFAULT_MAX_MASTER_WAV_FILE_BYTES,
        }
    }
}

/// Result of one callback-side frame submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturePushResult {
    Captured {
        timeline_frame: u64,
        source_gap_frames: u64,
    },
    Overflow {
        timeline_frame: u64,
        source_gap_frames: u64,
    },
    RejectedOutOfOrder,
    LimitReached,
}

/// Aggregate result of submitting a contiguous callback block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureBlockReport {
    pub input_frames: usize,
    pub captured_frames: usize,
    pub overflow_frames: usize,
    pub rejected_frames: usize,
    pub limit_reached: bool,
}

/// Cheap, lock-free session progress suitable for status UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasterCaptureStatus {
    pub sample_rate: u32,
    pub maximum_frames: u64,
    pub timeline_frames: u64,
    pub accepted_frames: u64,
    pub overflow_frames: u64,
    pub source_gap_frames: u64,
    pub out_of_order_frames: u64,
    pub inserted_silence_frames: u64,
    pub non_finite_samples: u64,
    pub limit_discarded_frames: u64,
    pub writer_errors: u64,
    pub first_device_frame: Option<u64>,
    pub last_device_frame: Option<u64>,
    pub limit_reached: bool,
    pub discontinuous: bool,
    pub invalid: bool,
}

/// Durable metadata returned only after the target WAV has been atomically published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MasterCaptureMetadata {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    /// Total frames in the WAV, including silence inserted to preserve the device timeline.
    pub frames: u64,
    /// Real frames that entered the bounded SPSC ring.
    pub captured_frames: u64,
    pub inserted_silence_frames: u64,
    pub overflow_frames: u64,
    pub source_gap_frames: u64,
    pub out_of_order_frames: u64,
    pub non_finite_samples: u64,
    pub limit_discarded_frames: u64,
    pub maximum_frames: u64,
    pub duration: Duration,
    pub first_device_frame: Option<u64>,
    pub last_device_frame: Option<u64>,
    pub limit_reached: bool,
    /// True when the captured device timeline contained a source jump, ring overflow, or
    /// out-of-order frame.
    pub discontinuous: bool,
    /// True when the file must not be treated as a clean bounce.
    pub invalid: bool,
}

impl MasterCaptureMetadata {
    pub fn is_valid(&self) -> bool {
        !self.invalid
    }
}

#[derive(Default)]
struct CaptureStats {
    timeline_frames: AtomicU64,
    accepted_frames: AtomicU64,
    overflow_frames: AtomicU64,
    source_gap_frames: AtomicU64,
    out_of_order_frames: AtomicU64,
    inserted_silence_frames: AtomicU64,
    non_finite_samples: AtomicU64,
    limit_discarded_frames: AtomicU64,
    writer_errors: AtomicU64,
    first_device_frame: AtomicU64,
    last_device_frame: AtomicU64,
    has_device_frame: AtomicBool,
    limit_reached: AtomicBool,
}

impl CaptureStats {
    fn new() -> Self {
        Self::default()
    }

    fn snapshot(&self, sample_rate: u32, maximum_frames: u64) -> MasterCaptureStatus {
        let overflow_frames = self.overflow_frames.load(Ordering::Relaxed);
        let source_gap_frames = self.source_gap_frames.load(Ordering::Relaxed);
        let out_of_order_frames = self.out_of_order_frames.load(Ordering::Relaxed);
        let non_finite_samples = self.non_finite_samples.load(Ordering::Relaxed);
        let limit_reached = self.limit_reached.load(Ordering::Acquire);
        let writer_errors = self.writer_errors.load(Ordering::Relaxed);
        let inserted_silence_frames = self.inserted_silence_frames.load(Ordering::Relaxed);
        let has_device_frame = self.has_device_frame.load(Ordering::Acquire);
        let discontinuous = overflow_frames != 0
            || source_gap_frames != 0
            || out_of_order_frames != 0
            || inserted_silence_frames != 0;
        MasterCaptureStatus {
            sample_rate,
            maximum_frames,
            timeline_frames: self.timeline_frames.load(Ordering::Acquire),
            accepted_frames: self.accepted_frames.load(Ordering::Relaxed),
            overflow_frames,
            source_gap_frames,
            out_of_order_frames,
            inserted_silence_frames,
            non_finite_samples,
            limit_discarded_frames: self.limit_discarded_frames.load(Ordering::Relaxed),
            writer_errors,
            first_device_frame: has_device_frame
                .then(|| self.first_device_frame.load(Ordering::Relaxed)),
            last_device_frame: has_device_frame
                .then(|| self.last_device_frame.load(Ordering::Relaxed)),
            limit_reached,
            discontinuous,
            invalid: discontinuous
                || non_finite_samples != 0
                || limit_reached
                || writer_errors != 0,
        }
    }
}

#[derive(Clone, Copy)]
struct CapturedFrame {
    timeline_frame: u64,
    stereo: [f32; 2],
}

/// Callback-owned half of a master capture.
///
/// This type is intentionally neither cloneable nor shareable. One audio callback must own it,
/// submit frames in device-clock order, and return it to the control thread before finalization.
pub struct MasterCaptureEndpoint {
    session_id: u64,
    producer: Producer<CapturedFrame>,
    stats: Arc<CaptureStats>,
    sample_rate: u32,
    maximum_frames: u64,
    next_timeline_frame: u64,
    last_device_frame: Option<u64>,
}

impl MasterCaptureEndpoint {
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn maximum_frames(&self) -> u64 {
        self.maximum_frames
    }

    /// Submit one rendered stereo frame at its absolute output-device frame number.
    ///
    /// The method is callback-safe: no allocation, locks, waits, or I/O. A forward jump advances
    /// the capture-relative timeline before the current frame is submitted. A full ring advances
    /// the timeline too, allowing the writer to insert silence later instead of shortening time.
    pub fn push_frame(
        &mut self,
        absolute_device_frame: u64,
        stereo: [f32; 2],
    ) -> CapturePushResult {
        if self.stats.limit_reached.load(Ordering::Relaxed) {
            self.stats
                .limit_discarded_frames
                .fetch_add(1, Ordering::Relaxed);
            return CapturePushResult::LimitReached;
        }

        let source_gap_frames = match self.last_device_frame {
            None => {
                self.stats
                    .first_device_frame
                    .store(absolute_device_frame, Ordering::Release);
                0
            }
            Some(previous) if absolute_device_frame <= previous => {
                self.stats
                    .out_of_order_frames
                    .fetch_add(1, Ordering::Relaxed);
                return CapturePushResult::RejectedOutOfOrder;
            }
            Some(previous) => absolute_device_frame - previous - 1,
        };
        self.last_device_frame = Some(absolute_device_frame);
        self.stats
            .last_device_frame
            .store(absolute_device_frame, Ordering::Release);
        self.stats.has_device_frame.store(true, Ordering::Release);

        if source_gap_frames != 0 {
            self.stats
                .source_gap_frames
                .fetch_add(source_gap_frames, Ordering::Relaxed);
            let available = self.maximum_frames.saturating_sub(self.next_timeline_frame);
            let represented_gap = source_gap_frames.min(available);
            self.next_timeline_frame = self.next_timeline_frame.saturating_add(represented_gap);
            if represented_gap != source_gap_frames {
                self.stats
                    .limit_discarded_frames
                    .fetch_add(source_gap_frames - represented_gap + 1, Ordering::Relaxed);
                self.reach_limit();
                return CapturePushResult::LimitReached;
            }
        }

        if self.next_timeline_frame >= self.maximum_frames {
            self.stats
                .limit_discarded_frames
                .fetch_add(1, Ordering::Relaxed);
            self.reach_limit();
            return CapturePushResult::LimitReached;
        }

        let timeline_frame = self.next_timeline_frame;
        self.next_timeline_frame += 1;
        self.stats
            .timeline_frames
            .store(self.next_timeline_frame, Ordering::Release);

        let mut sanitized = stereo;
        let mut non_finite = 0_u64;
        for sample in &mut sanitized {
            if !sample.is_finite() {
                *sample = 0.0;
                non_finite += 1;
            }
        }
        if non_finite != 0 {
            self.stats
                .non_finite_samples
                .fetch_add(non_finite, Ordering::Relaxed);
        }

        let frame = CapturedFrame {
            timeline_frame,
            stereo: sanitized,
        };
        match self.producer.push(frame) {
            Ok(()) => {
                self.stats.accepted_frames.fetch_add(1, Ordering::Relaxed);
                CapturePushResult::Captured {
                    timeline_frame,
                    source_gap_frames,
                }
            }
            Err(PushError::Full(_)) => {
                self.stats.overflow_frames.fetch_add(1, Ordering::Relaxed);
                CapturePushResult::Overflow {
                    timeline_frame,
                    source_gap_frames,
                }
            }
        }
    }

    /// Submit a contiguous callback block without allocating.
    pub fn push_block(
        &mut self,
        first_absolute_device_frame: u64,
        frames: &[[f32; 2]],
    ) -> CaptureBlockReport {
        let mut report = CaptureBlockReport {
            input_frames: frames.len(),
            ..CaptureBlockReport::default()
        };
        for (index, &stereo) in frames.iter().enumerate() {
            let Some(device_frame) = first_absolute_device_frame.checked_add(index as u64) else {
                let rejected = frames.len() - index;
                report.rejected_frames += rejected;
                self.stats
                    .out_of_order_frames
                    .fetch_add(rejected as u64, Ordering::Relaxed);
                break;
            };
            match self.push_frame(device_frame, stereo) {
                CapturePushResult::Captured { .. } => report.captured_frames += 1,
                CapturePushResult::Overflow { .. } => report.overflow_frames += 1,
                CapturePushResult::RejectedOutOfOrder => report.rejected_frames += 1,
                CapturePushResult::LimitReached => {
                    report.limit_reached = true;
                    report.rejected_frames += 1;
                }
            }
        }
        report
    }

    pub fn status(&self) -> MasterCaptureStatus {
        self.stats.snapshot(self.sample_rate, self.maximum_frames)
    }

    fn reach_limit(&self) {
        self.stats
            .timeline_frames
            .store(self.maximum_frames, Ordering::Release);
        self.stats.limit_reached.store(true, Ordering::Release);
    }
}

/// Control-thread half of a capture session.
pub struct MasterCaptureControl {
    session_id: u64,
    shutdown: Arc<AtomicU8>,
    writer: Option<JoinHandle<Result<MasterCaptureMetadata>>>,
    stats: Arc<CaptureStats>,
    sample_rate: u32,
    maximum_frames: u64,
}

impl MasterCaptureControl {
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn status(&self) -> MasterCaptureStatus {
        self.stats.snapshot(self.sample_rate, self.maximum_frames)
    }

    /// Finalize after the audio engine has detached and returned the matching endpoint.
    pub fn stop(mut self, endpoint: MasterCaptureEndpoint) -> Result<PendingMasterCapture> {
        if endpoint.session_id != self.session_id {
            bail!(
                "Master capture endpoint {} does not belong to control {}",
                endpoint.session_id,
                self.session_id
            );
        }
        drop(endpoint);
        self.shutdown.store(COLLECTOR_COMMIT, Ordering::Release);
        let writer = self
            .writer
            .take()
            .ok_or_else(|| anyhow!("Master capture writer is no longer available"))?;
        Ok(PendingMasterCapture {
            writer: Some(writer),
        })
    }

    pub fn stop_blocking(self, endpoint: MasterCaptureEndpoint) -> Result<MasterCaptureMetadata> {
        self.stop(endpoint)?.finish()
    }

    fn abort_and_join(&mut self) {
        if self.writer.is_some() {
            self.shutdown.store(COLLECTOR_ABORT, Ordering::Release);
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

impl Drop for MasterCaptureControl {
    fn drop(&mut self) {
        self.abort_and_join();
    }
}

/// Background finalization returned after a confirmed stop.
pub struct PendingMasterCapture {
    writer: Option<JoinHandle<Result<MasterCaptureMetadata>>>,
}

impl PendingMasterCapture {
    pub fn is_finished(&self) -> bool {
        self.writer.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn finish(mut self) -> Result<MasterCaptureMetadata> {
        self.join_writer()
    }

    fn join_writer(&mut self) -> Result<MasterCaptureMetadata> {
        self.writer
            .take()
            .ok_or_else(|| anyhow!("Master capture writer was already joined"))?
            .join()
            .map_err(|_| anyhow!("Master capture writer thread panicked"))?
    }
}

impl Drop for PendingMasterCapture {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// Convenience owner used before the callback endpoint is installed.
pub struct MasterCaptureSession {
    endpoint: Option<MasterCaptureEndpoint>,
    control: Option<MasterCaptureControl>,
}

impl MasterCaptureSession {
    pub fn start_to_path(path: impl Into<PathBuf>, sample_rate: u32) -> Result<Self> {
        Self::start_to_path_with_options(path, sample_rate, MasterCaptureOptions::default())
    }

    pub fn start_to_path_with_options(
        path: impl Into<PathBuf>,
        sample_rate: u32,
        options: MasterCaptureOptions,
    ) -> Result<Self> {
        let (endpoint, control) = create_capture(path.into(), sample_rate, options)?;
        Ok(Self {
            endpoint: Some(endpoint),
            control: Some(control),
        })
    }

    pub fn endpoint_mut(&mut self) -> &mut MasterCaptureEndpoint {
        self.endpoint
            .as_mut()
            .expect("master capture endpoint was already taken")
    }

    pub fn status(&self) -> MasterCaptureStatus {
        self.control
            .as_ref()
            .expect("master capture control was already taken")
            .status()
    }

    pub fn into_parts(mut self) -> (MasterCaptureEndpoint, MasterCaptureControl) {
        let endpoint = self
            .endpoint
            .take()
            .expect("master capture endpoint was already taken");
        let control = self
            .control
            .take()
            .expect("master capture control was already taken");
        (endpoint, control)
    }

    pub fn stop(mut self) -> Result<PendingMasterCapture> {
        let endpoint = self
            .endpoint
            .take()
            .ok_or_else(|| anyhow!("Master capture endpoint was already taken"))?;
        let control = self
            .control
            .take()
            .ok_or_else(|| anyhow!("Master capture control was already taken"))?;
        control.stop(endpoint)
    }

    pub fn stop_blocking(self) -> Result<MasterCaptureMetadata> {
        self.stop()?.finish()
    }
}

impl Drop for MasterCaptureSession {
    fn drop(&mut self) {
        // The producer must disappear before the control aborts and joins the consumer.
        drop(self.endpoint.take());
        drop(self.control.take());
    }
}

fn create_capture(
    target_path: PathBuf,
    sample_rate: u32,
    options: MasterCaptureOptions,
) -> Result<(MasterCaptureEndpoint, MasterCaptureControl)> {
    validate_capture_format(sample_rate, MASTER_CAPTURE_CHANNELS)?;
    validate_output_path(&target_path)?;
    let maximum_frames =
        maximum_frames_for_file_bytes(MASTER_CAPTURE_CHANNELS, options.maximum_file_bytes)?;
    let ring_frames = ring_frames_for_options(options, sample_rate)?;
    let (producer, consumer) = RingBuffer::new(ring_frames);
    let stats = Arc::new(CaptureStats::new());
    let shutdown = Arc::new(AtomicU8::new(COLLECTOR_RUNNING));
    let writer = StreamingPcm24Writer::create(
        target_path,
        sample_rate,
        MASTER_CAPTURE_CHANNELS,
        maximum_frames,
        options.maximum_file_bytes,
    )?;
    let session_id = SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed).max(1);
    let worker_stats = Arc::clone(&stats);
    let worker_shutdown = Arc::clone(&shutdown);
    let thread_name = format!("citrus-master-capture-{session_id}");
    let writer_thread = thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let result = run_collector(
                consumer,
                &worker_shutdown,
                writer,
                &worker_stats,
                maximum_frames,
            );
            if result.is_err() {
                worker_stats.writer_errors.fetch_add(1, Ordering::Relaxed);
            }
            result
        })
        .context("Unable to start the master capture writer thread")?;

    Ok((
        MasterCaptureEndpoint {
            session_id,
            producer,
            stats: Arc::clone(&stats),
            sample_rate,
            maximum_frames,
            next_timeline_frame: 0,
            last_device_frame: None,
        },
        MasterCaptureControl {
            session_id,
            shutdown,
            writer: Some(writer_thread),
            stats,
            sample_rate,
            maximum_frames,
        },
    ))
}

fn ring_frames_for_options(options: MasterCaptureOptions, sample_rate: u32) -> Result<usize> {
    if options.ring_buffer_duration.is_zero() {
        bail!("Master capture ring duration must be greater than zero");
    }
    let requested = options.ring_buffer_duration.as_secs_f64() * f64::from(sample_rate);
    if !requested.is_finite() || requested >= usize::MAX as f64 {
        bail!("Requested master capture ring is too large for this platform");
    }
    let maximum = MAX_RING_BUFFER_BYTES / size_of::<CapturedFrame>();
    let frames = (requested.ceil() as usize)
        .max(MIN_RING_BUFFER_FRAMES)
        .min(maximum);
    if frames == 0 {
        bail!("Master capture ring cannot hold one frame");
    }
    Ok(frames)
}

/// Maximum complete stereo frames that fit in the configured RIFF/WAV limit.
pub fn maximum_master_capture_frames(maximum_file_bytes: u64) -> Result<u64> {
    maximum_frames_for_file_bytes(MASTER_CAPTURE_CHANNELS, maximum_file_bytes)
}

fn maximum_frames_for_file_bytes(channels: u16, maximum_file_bytes: u64) -> Result<u64> {
    validate_channels(channels)?;
    let frame_bytes = u64::from(channels) * PCM24_BYTES_PER_SAMPLE;
    let available = maximum_file_bytes
        .checked_sub(PCM_WAV_HEADER_BYTES)
        .ok_or_else(|| anyhow!("Configured WAV limit cannot hold a PCM header"))?;
    let riff_available = u64::from(u32::MAX)
        .checked_sub(36)
        .ok_or_else(|| anyhow!("RIFF frame limit underflowed"))?;
    let mut frames = (available.min(riff_available)) / frame_bytes;
    while frames != 0 {
        let data_bytes = frames
            .checked_mul(frame_bytes)
            .ok_or_else(|| anyhow!("Master capture size limit overflowed"))?;
        let padding = data_bytes & 1;
        if PCM_WAV_HEADER_BYTES + data_bytes + padding <= maximum_file_bytes
            && 36 + data_bytes + padding <= u64::from(u32::MAX)
        {
            return Ok(frames);
        }
        frames -= 1;
    }
    bail!("Configured WAV limit cannot hold one complete {channels}-channel frame")
}

fn run_collector(
    consumer: Consumer<CapturedFrame>,
    shutdown: &AtomicU8,
    writer: StreamingPcm24Writer,
    stats: &CaptureStats,
    maximum_frames: u64,
) -> Result<MasterCaptureMetadata> {
    run_collector_with_drain_observer(consumer, shutdown, writer, stats, maximum_frames, || {})
}

// The observer is a deterministic test seam on the disk thread, never the audio callback.
fn run_collector_with_drain_observer(
    mut consumer: Consumer<CapturedFrame>,
    shutdown: &AtomicU8,
    mut writer: StreamingPcm24Writer,
    stats: &CaptureStats,
    maximum_frames: u64,
    mut after_drain: impl FnMut(),
) -> Result<MasterCaptureMetadata> {
    let mut encoded = [0_u8; STREAMING_ENCODE_BUFFER_BYTES];
    let mut encoded_len = 0_usize;
    let mut next_timeline_frame = 0_u64;

    loop {
        // COMMIT is published after the producer is dropped. Observe it BEFORE draining:
        // otherwise a final push between an empty pop and this load can be mistaken for
        // a missing frame and replaced with silence without ever reading the queued audio.
        let shutdown_state = shutdown.load(Ordering::Acquire);
        let mut made_progress = false;
        while let Ok(frame) = consumer.pop() {
            made_progress = true;
            if frame.timeline_frame < next_timeline_frame {
                stats.out_of_order_frames.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if frame.timeline_frame > next_timeline_frame {
                flush_encoded(&mut writer, &encoded, &mut encoded_len)?;
                let gap = frame.timeline_frame - next_timeline_frame;
                writer.write_silence_frames(gap)?;
                stats
                    .inserted_silence_frames
                    .fetch_add(gap, Ordering::Relaxed);
                next_timeline_frame = frame.timeline_frame;
            }
            append_stereo_frame(&mut writer, &mut encoded, &mut encoded_len, frame.stereo)?;
            next_timeline_frame += 1;
        }

        after_drain();
        match shutdown_state {
            COLLECTOR_ABORT => bail!("Master capture was aborted before commit"),
            COLLECTOR_COMMIT => {
                let timeline_frames = stats
                    .timeline_frames
                    .load(Ordering::Acquire)
                    .min(maximum_frames);
                if next_timeline_frame < timeline_frames {
                    flush_encoded(&mut writer, &encoded, &mut encoded_len)?;
                    let trailing_gap = timeline_frames - next_timeline_frame;
                    writer.write_silence_frames(trailing_gap)?;
                    stats
                        .inserted_silence_frames
                        .fetch_add(trailing_gap, Ordering::Relaxed);
                    next_timeline_frame = timeline_frames;
                }
                if next_timeline_frame == 0 {
                    bail!("Master capture contains no audio frames");
                }
                if next_timeline_frame != timeline_frames {
                    bail!(
                        "Master capture timeline mismatch: wrote {next_timeline_frame} frames, expected {timeline_frames}"
                    );
                }
                flush_encoded(&mut writer, &encoded, &mut encoded_len)?;
                let sample_rate = writer.sample_rate;
                let target_path = writer.finish_file()?;
                let status = stats.snapshot(sample_rate, maximum_frames);
                return Ok(MasterCaptureMetadata {
                    path: target_path,
                    sample_rate: status.sample_rate,
                    channels: MASTER_CAPTURE_CHANNELS,
                    bits_per_sample: MASTER_CAPTURE_BITS_PER_SAMPLE,
                    frames: timeline_frames,
                    captured_frames: status.accepted_frames,
                    inserted_silence_frames: status.inserted_silence_frames,
                    overflow_frames: status.overflow_frames,
                    source_gap_frames: status.source_gap_frames,
                    out_of_order_frames: status.out_of_order_frames,
                    non_finite_samples: status.non_finite_samples,
                    limit_discarded_frames: status.limit_discarded_frames,
                    maximum_frames,
                    duration: duration_for_frames(timeline_frames, status.sample_rate),
                    first_device_frame: status.first_device_frame,
                    last_device_frame: status.last_device_frame,
                    limit_reached: status.limit_reached,
                    discontinuous: status.discontinuous,
                    invalid: status.invalid,
                });
            }
            COLLECTOR_RUNNING => {}
            state => bail!("Master capture collector received invalid shutdown state {state}"),
        }

        if !made_progress {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn append_stereo_frame(
    writer: &mut StreamingPcm24Writer,
    encoded: &mut [u8; STREAMING_ENCODE_BUFFER_BYTES],
    encoded_len: &mut usize,
    stereo: [f32; 2],
) -> Result<()> {
    const FRAME_BYTES: usize = MASTER_CAPTURE_CHANNELS as usize * PCM24_BYTES_PER_SAMPLE as usize;
    if encoded.len() - *encoded_len < FRAME_BYTES {
        flush_encoded(writer, encoded, encoded_len)?;
    }
    for sample in stereo {
        let bytes = encode_pcm24(sample);
        encoded[*encoded_len..*encoded_len + 3].copy_from_slice(&bytes);
        *encoded_len += 3;
    }
    Ok(())
}

fn flush_encoded(
    writer: &mut StreamingPcm24Writer,
    encoded: &[u8; STREAMING_ENCODE_BUFFER_BYTES],
    encoded_len: &mut usize,
) -> Result<()> {
    if *encoded_len != 0 {
        writer.write_pcm_bytes(&encoded[..*encoded_len])?;
        *encoded_len = 0;
    }
    Ok(())
}

fn duration_for_frames(frames: u64, sample_rate: u32) -> Duration {
    Duration::from_secs_f64(frames as f64 / f64::from(sample_rate.max(1)))
}

struct StreamingPcm24Writer {
    writer: Option<BufWriter<File>>,
    target_path: PathBuf,
    temp_path: PathBuf,
    sample_rate: u32,
    channels: u16,
    maximum_data_bytes: u64,
    data_bytes: u64,
    committed: bool,
}

impl StreamingPcm24Writer {
    fn create(
        target_path: PathBuf,
        sample_rate: u32,
        channels: u16,
        maximum_frames: u64,
        maximum_file_bytes: u64,
    ) -> Result<Self> {
        validate_capture_format(sample_rate, channels)?;
        validate_output_path(&target_path)?;
        let maximum_data_bytes = maximum_frames
            .checked_mul(u64::from(channels) * PCM24_BYTES_PER_SAMPLE)
            .ok_or_else(|| anyhow!("Master capture data limit overflowed"))?;
        let padding = maximum_data_bytes & 1;
        if PCM_WAV_HEADER_BYTES + maximum_data_bytes + padding > maximum_file_bytes {
            bail!("Master capture frame limit exceeds the configured file limit");
        }
        let (file, temp_path) = create_same_directory_temp(&target_path)?;
        let mut writer = BufWriter::new(file);
        write_pcm24_header(&mut writer, sample_rate, channels, 0)?;
        Ok(Self {
            writer: Some(writer),
            target_path,
            temp_path,
            sample_rate,
            channels,
            maximum_data_bytes,
            data_bytes: 0,
            committed: false,
        })
    }

    fn write_pcm_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if !bytes.len().is_multiple_of(PCM24_BYTES_PER_SAMPLE as usize) {
            bail!("PCM24 capture block is not sample-aligned");
        }
        let new_data_bytes = self
            .data_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| anyhow!("Master capture data size overflowed"))?;
        if new_data_bytes > self.maximum_data_bytes {
            bail!("Master capture block exceeds the configured file limit");
        }
        self.writer
            .as_mut()
            .ok_or_else(|| anyhow!("Master capture temporary file is already closed"))?
            .write_all(bytes)
            .with_context(|| {
                format!(
                    "Unable to stream master capture data to '{}'",
                    self.temp_path.display()
                )
            })?;
        self.data_bytes = new_data_bytes;
        Ok(())
    }

    fn write_silence_frames(&mut self, mut frames: u64) -> Result<()> {
        static ZERO_BYTES: [u8; STREAMING_ENCODE_BUFFER_BYTES] = [0; STREAMING_ENCODE_BUFFER_BYTES];
        let frame_bytes = usize::from(self.channels) * PCM24_BYTES_PER_SAMPLE as usize;
        let frames_per_block = (ZERO_BYTES.len() / frame_bytes).max(1) as u64;
        while frames != 0 {
            let block_frames = frames.min(frames_per_block);
            let byte_count = usize::try_from(block_frames)
                .ok()
                .and_then(|count| count.checked_mul(frame_bytes))
                .ok_or_else(|| anyhow!("Master capture silence block size overflowed"))?;
            self.write_pcm_bytes(&ZERO_BYTES[..byte_count])?;
            frames -= block_frames;
        }
        Ok(())
    }

    fn finish_file(mut self) -> Result<PathBuf> {
        if self.data_bytes == 0 {
            bail!("Master capture contains no complete audio frames");
        }
        let block_align = u64::from(self.channels) * PCM24_BYTES_PER_SAMPLE;
        if !self.data_bytes.is_multiple_of(block_align) {
            bail!("Master capture ended with a partial audio frame");
        }
        let padding = self.data_bytes & 1;
        let mut writer = self
            .writer
            .take()
            .ok_or_else(|| anyhow!("Master capture temporary file is already closed"))?;
        if padding != 0 {
            writer.write_all(&[0]).with_context(|| {
                format!(
                    "Unable to pad master capture temporary file '{}'",
                    self.temp_path.display()
                )
            })?;
        }
        writer
            .flush()
            .with_context(|| format!("Unable to flush '{}'", self.temp_path.display()))?;
        writer
            .seek(SeekFrom::Start(0))
            .with_context(|| format!("Unable to seek '{}'", self.temp_path.display()))?;
        write_pcm24_header(
            &mut writer,
            self.sample_rate,
            self.channels,
            self.data_bytes,
        )?;
        writer
            .flush()
            .with_context(|| format!("Unable to flush '{}'", self.temp_path.display()))?;
        let file = writer
            .into_inner()
            .map_err(|error| error.into_error())
            .with_context(|| format!("Unable to close '{}'", self.temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("Unable to sync '{}'", self.temp_path.display()))?;
        drop(file);

        commit_no_clobber(&self.temp_path, &self.target_path)?;
        self.committed = true;
        sync_parent_directory(parent_directory(&self.target_path))?;
        Ok(self.target_path.clone())
    }
}

impl Drop for StreamingPcm24Writer {
    fn drop(&mut self) {
        self.writer.take();
        if !self.committed {
            let _ = std::fs::remove_file(&self.temp_path);
        }
    }
}

fn validate_capture_format(sample_rate: u32, channels: u16) -> Result<()> {
    validate_channels(channels)?;
    if sample_rate == 0 || sample_rate > MAX_CAPTURE_SAMPLE_RATE {
        bail!("Master capture sample rate {sample_rate} is outside the supported range");
    }
    let block_align = channels
        .checked_mul(MASTER_CAPTURE_BITS_PER_SAMPLE / 8)
        .ok_or_else(|| anyhow!("Master capture block alignment overflowed"))?;
    sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or_else(|| anyhow!("Master capture byte rate exceeds the WAV limit"))?;
    Ok(())
}

fn validate_channels(channels: u16) -> Result<()> {
    if channels == 0 || channels > 64 {
        bail!("Master capture channel count {channels} is outside the supported range");
    }
    Ok(())
}

fn validate_output_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        bail!("A master capture output file path is required");
    }
    if path
        .try_exists()
        .with_context(|| format!("Unable to inspect capture path '{}'", path.display()))?
    {
        bail!(
            "Master capture target '{}' already exists; refusing to overwrite it",
            path.display()
        );
    }
    Ok(())
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn create_same_directory_temp(target_path: &Path) -> Result<(File, PathBuf)> {
    let parent = parent_directory(target_path);
    std::fs::create_dir_all(parent).with_context(|| {
        format!(
            "Unable to create master capture directory '{}'",
            parent.display()
        )
    })?;
    let file_name = target_path
        .file_name()
        .ok_or_else(|| anyhow!("A master capture output file name is required"))?
        .to_string_lossy();
    for _ in 0..128 {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{file_name}.citrus-master-{}-{sequence}.part",
            std::process::id()
        ));
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((file, temp_path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Unable to create master capture temporary file '{}'",
                        temp_path.display()
                    )
                });
            }
        }
    }
    bail!(
        "Unable to reserve a unique temporary file beside '{}'",
        target_path.display()
    )
}

fn write_pcm24_header(
    writer: &mut impl Write,
    sample_rate: u32,
    channels: u16,
    data_bytes: u64,
) -> Result<()> {
    validate_capture_format(sample_rate, channels)?;
    let padding = data_bytes & 1;
    let data_bytes = u32::try_from(data_bytes)
        .map_err(|_| anyhow!("Master capture data exceeds the RIFF chunk-size limit"))?;
    let riff_size = 36_u64
        .checked_add(u64::from(data_bytes))
        .and_then(|size| size.checked_add(padding))
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| anyhow!("Master capture exceeds the RIFF container-size limit"))?;
    let block_align = channels * (MASTER_CAPTURE_BITS_PER_SAMPLE / 8);
    let byte_rate = sample_rate * u32::from(block_align);

    writer.write_all(b"RIFF")?;
    writer.write_all(&riff_size.to_le_bytes())?;
    writer.write_all(b"WAVE")?;
    writer.write_all(b"fmt ")?;
    writer.write_all(&16_u32.to_le_bytes())?;
    writer.write_all(&1_u16.to_le_bytes())?;
    writer.write_all(&channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&MASTER_CAPTURE_BITS_PER_SAMPLE.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_bytes.to_le_bytes())?;
    Ok(())
}

fn encode_pcm24(sample: f32) -> [u8; 3] {
    let sample = if sample.is_finite() { sample } else { 0.0 };
    let clamped = sample.clamp(-1.0, 1.0);
    let value = if clamped < 0.0 {
        (clamped * 8_388_608.0).round() as i32
    } else {
        (clamped * 8_388_607.0).round() as i32
    };
    let bytes = value.to_le_bytes();
    [bytes[0], bytes[1], bytes[2]]
}

#[cfg(windows)]
fn commit_no_clobber(temp_path: &Path, target_path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileW(existing: *const u16, destination: *const u16) -> i32;
    }

    let mut existing = temp_path.as_os_str().encode_wide().collect::<Vec<_>>();
    let mut destination = target_path.as_os_str().encode_wide().collect::<Vec<_>>();
    if existing.contains(&0) || destination.contains(&0) {
        bail!("Master capture paths cannot contain embedded NUL characters");
    }
    existing.push(0);
    destination.push(0);
    // SAFETY: both pointers are live, NUL-terminated UTF-16 buffers for the duration of this
    // non-callback Win32 call. MoveFileW does not replace an existing destination.
    let moved = unsafe { MoveFileW(existing.as_ptr(), destination.as_ptr()) };
    if moved == 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "Unable to atomically publish master capture '{}' to '{}'; the target may already exist",
                temp_path.display(),
                target_path.display()
            )
        });
    }
    Ok(())
}

#[cfg(unix)]
fn commit_no_clobber(temp_path: &Path, target_path: &Path) -> Result<()> {
    std::fs::hard_link(temp_path, target_path).with_context(|| {
        format!(
            "Unable to atomically publish master capture '{}' to '{}'; the target may already exist",
            temp_path.display(),
            target_path.display()
        )
    })?;
    std::fs::remove_file(temp_path).with_context(|| {
        format!(
            "Master capture was published to '{}' but temporary link '{}' could not be removed",
            target_path.display(),
            temp_path.display()
        )
    })?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn commit_no_clobber(_temp_path: &Path, _target_path: &Path) -> Result<()> {
    bail!("Atomic no-overwrite master capture publishing is unsupported on this platform")
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<()> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| {
            format!(
                "Unable to sync master capture directory '{}'",
                parent.display()
            )
        })
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(label: &str) -> Self {
            for _ in 0..128 {
                let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "citrus-master-capture-{label}-{}-{sequence}",
                    std::process::id()
                ));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("unable to create test directory: {error}"),
                }
            }
            panic!("unable to reserve a unique test directory")
        }

        fn target(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }

        fn part_files(&self) -> Vec<PathBuf> {
            std::fs::read_dir(&self.path)
                .unwrap()
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "part")
                })
                .collect()
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn small_options(maximum_frames: u64) -> MasterCaptureOptions {
        MasterCaptureOptions {
            ring_buffer_duration: Duration::from_millis(10),
            maximum_file_bytes: PCM_WAV_HEADER_BYTES
                + maximum_frames * u64::from(MASTER_CAPTURE_CHANNELS) * PCM24_BYTES_PER_SAMPLE,
        }
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn continuous_capture_preserves_frames_and_publishes_pcm24() {
        let directory = TestDirectory::new("continuous");
        let target = directory.target("continuous.wav");
        let mut session =
            MasterCaptureSession::start_to_path_with_options(&target, 48_000, small_options(32))
                .unwrap();
        let report = session.endpoint_mut().push_block(
            10_000,
            &[[0.0, 0.0], [0.5, -0.5], [1.0, -1.0], [0.25, -0.25]],
        );
        assert_eq!(report.captured_frames, 4);
        assert_eq!(report.overflow_frames, 0);

        let metadata = session.stop_blocking().unwrap();
        assert_eq!(metadata.frames, 4);
        assert_eq!(metadata.captured_frames, 4);
        assert_eq!(metadata.inserted_silence_frames, 0);
        assert_eq!(metadata.first_device_frame, Some(10_000));
        assert_eq!(metadata.last_device_frame, Some(10_003));
        assert!(metadata.is_valid());
        let bytes = std::fs::read(&target).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(read_u32(&bytes, 40), 24);
        assert_eq!(bytes.len(), PCM_WAV_HEADER_BYTES as usize + 24);
        assert!(directory.part_files().is_empty());
    }

    #[test]
    fn absolute_frame_gap_is_silenced_and_marks_file_invalid() {
        let directory = TestDirectory::new("source-gap");
        let target = directory.target("gap.wav");
        let mut session =
            MasterCaptureSession::start_to_path_with_options(&target, 48_000, small_options(16))
                .unwrap();
        assert!(matches!(
            session.endpoint_mut().push_frame(500, [0.5, 0.5]),
            CapturePushResult::Captured { .. }
        ));
        assert!(matches!(
            session.endpoint_mut().push_frame(502, [-0.5, -0.5]),
            CapturePushResult::Captured {
                source_gap_frames: 1,
                ..
            }
        ));

        let metadata = session.stop_blocking().unwrap();
        assert_eq!(metadata.frames, 3);
        assert_eq!(metadata.source_gap_frames, 1);
        assert_eq!(metadata.inserted_silence_frames, 1);
        assert!(metadata.discontinuous);
        assert!(metadata.invalid);
        let bytes = std::fs::read(target).unwrap();
        assert_eq!(&bytes[50..56], &[0; 6]);
    }

    fn capture_fixture(
        capacity: usize,
        maximum_frames: u64,
    ) -> (
        MasterCaptureEndpoint,
        Consumer<CapturedFrame>,
        Arc<CaptureStats>,
        Arc<AtomicU8>,
    ) {
        let (producer, consumer) = RingBuffer::new(capacity);
        let stats = Arc::new(CaptureStats::new());
        let shutdown = Arc::new(AtomicU8::new(COLLECTOR_RUNNING));
        (
            MasterCaptureEndpoint {
                session_id: 1,
                producer,
                stats: Arc::clone(&stats),
                sample_rate: 48_000,
                maximum_frames,
                next_timeline_frame: 0,
                last_device_frame: None,
            },
            consumer,
            stats,
            shutdown,
        )
    }

    #[test]
    fn ring_overflow_advances_timeline_and_pads_trailing_silence() {
        let directory = TestDirectory::new("overflow");
        let target = directory.target("overflow.wav");
        let (mut endpoint, consumer, stats, shutdown) = capture_fixture(1, 8);
        assert!(matches!(
            endpoint.push_frame(1_000, [0.25, -0.25]),
            CapturePushResult::Captured { .. }
        ));
        assert!(matches!(
            endpoint.push_frame(1_001, [0.5, -0.5]),
            CapturePushResult::Overflow { .. }
        ));
        assert!(matches!(
            endpoint.push_frame(1_002, [0.75, -0.75]),
            CapturePushResult::Overflow { .. }
        ));
        drop(endpoint);
        shutdown.store(COLLECTOR_COMMIT, Ordering::Release);
        let writer = StreamingPcm24Writer::create(
            target.clone(),
            48_000,
            MASTER_CAPTURE_CHANNELS,
            8,
            PCM_WAV_HEADER_BYTES + 8 * 6,
        )
        .unwrap();
        let metadata = run_collector(consumer, &shutdown, writer, &stats, 8).unwrap();
        assert_eq!(metadata.frames, 3);
        assert_eq!(metadata.captured_frames, 1);
        assert_eq!(metadata.overflow_frames, 2);
        assert_eq!(metadata.inserted_silence_frames, 2);
        assert!(metadata.discontinuous);
        assert!(metadata.invalid);
        let bytes = std::fs::read(target).unwrap();
        assert_eq!(&bytes[50..62], &[0; 12]);
    }

    #[test]
    fn commit_after_empty_drain_preserves_final_queued_audio() {
        let directory = TestDirectory::new("stop-race");
        let target = directory.target("final-frame.wav");
        let (endpoint, consumer, stats, shutdown) = capture_fixture(4, 8);
        let mut endpoint = Some(endpoint);
        let writer = StreamingPcm24Writer::create(
            target.clone(),
            48_000,
            MASTER_CAPTURE_CHANNELS,
            8,
            PCM_WAV_HEADER_BYTES + 8 * 6,
        )
        .unwrap();
        let metadata =
            run_collector_with_drain_observer(consumer, &shutdown, writer, &stats, 8, || {
                if let Some(mut endpoint) = endpoint.take() {
                    assert!(matches!(
                        endpoint.push_frame(100, [0.5, -0.5]),
                        CapturePushResult::Captured { .. }
                    ));
                    drop(endpoint);
                    shutdown.store(COLLECTOR_COMMIT, Ordering::Release);
                }
            })
            .unwrap();
        assert_eq!(metadata.frames, 1);
        assert_eq!(metadata.captured_frames, 1);
        assert_eq!(metadata.inserted_silence_frames, 0);
        assert!(metadata.is_valid());
        let bytes = std::fs::read(target).unwrap();
        assert_eq!(&bytes[44..47], &encode_pcm24(0.5));
        assert_eq!(&bytes[47..50], &encode_pcm24(-0.5));
    }

    #[test]
    fn mono_writer_pads_odd_pcm24_data_and_reports_riff_size() {
        let directory = TestDirectory::new("odd-pad");
        let target = directory.target("odd.wav");
        let mut writer = StreamingPcm24Writer::create(target.clone(), 48_000, 1, 1, 48).unwrap();
        writer.write_pcm_bytes(&encode_pcm24(0.5)).unwrap();
        writer.finish_file().unwrap();
        let bytes = std::fs::read(target).unwrap();
        assert_eq!(bytes.len(), 48);
        assert_eq!(read_u32(&bytes, 4), 40);
        assert_eq!(read_u32(&bytes, 40), 3);
        assert_eq!(bytes[47], 0);
    }

    #[test]
    fn target_appearing_before_commit_is_never_overwritten() {
        let directory = TestDirectory::new("collision");
        let target = directory.target("collision.wav");
        let mut session =
            MasterCaptureSession::start_to_path_with_options(&target, 48_000, small_options(8))
                .unwrap();
        let _ = session.endpoint_mut().push_frame(100, [0.25, -0.25]);
        std::fs::write(&target, b"do not replace").unwrap();
        let error = session.stop_blocking().unwrap_err();
        assert!(error.to_string().contains("atomically publish"));
        assert_eq!(std::fs::read(&target).unwrap(), b"do not replace");
        assert!(directory.part_files().is_empty());
    }

    #[test]
    fn dropping_live_session_aborts_and_removes_part_file() {
        let directory = TestDirectory::new("drop-abort");
        let target = directory.target("aborted.wav");
        {
            let mut session =
                MasterCaptureSession::start_to_path_with_options(&target, 48_000, small_options(8))
                    .unwrap();
            let _ = session.endpoint_mut().push_frame(10, [0.1, -0.1]);
            assert!(!directory.part_files().is_empty());
        }
        assert!(!target.exists());
        assert!(directory.part_files().is_empty());
    }

    #[test]
    fn dropping_pending_stop_waits_for_durable_publish() {
        let directory = TestDirectory::new("pending-drop");
        let target = directory.target("durable.wav");
        let mut session =
            MasterCaptureSession::start_to_path_with_options(&target, 48_000, small_options(8))
                .unwrap();
        let _ = session.endpoint_mut().push_frame(10, [0.1, -0.1]);
        let pending = session.stop().unwrap();
        drop(pending);
        assert!(target.exists());
        assert!(directory.part_files().is_empty());
        let bytes = std::fs::read(target).unwrap();
        assert_eq!(read_u32(&bytes, 40), 6);
    }

    #[test]
    fn file_limit_rejects_tail_without_exceeding_riff_bound() {
        let directory = TestDirectory::new("limit");
        let target = directory.target("limit.wav");
        let mut session =
            MasterCaptureSession::start_to_path_with_options(&target, 48_000, small_options(2))
                .unwrap();
        assert!(matches!(
            session.endpoint_mut().push_frame(0, [0.0; 2]),
            CapturePushResult::Captured { .. }
        ));
        assert!(matches!(
            session.endpoint_mut().push_frame(1, [0.0; 2]),
            CapturePushResult::Captured { .. }
        ));
        assert_eq!(
            session.endpoint_mut().push_frame(2, [0.0; 2]),
            CapturePushResult::LimitReached
        );
        let metadata = session.stop_blocking().unwrap();
        assert_eq!(metadata.frames, 2);
        assert_eq!(metadata.maximum_frames, 2);
        assert_eq!(metadata.limit_discarded_frames, 1);
        assert!(metadata.limit_reached);
        assert!(metadata.invalid);
        assert_eq!(std::fs::metadata(target).unwrap().len(), 56);
        assert_eq!(maximum_master_capture_frames(56).unwrap(), 2);
        assert!(maximum_master_capture_frames(49).is_err());
    }

    #[test]
    fn out_of_order_and_non_finite_input_are_reported_without_callback_failure() {
        let directory = TestDirectory::new("invalid-input");
        let target = directory.target("invalid.wav");
        let mut session =
            MasterCaptureSession::start_to_path_with_options(&target, 48_000, small_options(8))
                .unwrap();
        assert!(matches!(
            session
                .endpoint_mut()
                .push_frame(7, [f32::NAN, f32::INFINITY]),
            CapturePushResult::Captured { .. }
        ));
        assert_eq!(
            session.endpoint_mut().push_frame(7, [0.0; 2]),
            CapturePushResult::RejectedOutOfOrder
        );
        let metadata = session.stop_blocking().unwrap();
        assert_eq!(metadata.frames, 1);
        assert_eq!(metadata.non_finite_samples, 2);
        assert_eq!(metadata.out_of_order_frames, 1);
        assert!(metadata.discontinuous);
        assert!(metadata.invalid);
        let bytes = std::fs::read(target).unwrap();
        assert_eq!(&bytes[44..50], &[0; 6]);
    }

    #[test]
    fn existing_target_is_refused_before_part_file_is_created() {
        let directory = TestDirectory::new("existing");
        let target = directory.target("existing.wav");
        std::fs::write(&target, b"existing").unwrap();
        let result =
            MasterCaptureSession::start_to_path_with_options(&target, 48_000, small_options(8));
        assert!(result.is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"existing");
        assert!(directory.part_files().is_empty());
    }
}
