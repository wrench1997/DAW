//! Strict RIFF/WAVE asset decoding and deterministic linear resampling.
//!
//! This module intentionally supports a small, production-useful subset instead
//! of guessing at malformed files: integer PCM 16/24/32-bit and IEEE float32,
//! including `WAVE_FORMAT_EXTENSIBLE` multichannel files. All decoded audio is
//! interleaved `f32`; integer PCM is normalized and float input is clamped to the
//! normalized `[-1.0, 1.0]` range after non-finite values are rejected.

use std::{fs, path::Path};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

const RIFF_HEADER_BYTES: usize = 12;
const CHUNK_HEADER_BYTES: usize = 8;
const PCM_FORMAT_TAG: u16 = 0x0001;
const IEEE_FLOAT_FORMAT_TAG: u16 = 0x0003;
const EXTENSIBLE_FORMAT_TAG: u16 = 0xfffe;
const PCM_SUBFORMAT: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];
const IEEE_FLOAT_SUBFORMAT: [u8; 16] = [
    0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

/// Default upper bound for a source file read into memory (1 GiB).
pub const DEFAULT_MAX_WAV_FILE_BYTES: u64 = 1_073_741_824;
/// Default upper bound for the decoded interleaved buffer (128M f32 samples / 512 MiB).
pub const DEFAULT_MAX_DECODED_SAMPLES: usize = 134_217_728;
/// Practical safety ceiling for an imported hardware/file sample rate.
pub const DEFAULT_MAX_SAMPLE_RATE: u32 = 768_000;
/// Practical safety ceiling that still covers large immersive-audio layouts.
pub const DEFAULT_MAX_CHANNELS: u16 = 256;

/// Explicit allocation and format limits for untrusted WAV assets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WavReadLimits {
    pub max_file_bytes: u64,
    pub max_decoded_samples: usize,
    pub max_sample_rate: u32,
    pub max_channels: u16,
}

impl Default for WavReadLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_WAV_FILE_BYTES,
            max_decoded_samples: DEFAULT_MAX_DECODED_SAMPLES,
            max_sample_rate: DEFAULT_MAX_SAMPLE_RATE,
            max_channels: DEFAULT_MAX_CHANNELS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WavSampleEncoding {
    PcmInteger,
    IeeeFloat,
}

/// Serializable source/current-buffer metadata for the DAW asset database.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WavMetadata {
    pub sample_rate: u32,
    pub channels: u16,
    pub encoding: WavSampleEncoding,
    /// Storage width in the source container (or 32 after resampling to f32).
    pub bits_per_sample: u16,
    /// Significant PCM bits. This may be smaller than the extensible container.
    pub valid_bits_per_sample: u16,
    pub channel_mask: Option<u32>,
    pub block_align: u16,
    pub byte_rate: u32,
    pub frames: u64,
    pub duration_seconds: f64,
    pub data_bytes: u64,
}

/// Decoded normalized, interleaved audio and its metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct WavAsset {
    pub metadata: WavMetadata,
    pub samples: Vec<f32>,
}

impl WavAsset {
    pub fn frame_count(&self) -> usize {
        self.samples.len() / usize::from(self.metadata.channels.max(1))
    }

    /// Returns a new f32 asset at `target_sample_rate` while preserving all channels.
    pub fn resample_linear(&self, target_sample_rate: u32) -> Result<Self> {
        let samples = resample_interleaved_linear(
            &self.samples,
            self.metadata.channels,
            self.metadata.sample_rate,
            target_sample_rate,
        )?;
        let frames = samples.len() / usize::from(self.metadata.channels);
        let block_align = self
            .metadata
            .channels
            .checked_mul(4)
            .ok_or_else(|| anyhow!("Resampled WAV block alignment overflows u16"))?;
        let byte_rate = target_sample_rate
            .checked_mul(u32::from(block_align))
            .ok_or_else(|| anyhow!("Resampled WAV byte rate overflows u32"))?;
        let data_bytes = samples
            .len()
            .checked_mul(4)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| anyhow!("Resampled WAV data size overflows u64"))?;

