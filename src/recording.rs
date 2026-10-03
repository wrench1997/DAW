//! Real-time-safe audio-input recording.
//!
//! The CPAL callback only converts samples, writes complete interleaved frames to
//! a pre-allocated SPSC ring buffer, and updates atomics. A non-real-time collector
//! drains that bounded channel into a same-directory temporary PCM24 WAV using a
//! fixed-size encoding buffer. Stopping patches and syncs the header before an
//! atomic, no-clobber commit makes the target path visible.

use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::audio_device::{AudioDeviceDirection, AudioDeviceProfile, resolve_audio_device};
use crate::wav::{
    DEFAULT_MAX_CHANNELS, DEFAULT_MAX_DECODED_SAMPLES, DEFAULT_MAX_SAMPLE_RATE,
    DEFAULT_MAX_WAV_FILE_BYTES,
};

/// Bit depth used by recordings written by this module.
pub const RECORDING_BITS_PER_SAMPLE: u16 = 24;

const PCM24_BYTES_PER_SAMPLE: u64 = 3;
const PCM_WAV_HEADER_BYTES: u64 = 44;
const STREAMING_ENCODE_BUFFER_BYTES: usize = 64 * 1024;
const MAX_RING_BUFFER_BYTES: usize = 32 * 1024 * 1024;
const COLLECTOR_RUNNING: u8 = 0;
const COLLECTOR_COMMIT: u8 = 1;
const COLLECTOR_ABORT: u8 = 2;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Information suitable for an audio-input device picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub sample_format: Option<String>,
}

/// Enumerates input devices using the platform's default CPAL host.
///
/// An empty device set is reported as an error rather than silently returning an
/// empty list, so the UI can present a useful microphone/permissions message.
pub fn input_devices() -> Result<Vec<InputDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|device| {
        device
            .description()
            .ok()
            .map(|description| description.name().to_owned())
    });
    let devices = host
        .input_devices()
        .context("Unable to enumerate audio input devices")?;

    let mut result = devices
        .enumerate()
        .map(|(index, device)| {
            let name = device
                .description()
                .map(|description| description.name().to_owned())
                .unwrap_or_else(|_| format!("Unnamed input device {}", index + 1));
            let config = device.default_input_config().ok();
            InputDeviceInfo {
                is_default: default_name.as_deref() == Some(name.as_str()),
                name,
                sample_rate: config.as_ref().map(|value| value.sample_rate()),
                channels: config.as_ref().map(|value| value.channels()),
                sample_format: config
                    .as_ref()
                    .map(|value| value.sample_format().to_string()),
            }
        })
        .collect::<Vec<_>>();

    if result.is_empty() {
        bail!(no_input_device_message());
    }

    result.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(result)
}

/// Bounded callback-memory and latency policy for a new recording.
#[derive(Clone, Copy, Debug)]
pub struct RecorderOptions {
    /// Number of complete input frames held by the real-time SPSC channel.
    ///
    /// Two seconds leaves enough scheduling headroom for a temporarily busy UI or
    /// render thread while remaining bounded. This memory is allocated before the
    /// CPAL stream starts.
    pub ring_buffer_duration: Duration,
}

impl Default for RecorderOptions {
    fn default() -> Self {
        Self {
            ring_buffer_duration: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Default)]
struct CaptureStats {
    accepted_samples: AtomicU64,
    written_samples: AtomicU64,
    dropped_samples: AtomicU64,
    limit_discarded_samples: AtomicU64,
    overflow_callbacks: AtomicU64,
    stream_errors: AtomicU64,
    writer_errors: AtomicU64,
    limit_reached: AtomicBool,
}

/// A cheap, lock-free status snapshot that can be polled by the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingStatus {
    pub sample_rate: u32,
    pub channels: u16,
    pub captured_frames: u64,
    pub duration: Duration,
    /// Maximum number of frames accepted by the application's strict WAV importer.
    pub maximum_frames: u64,
    /// Samples discarded because the bounded callback channel was full.
    pub dropped_samples: u64,
    /// Number of callbacks in which at least one complete input frame was lost.
    pub overflow_events: u64,
    /// Errors reported asynchronously by the CPAL stream backend.
    pub stream_errors: u64,
    /// Non-real-time disk/encoding errors detected while capture was active.
    pub writer_errors: u64,
    /// Samples discarded after the safe import limit was reached.
    pub limit_discarded_samples: u64,
    /// True once at least one complete frame was discarded at that limit.
    pub limit_reached: bool,
    /// Sum of bounded-channel overflow events and backend stream errors.
    pub dropouts: u64,
}

/// Metadata returned after a WAV file has been fully written.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingMetadata {
    pub path: PathBuf,
    pub sample_rate: u32,
    /// Original input channel count, preserved in the interleaved WAV.
    pub channels: u16,
    pub bits_per_sample: u16,
    pub frames: u64,
    pub duration: Duration,
    /// Maximum number of frames accepted by the application's strict WAV importer.
    pub maximum_frames: u64,
    /// True when the take reached the safe import limit and trailing frames were discarded.
    pub truncated: bool,
    pub limit_discarded_samples: u64,
    pub dropped_samples: u64,
    pub overflow_events: u64,
    pub stream_errors: u64,
    pub dropouts: u64,
}

/// Handle for WAV encoding that is running away from both the audio and UI threads.
pub struct PendingRecording {
    writer: Option<JoinHandle<Result<RecordingMetadata>>>,
}

