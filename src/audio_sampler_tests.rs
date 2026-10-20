use super::*;
use crate::sampler::SamplerConfig;

type SamplerFixture = (DspState, Producer<Arc<[f32]>>, Consumer<Arc<[f32]>>);

fn fixture() -> SamplerFixture {
    let mut dsp = DspState::new(1_000.0);
    let (mut retired, consumer) = RingBuffer::new(8);
    assert!(dsp.register_asset(
        7,
        Arc::from([0.25_f32, -0.5, 0.5, -0.25]),
        1000,
        2,
        &mut retired
    ));
    assert!(
        dsp.timeline_channel_bases
            .reset_from(&[ChannelBaseDescriptor {
                channel_id: 1,
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: 1,
                sampler: Some(SamplerConfig {
                    attack_seconds: 0.0,
                    decay_seconds: 0.0,
                    sustain: 1.0,
                    release_seconds: 0.002,
                    loop_start_frame: Some(0),
                    loop_end_frame: Some(2),
                    ..SamplerConfig::new(7)
                }),
            }])
    );
    (dsp, retired, consumer)
}
fn note() -> ChasedNote {
    ChasedNote {
        elapsed_frames: 0,
        note_id: 1,
        channel_id: 1,
        note: 60,
        velocity: 1.0,
        gain: 1.0,
        mixer_track: 1,
        source: crate::timeline::NoteSourceDescriptor::ChannelStep {
            clip_id: 1,
            pattern_id: 1,
            step: 0,
            repetition: 0,
        },
    }
}
#[test]
fn callback_plays_stereo_pcm_and_releases_without_allocation() {
    let (mut dsp, _, _) = fixture();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        dsp.start_timeline_native_note(note());
        dsp.render_synth_frame_filtered(0, false);
        dsp.render_synth_frame_filtered(1, false);
        dsp.stop_timeline_native_note(1);
        dsp.render_synth_frame_filtered(2, false);
        dsp.render_synth_frame_filtered(3, false);
        dsp.render_synth_frame_filtered(4, false);
    });
    assert_eq!(dsp.track_block[MAX_MIXER_BLOCK_FRAMES], [0.25, -0.5]);
    assert_eq!(dsp.track_block[MAX_MIXER_BLOCK_FRAMES + 1], [0.5, -0.25]);
    assert!((dsp.track_block[MAX_MIXER_BLOCK_FRAMES + 3][0] - 0.25).abs() < 1e-6);
    assert_eq!(dsp.track_block[MAX_MIXER_BLOCK_FRAMES + 4], [0.0; 2]);
    assert!(dsp.voices.iter().all(|voice| !voice.active));
}
#[test]
fn missing_asset_never_falls_back_and_unregister_stops_voice() {
    let (mut dsp, mut retired, _consumer) = fixture();
    dsp.start_timeline_native_note(note());
    assert!(dsp.voices.iter().any(|voice| voice.active));
    assert!(dsp.unregister_asset(7, &mut retired));
    dsp.start_timeline_native_note(note());
    assert!(dsp.voices.iter().all(|voice| !voice.active));
    dsp.render_synth_frame_filtered(0, false);
    assert_eq!(dsp.track_block[MAX_MIXER_BLOCK_FRAMES], [0.; 2]);
}
#[test]
fn chase_restores_pcm_cursor_and_envelope() {
    let (mut dsp, _, _) = fixture();
    let mut chased = note();
    chased.elapsed_frames = 3;
    dsp.start_timeline_native_note(chased);
    dsp.render_synth_frame_filtered(0, false);
    assert_eq!(dsp.track_block[MAX_MIXER_BLOCK_FRAMES], [0.5, -0.25]);
}

#[test]
fn release_tail_keeps_timeline_ownership_until_stop_seek_or_clear() {
    for reset in 0..3 {
        let project = super::tests::timeline_test_project(4.0);
        let timeline = super::tests::compile_timeline_test_project(&project);
        let (_controller, mut dsp, _status) = super::tests::install_and_activate_timeline(
            timeline,
            101,
            2,
            0,
            TimelineChaseOptions::default(),
        );
        let (mut retired, _consumer) = RingBuffer::new(8);
        assert!(dsp.register_asset(7, Arc::from([0.25_f32, -0.25]), 48_000, 1, &mut retired));
        assert!(
            dsp.timeline_channel_bases
                .reset_from(&[ChannelBaseDescriptor {
                    channel_id: 1,
                    volume: 1.0,
                    pan: 0.0,
                    muted: false,
                    solo: false,
                    mixer_track: 1,
                    sampler: Some(SamplerConfig {
                        attack_seconds: 0.0,
                        decay_seconds: 0.0,
                        sustain: 1.0,
                        release_seconds: 1.0,
                        loop_start_frame: Some(0),
                        loop_end_frame: Some(2),
                        ..SamplerConfig::new(7)
                    }),
                }])
        );
        assert_eq!(
            dsp.timeline_runtime.as_ref().unwrap().active_epoch(),
            Some(2)
        );
        dsp.start_timeline_native_note(note());
        dsp.stop_timeline_native_note(1);
        assert!(
            dsp.voices
                .iter()
                .any(|voice| voice.active && voice.timeline_note_id == Some(1))
        );
        match reset {
            0 => dsp.pause_timeline(),
            1 => dsp.reset_voice_and_pdc_state(0),
            _ => dsp.clear_timeline_render_binding(),
        }
        assert!(dsp.voices.iter().all(|voice| !voice.active));
        dsp.render_synth_frame_filtered(0, false);
        assert_eq!(dsp.track_block[MAX_MIXER_BLOCK_FRAMES], [0.; 2]);
    }
}

