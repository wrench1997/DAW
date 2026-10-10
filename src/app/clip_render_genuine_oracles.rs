//! Independent, plugin-free qualification oracles.
//!
//! This module does not import the application decoder, renderer, or plugin host.
//! PCM stays private: JSON receipts contain counts, hashes, and numeric metrics.
//! No routine clamps, normalizes, sanitizes, or changes sample bits.

use serde_json::{Value, json};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const PROCESS_QUANTUM: usize = 128;
pub const FIXED_SONG_FRAMES: usize = 24_000;
pub const FIXED_TAIL_FRAMES: usize = 12_000;
pub const FIXED_LATENCY_FRAMES: usize = 0;
pub const FIXED_GENERATED_FRAMES: usize = 36_096;
pub const FIXED_RETAINED_FRAMES: usize = 36_000;

/// SHA-256 over exactly the supplied bytes, represented as lowercase hex.
/// Uses the standard SHA-256 compression operation; no external hash dependency.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let (chunks, remainder) = bytes.as_chunks::<64>();
    for block in chunks {
        sha256_compress(&mut state, block);
    }
    let mut suffix = [0_u8; 128];
    suffix[..remainder.len()].copy_from_slice(remainder);
    suffix[remainder.len()] = 0x80;
    let suffix_len = if remainder.len() < 56 { 64 } else { 128 };
    let bit_length = (bytes.len() as u64).wrapping_mul(8);
    suffix[suffix_len - 8..suffix_len].copy_from_slice(&bit_length.to_be_bytes());
    for block in suffix[..suffix_len].as_chunks::<64>().0 {
        sha256_compress(&mut state, block);
    }
    let mut result = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for word in state {
        for byte in word.to_be_bytes() {
            result.push(HEX[usize::from(byte >> 4)] as char);
            result.push(HEX[usize::from(byte & 15)] as char);
        }
    }
    result
}