        Ok(Self {
            metadata: WavMetadata {
                sample_rate: target_sample_rate,
                channels: self.metadata.channels,
                encoding: WavSampleEncoding::IeeeFloat,
                bits_per_sample: 32,
                valid_bits_per_sample: 32,
                channel_mask: self.metadata.channel_mask,
                block_align,
                byte_rate,
                frames: frames as u64,
                duration_seconds: frames as f64 / f64::from(target_sample_rate),
                data_bytes,
            },
            samples,
        })
    }
}

/// Reads and decodes a WAV asset using conservative commercial-project limits.
pub fn read_wav(path: impl AsRef<Path>) -> Result<WavAsset> {
    read_wav_with_limits(path, WavReadLimits::default())
}

/// Reads and decodes a WAV asset after checking the file size before allocation.
pub fn read_wav_with_limits(path: impl AsRef<Path>, limits: WavReadLimits) -> Result<WavAsset> {
    validate_limits(limits)?;
    let path = path.as_ref();
    let metadata = fs::metadata(path)
        .with_context(|| format!("Unable to inspect WAV asset '{}'", path.display()))?;
    if !metadata.is_file() {
        bail!("WAV asset '{}' is not a regular file", path.display());
    }
    if metadata.len() > limits.max_file_bytes {
        bail!(
            "WAV asset '{}' is {} bytes, exceeding the configured {} byte limit",
            path.display(),
            metadata.len(),
            limits.max_file_bytes
        );
    }
    let bytes =
        fs::read(path).with_context(|| format!("Unable to read WAV asset '{}'", path.display()))?;
    decode_wav_with_limits(&bytes, limits)
        .with_context(|| format!("Invalid WAV asset '{}'", path.display()))
}

/// Decodes WAV bytes using conservative commercial-project limits.
pub fn decode_wav(bytes: &[u8]) -> Result<WavAsset> {
    decode_wav_with_limits(bytes, WavReadLimits::default())
}

