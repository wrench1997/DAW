//! Non-realtime Audio Clip edits and source-grid provenance.
//!
//! Source in-points are sample-locked: a later move or tempo edit does not
//! reinterpret an existing cut. A new cut records the current tempo interval.
//! Keep individual clock intervals because round(b * rate) - round(a * rate)
//! cannot in general be replaced with round((b - a) * rate).
use crate::{
    model::{
        AudioFadeReference, AudioLengthReference, AudioSourceReference, AudioSourceSpan, Clip,
        ClipKind,
    },
    tempo_map::TempoMap,
};

/// The actual Playlist split operation. Both children retain complete edit
/// state, including inherited fades; no asset decoding is required.
pub fn split_audio_clip(
    clip: &Clip,
    beat: f32,
    right_id: u32,
    tempo_map: Option<&TempoMap>,
    fallback_tempo: f32,
) -> Result<(Clip, Clip), &'static str> {
    if clip.kind != ClipKind::Audio
        || !beat.is_finite()
        || !clip.start.is_finite()
        || !clip.length.is_finite()
        || beat <= clip.start
        || beat >= clip.start + clip.length
        || !audio_metadata_valid(clip)
    {
        return Err("Audio Clip split has invalid bounds or timing metadata");
    }
    let realtime_end = audio_end_beat(clip, false).ok_or("Invalid Audio Clip extent")?;
    let export_end = audio_end_beat(clip, true).ok_or("Invalid Audio Clip extent")?;
    if f64::from(beat) >= realtime_end.min(export_end) {
        return Err("Audio split is outside the exact clip extent");
    }
    let delta = f64::from(beat) - f64::from(clip.start);
    let domain = AudioFadeReference {
        offset_beats: 0.0,
        length_beats: realtime_end - f64::from(clip.start),
        export_length_beats: clip
            .audio_length_reference
            .filter(|reference| reference.stored_length_beats == clip.length)
            .map(|_| export_end - f64::from(clip.start)),
        end_limit_beats: tempo_map
            .filter(|map| f64::from(clip.start) + f64::from(clip.length) > map.max_beat())
            .map(|map| map.max_beat() - f64::from(clip.start)),
    };
    let mut left = clip.clone();
    let mut right = clip.clone();
    left.length = beat - clip.start;
    right.id = right_id;
    right.start = beat;
    right.length = (clip.start + clip.length) - beat;
    left.audio_length_reference = Some(AudioLengthReference {
        stored_length_beats: left.length,
        length_beats: delta,
        export_length_beats: delta,
    });
    right.audio_length_reference = Some(AudioLengthReference {
        stored_length_beats: right.length,
        length_beats: realtime_end - f64::from(beat),
        export_length_beats: export_end - f64::from(beat),
    });
    for (left_side, right_side, inherited) in [
        (
            &mut left.fade_in_reference,
            &mut right.fade_in_reference,
            clip.fade_in_reference,
        ),
        (
            &mut left.fade_out_reference,
            &mut right.fade_out_reference,
            clip.fade_out_reference,
        ),
    ] {
        let reference = inherited.unwrap_or(domain);
        *left_side = Some(reference);
        *right_side = Some(AudioFadeReference {
            offset_beats: reference.offset_beats - delta,
            end_limit_beats: reference.end_limit_beats.map(|limit| limit - delta),
            ..reference
        });
    }
    let start_seconds = beat_seconds(tempo_map, f64::from(clip.start), fallback_tempo)?;
    let end_seconds = beat_seconds(tempo_map, f64::from(beat), fallback_tempo)?;
    let reference = right
        .audio_source_reference
        .get_or_insert_with(|| AudioSourceReference {
            elapsed_spans_seconds: Vec::new(),
        });
    if let Some(last) = reference.elapsed_spans_seconds.last_mut()
        && last.end_seconds == start_seconds
    {
        last.end_seconds = end_seconds;
    } else {
        reference.elapsed_spans_seconds.push(AudioSourceSpan {
            start_seconds,
            end_seconds,
        });
    }
    if !audio_metadata_valid(&left) || !audio_metadata_valid(&right) {
        return Err("Audio Clip timing history exceeds the supported bounds");
    }
    Ok((left, right))
}

