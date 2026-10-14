//! Real WAV regressions for authored native Channel Pan and Channel Solo.
//! Both event collectors are exercised without a device, SDK, or UI fixture.

use super::*;
use crate::{export_options::WavLevelPolicy, mixer_graph::MASTER_MIXER_TRACK_ID, model::PianoNote};

static CHANNEL_CONTROL_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct FixtureFiles {
    directory: PathBuf,
    files: Vec<PathBuf>,
}

impl FixtureFiles {
    fn new() -> Self {
        let sequence = CHANNEL_CONTROL_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "citrus-export-channel-controls-{}-{sequence}",
            std::process::id(),
        ));
        std::fs::create_dir(&directory).unwrap();
        Self {
            directory,
            files: Vec::new(),
        }
    }

    fn render(&mut self, project: &Project) -> RenderedWav {
        let path = self
            .directory
            .join(format!("render-{}.wav", self.files.len()));
        self.files.push(path.clone());
        let report = render_project_wav_with_options(
            project,
            &path,
            WavExportOptions {
                sample_rate: 8_000,
                sample_format: WavSampleFormat::Float32,
                level_policy: WavLevelPolicy::PreserveLevel,
                tail_milliseconds: 40,
            },
            &ExportControl::default(),
        )
        .unwrap();
        let decoded = wav::read_wav(&path).unwrap();
        assert_eq!(report.music_frames, 4_000);
        assert_eq!(report.tail_frames, 320);
        assert_eq!(decoded.metadata.frames, 4_320);
        assert_eq!(decoded.metadata.channels, 2);
        assert_eq!(decoded.metadata.sample_rate, 8_000);
        assert_eq!(report.applied_gain, 1.0);
        assert_eq!(decoded.samples.len(), 8_640);
        RenderedWav {
            bytes: std::fs::read(path).unwrap(),
            pcm: decoded.samples.as_chunks::<2>().0.to_vec(),
        }
    }
}