/// Strictly decodes WAV bytes under explicit allocation and format limits.
pub fn decode_wav_with_limits(bytes: &[u8], limits: WavReadLimits) -> Result<WavAsset> {
    validate_limits(limits)?;
    if bytes.len() as u64 > limits.max_file_bytes {
        bail!(
            "WAV input is {} bytes, exceeding the configured {} byte limit",
            bytes.len(),
            limits.max_file_bytes
        );
    }
    if bytes.len() < RIFF_HEADER_BYTES {
        bail!("Truncated RIFF/WAVE header: expected at least 12 bytes");
    }
    if &bytes[0..4] != b"RIFF" {
        bail!("Unsupported WAV container: expected little-endian RIFF");
    }
    if &bytes[8..12] != b"WAVE" {
        bail!("Invalid RIFF form type: expected WAVE");
    }

    let riff_payload_size = read_u32(bytes, 4)? as usize;
    let riff_end = riff_payload_size
        .checked_add(8)
        .ok_or_else(|| anyhow!("RIFF size overflows this platform"))?;
    if riff_end > bytes.len() {
        bail!(
            "Truncated RIFF container: header declares {riff_end} bytes but only {} are present",
            bytes.len()
        );
    }
    if riff_end < bytes.len() {
        bail!(
            "Invalid RIFF container: {} trailing bytes exist outside the declared container",
            bytes.len() - riff_end
        );
    }
    if riff_end < RIFF_HEADER_BYTES {
        bail!("Invalid RIFF size: WAVE form header is incomplete");
    }

    let mut format = None;
    let mut audio_data = None;
    let mut cursor = RIFF_HEADER_BYTES;
    while cursor < riff_end {
        let remaining = riff_end - cursor;
        if remaining < CHUNK_HEADER_BYTES {
            bail!("Truncated RIFF chunk header at byte {cursor}: only {remaining} bytes remain");
        }
        let id = &bytes[cursor..cursor + 4];
        let chunk_size = read_u32(bytes, cursor + 4)? as usize;
        let data_start = cursor + CHUNK_HEADER_BYTES;
        let data_end = data_start
            .checked_add(chunk_size)
            .ok_or_else(|| anyhow!("RIFF chunk at byte {cursor} overflows this platform"))?;
        if data_end > riff_end {
            bail!(
                "Truncated RIFF chunk '{}' at byte {cursor}: declares {chunk_size} data bytes",
                chunk_name(id)
            );
        }
        let chunk = &bytes[data_start..data_end];

        match id {
            b"fmt " => {
                if format.is_some() {
                    bail!("Invalid WAV: duplicate fmt chunk");
                }
                format = Some(parse_format(chunk, limits)?);
            }
            b"data" => {
                if audio_data.is_some() {
                    bail!("Invalid WAV: duplicate data chunk");
                }
                audio_data = Some(chunk);
            }
            _ => {}
        }

        cursor = data_end;
        if !chunk_size.is_multiple_of(2) {
            if cursor >= riff_end {
                bail!(
                    "Truncated RIFF padding after odd-sized '{}' chunk",
                    chunk_name(id)
                );
            }
            cursor += 1;
        }
    }

    let format = format.ok_or_else(|| anyhow!("Invalid WAV: missing fmt chunk"))?;
    let audio_data = audio_data.ok_or_else(|| anyhow!("Invalid WAV: missing data chunk"))?;
    let block_align = usize::from(format.block_align);
    if audio_data.len() % block_align != 0 {
        bail!(
            "Invalid data chunk: {} bytes are not divisible by block alignment {}",
            audio_data.len(),
            block_align
        );
    }
    let bytes_per_sample = usize::from(format.bits_per_sample / 8);
    let sample_count = audio_data.len() / bytes_per_sample;
    if sample_count > limits.max_decoded_samples {
        bail!(
            "Decoded WAV requires {sample_count} samples, exceeding the configured {} sample limit",
            limits.max_decoded_samples
        );
    }
    let expected_samples = (audio_data.len() / block_align)
        .checked_mul(usize::from(format.channels))
        .ok_or_else(|| anyhow!("Decoded WAV sample count overflows this platform"))?;
    if sample_count != expected_samples {
        bail!("Invalid WAV layout: data size and channel layout disagree");
    }

    let samples = decode_samples(audio_data, format, sample_count)?;
    let frames = audio_data.len() / block_align;
    Ok(WavAsset {
        metadata: WavMetadata {
            sample_rate: format.sample_rate,
            channels: format.channels,
            encoding: format.encoding,
            bits_per_sample: format.bits_per_sample,
            valid_bits_per_sample: format.valid_bits_per_sample,
            channel_mask: format.channel_mask,
            block_align: format.block_align,
            byte_rate: format.byte_rate,
            frames: frames as u64,
            duration_seconds: frames as f64 / f64::from(format.sample_rate),
            data_bytes: audio_data.len() as u64,
        },
        samples,
    })
}