pub fn audio_metadata_valid(clip: &Clip) -> bool {
    clip.audio_length_reference
        .is_none_or(|reference| reference.is_valid())
        && clip
            .fade_in_reference
            .is_none_or(|reference| reference.is_valid())
        && clip
            .fade_out_reference
            .is_none_or(|reference| reference.is_valid())
        && clip
            .audio_source_reference
            .as_ref()
            .is_none_or(|reference| reference.is_valid())
}

pub fn beat_seconds(
    map: Option<&TempoMap>,
    beat: f64,
    fallback_tempo: f32,
) -> Result<f64, &'static str> {
    if !beat.is_finite() || beat < 0.0 {
        return Err("Invalid Audio Clip beat position");
    }
    if let Some(map) = map {
        map.beat_to_seconds(beat)
            .map_err(|_| "Audio Clip beat is outside the tempo map")
    } else if fallback_tempo.is_finite() {
        Ok(beat * 60.0 / f64::from(fallback_tempo.clamp(20.0, 400.0)))
    } else {
        Err("Invalid Audio Clip tempo")
    }
}

/// Compiles all saved intervals once. Never call this from an audio callback.
pub fn source_elapsed_frames(
    reference: Option<&AudioSourceReference>,
    sample_rate: u32,
) -> Option<i64> {
    if sample_rate == 0 {
        return None;
    }
    let Some(reference) = reference else {
        return Some(0);
    };
    if !reference.is_valid() {
        return None;
    }
    let rate = f64::from(sample_rate);
    let mut frames = 0_i128;
    for span in &reference.elapsed_spans_seconds {
        let start = (span.start_seconds * rate).round();
        let end = (span.end_seconds * rate).round();
        if start > i64::MAX as f64 || end > i64::MAX as f64 {
            return None;
        }
        frames = frames.checked_add(end as i128 - start as i128)?;
    }
    i64::try_from(frames).ok()
}

/// Continuous-time fallback and waveform positioning (the scheduled renderers
/// use the output-grid-aware form above).
pub fn source_position_seconds(clip: &Clip, source_sample_rate: u32) -> Option<f64> {
    if source_sample_rate == 0 || !audio_metadata_valid(clip) {
        return None;
    }
    let root = clip.audio_source_offset_frame? as f64;
    let seconds: f64 = clip
        .audio_source_reference
        .as_ref()
        .map_or(0.0, |reference| {
            reference
                .elapsed_spans_seconds
                .iter()
                .map(|span| span.end_seconds - span.start_seconds)
                .sum()
        });
    let frame = root + seconds * f64::from(source_sample_rate);
    (frame.is_finite() && frame >= 0.0).then_some(frame)
}

/// An explicit Slip edit chooses a new native-frame in-point. A zero movement
/// leaves the exact inherited phase alone; a real edit deliberately reanchors.
pub fn slip_audio_clip_source(
    clip: &mut Clip,
    delta_frames: i128,
    maximum_frame: u64,
    source_rate: u32,
) -> bool {
    if delta_frames == 0 {
        return false;
    }
    let Some(origin) = source_position_seconds(clip, source_rate) else {
        return false;
    };
    let target = crate::playlist::slipped_audio_source_offset(
        origin.round() as u64,
        delta_frames,
        maximum_frame,
    );
    clip.audio_source_offset_frame = Some(target);
    clip.audio_source_reference = None;
    true
}

/// Keeps inherited split boundaries exact despite the Playlist's f32 geometry.
/// An explicit length change opts back into the normal resize geometry.
pub fn audio_end_beat_for(
    start: f32,
    length: f32,
    reference: Option<AudioLengthReference>,
    export: bool,
) -> Option<f64> {
    if !start.is_finite() || !length.is_finite() || length <= 0.0 {
        return None;
    }
    if let Some(reference) = reference {
        if !reference.is_valid() {
            return None;
        }
        if reference.stored_length_beats == length {
            return Some(
                f64::from(start)
                    + if export {
                        reference.export_length_beats
                    } else {
                        reference.length_beats
                    },
            );
        }
    }
    Some(if export {
        f64::from(start + length)
    } else {
        f64::from(start) + f64::from(length)
    })
}

