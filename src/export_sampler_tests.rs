//! Published-WAV regressions for native sampler scheduling, DSP, and routing.
//! Fixtures use the production WAV decoder and require no audio device or SDK.

use super::tests::{arrangement_project, test_plugin, workflow_test_directory, write_pcm16_wav};
use super::*;
use crate::{
    export_options::WavLevelPolicy,
    model::{AudioClipMixerDestination, PianoNote},
};

struct Files {
    directory: PathBuf,
    paths: Vec<PathBuf>,
}

impl Files {
    fn new() -> Self {
        Self {
            directory: workflow_test_directory("sampler"),
            paths: Vec::new(),
        }
    }

    fn path(&mut self, name: &str) -> PathBuf {
        let path = self.directory.join(name);
        self.paths.push(path.clone());
        path
    }

    fn project(&mut self, source_rate: u32, channels: u16, pcm: &[i16]) -> Project {
        let source = self.path("source.wav");
        write_pcm16_wav(&source, source_rate, channels, pcm);
        let mut project = arrangement_project();
        project.tempo = 60.0;
        project.swing = 0.0;
        project.song_length_beats = 0.5;
        project.channels[0].pan = 0.0;
        project.channels[0].sampler = Some(config());
        for track in &mut project.mixer_tracks {
            track.pan = 0.0;
        }
        project.clips[0].start = 0.0;
        project.clips[0].length = 0.5;
        project.patterns[0].channel_steps[0] = [false; 16];
        project.patterns[0].notes = vec![PianoNote {
            id: 11,
            channel_id: Some(project.channels[0].id),
            group_id: None,
            note: 60,
            start: 0.0,
            length: 0.5,
            velocity: 1.0,
            selected: false,
            muted: false,
        }];
        project.audio_assets = vec![AudioAsset {
            id: 700,
            name: "Native sampler fixture".into(),
            path: source,
            sample_rate: source_rate,
            channels,
            bits_per_sample: 16,
            frames: (pcm.len() / usize::from(channels)) as u64,
            waveform_peaks: Vec::new(),
        }];
        project
    }

    fn render(&mut self, project: &Project, tail: u32) -> (Vec<[f32; 2]>, WavExportReport) {
        let path = self.path(&format!("render-{}.wav", self.paths.len()));
        let report = render_project_wav_with_options(
            project,
            &path,
            options(tail),
            &ExportControl::default(),
        )
        .unwrap();
        let decoded = wav::read_wav(&path).unwrap();
        assert_eq!(decoded.metadata.channels, 2);
        assert_eq!(decoded.metadata.sample_rate, 8_000);
        assert_eq!(decoded.metadata.frames as usize, report.frames);
        assert_eq!(report.applied_gain, 1.0);
        (decoded.samples.as_chunks::<2>().0.to_vec(), report)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}

fn config() -> SamplerConfig {
    SamplerConfig {
        asset_id: Some(700),
        root_note: 60,
        start_frame: 0,
        end_frame: None,
        loop_start_frame: None,
        loop_end_frame: None,
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    }
}

fn options(tail_milliseconds: u32) -> WavExportOptions {
    WavExportOptions {
        sample_rate: 8_000,
        sample_format: WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds,
    }
}

#[test]
fn midi_drop_sampler_saved_score_exports_exact_note_gate_and_velocity() {
    let mut files = Files::new();
    let mut source = files.project(8_000, 1, &vec![8_192; 8_000]);
    // Unlike the small render-only fixture, saved v8 projects require all 32 inserts.
    let defaults = Project::default();
    source.mixer_tracks = defaults.mixer_tracks;
    source.mixer_routes = defaults.mixer_routes;
    for track in &mut source.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
        track.muted = false;
        track.solo = false;
    }
    source.patterns[0].notes[0].length = 0.25;
    source.patterns[0].notes[0].velocity = 64.0 / 127.0;
    let channel = source.channels[0].id;
    let bytes = crate::midi::export_active_pattern(
        &source,
        &crate::midi::MidiExportOptions {
            channel_map: crate::midi::MidiExportChannelMap::try_new([(channel, 7)]).unwrap(),
            ..Default::default()
        },
    )
    .unwrap();
    let mut target = source.clone();
    target.patterns[0].notes.clear();
    target.tempo = 120.0;
    crate::midi::append_to_channel(&mut target, &bytes, channel).unwrap();
    assert_eq!(target.tempo, 120.0);
    let path = files.path("midi-drop.citrus");
    target.save(&path).unwrap();
    let reopened = Project::load(&path).unwrap();
    let (pcm, _) = files.render(&reopened, 0);
    // 0.25 beat at 120 BPM = 1000 frames at 8 kHz, not the source SMF's 60 BPM.
    close(pcm[100][0], 0.25 * 64.0 / 127.0);
    assert!(pcm[..1000].iter().all(|frame| frame[0] > 0.0));
    assert!(pcm[1000..].iter().all(|frame| *frame == [0.0; 2]));
}