#[test]
fn owned_sampler_keyboard_callback_sustains_then_releases_without_allocations() {
    let (mut engine, mut callback) = AudioEngine::test_engine_with_callback();
    assert!(engine.set_project_session(7));
    callback.pump(&engine, 128);
    assert!(callback.dsp.register_asset(
        77,
        Arc::from([0.25_f32, -0.25]),
        48_000,
        1,
        &mut callback.retired_assets
    ));
    assert!(
        callback
            .dsp
            .timeline_channel_bases
            .reset_from(&[ChannelBaseDescriptor {
                channel_id: 7,
                volume: 1.0,
                pan: 0.0,
                muted: false,
                solo: false,
                mixer_track: 1,
                sampler: Some(SamplerConfig {
                    attack_seconds: 0.0,
                    decay_seconds: 0.0,
                    sustain: 1.0,
                    release_seconds: 0.01,
                    loop_start_frame: Some(0),
                    loop_end_frame: Some(2),
                    ..SamplerConfig::new(77)
                }),
            }])
    );
    let token = engine
        .begin_audition(
            AuditionDestination::Native {
                channel_id: 7,
                mixer_track: 1,
            },
            60,
            1.0,
        )
        .unwrap();
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
    assert_eq!(
        engine.audition_status(token),
        Some(AuditionStatus::Accepted)
    );
    for _ in 0..8 {
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
    }
    assert!(
        callback
            .last_output()
            .iter()
            .any(|frame| frame[0].abs() > 0.01)
    );
    assert!(engine.end_audition(token));
    for _ in 0..8 {
        crate::realtime_test_alloc::assert_no_alloc_or_drop(|| callback.pump(&engine, 128));
    }
    assert!(callback.dsp.voices.iter().all(|voice| !voice.active));
    assert!(
        callback
            .last_output()
            .iter()
            .all(|frame| frame[0].abs() < 0.0001)
    );
}

#[test]
fn released_sampler_auditions_do_not_exhaust_non_audition_voice_selection() {
    let (mut dsp, _, _) = fixture();
    for index in 0..MAX_VOICES {
        dsp.start_timeline_native_note(ChasedNote {
            note_id: index as u64 + 1,
            ..note()
        });
        dsp.voices[index].audition_token_id = Some(index as u64 + 1);
        dsp.voices[index].sampler.as_mut().unwrap().1.release();
    }
    let index = dsp.next_non_audition_voice();
    assert!(index < MAX_VOICES);
}

#[test]
fn empty_sampler_is_silent_without_silencing_another_channel_or_legacy_oscillator() {
    let (mut dsp, _retired, _consumer) = fixture();
    let loaded = dsp.timeline_channel_bases.get(1).unwrap();
    let mut empty = loaded;
    empty.channel_id = 2;
    empty.sampler = Some(SamplerConfig::empty());
    assert!(dsp.timeline_channel_bases.insert(empty));
    crate::realtime_test_alloc::assert_no_alloc_or_drop(|| {
        dsp.start_timeline_native_note(ChasedNote {
            channel_id: 2,
            note_id: 2,
            ..note()
        });
        dsp.render_synth_frame_filtered(0, false);
    });
    assert_eq!(dsp.track_block[MAX_MIXER_BLOCK_FRAMES], [0.0; 2]);
    assert!(dsp.voices.iter().all(|voice| !voice.active));
    dsp.start_timeline_native_note(note());
    dsp.start_timeline_native_note(ChasedNote {
        channel_id: 2,
        note_id: 2,
        ..note()
    });
    dsp.render_synth_frame_filtered(1, false);
    assert_eq!(dsp.track_block[MAX_MIXER_BLOCK_FRAMES + 1], [0.25, -0.5]);
    dsp.reset_voice_and_pdc_state(0);
    empty.sampler = None;
    assert!(dsp.timeline_channel_bases.insert(empty));
    dsp.start_timeline_native_note(ChasedNote {
        channel_id: 2,
        note_id: 2,
        ..note()
    });
    assert!(
        dsp.voices
            .iter()
            .any(|voice| voice.active && voice.sampler.is_none())
    );
}