fn sha256_compress(state: &mut [u32; 8], block: &[u8]) {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut words = [0_u32; 64];
    for (index, bytes) in block.as_chunks::<4>().0.iter().enumerate() {
        words[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    }
    for index in 16..64 {
        let a = words[index - 15];
        let b = words[index - 2];
        let small_a = a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3);
        let small_b = b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10);
        words[index] = words[index - 16]
            .wrapping_add(small_a)
            .wrapping_add(words[index - 7])
            .wrapping_add(small_b);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for index in 0..64 {
        let big_e = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choose = (e & f) ^ ((!e) & g);
        let t1 = h
            .wrapping_add(big_e)
            .wrapping_add(choose)
            .wrapping_add(K[index])
            .wrapping_add(words[index]);
        let big_a = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = big_a.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (destination, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *destination = destination.wrapping_add(value);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Q128FrameOracle {
    pub generated_frames: usize,
    pub retained_frames: usize,
    pub retained_start_frame: usize,
    pub process_calls: usize,
}

/// Independently derives Q128 padding and the latency-compensated retained range.
/// Generated frames are ceil(max(L + N + T, N + 1) / 128) * 128.
pub fn q128_frame_oracle(
    song_frames: usize,
    tail_frames: usize,
    latency_frames: usize,
) -> Result<Q128FrameOracle, String> {
    let retained_frames = song_frames
        .checked_add(tail_frames)
        .ok_or_else(|| "song + tail frame count overflow".to_string())?;
    let retained_end = latency_frames
        .checked_add(retained_frames)
        .ok_or_else(|| "latency + retained frame count overflow".to_string())?;
    let song_fence = song_frames
        .checked_add(1)
        .ok_or_else(|| "song + 1 frame count overflow".to_string())?;
    let minimum_generated = retained_end.max(song_fence);
    let remainder = minimum_generated % PROCESS_QUANTUM;
    let generated_frames = if remainder == 0 {
        minimum_generated
    } else {
        minimum_generated
            .checked_add(PROCESS_QUANTUM - remainder)
            .ok_or_else(|| "Q128 rounded frame count overflow".to_string())?
    };
    generated_frames
        .checked_mul(CHANNELS)
        .ok_or_else(|| "generated interleaved sample count overflow".to_string())?;
    Ok(Q128FrameOracle {
        generated_frames,
        retained_frames,
        retained_start_frame: latency_frames,
        process_calls: generated_frames / PROCESS_QUANTUM,
    })
}

/// Checks the complete generated stereo shape, then copies exactly [L,L+N+T).
/// It preserves IEEE bits, including signed zero; validation is a separate step.
pub fn trim_q128_stereo(
    generated: &[f32],
    song_frames: usize,
    tail_frames: usize,
    latency_frames: usize,
) -> Result<Vec<f32>, String> {
    if !generated.len().is_multiple_of(CHANNELS) {
        return Err(format!(
            "odd interleaved stereo sample count: {}",
            generated.len()
        ));
    }
    let oracle = q128_frame_oracle(song_frames, tail_frames, latency_frames)?;
    let expected_samples = oracle
        .generated_frames
        .checked_mul(CHANNELS)
        .ok_or_else(|| "generated interleaved sample count overflow".to_string())?;
    if generated.len() != expected_samples {
        return Err(format!(
            "generated shape mismatch: {} samples / {} frames, expected {} samples / {} frames",
            generated.len(),
            generated.len() / CHANNELS,
            expected_samples,
            oracle.generated_frames,
        ));
    }
    let start = oracle
        .retained_start_frame
        .checked_mul(CHANNELS)
        .ok_or_else(|| "retained start sample overflow".to_string())?;
    let retained_samples = oracle
        .retained_frames
        .checked_mul(CHANNELS)
        .ok_or_else(|| "retained sample count overflow".to_string())?;
    let end = start
        .checked_add(retained_samples)
        .ok_or_else(|| "retained end sample overflow".to_string())?;
    let retained = generated
        .get(start..end)
        .ok_or_else(|| "retained interval lies outside generated audio".to_string())?;
    Ok(retained.to_vec())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFloat32Wav {
    pub frames: usize,
    /// Interleaved little-endian IEEE-754 payload interpreted only as raw bits.
    /// The parser has separately rejected nonfinite and out-of-range values.
    pub sample_bits: Vec<u32>,
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| "u16 offset overflow".to_string())?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| format!("truncated u16 at byte {offset}"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| "u32 offset overflow".to_string())?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| format!("truncated u32 at byte {offset}"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn check_format(bytes: &[u8]) -> Result<(), String> {
    // The fixed IEEE-Float32 writer may emit the 16-byte base or an empty
    // WAVEFORMATEX extension. Extensible/PCM encodings are never aliases here.
    if bytes.len() != 16 && bytes.len() != 18 {
        return Err(format!(
            "unsupported fmt chunk size {}; expected 16 or 18",
            bytes.len()
        ));
    }
    if bytes.len() == 18 && read_u16(bytes, 16)? != 0 {
        return Err("nonempty IEEE-Float32 fmt extension is unsupported".to_string());
    }
    let observed = (
        read_u16(bytes, 0)?,
        read_u16(bytes, 2)?,
        read_u32(bytes, 4)?,
        read_u32(bytes, 8)?,
        read_u16(bytes, 12)?,
        read_u16(bytes, 14)?,
    );
    if observed != (3, 2, SAMPLE_RATE, SAMPLE_RATE * 8, 8, 32) {
        return Err(format!(
            "WAV format mismatch: (tag, channels, rate, byte_rate, block_align, bits)={observed:?}; expected (3, 2, 48000, 384000, 8, 32)",
        ));
    }
    Ok(())
}

fn check_sample(value: f32, index: usize, context: &str) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!(
            "{context} nonfinite sample {index} (frame {}, channel {}, bits {:08x})",
            index / CHANNELS,
            index % CHANNELS,
            value.to_bits(),
        ));
    }
    if value.abs() > 1.0 {
        return Err(format!(
            "{context} out-of-range sample {index} (frame {}, channel {}, value {value}, bits {:08x}); no clamping permitted",
            index / CHANNELS,
            index % CHANNELS,
            value.to_bits(),
        ));
    }
    Ok(())
}

/// Strict, bounded little-endian RIFF parser, independent of the app decoder.
/// Exactly one fmt and data chunk are required. An optional fact chunk must
/// agree with the frame count. Unknown chunks are skipped only within verified
/// bounds, including their mandatory odd-size pad byte. No trailing data is
/// accepted outside RIFF. Odd numbers of complete frames are legal WAV here;
/// the fixed 36,000-frame qualification is enforced by validate_float32_wav.
pub fn parse_float32_stereo_48k_wav(bytes: &[u8]) -> Result<RawFloat32Wav, String> {
    if bytes.len() < 12 {
        return Err("truncated RIFF/WAVE header".to_string());
    }
    if &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("expected little-endian RIFF/WAVE container".to_string());
    }
    let riff_size = usize::try_from(read_u32(bytes, 4)?)
        .map_err(|_| "RIFF size cannot fit this platform".to_string())?;
    let riff_end = riff_size
        .checked_add(8)
        .ok_or_else(|| "RIFF size overflow".to_string())?;
    if riff_end != bytes.len() || riff_end < 12 {
        return Err(format!(
            "RIFF size mismatch: declares {riff_end} total bytes, actual {}",
            bytes.len(),
        ));
    }
    let mut cursor = 12_usize;
    let mut have_format = false;
    let mut payload: Option<&[u8]> = None;
    let mut fact_frames: Option<u32> = None;
    while cursor < riff_end {
        if riff_end - cursor < 8 {
            return Err(format!("truncated RIFF chunk header at byte {cursor}"));
        }
        let id = &bytes[cursor..cursor + 4];
        let length = usize::try_from(read_u32(bytes, cursor + 4)?)
            .map_err(|_| "chunk length cannot fit this platform".to_string())?;
        let start = cursor
            .checked_add(8)
            .ok_or_else(|| "chunk start overflow".to_string())?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| "chunk end overflow".to_string())?;
        if end > riff_end {
            return Err(format!(
                "truncated RIFF chunk at byte {cursor}: length {length}"
            ));
        }
        let chunk = &bytes[start..end];
        match id {
            b"fmt " => {
                if have_format {
                    return Err("duplicate fmt chunk".to_string());
                }
                check_format(chunk)?;
                have_format = true;
            }
            b"data" => {
                if !have_format {
                    return Err("data chunk precedes fmt chunk".to_string());
                }
                if payload.is_some() {
                    return Err("duplicate data chunk".to_string());
                }
                if !chunk.len().is_multiple_of(8) {
                    return Err(format!(
                        "data chunk contains {} bytes, not complete Float32 stereo frames",
                        chunk.len(),
                    ));
                }
                payload = Some(chunk);
            }
            b"fact" => {
                if fact_frames.is_some() {
                    return Err("duplicate fact chunk".to_string());
                }
                if chunk.len() != 4 {
                    return Err(format!(
                        "invalid fact chunk size {}; expected 4",
                        chunk.len()
                    ));
                }
                fact_frames = Some(read_u32(chunk, 0)?);
            }
            _ => {}
        }
        cursor = end
            .checked_add(length % 2)
            .ok_or_else(|| "chunk padding offset overflow".to_string())?;
        if cursor > riff_end {
            return Err("missing odd-sized RIFF chunk padding byte".to_string());
        }
    }
    if !have_format {
        return Err("missing fmt chunk".to_string());
    }
    let payload = payload.ok_or_else(|| "missing data chunk".to_string())?;
    let frames = payload.len() / 8;
    if let Some(declared) = fact_frames
        && u64::from(declared) != frames as u64
    {
        return Err(format!(
            "fact frame mismatch: {declared} declared, {frames} in data"
        ));
    }
    let mut sample_bits = Vec::with_capacity(payload.len() / 4);
    for (index, raw) in payload.as_chunks::<4>().0.iter().enumerate() {
        let bits = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        check_sample(f32::from_bits(bits), index, "WAV")?;
        sample_bits.push(bits);
    }
    Ok(RawFloat32Wav {
        frames,
        sample_bits,
    })
}