impl PendingRecording {
    /// Returns whether the background encoder has completed without blocking.
    pub fn is_finished(&self) -> bool {
        self.writer.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits for the background encoder and returns the completed file metadata.
    pub fn finish(mut self) -> Result<RecordingMetadata> {
        self.join_writer()
    }

    fn join_writer(&mut self) -> Result<RecordingMetadata> {
        self.writer
            .take()
            .ok_or_else(|| anyhow!("The recording writer was already joined"))?
            .join()
            .map_err(|_| anyhow!("The recording writer thread panicked"))?
    }
}

impl Drop for PendingRecording {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// An active capture from the system's default input device.
pub struct InputRecorder {
    stream: Option<Stream>,
    collector_shutdown: Arc<AtomicU8>,
    collector: Option<JoinHandle<Result<RecordingMetadata>>>,
    stats: Arc<CaptureStats>,
    device_name: String,
    output_path: PathBuf,
    sample_rate: u32,
    channels: u16,
    maximum_frames: u64,
    device_profile: AudioDeviceProfile,
}

impl InputRecorder {
    /// Starts the default input device and streams into a hidden temporary file next
    /// to `path`. The final path remains absent until `stop` durably commits the WAV.
    pub fn start_default_to_path(path: impl Into<PathBuf>) -> Result<Self> {
        Self::start_default_to_path_with_options(path, RecorderOptions::default())
    }

    /// Starts path-bound recording with an explicit bounded callback-memory policy.
    pub fn start_default_to_path_with_options(
        path: impl Into<PathBuf>,
        options: RecorderOptions,
    ) -> Result<Self> {
        Self::start_with_profile_to_path_with_options(
            path,
            &AudioDeviceProfile::system_default_input(),
            options,
        )
    }

    /// Starts the exact selected input profile. Stable identities are resolved
    /// again at arm time so a hot-unplugged device cannot be used accidentally.
    pub fn start_with_profile_to_path(
        path: impl Into<PathBuf>,
        profile: &AudioDeviceProfile,
    ) -> Result<Self> {
        Self::start_with_profile_to_path_with_options(path, profile, RecorderOptions::default())
    }

