//! Explicit offline WAV encoding and output-level policy. These preferences are
//! session-only UI state, never project data or audio-device configuration.

use anyhow::{Context, Result, ensure};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WavSampleFormat {
    Pcm16,
    #[default]
    Pcm24,
    Float32,
}

impl WavSampleFormat {
    pub const ALL: [Self; 3] = [Self::Pcm16, Self::Pcm24, Self::Float32];

    pub fn label(self) -> &'static str {
        match self {
            Self::Pcm16 => "16-bit integer PCM",
            Self::Pcm24 => "24-bit integer PCM",
            Self::Float32 => "32-bit IEEE float",
        }
    }

    pub fn bits(self) -> u16 {
        match self {
            Self::Pcm16 => 16,
            Self::Pcm24 => 24,
            Self::Float32 => 32,
        }
    }

    pub fn block_align(self) -> u16 {
        2 * (self.bits() / 8)
    }

    pub fn format_tag(self) -> u16 {
        if self == Self::Float32 { 3 } else { 1 }
    }

    pub fn data_size_for_frames(self, frames: usize) -> Result<u32> {
        let size = frames
            .checked_mul(usize::from(self.block_align()))
            .and_then(|bytes| u32::try_from(bytes).ok())
            .context("Rendered audio is too large for a standard RIFF WAV file")?;
        ensure!(
            size <= u32::MAX - self.riff_overhead(),
            "Rendered audio is too large for a standard RIFF WAV file"
        );
        Ok(size)
    }

    /// RIFF payload bytes before sample data (excludes RIFF id and size).
    /// Float uses WAVEFORMATEX (18 bytes) and the required fact frame count.
    pub fn riff_overhead(self) -> u32 {
        if self == Self::Float32 { 50 } else { 36 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WavLevelPolicy {
    #[default]
    AttenuatePeaks,
    PreserveLevel,
}

impl WavLevelPolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::AttenuatePeaks => "Attenuate peaks above 0.95 (legacy default)",
            Self::PreserveLevel => "Preserve level (unity export gain)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavExportOptions {
    pub sample_rate: u32,
    pub sample_format: WavSampleFormat,
    pub level_policy: WavLevelPolicy,
}

impl Default for WavExportOptions {
    fn default() -> Self {
        Self::legacy(48_000)
    }
}

impl WavExportOptions {
    pub const STANDARD_SAMPLE_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

    pub const fn legacy(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            sample_format: WavSampleFormat::Pcm24,
            level_policy: WavLevelPolicy::AttenuatePeaks,
        }
    }

    pub fn validate(self) -> Result<()> {
        ensure!(
            (8_000..=192_000).contains(&self.sample_rate),
            "WAV export sample rate {} Hz is outside the supported range 8000..=192000 Hz",
            self.sample_rate
        );
        Ok(())
    }

    pub fn gain_for_peak(self, peak: f32) -> Result<f32> {
        ensure!(
            peak.is_finite() && peak >= 0.0,
            "WAV export peak must be finite and non-negative"
        );
        match self.level_policy {
            WavLevelPolicy::AttenuatePeaks => Ok(if peak > 0.95 { 0.95 / peak } else { 1.0 }),
            WavLevelPolicy::PreserveLevel => {
                ensure!(
                    self.sample_format == WavSampleFormat::Float32 || peak <= 1.0,
                    "Preserve level would clip {} (sample peak {peak:.6}). Lower the mix level, choose 32-bit IEEE float, or explicitly select peak attenuation. The destination was not changed.",
                    self.sample_format.label()
                );
                Ok(1.0)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WavExportReport {
    pub frames: usize,
    pub sample_peak: f32,
    pub applied_gain: f32,
}

impl WavExportReport {
    pub fn level_summary(self) -> String {
        if self.applied_gain == 1.0 {
            format!("Unity export gain; sample peak {:.6}", self.sample_peak)
        } else {
            format!(
                "Peak attenuation applied: {:.2} dB; input sample peak {:.6}",
                20.0 * self.applied_gain.log10(),
                self.sample_peak
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_default_only_attenuates_never_boosts() {
        let options = WavExportOptions::default();
        assert_eq!(options, WavExportOptions::legacy(48_000));
        for peak in [0.0, 0.25, 0.95] {
            assert_eq!(options.gain_for_peak(peak).unwrap(), 1.0);
        }
        assert_eq!(options.gain_for_peak(1.9).unwrap(), 0.5);
        for peak in [f32::NAN, f32::INFINITY, -0.1] {
            assert!(options.gain_for_peak(peak).is_err());
        }
    }

    #[test]
    fn preserve_pcm_rejects_overload_and_float_preserves_headroom() {
        for format in WavSampleFormat::ALL {
            let options = WavExportOptions {
                sample_format: format,
                level_policy: WavLevelPolicy::PreserveLevel,
                ..WavExportOptions::default()
            };
            assert_eq!(options.gain_for_peak(1.0).unwrap(), 1.0);
            if format == WavSampleFormat::Float32 {
                assert_eq!(options.gain_for_peak(2.0).unwrap(), 1.0);
            } else {
                assert!(options.gain_for_peak(1.00001).is_err());
            }
        }
    }

    #[test]
    fn riff_size_limit_accounts_for_each_formats_header_without_allocating() {
        for format in WavSampleFormat::ALL {
            let max_frames =
                ((u32::MAX - format.riff_overhead()) / u32::from(format.block_align())) as usize;
            assert_eq!(format.data_size_for_frames(0).unwrap(), 0);
            assert_eq!(
                format.data_size_for_frames(max_frames).unwrap(),
                (max_frames * usize::from(format.block_align())) as u32
            );
            assert!(format.data_size_for_frames(max_frames + 1).is_err());
            assert!(format.data_size_for_frames(usize::MAX).is_err());
        }
    }
}
