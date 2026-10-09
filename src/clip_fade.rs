use crate::{
    model::{AudioFadeReference, AudioLengthReference, Clip},
    tempo_map::TempoMap,
};

/// Absolute, signed frame anchors let a moved slice retain an inherited ramp
/// whose original endpoint now lies before beat zero. Copy-only callback data.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CompiledClipFades {
    pub fade_in_start: i64,
    pub fade_out_end: i64,
    pub fade_in_frames: u64,
    pub fade_out_frames: u64,
}

impl CompiledClipFades {
    pub fn gain_at(self, timeline_frame: u64) -> f32 {
        let frame = i128::from(timeline_frame);
        signed_endpoint_gain(frame - i128::from(self.fade_in_start), self.fade_in_frames)
            * signed_endpoint_gain(
                i128::from(self.fade_out_end) - frame - 1,
                self.fade_out_frames,
            )
    }
}

fn signed_endpoint_gain(distance: i128, frames: u64) -> f32 {
    if frames == 0 {
        1.0
    } else if distance < 0 {
        0.0
    } else {
        endpoint_fade_gain(distance.min(i128::from(u64::MAX)) as u64, frames)
    }
}

/// Both renderers use the same domain rules. Legacy export computed its beat
/// endpoints at f32 precision; retain that arithmetic to preserve v10 PCM.
#[allow(clippy::too_many_arguments)]
pub fn compile_clip_fades(
    start: f32,
    length: f32,
    length_reference: Option<AudioLengthReference>,
    fade_in: f32,
    fade_out: f32,
    fade_in_reference: Option<AudioFadeReference>,
    fade_out_reference: Option<AudioFadeReference>,
    map: &TempoMap,
    legacy_export_precision: bool,
) -> Option<CompiledClipFades> {
    if !start.is_finite()
        || !length.is_finite()
        || length <= 0.0
        || !fade_in.is_finite()
        || !fade_out.is_finite()
    {
        return None;
    }
    let exact_length = length_reference.filter(|reference| reference.stored_length_beats == length);
    if length_reference.is_some_and(|reference| !reference.is_valid()) {
        return None;
    }
    let domain = |reference: Option<AudioFadeReference>| -> Option<(f64, f64, Option<f64>, bool)> {
        let use_legacy_precision = legacy_export_precision
            && reference.map_or(exact_length.is_none(), |reference| {
                reference.export_length_beats.is_none()
            });
        let (reference, limit) = match reference {
            Some(reference) => (
                reference,
                reference
                    .end_limit_beats
                    .map(|limit| f64::from(start) + limit),
            ),
            None => (
                AudioFadeReference {
                    offset_beats: 0.0,
                    length_beats: exact_length.map_or(f64::from(length), |reference| {
                        if legacy_export_precision {
                            reference.export_length_beats
                        } else {
                            reference.length_beats
                        }
                    }),
                    end_limit_beats: None,
                    export_length_beats: None,
                },
                Some(map.max_beat()),
            ),
        };
        if !reference.is_valid() {
            return None;
        }
        let root = f64::from(start) + reference.offset_beats;
        let length = if legacy_export_precision {
            reference
                .export_length_beats
                .unwrap_or(reference.length_beats)
        } else {
            limit.map_or(reference.length_beats, |end| {
                reference.length_beats.min(end - root)
            })
        };
        Some((root, length, limit, use_legacy_precision))
    };
    let (in_start, in_length, in_limit, in_precision) = domain(fade_in_reference)?;
    let (out_start, out_length, out_limit, out_precision) = domain(fade_out_reference)?;
    let point = |start: f64, length: f64, fraction: f32, legacy: bool| {
        if legacy {
            f64::from(start as f32 + length as f32 * fraction)
        } else {
            start + length * f64::from(fraction)
        }
    };
    let limited = |beat: f64, limit: Option<f64>| limit.map_or(beat, |end| beat.min(end));
    let in_start_frame = map.extended_beat_to_frame(in_start)?;
    let in_end_frame = map.extended_beat_to_frame(limited(
        point(in_start, in_length, fade_in.clamp(0.0, 1.0), in_precision),
        in_limit,
    ))?;
    let out_end_frame = map.extended_beat_to_frame(limited(
        point(out_start, out_length, 1.0, out_precision),
        out_limit,
    ))?;
    let out_start_beat = if out_precision {
        point(out_start, out_length, 1.0 - fade_out.clamp(0.0, 1.0), true)
    } else {
        out_start + out_length - out_length * f64::from(fade_out.clamp(0.0, 1.0))
    };
    let out_start_frame = map.extended_beat_to_frame(limited(out_start_beat, out_limit))?;
    Some(CompiledClipFades {
        fade_in_start: in_start_frame,
        fade_out_end: out_end_frame,
        fade_in_frames: u64::try_from(in_end_frame.checked_sub(in_start_frame)?).ok()?,
        fade_out_frames: u64::try_from(out_end_frame.checked_sub(out_start_frame)?).ok()?,
    })
}