pub fn audio_end_beat(clip: &Clip, export: bool) -> Option<f64> {
    audio_end_beat_for(clip.start, clip.length, clip.audio_length_reference, export)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget};
    use crate::model::Project;

    fn clip() -> Clip {
        let mut clip = Project::default().clips[0].clone();
        clip.kind = ClipKind::Audio;
        clip.start = 0.125;
        clip.length = 2.0;
        clip.audio_source_offset_frame = Some(137);
        clip.audio_source_reference = None;
        clip.fade_in = 0.75;
        clip.fade_out = 0.625;
        clip
    }

    fn map(rate: u32, curve: Option<AutomationCurve>) -> TempoMap {
        let automation = curve.map(|curve| {
            let mut lane = AutomationLane::new(AutomationTarget::Tempo);
            lane.set_curve(curve);
            lane.replace_points([
                AutomationPoint::new(0.0, 120.0),
                AutomationPoint::new(0.75, 79.0),
                AutomationPoint::new(3.0, 193.0),
            ]);
            lane
        });
        TempoMap::new(120.0, automation, 8.0, rate).unwrap()
    }

    fn elapsed(clip: &Clip, map: &TempoMap, frame: u64) -> i128 {
        i128::from(
            source_elapsed_frames(clip.audio_source_reference.as_ref(), map.sample_rate()).unwrap(),
        ) + i128::from(frame)
            - i128::from(map.beat_to_frame(f64::from(clip.start)).unwrap())
    }

    #[test]
    fn source_clock_is_exact_for_nested_cuts_rates_moves_and_changed_tempo() {
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000] {
            for curve in [
                None,
                Some(AutomationCurve::Hold),
                Some(AutomationCurve::Linear),
            ] {
                let map = map(rate, curve);
                let original = clip();
                let (left, right) =
                    split_audio_clip(&original, 0.375, 99, Some(&map), 120.0).unwrap();
                let (middle, last) =
                    split_audio_clip(&right, 0.8125, 100, Some(&map), 120.0).unwrap();
                assert_eq!(
                    last.audio_source_reference
                        .as_ref()
                        .unwrap()
                        .elapsed_spans_seconds
                        .len(),
                    1
                );
                for child in [&left, &middle, &last] {
                    let start = map.beat_to_frame(f64::from(child.start)).unwrap();
                    let end = map
                        .beat_to_frame(f64::from(child.start) + f64::from(child.length))
                        .unwrap();
                    for frame in (start..end).step_by(17) {
                        assert_eq!(elapsed(child, &map, frame), elapsed(&original, &map, frame));
                    }
                }
                let mut moved = right.clone();
                moved.start = 0.0;
                let reference_before = moved.audio_source_reference.clone();
                let changed = TempoMap::new(137.0, None, 8.0, rate).unwrap();
                let (moved_left, moved_right) =
                    split_audio_clip(&moved, 0.1875, 101, Some(&changed), 137.0).unwrap();
                assert_eq!(moved_left.audio_source_reference, reference_before);
                assert_eq!(
                    moved_right
                        .audio_source_reference
                        .as_ref()
                        .unwrap()
                        .elapsed_spans_seconds
                        .len(),
                    2
                );
                for child in [&moved_left, &moved_right] {
                    let start = changed.beat_to_frame(f64::from(child.start)).unwrap();
                    let end = changed
                        .beat_to_frame(f64::from(child.start) + f64::from(child.length))
                        .unwrap();
                    for frame in (start..end).step_by(19) {
                        assert_eq!(
                            elapsed(child, &changed, frame),
                            elapsed(&moved, &changed, frame)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn fractional_native_cut_retains_output_grid_phase_and_slip_is_explicit() {
        let map = map(48_000, None);
        let mut original = clip();
        original.start = 0.0;
        original.audio_source_offset_frame = Some(0);
        let (_, mut right) = split_audio_clip(&original, 0.25, 99, Some(&map), 120.0).unwrap();
        assert_eq!(
            source_elapsed_frames(right.audio_source_reference.as_ref(), 48_000),
            Some(6_000)
        );
        assert_eq!(source_position_seconds(&right, 44_100), Some(5_512.5));
        let unchanged = serde_json::to_string(&right).unwrap();
        assert!(!slip_audio_clip_source(&mut right, 0, 100_000, 44_100));
        assert_eq!(serde_json::to_string(&right).unwrap(), unchanged);
        assert!(slip_audio_clip_source(&mut right, 100, 100_000, 44_100));
        assert_eq!(right.audio_source_offset_frame, Some(5_413));
        assert!(right.audio_source_reference.is_none());
        assert_eq!(
            right.fade_in_reference,
            original.fade_in_reference.or(Some(AudioFadeReference {
                offset_beats: -0.25,
                length_beats: 2.0,
                end_limit_beats: None,
                export_length_beats: None,
            }))
        );
    }

    #[test]
    fn signed_spans_cancel_without_per_span_saturation_and_invalid_splits_are_atomic() {
        let reference = AudioSourceReference {
            elapsed_spans_seconds: vec![
                AudioSourceSpan {
                    start_seconds: 0.3,
                    end_seconds: 0.1,
                },
                AudioSourceSpan {
                    start_seconds: 0.1,
                    end_seconds: 0.3,
                },
            ],
        };
        for rate in [44_100, 48_000, 192_000] {
            assert_eq!(source_elapsed_frames(Some(&reference), rate), Some(0));
        }
        let mut original = clip();
        let map = map(48_000, None);
        original.audio_source_reference = Some(AudioSourceReference {
            elapsed_spans_seconds: vec![
                AudioSourceSpan {
                    start_seconds: 1.0,
                    end_seconds: 1.0
                };
                4096
            ],
        });
        let before = serde_json::to_string(&original).unwrap();
        assert!(split_audio_clip(&original, 0.375, 99, Some(&map), 120.0).is_err());
        assert_eq!(serde_json::to_string(&original).unwrap(), before);
        original.fade_in_reference = Some(AudioFadeReference {
            offset_beats: 0.0,
            length_beats: -1.0,
            end_limit_beats: None,
            export_length_beats: None,
        });
        assert!(split_audio_clip(&original, 0.375, 99, Some(&map), 120.0).is_err());
    }
    #[test]
    fn nonbinary_split_geometry_and_nested_joins_keep_exact_endpoints() {
        for (start, length, cut) in [
            (0.071_f32, 4.223_f32, 1.409_691_f32),
            (0.923_000_04, 6.489, 2.980_013_1),
        ] {
            let mut original = clip();
            original.start = start;
            original.length = length;
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let map = TempoMap::new(127.0, None, 16.0, rate).unwrap();
                let (left, right) =
                    split_audio_clip(&original, cut, 99, Some(&map), 127.0).unwrap();
                let (middle, last) =
                    split_audio_clip(&right, cut + 0.351_234_5, 100, Some(&map), 127.0).unwrap();
                for export in [false, true] {
                    let frame = |clip: &Clip| {
                        map.beat_to_frame(audio_end_beat(clip, export).unwrap())
                            .unwrap()
                    };
                    assert_eq!(frame(&left), map.beat_to_frame(f64::from(cut)).unwrap());
                    assert_eq!(
                        frame(&middle),
                        map.beat_to_frame(f64::from(last.start)).unwrap()
                    );
                    assert_eq!(frame(&last), frame(&original));
                }
                let mut resized = right.clone();
                resized.length += 1.0;
                assert_eq!(
                    audio_end_beat(&resized, false),
                    Some(f64::from(resized.start) + f64::from(resized.length))
                );
            }
        }
    }
}