/// Deterministic endpoint-preserving linear resampling for interleaved audio.
///
/// The first and last source frames are always the first and last output frames.
/// Output length is derived with integer round-half-up arithmetic from the interval
/// count, avoiding cumulative phase drift and architecture-dependent length changes.
pub fn resample_interleaved_linear(
    samples: &[f32],
    channels: u16,
    source_sample_rate: u32,
    target_sample_rate: u32,
) -> Result<Vec<f32>> {
    if channels == 0 || channels > DEFAULT_MAX_CHANNELS {
        bail!("Invalid resampling channel count: {channels}");
    }
    if source_sample_rate == 0 || source_sample_rate > DEFAULT_MAX_SAMPLE_RATE {
        bail!("Invalid source sample rate: {source_sample_rate} Hz");
    }
    if target_sample_rate == 0 || target_sample_rate > DEFAULT_MAX_SAMPLE_RATE {
        bail!("Invalid target sample rate: {target_sample_rate} Hz");
    }
    let channels_usize = usize::from(channels);
    if !samples.len().is_multiple_of(channels_usize) {
        bail!(
            "Interleaved sample count {} is not divisible by {channels} channels",
            samples.len()
        );
    }
    if samples.len() > DEFAULT_MAX_DECODED_SAMPLES {
        bail!(
            "Resampling input contains {} samples, exceeding the {} sample limit",
            samples.len(),
            DEFAULT_MAX_DECODED_SAMPLES
        );
    }
    if let Some((index, _)) = samples
        .iter()
        .enumerate()
        .find(|(_, sample)| !sample.is_finite())
    {
        bail!("Cannot resample non-finite sample at interleaved index {index}");
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    if source_sample_rate == target_sample_rate {
        return Ok(samples.to_vec());
    }

    let input_frames = samples.len() / channels_usize;
    if input_frames == 1 {
        return Ok(samples.to_vec());
    }
    let input_intervals = (input_frames - 1) as u128;
    let scaled_intervals = input_intervals
        .checked_mul(u128::from(target_sample_rate))
        .ok_or_else(|| anyhow!("Resampled frame count overflows u128"))?;
    let output_intervals = (scaled_intervals
        .checked_add(u128::from(source_sample_rate) / 2)
        .ok_or_else(|| anyhow!("Resampled frame rounding overflows u128"))?
        / u128::from(source_sample_rate))
    .max(1);
    let output_frames_u128 = output_intervals
        .checked_add(1)
        .ok_or_else(|| anyhow!("Resampled frame count overflows u128"))?;
    let output_frames = usize::try_from(output_frames_u128)
        .map_err(|_| anyhow!("Resampled frame count exceeds this platform"))?;
    let output_samples = output_frames
        .checked_mul(channels_usize)
        .ok_or_else(|| anyhow!("Resampled sample count overflows this platform"))?;
    if output_samples > DEFAULT_MAX_DECODED_SAMPLES {
        bail!(
            "Resampling requires {output_samples} samples, exceeding the {} sample limit",
            DEFAULT_MAX_DECODED_SAMPLES
        );
    }

    let mut output = Vec::new();
    output
        .try_reserve_exact(output_samples)
        .context("Unable to allocate the resampled audio buffer")?;
    for output_frame in 0..output_frames {
        let numerator = (output_frame as u128)
            .checked_mul(input_intervals)
            .ok_or_else(|| anyhow!("Resampling position overflows u128"))?;
        let left_frame = usize::try_from(numerator / output_intervals)
            .map_err(|_| anyhow!("Resampling position exceeds this platform"))?;
        let remainder = numerator % output_intervals;
        let fraction = remainder as f64 / output_intervals as f64;
        let right_frame = (left_frame + 1).min(input_frames - 1);
        let left_offset = left_frame * channels_usize;
        let right_offset = right_frame * channels_usize;
        for channel in 0..channels_usize {
            let left = f64::from(samples[left_offset + channel]);
            let right = f64::from(samples[right_offset + channel]);
            output.push((left + (right - left) * fraction) as f32);
        }
    }
    Ok(output)
}

#[derive(Clone, Copy, Debug)]
struct ParsedFormat {
    encoding: WavSampleEncoding,
    channels: u16,
    sample_rate: u32,
    byte_rate: u32,
    block_align: u16,
    bits_per_sample: u16,
    valid_bits_per_sample: u16,
    channel_mask: Option<u32>,
}

fn validate_limits(limits: WavReadLimits) -> Result<()> {
    if limits.max_file_bytes < RIFF_HEADER_BYTES as u64 {
        bail!("WAV file-size limit must allow at least a 12-byte RIFF header");
    }
    if limits.max_decoded_samples == 0 {
        bail!("WAV decoded-sample limit must be greater than zero");
    }
    if limits.max_sample_rate == 0 {
        bail!("WAV sample-rate limit must be greater than zero");
    }
    if limits.max_channels == 0 {
        bail!("WAV channel limit must be greater than zero");
    }
    Ok(())
}

fn parse_format(bytes: &[u8], limits: WavReadLimits) -> Result<ParsedFormat> {
    if bytes.len() < 16 {
        bail!(
            "Truncated fmt chunk: expected at least 16 bytes, found {}",
            bytes.len()
        );
    }
    let format_tag = read_u16(bytes, 0)?;
    let channels = read_u16(bytes, 2)?;
    let sample_rate = read_u32(bytes, 4)?;
    let byte_rate = read_u32(bytes, 8)?;
    let block_align = read_u16(bytes, 12)?;
    let bits_per_sample = read_u16(bytes, 14)?;

    if channels == 0 || channels > limits.max_channels {
        bail!(
            "Invalid/unsupported WAV channel count {channels}; configured maximum is {}",
            limits.max_channels
        );
    }
    if sample_rate == 0 || sample_rate > limits.max_sample_rate {
        bail!(
            "Invalid/unsupported WAV sample rate {sample_rate} Hz; configured maximum is {} Hz",
            limits.max_sample_rate
        );
    }

    let (encoding, valid_bits_per_sample, channel_mask) = match format_tag {
        PCM_FORMAT_TAG | IEEE_FLOAT_FORMAT_TAG => {
            if bytes.len() != 16 {
                if bytes.len() < 18 {
                    bail!("Truncated WAVEFORMATEX extension in fmt chunk");
                }
                let extension_size = usize::from(read_u16(bytes, 16)?);
                if extension_size != bytes.len() - 18 {
                    bail!(
                        "Invalid fmt extension size {extension_size}; chunk contains {} extension bytes",
                        bytes.len() - 18
                    );
                }
                if extension_size != 0 {
                    bail!("Unexpected extension data for non-extensible WAV format");
                }
            }
            let encoding = if format_tag == PCM_FORMAT_TAG {
                WavSampleEncoding::PcmInteger
            } else {
                WavSampleEncoding::IeeeFloat
            };
            (encoding, bits_per_sample, None)
        }
        EXTENSIBLE_FORMAT_TAG => {
            if bytes.len() < 40 {
                bail!("Truncated WAVE_FORMAT_EXTENSIBLE fmt chunk: expected at least 40 bytes");
            }
            let extension_size = usize::from(read_u16(bytes, 16)?);
            if extension_size < 22 {
                bail!(
                    "Invalid extensible fmt extension size {extension_size}; expected at least 22"
                );
            }
            if extension_size != bytes.len() - 18 {
                bail!(
                    "Invalid extensible fmt size {extension_size}; chunk contains {} extension bytes",
                    bytes.len() - 18
                );
            }
            let valid_bits = read_u16(bytes, 18)?;
            let mask = read_u32(bytes, 20)?;
            let subformat: [u8; 16] = bytes[24..40]
                .try_into()
                .map_err(|_| anyhow!("Truncated extensible subformat GUID"))?;
            let encoding = match subformat {
                PCM_SUBFORMAT => WavSampleEncoding::PcmInteger,
                IEEE_FLOAT_SUBFORMAT => WavSampleEncoding::IeeeFloat,
                _ => bail!("Unsupported WAVE_FORMAT_EXTENSIBLE subformat GUID"),
            };
            (encoding, valid_bits, Some(mask))
        }
        other => bail!("Unsupported WAV format tag 0x{other:04x}"),
    };

    match encoding {
        WavSampleEncoding::PcmInteger if !matches!(bits_per_sample, 16 | 24 | 32) => {
            bail!("Unsupported integer PCM width: {bits_per_sample} bits")
        }
        WavSampleEncoding::IeeeFloat if bits_per_sample != 32 => {
            bail!("Unsupported IEEE float width: {bits_per_sample} bits")
        }
        _ => {}
    }
    if valid_bits_per_sample == 0 || valid_bits_per_sample > bits_per_sample {
        bail!(
            "Invalid valid-bits value {valid_bits_per_sample} for a {bits_per_sample}-bit container"
        );
    }
    if encoding == WavSampleEncoding::IeeeFloat && valid_bits_per_sample != 32 {
        bail!("IEEE float WAV must declare 32 valid bits");
    }

    let bytes_per_sample = bits_per_sample / 8;
    let expected_block_align = channels
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| anyhow!("WAV block alignment overflows u16"))?;
    if block_align != expected_block_align {
        bail!(
            "Invalid WAV block alignment {block_align}; expected {expected_block_align} for {channels} channels at {bits_per_sample} bits"
        );
    }
    let expected_byte_rate = sample_rate
        .checked_mul(u32::from(block_align))
        .ok_or_else(|| anyhow!("WAV byte rate overflows u32"))?;
    if byte_rate != expected_byte_rate {
        bail!("Invalid WAV byte rate {byte_rate}; expected {expected_byte_rate}");
    }

    Ok(ParsedFormat {
        encoding,
        channels,
        sample_rate,
        byte_rate,
        block_align,
        bits_per_sample,
        valid_bits_per_sample,
        channel_mask,
    })
}