fn normalized_side_gain(
    relative_beat: f64,
    length: f64,
    fraction: f32,
    reference: Option<AudioFadeReference>,
    is_in: bool,
) -> f32 {
    if !relative_beat.is_finite() || !length.is_finite() || !fraction.is_finite() {
        return 0.0;
    }
    if fraction <= 0.0 {
        return 1.0;
    }
    let reference = reference.unwrap_or(AudioFadeReference {
        offset_beats: 0.0,
        length_beats: length,
        end_limit_beats: None,
        export_length_beats: None,
    });
    if !reference.is_valid() {
        return 0.0;
    }
    let domain_length = reference
        .end_limit_beats
        .map_or(reference.length_beats, |end| {
            reference.length_beats.min(end - reference.offset_beats)
        });
    let distance = if is_in {
        relative_beat - reference.offset_beats
    } else {
        reference.offset_beats + domain_length - relative_beat
    };
    equal_power_ramp((distance / (domain_length * f64::from(fraction.clamp(0.0, 1.0)))) as f32)
}

pub fn clip_normalized_gain(clip: &Clip, relative_beat: f64) -> f32 {
    normalized_side_gain(
        relative_beat,
        f64::from(clip.length),
        clip.fade_in,
        clip.fade_in_reference,
        true,
    ) * normalized_side_gain(
        relative_beat,
        f64::from(clip.length),
        clip.fade_out,
        clip.fade_out_reference,
        false,
    )
}

pub fn fade_handle_fraction(clip: &Clip, is_in: bool) -> f32 {
    let (fraction, reference) = if is_in {
        (clip.fade_in, clip.fade_in_reference)
    } else {
        (clip.fade_out, clip.fade_out_reference)
    };
    if fraction <= 0.0 {
        return 0.0;
    }
    let Some(reference) = reference else {
        return fraction.clamp(0.0, 1.0);
    };
    let domain_length = reference
        .end_limit_beats
        .map_or(reference.length_beats, |end| {
            reference.length_beats.min(end - reference.offset_beats)
        });
    let boundary = if is_in {
        reference.offset_beats + domain_length * f64::from(fraction)
    } else {
        reference.offset_beats + domain_length * (1.0 - f64::from(fraction))
    };
    let fraction = if is_in {
        boundary / f64::from(clip.length)
    } else {
        1.0 - boundary / f64::from(clip.length)
    };
    fraction.clamp(0.0, 1.0) as f32
}

pub fn set_clip_fade(clip: &mut Clip, is_in: bool, fraction: f32) {
    if is_in {
        clip.fade_in = fraction.clamp(0.0, 1.0);
        clip.fade_in_reference = None;
    } else {
        clip.fade_out = fraction.clamp(0.0, 1.0);
        clip.fade_out_reference = None;
    }
}

