//! Allocation-free native-rate sample playback shared by realtime and export.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplerConfig {
    pub asset_id: Option<u64>,
    pub root_note: u8,
    pub start_frame: u64,
    pub end_frame: Option<u64>,
    pub loop_start_frame: Option<u64>,
    pub loop_end_frame: Option<u64>,
    pub attack_seconds: f32,
    pub decay_seconds: f32,
    pub sustain: f32,
    pub release_seconds: f32,
}
impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            asset_id: None,
            root_note: 60,
            start_frame: 0,
            end_frame: None,
            loop_start_frame: None,
            loop_end_frame: None,
            attack_seconds: 0.005,
            decay_seconds: 0.05,
            sustain: 0.8,
            release_seconds: 0.1,
        }
    }
}
impl SamplerConfig {
    pub fn new(asset_id: u64) -> Self {
        Self {
            asset_id: Some(asset_id),
            ..Self::default()
        }
    }
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_valid(&self) -> bool {
        self.root_note <= 127
            && [
                self.attack_seconds,
                self.decay_seconds,
                self.release_seconds,
            ]
            .iter()
            .all(|x| x.is_finite() && (0.0..=60.0).contains(x))
            && self.sustain.is_finite()
            && (0.0..=1.0).contains(&self.sustain)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SamplerVoice {
    start: f64,
    position: f64,
    increment: f64,
    end: usize,
    loop_region: Option<(usize, usize)>,
    age: u64,
    attack: f64,
    decay: f64,
    sustain: f32,
    release_frames: f64,
    release_age: Option<u64>,
    release_level: f32,
    velocity: f32,
    active: bool,
}
impl SamplerVoice {
    pub fn new(
        config: SamplerConfig,
        note: u8,
        velocity: f32,
        source_rate: u32,
        output_rate: u32,
        frames: usize,
    ) -> Option<Self> {
        if config.asset_id.is_none()
            || !config.is_valid()
            || note > 127
            || !velocity.is_finite()
            || source_rate == 0
            || output_rate == 0
        {
            return None;
        }
        let start = usize::try_from(config.start_frame).ok()?;
        let end = usize::try_from(config.end_frame.unwrap_or(frames as u64)).ok()?;
        if start >= end || end > frames {
            return None;
        }
        let loop_region = match (config.loop_start_frame, config.loop_end_frame) {
            (None, None) => None,
            (Some(a), Some(b)) if a >= start as u64 && a < b && b <= end as u64 => {
                Some((a as usize, b as usize))
            }
            _ => return None,
        };
        Some(Self {
            start: start as f64,
            position: start as f64,
            increment: f64::from(source_rate) / f64::from(output_rate)
                * 2.0_f64.powf((f64::from(note) - f64::from(config.root_note)) / 12.0),
            end,
            loop_region,
            age: 0,
            attack: (f64::from(config.attack_seconds) * f64::from(output_rate)).round(),
            decay: (f64::from(config.decay_seconds) * f64::from(output_rate)).round(),
            sustain: config.sustain,
            release_frames: (f64::from(config.release_seconds) * f64::from(output_rate)).round(),
            release_age: None,
            release_level: 0.0,
            velocity: velocity.clamp(0.0, 1.0),
            active: true,
        })
    }
    fn level(&self) -> f32 {
        if let Some(age) = self.release_age {
            return if self.release_frames <= 0.0 {
                0.0
            } else {
                self.release_level * (1.0 - age as f64 / self.release_frames).max(0.0) as f32
            };
        }
        let age = self.age as f64;
        if age < self.attack {
            (age / self.attack) as f32
        } else if age < self.attack + self.decay {
            1.0 - (1.0 - self.sustain) * ((age - self.attack) / self.decay) as f32
        } else {
            self.sustain
        }
    }
    pub fn release(&mut self) {
        if self.release_age.is_none() {
            self.release_level = self.level();
            self.release_age = Some(0);
            if self.release_frames == 0.0 {
                self.active = false;
            }
        }
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
    pub fn is_releasing(&self) -> bool {
        self.release_age.is_some()
    }
    /// Chase a held note without iteration or allocation. Release is scheduled by its note-off.
    pub fn chase(&mut self, elapsed_output_frames: u64) {
        self.age = elapsed_output_frames;
        self.active = true;
        self.release_age = None;
        self.release_level = 0.0;
        self.position = self.start + self.increment * elapsed_output_frames as f64;
        self.wrap_position();
    }
    fn wrap_position(&mut self) {
        if let Some((a, b)) = self.loop_region
            && self.position >= b as f64
        {
            self.position = a as f64 + (self.position - a as f64).rem_euclid((b - a) as f64);
        }
        if self.position >= self.end as f64 {
            self.active = false;
        }
    }
    pub fn next_frame(&mut self, samples: &[f32], channels: usize) -> [f32; 2] {
        if !self.active || !(1..=2).contains(&channels) || samples.len() / channels < self.end {
            self.active = false;
            return [0.0; 2];
        }
        self.wrap_position();
        if !self.active {
            return [0.0; 2];
        }
        let index = self.position as usize;
        let next = if let Some((a, b)) = self.loop_region {
            if index + 1 >= b { a } else { index + 1 }
        } else {
            (index + 1).min(self.end - 1)
        };
        let fraction = (self.position - index as f64) as f32;
        let gain = self.level() * self.velocity;
        let mut output = [0.0; 2];
        for (channel, value) in output.iter_mut().enumerate() {
            let c = channel.min(channels - 1);
            let a = samples[index * channels + c];
            let a = if a.is_finite() {
                a.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            let b = samples[next * channels + c];
            let b = if b.is_finite() {
                b.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            *value = (a + (b - a) * fraction) * gain;
        }
        self.position += self.increment;
        self.age = self.age.saturating_add(1);
        if let Some(age) = &mut self.release_age {
            *age = age.saturating_add(1);
            if *age as f64 >= self.release_frames {
                self.active = false;
            }
        }
        self.wrap_position();
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> SamplerConfig {
        SamplerConfig {
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            ..SamplerConfig::new(7)
        }
    }
    #[test]
    fn pcm_pitch_and_rate() {
        let pcm = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
        let mut v = SamplerVoice::new(config(), 72, 1.0, 24000, 48000, 6).unwrap();
        assert_eq!(v.next_frame(&pcm, 1), [0.0; 2]);
        assert_eq!(v.next_frame(&pcm, 1), [0.2; 2]);
    }
    #[test]
    fn stereo_bounds_loop_and_release() {
        let c = SamplerConfig {
            start_frame: 1,
            end_frame: Some(3),
            loop_start_frame: Some(1),
            loop_end_frame: Some(3),
            release_seconds: 0.002,
            ..config()
        };
        let pcm = [0., 0., 0.25, -0.25, 0.5, -0.5];
        let mut v = SamplerVoice::new(c, 60, 1.0, 1000, 1000, 3).unwrap();
        assert_eq!(v.next_frame(&pcm, 2), [0.25, -0.25]);
        assert_eq!(v.next_frame(&pcm, 2), [0.5, -0.5]);
        assert_eq!(v.next_frame(&pcm, 2), [0.25, -0.25]);
        v.release();
        assert_eq!(v.next_frame(&pcm, 2), [0.5, -0.5]);
        let frame = v.next_frame(&pcm, 2);
        assert!((frame[0] - 0.125).abs() < 1e-6);
        assert!(!v.is_active());
    }
    #[test]
    fn adsr_and_chase() {
        let c = SamplerConfig {
            attack_seconds: 0.002,
            decay_seconds: 0.002,
            sustain: 0.5,
            ..config()
        };
        let mut v = SamplerVoice::new(c, 60, 1., 1000, 1000, 32).unwrap();
        let pcm = [1.; 32];
        assert_eq!(v.next_frame(&pcm, 1), [0.; 2]);
        assert!((v.next_frame(&pcm, 1)[0] - 0.5).abs() < 1e-6);
        v.chase(4);
        assert!((v.next_frame(&pcm, 1)[0] - 0.5).abs() < 1e-6);
    }
    #[test]
    fn invalid_ranges_fail_closed() {
        let mut c = config();
        c.end_frame = Some(11);
        assert!(SamplerVoice::new(c, 60, 1., 1000, 1000, 10).is_none());
        c.end_frame = Some(10);
        c.loop_end_frame = Some(5);
        assert!(SamplerVoice::new(c, 60, 1., 1000, 1000, 10).is_none());
    }
    #[test]
    fn serialization_roundtrip() {
        let c = config();
        assert_eq!(
            serde_json::from_str::<SamplerConfig>(&serde_json::to_string(&c).unwrap()).unwrap(),
            c
        );
        assert_eq!(
            serde_json::from_str::<SamplerConfig>("{\"asset_id\":7}").unwrap(),
            SamplerConfig::new(7)
        );
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    fn flat() -> SamplerConfig {
        SamplerConfig {
            attack_seconds: 0.0,
            decay_seconds: 0.0,
            sustain: 1.0,
            ..SamplerConfig::new(1)
        }
    }
    #[test]
    fn half_rate_interpolates_and_finishes() {
        let mut v = SamplerVoice::new(flat(), 60, 1., 24000, 48000, 2).unwrap();
        let pcm = [0., 1.];
        assert_eq!(v.next_frame(&pcm, 1), [0.; 2]);
        assert_eq!(v.next_frame(&pcm, 1), [0.5; 2]);
        assert_eq!(v.next_frame(&pcm, 1), [1.; 2]);
        v.next_frame(&pcm, 1);
        assert_eq!(v.next_frame(&pcm, 1), [0.; 2]);
        assert!(!v.is_active());
    }
    #[test]
    fn nonfinite_pcm_is_silent_and_invalid_envelopes_rejected() {
        let mut v = SamplerVoice::new(flat(), 60, 1., 1000, 1000, 2).unwrap();
        assert_eq!(v.next_frame(&[f32::NAN, f32::INFINITY], 1), [0.; 2]);
        let c = SamplerConfig {
            release_seconds: f32::NAN,
            ..flat()
        };
        assert!(SamplerVoice::new(c, 60, 1., 1000, 1000, 2).is_none());
    }
    #[test]
    fn seek_is_idempotent_and_exclusive_end_is_silent() {
        let mut v = SamplerVoice::new(flat(), 60, 1., 1000, 1000, 3).unwrap();
        v.chase(1);
        v.chase(1);
        assert_eq!(v.next_frame(&[0., 0.5, 1.], 1), [0.5; 2]);
        v.chase(3);
        assert_eq!(v.next_frame(&[0., 0.5, 1.], 1), [0.; 2]);
    }
    #[test]
    fn zero_release_stops_and_loop_wraps_large_pitch_steps() {
        let c = SamplerConfig {
            release_seconds: 0.,
            loop_start_frame: Some(1),
            loop_end_frame: Some(3),
            ..flat()
        };
        let mut v = SamplerVoice::new(c, 96, 1., 1000, 1000, 4).unwrap();
        v.next_frame(&[0., 0.25, 0.5, 1.], 1);
        assert_eq!(v.next_frame(&[0., 0.25, 0.5, 1.], 1), [0.5; 2]);
        v.release();
        assert!(!v.is_active());
    }
    #[test]
    fn exact_source_end_finishes_without_phantom_export_tail() {
        let mut voice = SamplerVoice::new(flat(), 60, 1.0, 1000, 1000, 1).unwrap();
        assert_eq!(voice.next_frame(&[0.25], 1), [0.25; 2]);
        assert!(!voice.is_active());
        voice.chase(0);
        assert!(voice.is_active());
        assert_eq!(voice.next_frame(&[0.25], 1), [0.25; 2]);
    }
}