/// Requires the fixed 36,000 stereo frames, then compares every IEEE bit.
/// This detects signed-zero differences and single-bit changes; approximate
/// numeric equality and application decoding are intentionally never used.
pub fn validate_float32_wav(wav_bytes: &[u8], expected_retained: &[f32]) -> Result<Value, String> {
    let expected_samples = FIXED_RETAINED_FRAMES * CHANNELS;
    if expected_retained.len() != expected_samples {
        return Err(format!(
            "retained oracle sample count {}, expected {expected_samples}",
            expected_retained.len(),
        ));
    }
    let parsed = parse_float32_stereo_48k_wav(wav_bytes)?;
    if parsed.frames != FIXED_RETAINED_FRAMES || parsed.sample_bits.len() != expected_samples {
        return Err(format!(
            "WAV shape mismatch: {} frames / {} samples, expected {} frames / {expected_samples} samples",
            parsed.frames,
            parsed.sample_bits.len(),
            FIXED_RETAINED_FRAMES,
        ));
    }
    let mut pcm_bytes = Vec::with_capacity(expected_samples * 4);
    for (index, (&actual, &expected)) in
        parsed.sample_bits.iter().zip(expected_retained).enumerate()
    {
        check_sample(expected, index, "retained oracle")?;
        let expected_bits = expected.to_bits();
        if actual != expected_bits {
            return Err(format!(
                "WAV IEEE bits differ at sample {index} (frame {}, channel {}): actual {actual:08x}, expected {expected_bits:08x}",
                index / CHANNELS,
                index % CHANNELS,
            ));
        }
        pcm_bytes.extend_from_slice(&actual.to_le_bytes());
    }
    Ok(json!({
        "format_tag": 3,
        "channels": CHANNELS,
        "sample_rate": SAMPLE_RATE,
        "bits_per_sample": 32,
        "frames": parsed.frames,
        "samples": parsed.sample_bits.len(),
        "exact_ieee_bits_equal": true,
        "wav_bytes": wav_bytes.len(),
        "wav_sha256": sha256_hex(wav_bytes),
        "interleaved_pcm_le_sha256": sha256_hex(&pcm_bytes),
    }))
}