/// Equal-power gain for one frame of a half-open Audio Clip envelope.
///
/// Fade lengths include both endpoints: a two-frame fade is `[0, 1]` for a
/// fade-in and `[1, 0]` for a fade-out. Matching fade-in/out lengths therefore
/// keep the sum of squared gains at unity across a crossfade.
#[must_use]
#[cfg(test)]
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
#[cfg(test)]
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

    fn test_audio_clip(start: f32, length: f32) -> Clip {
        let mut clip = crate::model::Project::default().clips[0].clone();
        clip.kind = crate::model::ClipKind::Audio;
        clip.start = start;
        clip.length = length;
        clip.fade_in = 0.625;
        clip.fade_out = 0.75;
        clip.audio_source_offset_frame = Some(17);
        clip
    }

    fn compiled(clip: &Clip, map: &TempoMap, legacy_export_precision: bool) -> CompiledClipFades {
        compile_clip_fades(
            clip.start,
            clip.length,
            clip.audio_length_reference,
            clip.fade_in,
            clip.fade_out,
            clip.fade_in_reference,
            clip.fade_out_reference,
            map,
            legacy_export_precision,
        )
        .unwrap()
    }

    #[test]
    fn compiled_split_fades_preserve_every_anchor_across_rates_tempo_and_nested_cuts() {
        use crate::{
            audio_clip::split_audio_clip,
            automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
        };

        let original = test_audio_clip(3.125, 9.75);
        for sample_rate in [8_000, 44_100, 48_000, 96_000] {
            for curve in [
                None,
                Some(AutomationCurve::Linear),
                Some(AutomationCurve::Hold),
            ] {
                let lane = curve.map(|curve| {
                    let mut lane = AutomationLane::new(AutomationTarget::Tempo);
                    lane.replace_points([
                        AutomationPoint::new(0.0, 73.0),
                        AutomationPoint::new(5.375, 173.0),
                        AutomationPoint::new(8.25, 61.0),
                        AutomationPoint::new(16.0, 147.0),
                    ]);
                    lane.set_curve(curve);
                    lane
                });
                let map = TempoMap::new(127.0, lane, 32.0, sample_rate).unwrap();
                for split_beat in [3.25, 6.375, 12.75] {
                    let (left, right) =
                        split_audio_clip(&original, split_beat, 900, Some(&map), 127.0).unwrap();
                    // A second cut must continue using the root, including when only a
                    // short remnant of a fade remains in one child.
                    let nested_beat = right.start + right.length * 0.5;
                    let (middle, end) =
                        split_audio_clip(&right, nested_beat, 901, Some(&map), 127.0).unwrap();
                    for legacy_export_precision in [false, true] {
                        let root_fades = compiled(&original, &map, legacy_export_precision);
                        for child in [&left, &right, &middle, &end] {
                            let child_fades = compiled(child, &map, legacy_export_precision);
                            // Integer anchor identity proves every frame uses the exact
                            // same envelope, independent of callback block boundaries.
                            assert_eq!(child_fades, root_fades);
                            let start = map.beat_to_frame(f64::from(child.start)).unwrap();
                            let stop = map
                                .beat_to_frame(f64::from(child.start + child.length))
                                .unwrap();
                            for frame in [start, start + 1, (start + stop) / 2, stop - 1] {
                                assert_eq!(
                                    child_fades.gain_at(frame).to_bits(),
                                    root_fades.gain_at(frame).to_bits()
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn inherited_fades_keep_signed_anchors_when_moved_before_the_original_domain() {
        use crate::audio_clip::split_audio_clip;

        for sample_rate in [8_000, 44_100, 48_000, 96_000] {
            let map = TempoMap::new(120.0, None, 16.0, sample_rate).unwrap();
            let mut original = test_audio_clip(4.0, 8.0);
            original.fade_in = 0.75;
            original.fade_out = 0.5;
            let (_, mut moved) = split_audio_clip(&original, 6.0, 900, Some(&map), 120.0).unwrap();
            moved.start = 0.0;
            // The visible clip extends beyond this map; reference phase is continued
            // at the boundary tempo rather than clipping endpoints or dropping it.
            let shorter_map = TempoMap::new(120.0, None, 4.0, sample_rate).unwrap();
            for legacy_export_precision in [false, true] {
                let fades = compiled(&moved, &shorter_map, legacy_export_precision);
                assert_eq!(fades.fade_in_start, -i64::from(sample_rate));
                assert_eq!(fades.fade_out_end, 3 * i64::from(sample_rate));
                assert_eq!(fades.fade_in_frames, 3 * u64::from(sample_rate));
                assert_eq!(fades.fade_out_frames, 2 * u64::from(sample_rate));
                for frame in [0, 1, u64::from(sample_rate), 2 * u64::from(sample_rate)] {
                    let expected = equal_power_frame_gain(
                        frame + u64::from(sample_rate),
                        4 * u64::from(sample_rate),
                        3 * u64::from(sample_rate),
                        2 * u64::from(sample_rate),
                    );
                    assert_eq!(fades.gain_at(frame).to_bits(), expected.to_bits());
                }
            }
        }
    }

    #[test]
    fn inherited_zero_fades_do_not_display_handles_as_the_clip_extends() {
        let mut clip = test_audio_clip(0.0, 10.0);
        clip.fade_in = 0.0;
        clip.fade_out = 0.0;
        clip.fade_in_reference = Some(AudioFadeReference {
            offset_beats: -2.0,
            length_beats: 8.0,
            end_limit_beats: None,
            export_length_beats: None,
        });
        clip.fade_out_reference = clip.fade_in_reference;
        assert_eq!(fade_handle_fraction(&clip, true), 0.0);
        assert_eq!(fade_handle_fraction(&clip, false), 0.0);
        for beat in [0.0, 1.0, 5.0, 9.0] {
            assert_eq!(clip_normalized_gain(&clip, beat), 1.0);
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
    #[test]
    fn edited_exact_length_fades_and_resplits_keep_the_last_sample_silent() {
        use crate::audio_clip::{audio_end_beat, split_audio_clip};
        let map = TempoMap::new(127.0, None, 8.0, 96_000).unwrap();
        let mut original = test_audio_clip(0.071, 4.223);
        original.fade_in = 0.0;
        let (_, right) = split_audio_clip(&original, 1.409_691, 900, Some(&map), 127.0).unwrap();
        for fraction in [1.0, (127.0 / (60.0 * 96_000.0 * right.length))] {
            let mut edited = right.clone();
            set_clip_fade(&mut edited, false, fraction);
            for export in [false, true] {
                let expected = compiled(&edited, &map, export);
                let end = map
                    .beat_to_frame(audio_end_beat(&edited, export).unwrap())
                    .unwrap();
                assert_eq!(expected.fade_out_end, end as i64);
                assert_eq!(expected.gain_at(end - 1), 0.0);
                let (left, right) =
                    split_audio_clip(&edited, 1.791_139, 901, Some(&map), 127.0).unwrap();
                assert_eq!(compiled(&left, &map, export), expected);
                assert_eq!(compiled(&right, &map, export), expected);
            }
        }
    }

    #[test]
    fn splitting_an_already_song_cropped_clip_preserves_both_legacy_domains() {
        use crate::audio_clip::split_audio_clip;
        let map = TempoMap::new(127.0, None, 4.0, 48_000).unwrap();
        let original = test_audio_clip(1.0, 8.0);
        let (left, right) = split_audio_clip(&original, 2.0, 901, Some(&map), 127.0).unwrap();
        for export in [false, true] {
            let root = compiled(&original, &map, export);
            assert_eq!(compiled(&left, &map, export), root);
            assert_eq!(compiled(&right, &map, export), root);
        }
    }
}