impl Drop for FixtureFiles {
    fn drop(&mut self) {
        // Delete only the individually recorded files created by this fixture.
        // A nonempty directory is retained rather than recursively traversed.
        for path in &self.files {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}

struct RenderedWav {
    bytes: Vec<u8>,
    pcm: Vec<[f32; 2]>,
}

#[derive(Clone, Copy, Debug)]
enum Source {
    Rack,
    Piano,
}

fn two_native_channels(source: Source) -> Project {
    let mut project = Project::blank();
    project.tempo = 60.0;
    project.swing = 0.0;
    project.song_length_beats = 0.5;
    let mut second = project.channels[0].clone();
    second.id = 2;
    second.name = "Second native source".into();
    project.channels.push(second);
    for channel in &mut project.channels {
        channel.volume = 0.6;
        channel.pan = 0.0;
        channel.muted = false;
        channel.solo = false;
    }
    for track in &mut project.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
        track.muted = false;
        track.solo = false;
    }
    let mut pattern = Pattern {
        id: 1,
        name: format!("Channel controls {source:?}"),
        length_steps: 16,
        channel_steps: vec![[false; 16]; 2],
        notes: Vec::new(),
    };
    match source {
        Source::Rack => {
            // The index-dependent native sources differ: Kick / MIDI 36 and
            // Noise / MIDI 39. Preserve channel indices in reference projects.
            pattern.channel_steps[0][0] = true;
            pattern.channel_steps[1][1] = true;
        }
        Source::Piano => {
            for (index, pitch) in [60, 76].into_iter().enumerate() {
                pattern.notes.push(PianoNote {
                    id: index as u64 + 1,
                    channel_id: Some(project.channels[index].id),
                    group_id: None,
                    note: pitch,
                    start: index as f32 * 0.125,
                    length: 0.25,
                    velocity: 0.8,
                    selected: false,
                    muted: false,
                });
            }
        }
    }
    project.patterns = vec![pattern];
    project.piano_notes.clear();
    project.clips.push(Clip {
        id: 1,
        track: 0,
        start: 0.0,
        length: 0.5,
        name: "Native channel control clip".into(),
        color: [200, 130, 60],
        kind: ClipKind::Pattern,
        group_id: None,
        pattern_id: 1,
        automation_id: None,
        audio_asset_id: None,
        source_offset: 0.0,
        audio_source_offset_frame: None,
        audio_source_reference: None,
        audio_length_reference: None,
        fade_in_reference: None,
        fade_out_reference: None,
        gain: 1.0,
        fade_in: 0.0,
        fade_out: 0.0,
        muted: false,
    });
    project
}

fn remove_other_source(project: &mut Project, selected: usize) {
    let selected_id = project.channels[selected].id;
    for (index, steps) in project.patterns[0].channel_steps.iter_mut().enumerate() {
        if index != selected {
            *steps = [false; 16];
        }
    }
    project.patterns[0]
        .notes
        .retain(|note| note.channel_id == Some(selected_id));
    for channel in &mut project.channels {
        channel.solo = false;
    }
}

#[track_caller]
fn assert_hard_pan(pcm: &[[f32; 2]], pan: f32) {
    let silent_side = usize::from(pan > 0.0) ^ 1;
    assert!(pcm.iter().all(|frame| frame[silent_side] == 0.0));
    assert!(pcm.iter().any(|frame| frame[silent_side ^ 1].abs() > 0.001));
}

fn check_solo_and_hard_pan(source: Source) {
    let mut files = FixtureFiles::new();
    for selected in [0, 1] {
        for pan in [-1.0, 1.0] {
            let mut project = two_native_channels(source);
            project.channels[selected].pan = pan;
            project.channels[selected ^ 1].pan = -pan;
            let both = files.render(&project);
            project.channels[selected].solo = true;
            let solo = files.render(&project);
            assert_hard_pan(&solo.pcm, pan);
            assert_ne!(
                both.bytes, solo.bytes,
                "{source:?}: other source must be audible before solo"
            );

            // Remove only the other source events, preserving Rack oscillator
            // indices and using the exact production renderer as the reference.
            let mut reference = project.clone();
            remove_other_source(&mut reference, selected);
            let isolated = files.render(&reference);
            assert_eq!(
                solo.bytes, isolated.bytes,
                "{source:?}: solo must match the isolated source WAV"
            );
            assert_eq!(solo.pcm, isolated.pcm);

            project.channels[selected].muted = true;
            let muted_solo = files.render(&project);
            assert!(
                muted_solo.pcm.iter().all(|frame| *frame == [0.0; 2]),
                "{source:?}: a muted solo channel must not admit the other source"
            );
        }
    }
}

#[test]
fn rack_wav_hard_pan_solo_and_muted_solo_match_isolated_native_source() {
    check_solo_and_hard_pan(Source::Rack);
}

#[test]
fn piano_wav_hard_pan_solo_and_muted_solo_match_isolated_native_source() {
    check_solo_and_hard_pan(Source::Piano);
}

#[test]
fn rack_and_piano_wav_channel_insert_and_master_pan_each_multiply_once() {
    let mut files = FixtureFiles::new();
    for source in [Source::Rack, Source::Piano] {
        let mut reference = two_native_channels(source);
        remove_other_source(&mut reference, 0);
        let centered = files.render(&reference);
        assert!(centered.pcm.iter().any(|frame| frame[0].abs() > 0.001));
        assert!(centered.pcm.iter().all(|frame| frame[0] == frame[1]));
        for sign in [-1.0_f32, 1.0] {
            let mut project = reference.clone();
            project.channels[0].pan = sign * 0.5;
            let destination = project.channels[0].mixer_track;
            project
                .mixer_tracks
                .iter_mut()
                .find(|track| track.id == destination)
                .unwrap()
                .pan = sign * 0.25;
            project
                .mixer_tracks
                .iter_mut()
                .find(|track| track.id == MASTER_MIXER_TRACK_ID)
                .unwrap()
                .pan = -sign * 0.4;
            let mixed = files.render(&project);
            // These expected gains are independent of the production pan helper:
            // .5 * .75 on one side and the opposite Master attenuation .6.
            let gains = if sign > 0.0 {
                [0.375, 0.6]
            } else {
                [0.6, 0.375]
            };
            for (frame, (actual, original)) in mixed.pcm.iter().zip(&centered.pcm).enumerate() {
                for side in [0, 1] {
                    let expected = original[side] * gains[side];
                    assert!(
                        (actual[side] - expected).abs() <= 3.0e-7,
                        "{source:?}, frame {frame}, side {side}: {} vs {expected}",
                        actual[side]
                    );
                }
            }
        }
    }
}
