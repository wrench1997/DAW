/// Equal-power gain for one frame of a half-open Audio Clip envelope.
///
/// Fade lengths include both endpoints: a two-frame fade is `[0, 1]` for a
/// fade-in and `[1, 0]` for a fade-out. Matching fade-in/out lengths therefore
/// keep the sum of squared gains at unity across a crossfade.
#[must_use]
pub fn equal_power_frame_gain(
    frame: u64,
    timeline_frames: u64,
    fade_in_frames: u64,
    fade_out_frames: u64,
) -> f32 {
    if timeline_frames == 0 || frame >= timeline_frames {
        return 0.0;
    }

    let fade_in = endpoint_fade_gain(frame, fade_in_frames);
    let frames_after = timeline_frames - frame - 1;
    let fade_out = endpoint_fade_gain(frames_after, fade_out_frames);
    fade_in * fade_out
}

/// Equal-power envelope for UI previews and beat-domain fallback playback.
#[must_use]
pub fn equal_power_normalized_gain(progress: f32, fade_in: f32, fade_out: f32) -> f32 {
    if !progress.is_finite() || !fade_in.is_finite() || !fade_out.is_finite() {
        return 0.0;
    }
    let progress = progress.clamp(0.0, 1.0);
    let fade_in = fade_in.clamp(0.0, 1.0);
    let fade_out = fade_out.clamp(0.0, 1.0);
    let fade_in_gain = if fade_in == 0.0 || progress >= fade_in {
        1.0
    } else {
        equal_power_ramp(progress / fade_in)
    };
    let remaining = 1.0 - progress;
    let fade_out_gain = if fade_out == 0.0 || remaining >= fade_out {
        1.0
    } else {
        equal_power_ramp(remaining / fade_out)
    };
    fade_in_gain * fade_out_gain
}

fn endpoint_fade_gain(distance_from_silent_endpoint: u64, fade_frames: u64) -> f32 {
    if fade_frames == 0 || distance_from_silent_endpoint >= fade_frames {
        return 1.0;
    }
    if fade_frames == 1 {
        return 0.0;
    }
    equal_power_ramp(distance_from_silent_endpoint as f32 / (fade_frames - 1) as f32)
}

fn equal_power_ramp(progress: f32) -> f32 {
    (progress.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2).sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_fades_reach_silence_and_full_gain() {
        assert_eq!(equal_power_frame_gain(0, 6, 3, 0), 0.0);
        assert!(
            (equal_power_frame_gain(1, 6, 3, 0) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-6
        );
        assert_eq!(equal_power_frame_gain(2, 6, 3, 0), 1.0);
        assert_eq!(equal_power_frame_gain(5, 6, 0, 3), 0.0);
        assert!(
            (equal_power_frame_gain(4, 6, 0, 3) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1.0e-6
        );
        assert_eq!(equal_power_frame_gain(3, 6, 0, 3), 1.0);
    }

    #[test]
    fn matching_crossfade_curves_preserve_unit_power() {
        let frames = 17;
        for frame in 0..frames {
            let fade_in = equal_power_frame_gain(frame, frames, frames, 0);
            let fade_out = equal_power_frame_gain(frame, frames, 0, frames);
            assert!((fade_in * fade_in + fade_out * fade_out - 1.0).abs() < 1.0e-5);
        }
    }

    #[test]
    fn normalized_preview_matches_equal_power_midpoint() {
        assert_eq!(equal_power_normalized_gain(0.0, 0.5, 0.0), 0.0);
        assert!(
            (equal_power_normalized_gain(0.25, 0.5, 0.0) - std::f32::consts::FRAC_1_SQRT_2).abs()
                < 1.0e-6
        );
        assert_eq!(equal_power_normalized_gain(0.5, 0.5, 0.0), 1.0);
    }
}