    pub fn start_with_profile_to_path_with_options(
        path: impl Into<PathBuf>,
        profile: &AudioDeviceProfile,
        options: RecorderOptions,
    ) -> Result<Self> {
        if profile.direction != AudioDeviceDirection::Input {
            bail!("Audio recording requires an input device profile");
        }
        let output_path = path.into();
        validate_output_path(&output_path)?;
        let resolved = resolve_audio_device(profile).with_context(|| {
            "Unable to resolve the requested audio input. Check device availability and microphone permissions"
        })?;
        let device_name = resolved.name.clone();
        let sample_format = resolved.sample_format();
        let config = resolved.stream_config();
        let device_profile = resolved.negotiated.resolved_profile();
        let device = resolved.device;
        let sample_rate = config.sample_rate;
        let channels = config.channels;
        if channels == 0 || sample_rate == 0 {
            bail!(
                "Input device '{device_name}' returned an invalid configuration ({channels} channels at {sample_rate} Hz)"
            );
        }
        if channels > DEFAULT_MAX_CHANNELS {
            bail!(
                "Input device '{device_name}' exposes {channels} channels, exceeding the application's safe WAV import limit of {DEFAULT_MAX_CHANNELS}"
            );
        }
        if sample_rate > DEFAULT_MAX_SAMPLE_RATE {
            bail!(
                "Input device '{device_name}' uses {sample_rate} Hz, exceeding the application's safe WAV import limit of {DEFAULT_MAX_SAMPLE_RATE} Hz"
            );
        }

        let channel_count = usize::from(channels);
        let requested_ring_frames =
            frames_for_duration(options.ring_buffer_duration, sample_rate, 64);
        let maximum_ring_samples = MAX_RING_BUFFER_BYTES / size_of::<f32>();
        let maximum_ring_frames = (maximum_ring_samples / channel_count).max(64);
        let ring_frames = requested_ring_frames.min(maximum_ring_frames);
        let ring_samples = ring_frames.checked_mul(channel_count).ok_or_else(|| {
            anyhow!("Requested recording ring buffer is too large for this platform")
        })?;
        let maximum_samples = maximum_recording_samples(channels)?;
        let maximum_frames = maximum_samples / u64::from(channels);

        let (producer, consumer) = RingBuffer::new(ring_samples);
        let stats = Arc::new(CaptureStats::default());
        let stream = build_input_stream(
            &device,
            config,
            sample_format,
            producer,
            channel_count,
            stats.clone(),
        )
        .with_context(|| {
            format!(
                "Unable to open input device '{device_name}' ({channels} channels at {sample_rate} Hz, {sample_format})"
            )
        })?;

        let writer = StreamingWavWriter::create(
            output_path.clone(),
            sample_rate,
            channels,
            maximum_samples,
        )?;
        let collector_shutdown = Arc::new(AtomicU8::new(COLLECTOR_RUNNING));
        let collector =
            spawn_collector(consumer, collector_shutdown.clone(), writer, stats.clone())?;

        if let Err(error) = stream.play() {
            drop(stream);
            collector_shutdown.store(COLLECTOR_ABORT, Ordering::Release);
            let _ = collector.join();
            return Err(error).with_context(|| {
                format!(
                    "Unable to start input device '{device_name}'. Check microphone permissions and device availability"
                )
            });
        }

        Ok(Self {
            stream: Some(stream),
            collector_shutdown,
            collector: Some(collector),
            stats,
            device_name,
            output_path,
            sample_rate,
            channels,
            maximum_frames,
            device_profile,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn device_profile(&self) -> &AudioDeviceProfile {
        &self.device_profile
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    pub fn status(&self) -> RecordingStatus {
        status_from_stats(
            &self.stats,
            self.sample_rate,
            self.channels,
            self.maximum_frames,
        )
    }

    /// Stops capture immediately. The existing collector drains the bounded ring,
    /// patches/syncs the WAV and atomically commits the path in the background.
    pub fn stop(mut self) -> Result<PendingRecording> {
        self.stream.take();
        self.collector_shutdown
            .store(COLLECTOR_COMMIT, Ordering::Release);
        let writer = self
            .collector
            .take()
            .ok_or_else(|| anyhow!("Recording collector is no longer available"))?;

        Ok(PendingRecording {
            writer: Some(writer),
        })
    }

    /// Stops and waits for durable commit on the calling non-real-time thread.
    pub fn stop_blocking(self) -> Result<RecordingMetadata> {
        self.stop()?.finish()
    }
}

impl Drop for InputRecorder {
    fn drop(&mut self) {
        self.stream.take();
        self.collector_shutdown
            .store(COLLECTOR_COMMIT, Ordering::Release);
        if let Some(collector) = self.collector.take() {
            let _ = collector.join();
        }
    }
}

fn no_input_device_message() -> &'static str {
    "No audio input device is available. Connect or enable a microphone/audio interface and check operating-system microphone permissions"
}

fn frames_for_duration(duration: Duration, sample_rate: u32, minimum: usize) -> usize {
    let frames = duration.as_secs_f64() * f64::from(sample_rate);
    if !frames.is_finite() || frames >= usize::MAX as f64 {
        usize::MAX
    } else {
        (frames.ceil() as usize).max(minimum)
    }
}

fn build_input_stream(
    device: &cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    producer: Producer<f32>,
    channels: usize,
    stats: Arc<CaptureStats>,
) -> Result<Stream, cpal::Error> {
    match format {
        SampleFormat::I8 => {
            build_typed_input_stream::<i8>(device, config, producer, channels, stats)
        }
        SampleFormat::I16 => {
            build_typed_input_stream::<i16>(device, config, producer, channels, stats)
        }
        SampleFormat::I24 => {
            build_typed_input_stream::<cpal::I24>(device, config, producer, channels, stats)
        }
        SampleFormat::I32 => {
            build_typed_input_stream::<i32>(device, config, producer, channels, stats)
        }
        SampleFormat::I64 => {
            build_typed_input_stream::<i64>(device, config, producer, channels, stats)
        }
        SampleFormat::U8 => {
            build_typed_input_stream::<u8>(device, config, producer, channels, stats)
        }
        SampleFormat::U16 => {
            build_typed_input_stream::<u16>(device, config, producer, channels, stats)
        }
        SampleFormat::U24 => {
            build_typed_input_stream::<cpal::U24>(device, config, producer, channels, stats)
        }
        SampleFormat::U32 => {
            build_typed_input_stream::<u32>(device, config, producer, channels, stats)
        }
        SampleFormat::U64 => {
            build_typed_input_stream::<u64>(device, config, producer, channels, stats)
        }
        SampleFormat::F32 => {
            build_typed_input_stream::<f32>(device, config, producer, channels, stats)
        }
        SampleFormat::F64 => {
            build_typed_input_stream::<f64>(device, config, producer, channels, stats)
        }
        SampleFormat::DsdU8 | SampleFormat::DsdU16 | SampleFormat::DsdU32 => {
            Err(cpal::Error::new(cpal::ErrorKind::UnsupportedConfig))
        }
        _ => Err(cpal::Error::new(cpal::ErrorKind::UnsupportedConfig)),
    }
}

fn build_typed_input_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut producer: Producer<f32>,
    channels: usize,
    stats: Arc<CaptureStats>,
) -> Result<Stream, cpal::Error>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let error_stats = stats.clone();
    device.build_input_stream(
        config,
        move |input: &[T], _| {
            let dropped = enqueue_input(input, channels, &mut producer, &stats);
            if dropped != 0 {
                stats.overflow_callbacks.fetch_add(1, Ordering::Relaxed);
            }
        },
        move |_error| {
            error_stats.stream_errors.fetch_add(1, Ordering::Relaxed);
        },
        None,
    )
}

/// Writes only complete interleaved frames. Checking capacity once per frame means
/// an overflow cannot shift channel alignment in the captured data.
fn enqueue_input<T>(
    input: &[T],
    channels: usize,
    producer: &mut Producer<f32>,
    stats: &CaptureStats,
) -> usize
where
    T: Sample,
    f32: FromSample<T>,
{
    debug_assert_ne!(channels, 0);
    let mut accepted = 0_usize;
    let mut dropped = 0_usize;
    let mut frames = input.chunks_exact(channels);

    for frame in &mut frames {
        if producer.slots() < channels {
            dropped += channels;
            continue;
        }
        for &sample in frame {
            // A single producer owns the buffer. Once the full-frame capacity check
            // succeeds, only this loop can consume write slots, so push cannot fail.
            let _ = producer.push(f32::from_sample(sample));
        }
        accepted += channels;
    }
    dropped += frames.remainder().len();

    if accepted != 0 {
        stats
            .accepted_samples
            .fetch_add(accepted as u64, Ordering::Relaxed);
    }
    if dropped != 0 {
        stats
            .dropped_samples
            .fetch_add(dropped as u64, Ordering::Relaxed);
    }
    dropped
}

fn spawn_collector(
    mut consumer: Consumer<f32>,
    shutdown: Arc<AtomicU8>,
    writer: StreamingWavWriter,
    stats: Arc<CaptureStats>,
) -> Result<JoinHandle<Result<RecordingMetadata>>> {
    thread::Builder::new()
        .name("citrus-input-collector".to_owned())
        .spawn(move || run_collector(&mut consumer, &shutdown, writer, &stats))
        .context("Unable to start the recording collector thread")
}

fn run_collector(
    consumer: &mut Consumer<f32>,
    shutdown: &AtomicU8,
    writer: StreamingWavWriter,
    stats: &CaptureStats,
) -> Result<RecordingMetadata> {
    let channels = usize::from(writer.channels);
    let frame_bytes = channels * PCM24_BYTES_PER_SAMPLE as usize;
    debug_assert!(frame_bytes <= STREAMING_ENCODE_BUFFER_BYTES);
    let maximum_samples = writer.maximum_samples;
    let mut writer = Some(writer);
    let mut failure = None;
    let mut frame = [0.0_f32; DEFAULT_MAX_CHANNELS as usize];
    let mut frame_len = 0_usize;
    let mut encoded = [0_u8; STREAMING_ENCODE_BUFFER_BYTES];
    let mut encoded_len = 0_usize;
    let mut encoded_samples = 0_u64;
    let mut recorded_samples = 0_u64;

    loop {
        let mut made_progress = false;
        while let Ok(sample) = consumer.pop() {
            made_progress = true;
            frame[frame_len] = sample;
            frame_len += 1;
            if frame_len != channels {
                continue;
            }
            frame_len = 0;

            if failure.is_some() {
                continue;
            }
            if recorded_samples >= maximum_samples {
                publish_limit_discarded_frame(stats, channels as u64);
                continue;
            }

            if encoded.len() - encoded_len < frame_bytes {
                let result = writer
                    .as_mut()
                    .expect("writer exists until its first error")
                    .write_pcm_bytes(&encoded[..encoded_len]);
                match result {
                    Ok(()) => {
                        stats
                            .written_samples
                            .fetch_add(encoded_samples, Ordering::Relaxed);
                        encoded_len = 0;
                        encoded_samples = 0;
                    }
                    Err(error) => {
                        stats.writer_errors.fetch_add(1, Ordering::Relaxed);
                        failure = Some(error.context("Streaming recording write failed"));
                        writer.take();
                        encoded_len = 0;
                        encoded_samples = 0;
                        continue;
                    }
                }
            }

            for &sample in &frame[..channels] {
                let bytes = encode_pcm24(sample);
                encoded[encoded_len..encoded_len + 3].copy_from_slice(&bytes);
                encoded_len += 3;
            }
            encoded_samples += channels as u64;
            recorded_samples += channels as u64;
        }

        match shutdown.load(Ordering::Acquire) {
            COLLECTOR_ABORT => bail!("Recording was aborted before commit"),
            COLLECTOR_COMMIT => {
                if frame_len != 0 && failure.is_none() {
                    stats.writer_errors.fetch_add(1, Ordering::Relaxed);
                    failure = Some(anyhow!(
                        "Recording ended with a partial {channels}-channel audio frame"
                    ));
                    writer.take();
                }
                if let Some(error) = failure {
                    return Err(error);
                }
                let mut writer = writer.expect("successful collector retains its writer");
                if encoded_len != 0 {
                    if let Err(error) = writer.write_pcm_bytes(&encoded[..encoded_len]) {
                        stats.writer_errors.fetch_add(1, Ordering::Relaxed);
                        return Err(error.context("Unable to flush the final recording block"));
                    }
                    stats
                        .written_samples
                        .fetch_add(encoded_samples, Ordering::Relaxed);
                }
                return match writer.finalize(stats) {
                    Ok(metadata) => Ok(metadata),
                    Err(error) => {
                        stats.writer_errors.fetch_add(1, Ordering::Relaxed);
                        Err(error)
                    }
                };
            }
            COLLECTOR_RUNNING => {}
            state => bail!("Recording collector received invalid shutdown state {state}"),
        }

        if !made_progress {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Publishes one complete frame discarded at the recording limit.
///
/// The relaxed counter update is sequenced before the release flag store. A
/// reader that observes `limit_reached` with acquire ordering can therefore use
/// the flag as proof that at least one complete frame is already reflected by
/// `limit_discarded_samples`. This path must remain lock-free and allocation-free.
#[inline]
fn publish_limit_discarded_frame(stats: &CaptureStats, channels: u64) {
    debug_assert_ne!(channels, 0);
    stats
        .limit_discarded_samples
        .fetch_add(channels, Ordering::Relaxed);
    stats.limit_reached.store(true, Ordering::Release);
}

fn status_from_stats(
    stats: &CaptureStats,
    sample_rate: u32,
    channels: u16,
    maximum_frames: u64,
) -> RecordingStatus {
    let captured_samples = stats.written_samples.load(Ordering::Relaxed);
    let captured_frames = captured_samples / u64::from(channels.max(1));
    let dropped_samples = stats.dropped_samples.load(Ordering::Relaxed);
    let overflow_events = stats.overflow_callbacks.load(Ordering::Relaxed);
    let stream_errors = stats.stream_errors.load(Ordering::Relaxed);
    let writer_errors = stats.writer_errors.load(Ordering::Relaxed);
    let limit_reached = stats.limit_reached.load(Ordering::Acquire);
    let limit_discarded_samples = stats.limit_discarded_samples.load(Ordering::Relaxed);
    RecordingStatus {
        sample_rate,
        channels,
        captured_frames,
        duration: duration_for_frames(captured_frames, sample_rate),
        maximum_frames,
        dropped_samples,
        overflow_events,
        stream_errors,
        writer_errors,
        limit_discarded_samples,
        limit_reached,
        dropouts: overflow_events.saturating_add(stream_errors),
    }
}

fn duration_for_frames(frames: u64, sample_rate: u32) -> Duration {
    if sample_rate == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(frames as f64 / f64::from(sample_rate))
    }
}

struct StreamingWavWriter {
    writer: Option<BufWriter<File>>,
    target_path: PathBuf,
    temp_path: PathBuf,
    sample_rate: u32,
    channels: u16,
    maximum_samples: u64,
    data_bytes: u64,
    committed: bool,
}

impl StreamingWavWriter {
    fn create(
        target_path: PathBuf,
        sample_rate: u32,
        channels: u16,
        maximum_samples: u64,
    ) -> Result<Self> {
        validate_recording_format(sample_rate, channels)?;
        validate_output_path(&target_path)?;
        let (file, temp_path) = create_same_directory_temp(&target_path)?;
        let mut writer = BufWriter::new(file);
        write_pcm24_header(&mut writer, sample_rate, channels, 0)?;
        Ok(Self {
            writer: Some(writer),
            target_path,
            temp_path,
            sample_rate,
            channels,
            maximum_samples,
            data_bytes: 0,
            committed: false,
        })
    }

    fn write_pcm_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if !bytes.len().is_multiple_of(PCM24_BYTES_PER_SAMPLE as usize) {
            bail!("PCM24 byte block is not sample-aligned");
        }
        let new_data_bytes = self
            .data_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| anyhow!("Recording data size overflowed"))?;
        let maximum_data_bytes = self
            .maximum_samples
            .checked_mul(PCM24_BYTES_PER_SAMPLE)
            .ok_or_else(|| anyhow!("Recording limit overflowed"))?;
        if new_data_bytes > maximum_data_bytes {
            bail!(
                "Recording block would exceed the safe import limit of {} interleaved samples",
                self.maximum_samples
            );
        }
        self.writer
            .as_mut()
            .ok_or_else(|| anyhow!("Recording temporary file is already closed"))?
            .write_all(bytes)
            .with_context(|| {
                format!(
                    "Unable to stream recording data to '{}'",
                    self.temp_path.display()
                )
            })?;
        self.data_bytes = new_data_bytes;
        Ok(())
    }

    fn finalize(mut self, stats: &CaptureStats) -> Result<RecordingMetadata> {
        if self.data_bytes == 0 {
            bail!("Recording contains no complete audio frames");
        }
        let padding = self.data_bytes & 1;
        let mut writer = self
            .writer
            .take()
            .ok_or_else(|| anyhow!("Recording temporary file is already closed"))?;
        if padding != 0 {
            writer.write_all(&[0]).with_context(|| {
                format!(
                    "Unable to pad recording temporary file '{}'",
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

        let frames = self.data_bytes / PCM24_BYTES_PER_SAMPLE / u64::from(self.channels);
        let maximum_frames = self.maximum_samples / u64::from(self.channels);
        let status = status_from_stats(stats, self.sample_rate, self.channels, maximum_frames);
        Ok(RecordingMetadata {
            path: self.target_path.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits_per_sample: RECORDING_BITS_PER_SAMPLE,
            frames,
            duration: duration_for_frames(frames, self.sample_rate),
            maximum_frames,
            truncated: status.limit_reached,
            limit_discarded_samples: status.limit_discarded_samples,
            dropped_samples: status.dropped_samples,
            overflow_events: status.overflow_events,
            stream_errors: status.stream_errors,
            dropouts: status.dropouts,
        })
    }
}

impl Drop for StreamingWavWriter {
    fn drop(&mut self) {
        self.writer.take();
        if !self.committed {
            let _ = std::fs::remove_file(&self.temp_path);
        }
    }
}

fn validate_recording_format(sample_rate: u32, channels: u16) -> Result<()> {
    if channels == 0 || channels > DEFAULT_MAX_CHANNELS {
        bail!("Recording channel count {channels} is outside the supported range");
    }
    if sample_rate == 0 || sample_rate > DEFAULT_MAX_SAMPLE_RATE {
        bail!("Recording sample rate {sample_rate} is outside the supported range");
    }
    let block_align = channels
        .checked_mul(RECORDING_BITS_PER_SAMPLE / 8)
        .ok_or_else(|| anyhow!("WAV block alignment overflowed"))?;
    sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or_else(|| anyhow!("WAV byte rate exceeds the RIFF limit"))?;
    Ok(())
}

fn validate_output_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        bail!("A recording output file path is required");
    }
    if path
        .try_exists()
        .with_context(|| format!("Unable to inspect recording path '{}'", path.display()))?
    {
        bail!(
            "Recording target '{}' already exists; refusing to overwrite it",
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
            "Unable to create recording directory '{}'",
            parent.display()
        )
    })?;
    let file_name = target_path
        .file_name()
        .ok_or_else(|| anyhow!("A recording output file name is required"))?
        .to_string_lossy();
    for _ in 0..128 {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{file_name}.citrus-{}-{sequence}.part",
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
                        "Unable to create recording temporary file '{}'",
                        temp_path.display()
                    )
                });
            }
        }
    }
    bail!(
        "Unable to reserve a unique recording temporary file beside '{}'",
        target_path.display()
    )
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
        bail!("Recording paths cannot contain embedded NUL characters");
    }
    existing.push(0);
    destination.push(0);
    // SAFETY: both pointers refer to live, NUL-terminated UTF-16 buffers for the
    // duration of the call. MoveFileW has no callback and does not retain them.
    let moved = unsafe { MoveFileW(existing.as_ptr(), destination.as_ptr()) };
    if moved == 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "Unable to atomically commit recording '{}' to '{}'; the destination may already exist",
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
            "Unable to atomically commit recording '{}' to '{}'; the destination may already exist",
            temp_path.display(),
            target_path.display()
        )
    })?;
    std::fs::remove_file(temp_path).with_context(|| {
        format!(
            "Recording was committed to '{}' but temporary link '{}' could not be removed",
            target_path.display(),
            temp_path.display()
        )
    })?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn commit_no_clobber(temp_path: &Path, target_path: &Path) -> Result<()> {
    if target_path
        .try_exists()
        .with_context(|| format!("Unable to inspect '{}'", target_path.display()))?
    {
        bail!(
            "Recording target '{}' appeared before commit",
            target_path.display()
        );
    }
    std::fs::rename(temp_path, target_path).with_context(|| {
        format!(
            "Unable to atomically commit '{}' to '{}'",
            temp_path.display(),
            target_path.display()
        )
    })
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<()> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("Unable to sync recording directory '{}'", parent.display()))
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<()> {
    Ok(())
}