fn decode_samples(bytes: &[u8], format: ParsedFormat, sample_count: usize) -> Result<Vec<f32>> {
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(sample_count)
        .context("Unable to allocate the decoded WAV sample buffer")?;

    match (format.encoding, format.bits_per_sample) {
        (WavSampleEncoding::PcmInteger, 16) => {
            for sample in bytes.as_chunks::<2>().0 {
                let value = i16::from_le_bytes([sample[0], sample[1]]) as i64;
                samples.push(normalize_pcm(value, 16, format.valid_bits_per_sample));
            }
        }
        (WavSampleEncoding::PcmInteger, 24) => {
            for sample in bytes.as_chunks::<3>().0 {
                let packed = i32::from(sample[0])
                    | (i32::from(sample[1]) << 8)
                    | (i32::from(sample[2]) << 16);
                let value = ((packed << 8) >> 8) as i64;
                samples.push(normalize_pcm(value, 24, format.valid_bits_per_sample));
            }
        }
        (WavSampleEncoding::PcmInteger, 32) => {
            for sample in bytes.as_chunks::<4>().0 {
                let value = i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as i64;
                samples.push(normalize_pcm(value, 32, format.valid_bits_per_sample));
            }
        }
        (WavSampleEncoding::IeeeFloat, 32) => {
            for (index, sample) in bytes.as_chunks::<4>().0.iter().enumerate() {
                let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
                if !value.is_finite() {
                    bail!("Non-finite IEEE float sample at interleaved index {index}");
                }
                samples.push(value.clamp(-1.0, 1.0));
            }
        }
        _ => bail!("Internal WAV decoder format mismatch"),
    }
    if samples.len() != sample_count {
        bail!(
            "Decoded {} samples but the validated data layout requires {sample_count}",
            samples.len()
        );
    }
    Ok(samples)
}