#[track_caller]
fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1.0e-6,
        "actual {actual}, expected {expected}"
    );
}

#[test]
fn sampler_wav_preserves_native_trim_frames_and_octave_pitch() {
    let mut files = Files::new();
    let mut project = files.project(
        4_000,
        1,
        &[-30_000, -20_000, 4_096, 8_192, 12_288, 16_384, -10_000],
    );
    project.channels[0].sampler = Some(SamplerConfig {
        start_frame: 2,
        end_frame: Some(6),
        ..config()
    });
    let (root, _) = files.render(&project, 0);
    let expected = [
        4_096.0, 6_144.0, 8_192.0, 10_240.0, 12_288.0, 14_336.0, 16_384.0, 16_384.0,
    ];
    for (frame, expected) in root.iter().zip(expected) {
        close(frame[0], expected / 32_768.0);
        assert_eq!(frame[0], frame[1]);
    }
    assert!(root[8..].iter().all(|frame| *frame == [0.0; 2]));

    project.patterns[0].notes[0].note = 72;
    let (octave, _) = files.render(&project, 0);
    for (frame, expected) in octave.iter().zip([4_096.0, 8_192.0, 12_288.0, 16_384.0]) {
        close(frame[0], expected / 32_768.0);
        assert_eq!(frame[0], frame[1]);
    }
    assert!(octave[4..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn sampler_and_playlist_audio_can_share_an_asset_at_different_rates() {
    let mut files = Files::new();
    let mut project = files.project(4_000, 1, &[2_048, 4_096, 8_192, 16_384]);
    project.channels[0].sampler = Some(SamplerConfig {
        start_frame: 2,
        end_frame: Some(4),
        ..config()
    });
    let mut audio = project.clips[0].clone();
    audio.id = 90;
    audio.kind = ClipKind::Audio;
    audio.name = "Shared native-rate sample".into();
    audio.start = 0.25;
    audio.length = 0.001;
    audio.audio_asset_id = Some(700);
    audio.audio_source_offset_frame = Some(0);
    project.clips.push(audio);
    project
        .audio_clip_mixer_destinations
        .push(AudioClipMixerDestination {
            clip_id: 90,
            mixer_track_id: 1,
        });
    let (pcm, _) = files.render(&project, 0);
    close(pcm[0][0], 0.25);
    close(pcm[1][0], 0.375);
    close(pcm[2][0], 0.5);
    close(pcm[3][0], 0.5);
    assert!(pcm[4..2_000].iter().all(|frame| *frame == [0.0; 2]));
    close(pcm[2_000][0], 0.0625);
    close(pcm[2_001][0], 0.09375);
    close(pcm[2_002][0], 0.125);
}

#[test]
fn sampler_wav_rack_uses_root_note_and_a_quarter_beat_gate() {
    let mut files = Files::new();
    let mut project = files.project(8_000, 1, &[4_096, 8_192]);
    project.channels[0].sampler = Some(SamplerConfig {
        root_note: 72,
        loop_start_frame: Some(0),
        loop_end_frame: Some(2),
        ..config()
    });
    project.patterns[0].notes.clear();
    project.patterns[0].channel_steps[0][0] = true;
    let (pcm, report) = files.render(&project, 0);
    for (index, frame) in pcm[..2_000].iter().enumerate() {
        close(frame[0], if index % 2 == 0 { 0.125 } else { 0.25 });
        assert_eq!(frame[0], frame[1]);
    }
    assert!(pcm[2_000..].iter().all(|frame| *frame == [0.0; 2]));
    assert!(!report.tail_limit_reached);
}

#[test]
fn sampler_wav_adsr_and_exact_piano_note_off() {
    let mut files = Files::new();
    let mut project = files.project(8_000, 1, &[16_384; 2]);
    project.channels[0].sampler = Some(SamplerConfig {
        loop_start_frame: Some(0),
        loop_end_frame: Some(2),
        attack_seconds: 0.125,
        decay_seconds: 0.125,
        sustain: 0.5,
        release_seconds: 0.125,
        ..config()
    });
    project.patterns[0].notes[0].length = 0.25;
    let (pcm, report) = files.render(&project, 0);
    for (index, expected) in [
        (0, 0.0),
        (500, 0.25),
        (1_000, 0.5),
        (1_500, 0.375),
        (2_000, 0.25),
        (2_500, 0.125),
    ] {
        close(pcm[index][0], expected);
        assert_eq!(pcm[index][0], pcm[index][1]);
    }
    assert!(pcm[3_000..].iter().all(|frame| *frame == [0.0; 2]));
    assert!(!report.tail_limit_reached);
}

#[test]
fn sampler_wav_release_drains_into_tail_and_reports_truncation() {
    let mut files = Files::new();
    let mut project = files.project(8_000, 1, &[16_384; 2]);
    project.channels[0].sampler = Some(SamplerConfig {
        loop_start_frame: Some(0),
        loop_end_frame: Some(2),
        release_seconds: 0.125,
        ..config()
    });
    let (pcm, report) = files.render(&project, 250);
    assert_eq!(report.music_frames, 4_000);
    assert_eq!(report.tail_frames, 2_000);
    close(pcm[3_999][0], 0.5);
    close(pcm[4_000][0], 0.5);
    close(pcm[4_500][0], 0.25);
    assert!(pcm[5_000..].iter().all(|frame| *frame == [0.0; 2]));
    assert!(!report.tail_limit_reached);

    let (short, report) = files.render(&project, 50);
    assert_eq!(report.frames, 4_400);
    assert!(short.last().unwrap()[0] > 0.0);
    assert!(report.tail_limit_reached);
}

#[test]
fn sampler_wav_stereo_velocity_clip_channel_and_mixer_gains_apply_once() {
    let mut files = Files::new();
    let mut project = files.project(8_000, 2, &[8_192, 16_384, 8_192, 16_384]);
    project.channels[0].volume = 0.5;
    project.channels[0].pan = 0.25;
    project.clips[0].gain = 0.6;
    project.patterns[0].notes[0].velocity = 0.75;
    let insert = project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == 1)
        .unwrap();
    insert.volume = 0.4;
    insert.pan = -0.5;
    let master = project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    master.volume = 0.8;
    let (pcm, _) = files.render(&project, 0);
    let gain = 0.75 * 0.5 * 0.6 * 0.4 * 0.8;
    close(pcm[0][0], 0.25 * gain * 0.75);
    close(pcm[0][1], 0.5 * gain * 0.5);
    assert!(pcm[2..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn sampler_wav_missing_corrupt_and_changed_rate_fail_without_replacing_destination() {
    let mut files = Files::new();
    let mut project = files.project(8_000, 1, &[8_192; 8]);
    let destination = files.path("protected.wav");
    let original = b"existing destination";
    std::fs::write(&destination, original).unwrap();
    let asset = project.audio_assets.pop().unwrap();
    let error = render_project_wav(&project, &destination, 8_000).unwrap_err();
    assert!(format!("{error:#}").contains("Sampler references missing audio asset 700"));
    assert_eq!(std::fs::read(&destination).unwrap(), original);

    project.audio_assets.push(asset);
    std::fs::write(&project.audio_assets[0].path, b"not a wave file").unwrap();
    let error = render_project_wav(&project, &destination, 8_000).unwrap_err();
    assert!(format!("{error:#}").contains("Unable to prepare sampler audio asset 700"));
    assert_eq!(std::fs::read(&destination).unwrap(), original);

    write_pcm16_wav(&project.audio_assets[0].path, 4_000, 1, &[8_192; 8]);
    let error = render_project_wav(&project, &destination, 8_000).unwrap_err();
    assert!(format!("{error:#}").contains("changed sample rate from 8000 Hz to 4000 Hz"));
    assert_eq!(std::fs::read(&destination).unwrap(), original);
}

#[test]
fn sampler_wav_invalid_trim_loop_and_multichannel_fail_explicitly() {
    let mut files = Files::new();
    let mut project = files.project(8_000, 1, &[8_192; 8]);
    let destination = files.path("invalid.wav");
    for invalid in [
        SamplerConfig {
            start_frame: 8,
            ..config()
        },
        SamplerConfig {
            end_frame: Some(9),
            ..config()
        },
        SamplerConfig {
            loop_end_frame: Some(4),
            ..config()
        },
    ] {
        project.channels[0].sampler = Some(invalid);
        let error = render_project_wav(&project, &destination, 8_000).unwrap_err();
        assert!(format!("{error:#}").contains("Invalid sampler configuration for audio asset 700"));
        assert!(!destination.exists());
    }
    project.channels[0].sampler = Some(config());
    write_pcm16_wav(&project.audio_assets[0].path, 8_000, 3, &[8_192; 9]);
    let error = render_project_wav(&project, &destination, 8_000).unwrap_err();
    assert!(format!("{error:#}").contains("only mono or stereo sources are supported"));
    assert!(!destination.exists());
}

#[test]
fn sampler_wav_respects_gating_and_never_falls_back_when_a_plugin_is_assigned() {
    let mut files = Files::new();
    let mut project = files.project(8_000, 1, &[8_192; 8]);
    project.audio_assets.clear();
    project.channels[0].muted = true;
    let (muted, _) = files.render(&project, 0);
    assert!(muted.iter().all(|frame| *frame == [0.0; 2]));
    project.channels[0].muted = false;
    project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == 1)
        .unwrap()
        .muted = true;
    let (route_muted, _) = files.render(&project, 0);
    assert!(route_muted.iter().all(|frame| *frame == [0.0; 2]));
    project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == 1)
        .unwrap()
        .muted = false;

    project.channels[0].instrument_plugin_instance_id = Some(10);
    project.plugin_instances.push(test_plugin(10));
    let destination = files.path("plugin.wav");
    let error = render_project_wav(&project, &destination, 8_000).unwrap_err();
    assert!(format!("{error:#}").contains("cannot render active instrument plug-in 10"));
    project.plugin_instances[0].bypass = true;
    let (bypassed, _) = files.render(&project, 0);
    assert!(bypassed.iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn empty_sampler_export_is_silent_and_legacy_oscillator_still_plays() {
    let mut files = Files::new();
    let mut project = files.project(8000, 1, &[8192; 64]);
    project.channels[0].sampler = Some(SamplerConfig::empty());
    project.audio_assets.clear();
    let (empty, report) = files.render(&project, 0);
    assert!(empty.iter().all(|frame| *frame == [0.0; 2]));
    assert!(!report.tail_limit_reached);
    project.channels[0].sampler = None;
    let (legacy, _) = files.render(&project, 0);
    assert!(legacy.iter().any(|frame| frame[0].abs() > 0.0001));
}

#[test]
fn empty_sampler_does_not_block_other_channels_or_require_an_asset() {
    let mut files = Files::new();
    let mut project = files.project(8000, 1, &[8192; 64]);
    let (baseline, _) = files.render(&project, 0);
    let mut empty = project.channels[0].clone();
    empty.id = 222;
    empty.sampler = Some(SamplerConfig::empty());
    project.channels.push(empty);
    let mut empty_note = project.patterns[0].notes[0].clone();
    empty_note.id = 222;
    empty_note.channel_id = Some(222);
    project.patterns[0].notes.push(empty_note);
    project.patterns[0].channel_steps.push([false; 16]);
    let (combined, _) = files.render(&project, 0);
    assert_eq!(combined, baseline);
    let map = TempoMap::from_project(&project, 8000).unwrap();
    let timeline = crate::timeline::CompiledTimeline::from_project(
        &project,
        &map,
        crate::timeline::TimelineCompileOptions::default(),
    )
    .unwrap();
    assert_eq!(timeline.sampler_asset_ids().collect::<Vec<_>>(), vec![700]);
}