/// Maximum complete frames that this module can write and the strict WAV importer
/// can subsequently decode without exceeding any configured allocation/RIFF limit.
pub fn maximum_recording_frames(channels: u16) -> Result<u64> {
    Ok(maximum_recording_samples(channels)? / u64::from(channels))
}

fn maximum_recording_samples(channels: u16) -> Result<u64> {
    if channels == 0 || channels > DEFAULT_MAX_CHANNELS {
        bail!("Recording channel count {channels} is outside the supported range");
    }
    let decoded_limit = DEFAULT_MAX_DECODED_SAMPLES as u64;
    let file_limit = DEFAULT_MAX_WAV_FILE_BYTES
        .checked_sub(PCM_WAV_HEADER_BYTES)
        .ok_or_else(|| anyhow!("Configured WAV file limit cannot hold a PCM header"))?
        / PCM24_BYTES_PER_SAMPLE;
    let riff_limit = (u64::from(u32::MAX) - 36) / PCM24_BYTES_PER_SAMPLE;
    let channel_count = u64::from(channels);
    let mut samples = decoded_limit.min(file_limit).min(riff_limit);
    samples -= samples % channel_count;
    while samples != 0 {
        let data_bytes = samples
            .checked_mul(PCM24_BYTES_PER_SAMPLE)
            .ok_or_else(|| anyhow!("Recording size limit overflowed"))?;
        let padding = data_bytes & 1;
        if PCM_WAV_HEADER_BYTES + data_bytes + padding <= DEFAULT_MAX_WAV_FILE_BYTES
            && 36 + data_bytes + padding <= u64::from(u32::MAX)
        {
            return Ok(samples);
        }
        samples = samples.saturating_sub(channel_count);
    }
    bail!("Configured WAV limits cannot hold one complete {channels}-channel frame")
}