#[derive(Clone, Copy, Debug)]
struct WindowMetric {
    start: usize,
    end: usize,
    peak: f64,
    rms: f64,
}

impl WindowMetric {
    fn json(self) -> Value {
        json!({
            "start_frame": self.start,
            "end_frame_exclusive": self.end,
            "sample_count": (self.end - self.start) * CHANNELS,
            "absolute_peak": self.peak,
            "rms": self.rms,
        })
    }
}

// Called only after the complete generated shape and every sample are checked.
fn window_metric(samples: &[f32], start: usize, end: usize) -> WindowMetric {
    let interval = &samples[start * CHANNELS..end * CHANNELS];
    let mut peak = 0.0_f64;
    let mut sum_squares = 0.0_f64;
    for &value in interval {
        let value = f64::from(value);
        peak = peak.max(value.abs());
        sum_squares += value * value;
    }
    WindowMetric {
        start,
        end,
        peak,
        rms: (sum_squares / interval.len() as f64).sqrt(),
    }
}

/// Validates every generated sample, including all 96 discarded padding frames.
/// Reports each fixed held-note window separately and adds non-gating release,
/// fixed-tail, and padding metrics. No release-envelope assumption is imposed.
pub fn validate_generated_audio(generated: &[f32]) -> Result<Value, String> {
    let oracle = q128_frame_oracle(FIXED_SONG_FRAMES, FIXED_TAIL_FRAMES, FIXED_LATENCY_FRAMES)?;
    if oracle.generated_frames != FIXED_GENERATED_FRAMES
        || oracle.retained_frames != FIXED_RETAINED_FRAMES
        || oracle.process_calls != 282
    {
        return Err("internal fixed Q128 oracle mismatch".to_string());
    }
    if generated.len() != oracle.generated_frames * CHANNELS {
        return Err(format!(
            "generated audio sample count {}, expected {} (36096 complete stereo frames)",
            generated.len(),
            oracle.generated_frames * CHANNELS,
        ));
    }
    let mut first_above_threshold = None;
    for (index, &value) in generated.iter().enumerate() {
        check_sample(value, index, "generated audio")?;
        if first_above_threshold.is_none() && f64::from(value).abs() > 1e-8 {
            first_above_threshold = Some(index / CHANNELS);
        }
    }
    let global = window_metric(generated, 0, FIXED_GENERATED_FRAMES);
    let windows = [
        window_metric(generated, 4_800, 7_200),
        window_metric(generated, 7_200, 9_600),
        window_metric(generated, 9_600, 12_000),
    ];
    let mut failures = Vec::new();
    if global.peak <= 1e-6 || global.peak > 1.0 {
        failures.push(format!(
            "global absolute peak {} must be >1e-6 and <=1",
            global.peak
        ));
    }
    match first_above_threshold {
        Some(frame) if frame < 4_800 => {}
        other => failures.push(format!(
            "first frame with abs>1e-8 is {other:?}; expected Some(frame) with frame <4800",
        )),
    }
    for window in &windows {
        if window.peak <= 1e-6 || window.rms <= 1e-8 {
            failures.push(format!(
                "held-note window [{},{}) peak {} / RMS {} must be >1e-6 / >1e-8",
                window.start, window.end, window.peak, window.rms,
            ));
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(json!({
        "generated_frames": oracle.generated_frames,
        "generated_samples": generated.len(),
        "retained_frames": oracle.retained_frames,
        "retained_samples": oracle.retained_frames * CHANNELS,
        "process_quantum": PROCESS_QUANTUM,
        "process_calls": oracle.process_calls,
        "discarded_padding_frames": oracle.generated_frames - oracle.retained_frames,
        "all_generated_finite": true,
        "all_generated_within_unit_range": true,
        "first_frame_above_1e_minus_8": first_above_threshold,
        "global": global.json(),
        "held_note_windows": windows.iter().map(|metric| metric.json()).collect::<Vec<_>>(),
        "release_after_note_off": window_metric(generated, 12_000, FIXED_RETAINED_FRAMES).json(),
        "fixed_tail": window_metric(generated, FIXED_SONG_FRAMES, FIXED_RETAINED_FRAMES).json(),
        "discarded_padding": window_metric(generated, FIXED_RETAINED_FRAMES, FIXED_GENERATED_FRAMES).json(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format_chunk() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&384_000_u32.to_le_bytes());
        bytes.extend_from_slice(&8_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes
    }

    fn riff(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        for (id, chunk) in chunks {
            bytes.extend_from_slice(id);
            bytes.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
            bytes.extend_from_slice(chunk);
            if chunk.len() % 2 != 0 {
                bytes.push(0);
            }
        }
        let riff_len = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff_len.to_le_bytes());
        bytes
    }

    fn wav(samples: &[f32]) -> Vec<u8> {
        let data = samples
            .iter()
            .flat_map(|value| value.to_bits().to_le_bytes())
            .collect();
        riff(&[(*b"fmt ", format_chunk()), (*b"data", data)])
    }

    fn valid_audio() -> Vec<f32> {
        vec![0.125; FIXED_GENERATED_FRAMES * CHANNELS]
    }

    #[test]
    fn sha256_known_empty_and_short_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn sha256_known_multiblock_and_padding_vectors() {
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(sha256_hex(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"), "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1");
        assert_eq!(
            sha256_hex(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn fixed_q128_counts_and_exact_trim() {
        let oracle = q128_frame_oracle(24_000, 12_000, 0).unwrap();
        assert_eq!(
            oracle,
            Q128FrameOracle {
                generated_frames: 36_096,
                retained_frames: 36_000,
                retained_start_frame: 0,
                process_calls: 282,
            }
        );
        let mut samples = valid_audio();
        samples[0] = -0.0;
        samples[71_999] = -0.25;
        samples[72_000] = 0.75;
        let retained = trim_q128_stereo(&samples, 24_000, 12_000, 0).unwrap();
        assert_eq!(retained.len(), 72_000);
        assert_eq!(retained[0].to_bits(), (-0.0_f32).to_bits());
        assert_eq!(retained[71_999].to_bits(), (-0.25_f32).to_bits());
    }

    #[test]
    fn q128_rounding_note_off_fence_and_latency_boundaries() {
        assert_eq!(q128_frame_oracle(127, 0, 0).unwrap().generated_frames, 128);
        assert_eq!(q128_frame_oracle(128, 0, 0).unwrap().generated_frames, 256);
        assert_eq!(q128_frame_oracle(127, 1, 0).unwrap().generated_frames, 128);
        assert_eq!(q128_frame_oracle(127, 2, 0).unwrap().generated_frames, 256);
        let oracle = q128_frame_oracle(2, 1, 3).unwrap();
        assert_eq!(oracle.retained_start_frame, 3);
        let samples: Vec<f32> = (0..256).map(|value| value as f32).collect();
        assert_eq!(trim_q128_stereo(&samples, 2, 1, 3).unwrap(), samples[6..12]);
        assert_eq!(q128_frame_oracle(0, 0, 0).unwrap().generated_frames, 128);
    }

    #[test]
    fn q128_overflow_and_incomplete_shape_rejected() {
        assert!(q128_frame_oracle(usize::MAX, 0, 0).is_err());
        assert!(q128_frame_oracle(1, usize::MAX, 0).is_err());
        assert!(q128_frame_oracle(1, 1, usize::MAX).is_err());
        assert!(q128_frame_oracle(usize::MAX - 1, 0, 0).is_err());
        assert!(
            trim_q128_stereo(&[0.0; 3], 1, 1, 0)
                .unwrap_err()
                .contains("odd")
        );
        assert!(
            trim_q128_stereo(&[0.0; 254], 1, 1, 0)
                .unwrap_err()
                .contains("shape")
        );
        assert!(
            trim_q128_stereo(&[0.0; 258], 1, 1, 0)
                .unwrap_err()
                .contains("shape")
        );
    }

    #[test]
    fn raw_wav_preserves_signed_zero_and_small_finite_bits() {
        let samples = [0.0, -0.0, f32::from_bits(1), -f32::from_bits(1), 1.0, -1.0];
        let parsed = parse_float32_stereo_48k_wav(&wav(&samples)).unwrap();
        assert_eq!(parsed.frames, 3); // Odd complete frame count is structurally valid.
        assert_eq!(
            parsed.sample_bits,
            samples
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn all_truncated_prefixes_and_external_trailing_bytes_rejected() {
        let bytes = wav(&[0.0, 0.0]);
        for length in 0..bytes.len() {
            assert!(
                parse_float32_stereo_48k_wav(&bytes[..length]).is_err(),
                "prefix {length}"
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(parse_float32_stereo_48k_wav(&trailing).is_err());
        let mut oversized_chunk = bytes;
        oversized_chunk[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_float32_stereo_48k_wav(&oversized_chunk).is_err());
    }

    #[test]
    fn malformed_headers_and_chunk_bounds_rejected() {
        for (offset, replacement) in [(0, b'R' + 1), (8, b'W' + 1)] {
            let mut bytes = wav(&[0.0, 0.0]);
            bytes[offset] = replacement;
            assert!(parse_float32_stereo_48k_wav(&bytes).is_err());
        }
        let mut bytes = b"RIFF\0\0\0\0WAVEx".to_vec();
        bytes[4..8].copy_from_slice(&5_u32.to_le_bytes());
        assert!(
            parse_float32_stereo_48k_wav(&bytes)
                .unwrap_err()
                .contains("chunk header")
        );
        bytes[4..8].copy_from_slice(&0_u32.to_le_bytes());
        assert!(parse_float32_stereo_48k_wav(&bytes).is_err());
    }

    #[test]
    fn format_identity_and_arithmetic_fields_are_exact() {
        for offset in [0_usize, 2, 4, 8, 12, 14] {
            let mut format = format_chunk();
            format[offset] ^= 1;
            let bytes = riff(&[(*b"fmt ", format), (*b"data", vec![0; 8])]);
            assert!(
                parse_float32_stereo_48k_wav(&bytes)
                    .unwrap_err()
                    .contains("format mismatch")
            );
        }
        for length in [0, 15, 17, 19, 40] {
            let bytes = riff(&[(*b"fmt ", vec![0; length]), (*b"data", vec![0; 8])]);
            assert!(parse_float32_stereo_48k_wav(&bytes).is_err());
        }
        let mut extended = format_chunk();
        extended.extend_from_slice(&0_u16.to_le_bytes());
        assert!(
            parse_float32_stereo_48k_wav(&riff(&[
                (*b"fmt ", extended.clone()),
                (*b"data", vec![0; 8])
            ]))
            .is_ok()
        );
        extended[16] = 1;
        assert!(
            parse_float32_stereo_48k_wav(&riff(&[(*b"fmt ", extended), (*b"data", vec![0; 8])]))
                .is_err()
        );
    }

    #[test]
    fn duplicate_missing_and_out_of_order_chunks_rejected() {
        let fmt = (*b"fmt ", format_chunk());
        let data = (*b"data", vec![0; 8]);
        assert!(parse_float32_stereo_48k_wav(&riff(&[])).is_err());
        assert!(parse_float32_stereo_48k_wav(&riff(std::slice::from_ref(&fmt))).is_err());
        assert!(parse_float32_stereo_48k_wav(&riff(&[data.clone(), fmt.clone()])).is_err());
        assert!(
            parse_float32_stereo_48k_wav(&riff(&[fmt.clone(), fmt.clone(), data.clone()])).is_err()
        );
        assert!(parse_float32_stereo_48k_wav(&riff(&[fmt, data.clone(), data])).is_err());
    }

    #[test]
    fn bounded_unknown_odd_chunk_requires_padding() {
        let bytes = riff(&[
            (*b"JUNK", vec![7]),
            (*b"fmt ", format_chunk()),
            (*b"data", vec![0; 8]),
        ]);
        assert!(parse_float32_stereo_48k_wav(&bytes).is_ok());
        let mut missing_pad = riff(&[
            (*b"fmt ", format_chunk()),
            (*b"data", vec![0; 8]),
            (*b"JUNK", vec![7]),
        ]);
        missing_pad.pop();
        let len = (missing_pad.len() - 8) as u32;
        missing_pad[4..8].copy_from_slice(&len.to_le_bytes());
        assert!(
            parse_float32_stereo_48k_wav(&missing_pad)
                .unwrap_err()
                .contains("padding")
        );
    }

    #[test]
    fn odd_sample_count_and_incomplete_float_payload_rejected() {
        for length in [1, 3, 4, 7, 9, 12] {
            let bytes = riff(&[(*b"fmt ", format_chunk()), (*b"data", vec![0; length])]);
            assert!(
                parse_float32_stereo_48k_wav(&bytes)
                    .unwrap_err()
                    .contains("complete Float32 stereo frames")
            );
        }
    }

    #[test]
    fn fact_chunk_checked_when_present() {
        let fmt = (*b"fmt ", format_chunk());
        let data = (*b"data", vec![0; 8]);
        let fact = (*b"fact", 1_u32.to_le_bytes().to_vec());
        assert!(
            parse_float32_stereo_48k_wav(&riff(&[fmt.clone(), fact.clone(), data.clone()])).is_ok()
        );
        assert!(
            parse_float32_stereo_48k_wav(&riff(&[fmt.clone(), fact.clone(), fact, data.clone()]))
                .is_err()
        );
        assert!(
            parse_float32_stereo_48k_wav(&riff(&[
                fmt.clone(),
                (*b"fact", vec![0; 3]),
                data.clone()
            ]))
            .is_err()
        );
        assert!(
            parse_float32_stereo_48k_wav(&riff(&[
                fmt,
                (*b"fact", 2_u32.to_le_bytes().to_vec()),
                data
            ]))
            .unwrap_err()
            .contains("fact frame mismatch")
        );
    }

    #[test]
    fn wav_nonfinite_and_out_of_range_rejected_without_clamping() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                parse_float32_stereo_48k_wav(&wav(&[value, 0.0]))
                    .unwrap_err()
                    .contains("nonfinite")
            );
        }
        for value in [f32::from_bits(1.0_f32.to_bits() + 1), -1.001, f32::MAX] {
            assert!(
                parse_float32_stereo_48k_wav(&wav(&[value, 0.0]))
                    .unwrap_err()
                    .contains("out-of-range")
            );
        }
    }

    #[test]
    fn exact_wav_comparison_and_shape() {
        let samples = vec![0.125; FIXED_RETAINED_FRAMES * CHANNELS];
        let receipt = validate_float32_wav(&wav(&samples), &samples).unwrap();
        assert_eq!(receipt["exact_ieee_bits_equal"], true);
        assert_eq!(receipt["frames"], 36_000);
        assert_eq!(receipt["samples"], 72_000);
        assert!(
            validate_float32_wav(&wav(&samples[..samples.len() - 2]), &samples)
                .unwrap_err()
                .contains("shape mismatch")
        );
        assert!(validate_float32_wav(&wav(&samples), &samples[..samples.len() - 1]).is_err());
        assert!(validate_float32_wav(&wav(&samples), &vec![f32::NAN; samples.len()]).is_err());
    }

    #[test]
    fn exact_bits_detect_signed_zero_and_one_bit_change() {
        let mut expected = vec![0.0; FIXED_RETAINED_FRAMES * CHANNELS];
        let mut actual = expected.clone();
        actual[71_999] = -0.0;
        assert!(
            validate_float32_wav(&wav(&actual), &expected)
                .unwrap_err()
                .contains("sample 71999")
        );
        expected[71_999] = 0.125;
        actual[71_999] = f32::from_bits(0.125_f32.to_bits() + 1);
        assert!(
            validate_float32_wav(&wav(&actual), &expected)
                .unwrap_err()
                .contains("IEEE bits differ")
        );
    }

    #[test]
    fn metrics_report_all_fixed_windows_and_padding() {
        let metrics = validate_generated_audio(&valid_audio()).unwrap();
        assert_eq!(metrics["generated_frames"], 36_096);
        assert_eq!(metrics["process_calls"], 282);
        assert_eq!(metrics["discarded_padding_frames"], 96);
        assert_eq!(metrics["first_frame_above_1e_minus_8"], 0);
        assert_eq!(metrics["held_note_windows"].as_array().unwrap().len(), 3);
        assert_eq!(metrics["held_note_windows"][0]["start_frame"], 4_800);
        assert_eq!(
            metrics["held_note_windows"][2]["end_frame_exclusive"],
            12_000
        );
        assert_eq!(metrics["fixed_tail"]["start_frame"], 24_000);
        assert_eq!(metrics["fixed_tail"]["end_frame_exclusive"], 36_000);
        assert_eq!(metrics["discarded_padding"]["sample_count"], 192);
        assert_eq!(metrics["global"]["absolute_peak"], 0.125);
        assert_eq!(metrics["global"]["rms"], 0.125);
    }

    #[test]
    fn finite_and_amplitude_gate_includes_discarded_padding() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0001, -1.0001] {
            let mut samples = valid_audio();
            samples[FIXED_GENERATED_FRAMES * CHANNELS - 1] = value;
            assert!(validate_generated_audio(&samples).is_err());
        }
        let mut samples = valid_audio();
        samples[0] = 1.0;
        samples[1] = -1.0;
        assert!(validate_generated_audio(&samples).is_ok());
        assert!(validate_generated_audio(&samples[..samples.len() - 1]).is_err());
        assert!(validate_generated_audio(&samples[..samples.len() - 2]).is_err());
    }

    #[test]
    fn first_note_frame_deadline_is_strict() {
        let mut samples = valid_audio();
        samples[..4_800 * CHANNELS].fill(0.0);
        assert!(
            validate_generated_audio(&samples)
                .unwrap_err()
                .contains("Some(4800)")
        );
        samples[4_799 * CHANNELS + 1] = 0.125;
        assert!(validate_generated_audio(&samples).is_ok());
        samples[4_799 * CHANNELS + 1] = 1e-8_f32; // Rounded below the f64 threshold.
        assert!(validate_generated_audio(&samples).is_err());
        samples[4_799 * CHANNELS + 1] = f32::from_bits(1e-8_f32.to_bits() + 1);
        assert!(validate_generated_audio(&samples).is_ok());
    }

    #[test]
    fn each_held_window_is_required_with_half_open_bounds() {
        for (start, end) in [(4_800, 7_200), (7_200, 9_600), (9_600, 12_000)] {
            let mut samples = valid_audio();
            samples[start * CHANNELS..end * CHANNELS].fill(0.0);
            // Adjacent samples stay nonzero and cannot satisfy this window.
            assert!(
                validate_generated_audio(&samples)
                    .unwrap_err()
                    .contains(&format!("[{start},{end})"))
            );
            samples[(end - 1) * CHANNELS + 1] = 0.125;
            assert!(validate_generated_audio(&samples).is_ok());
        }
    }

    #[test]
    fn silence_and_release_only_cannot_qualify_but_silent_tail_can() {
        let mut samples = vec![0.0; FIXED_GENERATED_FRAMES * CHANNELS];
        let error = validate_generated_audio(&samples).unwrap_err();
        assert!(error.contains("global absolute peak"));
        assert!(error.contains("None"));
        samples[12_000 * CHANNELS..].fill(0.125);
        assert!(
            validate_generated_audio(&samples)
                .unwrap_err()
                .contains("held-note window")
        );
        samples = valid_audio();
        samples[12_000 * CHANNELS..].fill(0.0);
        assert!(validate_generated_audio(&samples).is_ok());
    }

    #[test]
    fn held_peak_threshold_and_both_channel_rms_are_checked() {
        let mut samples = valid_audio();
        samples[4_800 * CHANNELS..7_200 * CHANNELS].fill(1e-6_f32);
        assert!(
            validate_generated_audio(&samples)
                .unwrap_err()
                .contains("[4800,7200)")
        );
        let above = f32::from_bits(1e-6_f32.to_bits() + 1);
        samples[4_800 * CHANNELS..7_200 * CHANNELS].fill(above);
        assert!(validate_generated_audio(&samples).is_ok());
        samples[4_800 * CHANNELS..7_200 * CHANNELS].fill(0.0);
        samples[4_800 * CHANNELS + 1] = 0.125;
        let metric = window_metric(&samples, 4_800, 7_200);
        assert_eq!(metric.peak, 0.125);
        assert!((metric.rms - 0.125 / 4_800_f64.sqrt()).abs() < 1e-15);
        assert!(validate_generated_audio(&samples).is_ok());
    }
}