fn normalize_pcm(value: i64, container_bits: u16, valid_bits: u16) -> f32 {
    let shifted = value >> u32::from(container_bits - valid_bits);
    let denominator = (1_u64 << u32::from(valid_bits - 1)) as f64;
    (shifted as f64 / denominator) as f32
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let data = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| anyhow!("Truncated little-endian u16 at byte {offset}"))?;
    Ok(u16::from_le_bytes([data[0], data[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let data = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow!("Truncated little-endian u32 at byte {offset}"))?;
    Ok(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

fn chunk_name(id: &[u8]) -> String {
    id.iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '�'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn base_format(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let block_align = channels * (bits / 8);
        let byte_rate = rate * u32::from(block_align);
        let mut format = Vec::new();
        format.extend_from_slice(&tag.to_le_bytes());
        format.extend_from_slice(&channels.to_le_bytes());
        format.extend_from_slice(&rate.to_le_bytes());
        format.extend_from_slice(&byte_rate.to_le_bytes());
        format.extend_from_slice(&block_align.to_le_bytes());
        format.extend_from_slice(&bits.to_le_bytes());
        format
    }

    fn extensible_format(
        channels: u16,
        rate: u32,
        bits: u16,
        valid_bits: u16,
        mask: u32,
        subformat: [u8; 16],
    ) -> Vec<u8> {
        let mut format = base_format(EXTENSIBLE_FORMAT_TAG, channels, rate, bits);
        format.extend_from_slice(&22_u16.to_le_bytes());
        format.extend_from_slice(&valid_bits.to_le_bytes());
        format.extend_from_slice(&mask.to_le_bytes());
        format.extend_from_slice(&subformat);
        format
    }

    fn riff(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        for (id, data) in chunks {
            bytes.extend_from_slice(id);
            bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
            bytes.extend_from_slice(data);
            if !data.len().is_multiple_of(2) {
                bytes.push(0);
            }
        }
        let riff_size = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
        bytes
    }

    #[test]
    fn decodes_pcm16_mono_and_skips_valid_unknown_chunks() {
        let format = base_format(PCM_FORMAT_TAG, 1, 44_100, 16);
        let data = [i16::MIN, 0, i16::MAX]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let wav = riff(&[
            (*b"JUNK", vec![1, 2, 3]),
            (*b"fmt ", format),
            (*b"data", data),
        ]);

        let asset = decode_wav(&wav).unwrap();

        assert_eq!(asset.metadata.channels, 1);
        assert_eq!(asset.metadata.frames, 3);
        assert_eq!(asset.metadata.encoding, WavSampleEncoding::PcmInteger);
        assert_eq!(asset.samples[0], -1.0);
        assert_eq!(asset.samples[1], 0.0);
        assert!((asset.samples[2] - 32_767.0 / 32_768.0).abs() < 1.0e-7);
    }

    #[test]
    fn decodes_pcm24_stereo_boundaries() {
        let format = base_format(PCM_FORMAT_TAG, 2, 48_000, 24);
        let data = vec![
            0x00, 0x00, 0x80, 0xff, 0xff, 0x7f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40,
        ];
        let asset = decode_wav(&riff(&[(*b"fmt ", format), (*b"data", data)])).unwrap();

        assert_eq!(asset.metadata.frames, 2);
        assert_eq!(asset.samples[0], -1.0);
        assert!((asset.samples[1] - 8_388_607.0 / 8_388_608.0).abs() < 1.0e-7);
        assert_eq!(asset.samples[2], 0.0);
        assert_eq!(asset.samples[3], 0.5);
    }

    #[test]
    fn decodes_pcm32_multichannel() {
        let format = base_format(PCM_FORMAT_TAG, 3, 96_000, 32);
        let data = [i32::MIN, 0, i32::MAX]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect::<Vec<_>>();
        let asset = decode_wav(&riff(&[(*b"fmt ", format), (*b"data", data)])).unwrap();

        assert_eq!(asset.metadata.channels, 3);
        assert_eq!(asset.metadata.frames, 1);
        assert_eq!(asset.samples[0], -1.0);
        assert_eq!(asset.samples[1], 0.0);
        // 2_147_483_647 / 2_147_483_648 rounds to 1.0 at f32 precision.
        assert_eq!(asset.samples[2], 1.0);
    }

    #[test]
    fn decodes_extensible_pcm_and_valid_bits() {
        let format = extensible_format(6, 48_000, 32, 24, 0x3f, PCM_SUBFORMAT);
        let values = [i32::MIN, -1 << 30, 0, 1 << 30, i32::MAX & !0xff, 0];
        let data = values
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect::<Vec<_>>();
        let asset = decode_wav(&riff(&[(*b"fmt ", format), (*b"data", data)])).unwrap();

        assert_eq!(asset.metadata.channels, 6);
        assert_eq!(asset.metadata.channel_mask, Some(0x3f));
        assert_eq!(asset.metadata.valid_bits_per_sample, 24);
        assert_eq!(asset.samples[0], -1.0);
        assert_eq!(asset.samples[1], -0.5);
        assert_eq!(asset.samples[3], 0.5);
    }

    #[test]
    fn float32_is_clamped_and_non_finite_is_rejected() {
        let format = base_format(IEEE_FLOAT_FORMAT_TAG, 2, 48_000, 32);
        let finite = [-1.5_f32, 0.25, 1.5, -0.25]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let asset = decode_wav(&riff(&[(*b"fmt ", format.clone()), (*b"data", finite)])).unwrap();
        assert_eq!(asset.samples, vec![-1.0, 0.25, 1.0, -0.25]);

        let invalid = [0.0_f32, f32::NAN]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        let error = decode_wav(&riff(&[(*b"fmt ", format), (*b"data", invalid)])).unwrap_err();
        assert!(error.to_string().contains("Non-finite"));
    }

    #[test]
    fn rejects_truncation_trailing_bytes_and_duplicate_chunks() {
        let format = base_format(PCM_FORMAT_TAG, 1, 44_100, 16);
        let valid = riff(&[(*b"fmt ", format.clone()), (*b"data", vec![0, 0])]);

        let mut truncated = valid.clone();
        truncated.pop();
        assert!(
            decode_wav(&truncated)
                .unwrap_err()
                .to_string()
                .contains("Truncated RIFF")
        );

        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(
            decode_wav(&trailing)
                .unwrap_err()
                .to_string()
                .contains("trailing bytes")
        );

        let duplicate = riff(&[
            (*b"fmt ", format.clone()),
            (*b"fmt ", format),
            (*b"data", vec![0, 0]),
        ]);
        assert!(
            decode_wav(&duplicate)
                .unwrap_err()
                .to_string()
                .contains("duplicate fmt")
        );

        let missing_padding = {
            let mut value = riff(&[
                (*b"fmt ", base_format(PCM_FORMAT_TAG, 1, 44_100, 16)),
                (*b"JUNK", vec![1]),
            ]);
            value.pop();
            let size = (value.len() - 8) as u32;
            value[4..8].copy_from_slice(&size.to_le_bytes());
            value
        };
        assert!(
            decode_wav(&missing_padding)
                .unwrap_err()
                .to_string()
                .contains("padding")
        );
    }

    #[test]
    fn rejects_invalid_layout_and_prevents_large_decode() {
        let mut format = base_format(PCM_FORMAT_TAG, 2, 48_000, 16);
        format[12..14].copy_from_slice(&2_u16.to_le_bytes());
        let invalid = riff(&[(*b"fmt ", format), (*b"data", vec![0, 0, 0, 0])]);
        assert!(
            decode_wav(&invalid)
                .unwrap_err()
                .to_string()
                .contains("block alignment")
        );

        let valid = riff(&[
            (*b"fmt ", base_format(PCM_FORMAT_TAG, 1, 44_100, 16)),
            (*b"data", vec![0, 0, 0, 0]),
        ]);
        let limits = WavReadLimits {
            max_decoded_samples: 1,
            ..WavReadLimits::default()
        };
        assert!(
            decode_wav_with_limits(&valid, limits)
                .unwrap_err()
                .to_string()
                .contains("exceeding")
        );
    }

    #[test]
    fn reads_from_path() {
        let wav = riff(&[
            (*b"fmt ", base_format(PCM_FORMAT_TAG, 1, 22_050, 16)),
            (*b"data", vec![0, 0]),
        ]);
        let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "citrus-wav-test-{}-{unique}.wav",
            std::process::id()
        ));
        fs::write(&path, wav).unwrap();

        let result = read_wav(&path);
        let _ = fs::remove_file(&path);

        let asset = result.unwrap();
        assert_eq!(asset.metadata.sample_rate, 22_050);
        assert_eq!(asset.metadata.frames, 1);
    }

    #[test]
    fn linear_resampling_preserves_stereo_endpoints_and_channels() {
        let input = [0.0_f32, 10.0, 1.0, 11.0, 2.0, 12.0];
        let output = resample_interleaved_linear(&input, 2, 2, 4).unwrap();

        assert_eq!(
            output,
            vec![0.0, 10.0, 0.5, 10.5, 1.0, 11.0, 1.5, 11.5, 2.0, 12.0]
        );
    }

    #[test]
    fn linear_resampling_handles_downsample_empty_single_and_invalid_data() {
        let downsampled = resample_interleaved_linear(&[0.0, 1.0, 2.0, 3.0, 4.0], 1, 4, 2).unwrap();
        assert_eq!(downsampled, vec![0.0, 2.0, 4.0]);
        assert!(
            resample_interleaved_linear(&[], 2, 48_000, 44_100)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            resample_interleaved_linear(&[0.25, -0.25], 2, 48_000, 96_000).unwrap(),
            vec![0.25, -0.25]
        );
        assert!(
            resample_interleaved_linear(&[0.0, f32::INFINITY], 1, 48_000, 44_100)
                .unwrap_err()
                .to_string()
                .contains("non-finite")
        );
    }
}