fn write_pcm24_header(
    writer: &mut impl Write,
    sample_rate: u32,
    channels: u16,
    data_bytes: u64,
) -> Result<()> {
    validate_recording_format(sample_rate, channels)?;
    let padding = data_bytes & 1;
    let data_bytes = u32::try_from(data_bytes)
        .map_err(|_| anyhow!("Recording data exceeds the RIFF chunk-size limit"))?;
    let riff_size = 36_u64
        .checked_add(u64::from(data_bytes))
        .and_then(|size| size.checked_add(padding))
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| anyhow!("Recording exceeds the RIFF container-size limit"))?;
    let block_align = channels * (RECORDING_BITS_PER_SAMPLE / 8);
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
    writer.write_all(&RECORDING_BITS_PER_SAMPLE.to_le_bytes())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "citrus-recording-{label}-{}-{}.wav",
            std::process::id(),
            TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn encoded_samples(samples: &[f32]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| encode_pcm24(*sample))
            .collect()
    }

    fn push_frame(producer: &mut Producer<f32>, frame: &[f32], stats: &CaptureStats) {
        while producer.slots() < frame.len() {
            thread::yield_now();
        }
        for &sample in frame {
            producer.push(sample).unwrap();
        }
        stats
            .accepted_samples
            .fetch_add(frame.len() as u64, Ordering::Relaxed);
    }

    #[test]
    fn bounded_channel_drops_whole_frames_and_preserves_alignment() {
        let (mut producer, mut consumer) = RingBuffer::new(4);
        let stats = CaptureStats::default();
        let input = [-1.0_f32, -0.5, 0.0, 0.5, 0.75, 1.0];

        let dropped = enqueue_input(&input, 2, &mut producer, &stats);

        assert_eq!(dropped, 2);
        assert_eq!(stats.accepted_samples.load(Ordering::Relaxed), 4);
        assert_eq!(stats.dropped_samples.load(Ordering::Relaxed), 2);
        let mut captured = Vec::new();
        while let Ok(sample) = consumer.pop() {
            captured.push(sample);
        }
        assert_eq!(captured, input[..4]);
        assert_eq!(captured.len() % 2, 0);
    }

    #[test]
    fn wav_encoder_writes_valid_pcm24_stereo_header_and_samples() {
        let path = test_path("header");
        let samples = [-1.0_f32, 1.0, 0.0, 0.5];
        let stats = CaptureStats::default();
        let mut writer = StreamingWavWriter::create(path.clone(), 48_000, 2, 4).unwrap();
        let encoded = encoded_samples(&samples);
        writer.write_pcm_bytes(&encoded).unwrap();
        stats.written_samples.store(4, Ordering::Relaxed);
        let metadata = writer.finalize(&stats).unwrap();
        let wav = std::fs::read(&path).unwrap();

        assert_eq!(metadata.frames, 2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 288_000);
        assert_eq!(u16::from_le_bytes([wav[32], wav[33]]), 6);
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 24);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 12);
        assert_eq!(wav.len(), 56);
        assert_eq!(&wav[44..47], &[0x00, 0x00, 0x80]);
        assert_eq!(&wav[47..50], &[0xff, 0xff, 0x7f]);
        assert_eq!(&wav[50..53], &[0x00, 0x00, 0x00]);
        assert_eq!(&wav[53..56], &[0x00, 0x00, 0x40]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pcm24_mono_odd_frame_is_padded_and_decodes_with_strict_reader() {
        let path = test_path("mono-padding");
        let stats = CaptureStats::default();
        let mut writer = StreamingWavWriter::create(path.clone(), 48_000, 1, 1).unwrap();
        writer.write_pcm_bytes(&encode_pcm24(0.5)).unwrap();
        stats.written_samples.store(1, Ordering::Relaxed);
        writer.finalize(&stats).unwrap();

        let wav = std::fs::read(&path).unwrap();
        assert_eq!(wav.len(), 48);
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 3);
        assert_eq!(wav[47], 0);
        let decoded = crate::wav::read_wav(&path).unwrap();
        assert_eq!(decoded.metadata.frames, 1);
        assert_eq!(decoded.metadata.channels, 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn pcm24_conversion_is_bounded_and_sanitizes_non_finite_values() {
        assert_eq!(encode_pcm24(-2.0), [0x00, 0x00, 0x80]);
        assert_eq!(encode_pcm24(2.0), [0xff, 0xff, 0x7f]);
        assert_eq!(encode_pcm24(f32::NAN), [0x00, 0x00, 0x00]);
        assert_eq!(encode_pcm24(f32::INFINITY), [0x00, 0x00, 0x00]);
    }

    #[test]
    fn dropping_pending_recording_waits_for_writer_completion() {
        let completed = Arc::new(AtomicBool::new(false));
        let writer_completed = completed.clone();
        let pending = PendingRecording {
            writer: Some(thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                writer_completed.store(true, Ordering::Release);
                Err(anyhow!("intentional test result"))
            })),
        };

        drop(pending);

        assert!(completed.load(Ordering::Acquire));
    }

    #[test]
    fn long_stream_uses_small_ring_and_commits_a_decodable_wav() {
        let path = test_path("long-stream");
        let stats = Arc::new(CaptureStats::default());
        let shutdown = Arc::new(AtomicU8::new(COLLECTOR_RUNNING));
        let (mut producer, consumer) = RingBuffer::new(64);
        let writer = StreamingWavWriter::create(
            path.clone(),
            48_000,
            2,
            maximum_recording_samples(2).unwrap(),
        )
        .unwrap();
        let collector = spawn_collector(consumer, shutdown.clone(), writer, stats.clone()).unwrap();
        let frames = 200_000_u64;
        for frame in 0..frames {
            let sample = if frame.is_multiple_of(2) { 0.25 } else { -0.25 };
            push_frame(&mut producer, &[sample, -sample], &stats);
        }
        drop(producer);
        shutdown.store(COLLECTOR_COMMIT, Ordering::Release);

        let metadata = collector.join().unwrap().unwrap();

        assert_eq!(metadata.frames, frames);
        assert!(!metadata.truncated);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 44 + frames * 6);
        let decoded = crate::wav::read_wav(&path).unwrap();
        assert_eq!(decoded.metadata.frames, frames);
        assert_eq!(decoded.samples.len(), frames as usize * 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn limit_truncates_on_a_complete_frame_and_reports_discarded_samples() {
        let path = test_path("limit");
        let stats = Arc::new(CaptureStats::default());
        let shutdown = Arc::new(AtomicU8::new(COLLECTOR_RUNNING));
        let (mut producer, consumer) = RingBuffer::new(8);
        let writer = StreamingWavWriter::create(path.clone(), 48_000, 2, 4).unwrap();
        let collector = spawn_collector(consumer, shutdown.clone(), writer, stats.clone()).unwrap();
        for _ in 0..3 {
            push_frame(&mut producer, &[0.25, -0.25], &stats);
        }
        drop(producer);
        shutdown.store(COLLECTOR_COMMIT, Ordering::Release);

        let metadata = collector.join().unwrap().unwrap();

        assert_eq!(metadata.frames, 2);
        assert!(metadata.truncated);
        assert_eq!(metadata.limit_discarded_samples, 2);
        let decoded = crate::wav::read_wav(&path).unwrap();
        assert_eq!(decoded.metadata.frames, 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn concurrent_limit_flag_proves_a_complete_discarded_frame_and_metadata_matches() {
        let path = test_path("limit-publication");
        let stats = Arc::new(CaptureStats::default());
        let shutdown = Arc::new(AtomicU8::new(COLLECTOR_RUNNING));
        let (mut producer, consumer) = RingBuffer::new(8);
        let writer = StreamingWavWriter::create(path.clone(), 48_000, 2, 4).unwrap();
        let collector = spawn_collector(consumer, shutdown.clone(), writer, stats.clone()).unwrap();

        // Fill exactly to the two-frame limit first. Draining those samples must
        // not publish truncation; only the following complete frame may do so.
        push_frame(&mut producer, &[0.25, -0.25], &stats);
        push_frame(&mut producer, &[0.5, -0.5], &stats);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while producer.slots() != 8 {
            assert!(
                std::time::Instant::now() < deadline,
                "collector did not drain the exact-limit frames"
            );
            thread::yield_now();
        }
        let exact_limit = status_from_stats(&stats, 48_000, 2, 2);
        assert!(!exact_limit.limit_reached);
        assert_eq!(exact_limit.limit_discarded_samples, 0);

        push_frame(&mut producer, &[0.75, -0.75], &stats);
        let observed = loop {
            let status = status_from_stats(&stats, 48_000, 2, 2);
            if status.limit_reached {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "collector did not publish the recording limit"
            );
            thread::yield_now();
        };

        assert!(observed.limit_discarded_samples >= 2);
        assert_eq!(observed.limit_discarded_samples % 2, 0);

        drop(producer);
        shutdown.store(COLLECTOR_COMMIT, Ordering::Release);
        let metadata = collector.join().unwrap().unwrap();
        let final_status = status_from_stats(&stats, 48_000, 2, 2);

        assert_eq!(metadata.frames, 2);
        assert_eq!(metadata.frames, final_status.captured_frames);
        assert_eq!(metadata.truncated, final_status.limit_reached);
        assert!(metadata.truncated);
        assert_eq!(
            metadata.limit_discarded_samples,
            final_status.limit_discarded_samples
        );
        assert_eq!(metadata.limit_discarded_samples, 2);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn commit_collision_preserves_target_and_cleans_temporary_file() {
        let path = test_path("collision");
        let stats = Arc::new(CaptureStats::default());
        let shutdown = Arc::new(AtomicU8::new(COLLECTOR_RUNNING));
        let (mut producer, consumer) = RingBuffer::new(8);
        let writer = StreamingWavWriter::create(path.clone(), 48_000, 2, 4).unwrap();
        let temp_path = writer.temp_path.clone();
        let collector = spawn_collector(consumer, shutdown.clone(), writer, stats.clone()).unwrap();
        push_frame(&mut producer, &[0.25, -0.25], &stats);
        drop(producer);
        std::fs::write(&path, b"existing project audio").unwrap();
        shutdown.store(COLLECTOR_COMMIT, Ordering::Release);

        let error = collector.join().unwrap().unwrap_err();

        assert!(error.to_string().contains("atomically commit"));
        assert_eq!(std::fs::read(&path).unwrap(), b"existing project audio");
        assert!(!temp_path.exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn dropping_active_recorder_waits_for_durable_commit_without_hardware() {
        let path = test_path("drop-durable");
        let stats = Arc::new(CaptureStats::default());
        let shutdown = Arc::new(AtomicU8::new(COLLECTOR_RUNNING));
        let (mut producer, consumer) = RingBuffer::new(8);
        let maximum_samples = maximum_recording_samples(2).unwrap();
        let writer = StreamingWavWriter::create(path.clone(), 48_000, 2, maximum_samples).unwrap();
        let collector = spawn_collector(consumer, shutdown.clone(), writer, stats.clone()).unwrap();
        push_frame(&mut producer, &[-1.0, 1.0], &stats);
        push_frame(&mut producer, &[0.0, 0.5], &stats);
        drop(producer);
        let recorder = InputRecorder {
            stream: None,
            collector_shutdown: shutdown,
            collector: Some(collector),
            stats,
            device_name: "test input".to_owned(),
            output_path: path.clone(),
            sample_rate: 48_000,
            channels: 2,
            maximum_frames: maximum_samples / 2,
            device_profile: AudioDeviceProfile::system_default_input(),
        };

        drop(recorder);

        let decoded = crate::wav::read_wav(&path).unwrap();
        assert_eq!(decoded.metadata.frames, 2);
        assert_eq!(decoded.metadata.bits_per_sample, 24);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn safe_limit_matches_decoded_sample_budget_without_allocating_it() {
        assert_eq!(
            maximum_recording_frames(2).unwrap() * 2,
            DEFAULT_MAX_DECODED_SAMPLES as u64
        );
        let three_channel_samples = maximum_recording_frames(3).unwrap() * 3;
        assert!(three_channel_samples <= DEFAULT_MAX_DECODED_SAMPLES as u64);
        assert!(DEFAULT_MAX_DECODED_SAMPLES as u64 - three_channel_samples < u64::from(3_u16));
        let data_bytes = three_channel_samples * PCM24_BYTES_PER_SAMPLE;
        let padding = data_bytes & 1;
        assert!(PCM_WAV_HEADER_BYTES + data_bytes + padding <= DEFAULT_MAX_WAV_FILE_BYTES);
        assert!(36 + data_bytes + padding <= u64::from(u32::MAX));
    }
}
