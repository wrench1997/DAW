//! Final WAV regressions. Register from export.rs as a sibling test module;
//! these fixtures use no audio device, SDK plug-in, or native UI.

use super::*;
use crate::{
    automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
    export_options::{WavExportReport, WavLevelPolicy},
    mixer_graph::MASTER_MIXER_TRACK_ID,
    model::{
        AudioClipMixerDestination, PianoNote, PluginFormat, PluginInstance, PluginRole,
        PluginRuntimeStatus, ProjectAutomation,
    },
};
use std::collections::BTreeMap;

static WORKFLOW_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);
const WORKFLOW_SAMPLE_RATE: u32 = 8_000;

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn new(label: &str) -> Self {
        let sequence = WORKFLOW_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "citrus-export-workflow-{label}-{}-{sequence}",
            std::process::id(),
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        // Resolve and bound the recursive cleanup to this newly created test
        // directory, never an arbitrary destination or the user's project.
        let (Ok(path), Ok(temp)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize())
        else {
            return;
        };
        if path.starts_with(&temp)
            && path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("citrus-export-workflow-")
            })
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

fn options(tail_milliseconds: u32) -> WavExportOptions {
    WavExportOptions {
        sample_rate: WORKFLOW_SAMPLE_RATE,
        sample_format: WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds,
    }
}

fn blank(beats: f32) -> Project {
    let mut project = Project::blank();
    project.tempo = 60.0;
    project.swing = 0.0;
    project.song_length_beats = beats;
    project.channels[0].volume = 1.0;
    for track in &mut project.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
        track.muted = false;
        track.solo = false;
    }
    project
}

fn clip(id: u32, kind: ClipKind, start: f32, length: f32) -> Clip {
    Clip {
        id,
        kind,
        start,
        length,
        track: 0,
        name: format!("workflow clip {id}"),
        color: [180, 120, 40],
        group_id: None,
        pattern_length_policy: None,
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
    }
}

fn audio_project(directory: &FixtureDirectory) -> Project {
    let mut project = blank(0.5);
    let source = directory.path("constant-source.wav");
    let pcm = vec![
        StereoFrame {
            left: 0.3,
            right: 0.2
        };
        4_000
    ];
    write_stereo_wav_controlled(&source, options(0), &pcm, 1.0, &ExportControl::default()).unwrap();
    project.audio_assets.push(AudioAsset {
        id: 700,
        name: "known float source".into(),
        path: source,
        sample_rate: WORKFLOW_SAMPLE_RATE,
        channels: 2,
        bits_per_sample: 32,
        frames: 4_000,
        waveform_peaks: Vec::new(),
    });
    let mut audio = clip(70, ClipKind::Audio, 0.0, 0.5);
    audio.audio_asset_id = Some(700);
    audio.audio_source_offset_frame = Some(0);
    project.clips.push(audio);
    project
        .audio_clip_mixer_destinations
        .push(AudioClipMixerDestination {
            clip_id: 70,
            mixer_track_id: project.mixer_track_id_at_runtime_slot(1).unwrap(),
        });
    project
}

fn add_lane(
    project: &mut Project,
    id: u64,
    target: AutomationTarget,
    curve: AutomationCurve,
    points: &[(f64, f64)],
) {
    let mut lane = AutomationLane::new(target);
    lane.set_curve(curve);
    lane.replace_points(
        points
            .iter()
            .map(|&(beat, value)| AutomationPoint::new(beat, value)),
    );
    project.automation_lanes.push(ProjectAutomation {
        id,
        name: format!("master workflow {id}"),
        lane,
    });
}

fn add_placement(
    project: &mut Project,
    id: u32,
    automation_id: u64,
    start: f32,
    length: f32,
    slip: f32,
) {
    let mut placement = clip(id, ClipKind::Automation, start, length);
    placement.automation_id = Some(automation_id);
    placement.source_offset = slip;
    project.clips.push(placement);
}

fn decoded_render(project: &Project, path: &Path, tail: u32) -> (Vec<[f32; 2]>, WavExportReport) {
    let report =
        render_project_wav_with_options(project, path, options(tail), &ExportControl::default())
            .unwrap();
    let decoded = wav::read_wav(path).unwrap();
    assert_eq!(decoded.metadata.sample_rate, WORKFLOW_SAMPLE_RATE);
    assert_eq!(decoded.metadata.channels, 2);
    assert_eq!(decoded.metadata.frames, report.frames as u64);
    assert_eq!(report.frames, report.music_frames + report.tail_frames);
    assert_eq!(report.applied_gain, 1.0);
    let pcm = decoded.samples.as_chunks::<2>().0.to_vec();
    (pcm, report)
}

#[track_caller]
fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= 1.0e-6,
        "WAV sample {actual}, expected {expected}"
    );
}

#[test]
fn workflow_wav_master_fade_pan_match_every_sample_of_unity_audio_reference() {
    let directory = FixtureDirectory::new("global-master");
    let project = audio_project(&directory);
    let (reference, _) = decoded_render(&project, &directory.path("unity.wav"), 0);
    assert!(
        reference
            .iter()
            .all(|frame| frame[0] > 0.29 && frame[1] > 0.19)
    );
    let mut automated = project.clone();
    let master = automated
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    master.volume = 0.37;
    master.pan = 0.4;
    add_lane(
        &mut automated,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 1.0), (0.5, 0.0)],
    );
    add_lane(
        &mut automated,
        2,
        AutomationTarget::MasterPan,
        AutomationCurve::Linear,
        &[(0.0, -1.0), (0.5, 1.0)],
    );
    let (actual, report) = decoded_render(&automated, &directory.path("automated.wav"), 0);
    assert_eq!(report.music_frames, 4_000);
    assert_eq!(report.tail_frames, 0);
    for (frame, (actual, source)) in actual.iter().zip(reference).enumerate() {
        let gain = 1.0 - frame as f32 / 4_000.0;
        let pan = -1.0 + frame as f32 / 2_000.0;
        close(actual[0], source[0] * gain * (1.0 - pan.max(0.0)));
        close(actual[1], source[1] * gain * (1.0 + pan.min(0.0)));
    }
}

#[test]
fn workflow_wav_slip_overlap_hold_and_lane_end_restore_authored_master_base() {
    let directory = FixtureDirectory::new("placed-master");
    let mut project = audio_project(&directory);
    project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .pan = 0.5;
    let (reference, _) = decoded_render(&project, &directory.path("reference.wav"), 0);
    project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .volume = 0.25;
    add_lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 0.0), (0.5, 1.0)],
    );
    add_lane(
        &mut project,
        2,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.9)],
    );
    add_placement(&mut project, 1, 1, 0.125, 0.25, 0.25);
    add_placement(&mut project, 2, 2, 0.25, 0.0625, 0.0);
    let (actual, _) = decoded_render(&project, &directory.path("slipped.wav"), 0);
    for (frame, (actual, reference)) in actual.iter().zip(&reference).enumerate() {
        let gain = match frame {
            0..1_000 | 3_000.. => 0.25,
            2_000..2_500 => 0.9,
            _ => 0.5 + (frame - 1_000) as f32 / 4_000.0,
        };
        close(actual[0], reference[0] * gain);
        close(actual[1], reference[1] * gain);
    }
    project.automation_lanes.truncate(1);
    project.automation_lanes[0]
        .lane
        .set_curve(AutomationCurve::Hold);
    project.automation_lanes[0]
        .lane
        .replace_points([AutomationPoint::new(0.0, 0.8)]);
    project
        .clips
        .retain(|clip| clip.kind != ClipKind::Automation);
    add_placement(&mut project, 1, 1, 0.0625, 0.0625, 0.0);
    project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .volume = 0.0;
    let (zero_base, _) = decoded_render(&project, &directory.path("zero-base.wav"), 0);
    assert!(zero_base[..500].iter().all(|frame| *frame == [0.0; 2]));
    close(zero_base[500][0], reference[500][0] * 0.8);
    assert!(zero_base[1_000..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn workflow_wav_master_aliases_disabled_muted_and_empty_lanes_keep_exact_static_sound() {
    let directory = FixtureDirectory::new("aliases-gates");
    let mut project = audio_project(&directory);
    add_lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Hold,
        &[(0.0, 0.8), (0.25, 0.2)],
    );
    add_lane(
        &mut project,
        2,
        AutomationTarget::MasterPan,
        AutomationCurve::Hold,
        &[(0.0, 0.0), (0.25, 1.0)],
    );
    decoded_render(&project, &directory.path("canonical.wav"), 0);
    project.automation_lanes[0]
        .lane
        .set_target(AutomationTarget::MixerVolume {
            track: MASTER_MIXER_TRACK_ID,
        });
    project.automation_lanes[1]
        .lane
        .set_target(AutomationTarget::MixerPan {
            track: MASTER_MIXER_TRACK_ID,
        });
    decoded_render(&project, &directory.path("alias.wav"), 0);
    assert_eq!(
        std::fs::read(directory.path("alias.wav")).unwrap(),
        std::fs::read(directory.path("canonical.wav")).unwrap()
    );
    let mut static_project = project.clone();
    static_project.automation_lanes.clear();
    let master = static_project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    master.volume = 0.3;
    master.pan = -0.4;
    decoded_render(&static_project, &directory.path("static.wav"), 0);
    project.mixer_tracks = static_project.mixer_tracks.clone();
    project.automation_lanes[0].lane.set_enabled(false);
    add_placement(&mut project, 1, 2, 0.0, 0.5, 0.0);
    project.clips.last_mut().unwrap().muted = true;
    decoded_render(&project, &directory.path("gated.wav"), 0);
    assert_eq!(
        std::fs::read(directory.path("gated.wav")).unwrap(),
        std::fs::read(directory.path("static.wav")).unwrap()
    );
    project.automation_lanes[0].lane.set_enabled(true);
    project.automation_lanes[0].lane.replace_points([]);
    decoded_render(&project, &directory.path("empty.wav"), 0);
    assert_eq!(
        std::fs::read(directory.path("empty.wav")).unwrap(),
        std::fs::read(directory.path("static.wav")).unwrap()
    );
}

#[test]
fn workflow_saved_and_cold_loaded_master_automation_produces_identical_wav_bytes() {
    let directory = FixtureDirectory::new("save-cold");
    let mut project = audio_project(&directory);
    // Persisted projects have a sixteen-beat minimum after normal loading.
    // Keep the local audio/control window short inside that legal extent.
    project.song_length_beats = 16.0;
    add_lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 0.8), (0.5, 0.2)],
    );
    add_lane(
        &mut project,
        2,
        AutomationTarget::MasterPan,
        AutomationCurve::Hold,
        &[(0.0, -0.5), (0.25, 0.5)],
    );
    add_placement(&mut project, 1, 1, 0.125, 0.25, 0.125);
    assert_eq!(project.tempo, 60.0);
    assert_eq!(project.song_length_beats, 16.0);
    let placements = |project: &Project| {
        project
            .clips
            .iter()
            .map(|clip| {
                (
                    clip.id,
                    clip.kind,
                    clip.start,
                    clip.length,
                    clip.source_offset,
                    clip.audio_source_offset_frame,
                    clip.automation_id,
                    clip.audio_asset_id,
                    clip.gain,
                    clip.muted,
                )
            })
            .collect::<Vec<_>>()
    };
    let expected_placements = placements(&project);
    let expected_automation = serde_json::to_value(&project.automation_lanes).unwrap();
    let expected_audio_routes =
        serde_json::to_value(&project.audio_clip_mixer_destinations).unwrap();
    project.save(&directory.path("own-test.citrus")).unwrap();
    let (before_pcm, before_report) = decoded_render(&project, &directory.path("before.wav"), 37);
    assert_eq!(before_report.music_frames, 128_000);
    assert_eq!(before_report.tail_frames, 296);
    assert_eq!(before_report.frames, 128_296);
    assert!(before_pcm[..4_000].iter().any(|frame| *frame != [0.0; 2]));
    assert!(before_pcm[4_000..].iter().all(|frame| *frame == [0.0; 2]));
    drop(project);
    let cold = Project::load(&directory.path("own-test.citrus")).unwrap();
    assert_eq!(cold.tempo, 60.0);
    assert_eq!(cold.song_length_beats, 16.0);
    assert_eq!(placements(&cold), expected_placements);
    assert_eq!(
        serde_json::to_value(&cold.automation_lanes).unwrap(),
        expected_automation
    );
    assert_eq!(
        serde_json::to_value(&cold.audio_clip_mixer_destinations).unwrap(),
        expected_audio_routes
    );
    let (_, cold_report) = decoded_render(&cold, &directory.path("cold.wav"), 37);
    assert_eq!(cold_report.music_frames, 128_000);
    assert_eq!(cold_report.tail_frames, 296);
    assert_eq!(cold_report.frames, before_report.frames);
    let before = std::fs::read(directory.path("before.wav")).unwrap();
    let cold = std::fs::read(directory.path("cold.wav")).unwrap();
    let first_difference = before
        .iter()
        .zip(&cold)
        .position(|(before, cold)| before != cold)
        .or_else(|| (before.len() != cold.len()).then_some(before.len().min(cold.len())));
    assert!(
        before == cold,
        "saved/cold WAV differs: before {} bytes, cold {} bytes, first difference {first_difference:?}",
        before.len(),
        cold.len()
    );
}

fn note(id: u64, start: f32, length: f32, pitch: u8) -> PianoNote {
    PianoNote {
        id,
        channel_id: Some(1),
        group_id: None,
        note: pitch,
        start,
        length,
        velocity: 1.0,
        selected: false,
        muted: false,
    }
}

fn native_tail_project() -> Project {
    let mut project = blank(0.03125);
    project.patterns[0].notes = vec![note(1, 0.015625, 0.015625, 69)];
    project
        .clips
        .push(clip(1, ClipKind::Pattern, 0.0, project.song_length_beats));
    project
}

#[test]
fn workflow_fixed_tail_encodes_real_native_release_and_reports_short_limit_without_new_notes() {
    let directory = FixtureDirectory::new("native-tail");
    let project = native_tail_project();
    let (_, legacy) = decoded_render(&project, &directory.path("legacy.wav"), 0);
    assert_eq!(legacy.music_frames, 250);
    assert_eq!(legacy.tail_frames, 0);
    let (short, short_report) = decoded_render(&project, &directory.path("short.wav"), 1);
    assert_eq!(short_report.frames, 258);
    assert_eq!(short_report.tail_frames, 8);
    assert!(
        short[250..]
            .iter()
            .any(|frame| frame[0] != 0.0 || frame[1] != 0.0)
    );
    assert!(short_report.tail_limit_reached);
    let summary = short_report.level_summary().to_ascii_lowercase();
    assert!(summary.contains("tail") && summary.contains("limit"));
    let (long, long_report) = decoded_render(&project, &directory.path("long.wav"), 100);
    assert_eq!(long_report.frames, 1_050);
    assert_eq!(long_report.tail_frames, 800);
    assert!(!long_report.tail_limit_reached);
    assert!(
        long[250..]
            .iter()
            .any(|frame| frame[0] != 0.0 || frame[1] != 0.0)
    );
    assert!(long[1_000..].iter().all(|frame| *frame == [0.0; 2]));
    let mut boundary = project.clone();
    boundary.patterns[0].notes.push(note(2, 0.03125, 0.125, 96));
    // The beat is inside by < half a sample, but its quantized frame is the
    // exclusive music end; it must not become a new voice in the fixed tail.
    boundary.patterns[0]
        .notes
        .push(note(3, 0.031249, 0.125, 108));
    decoded_render(&boundary, &directory.path("boundary.wav"), 100);
    assert_eq!(
        std::fs::read(directory.path("boundary.wav")).unwrap(),
        std::fs::read(directory.path("long.wav")).unwrap()
    );
}

#[test]
fn workflow_native_tail_freezes_last_music_master_controls_without_loop_or_base_revival() {
    let directory = FixtureDirectory::new("native-control-tail");
    let project = native_tail_project();
    let (reference, _) = decoded_render(&project, &directory.path("reference.wav"), 100);
    let mut automated = project.clone();
    let master = automated
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap();
    master.volume = 0.33;
    master.pan = 0.4;
    add_lane(
        &mut automated,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 1.0), (0.03125, 0.25)],
    );
    add_lane(
        &mut automated,
        2,
        AutomationTarget::MasterPan,
        AutomationCurve::Linear,
        &[(0.0, -0.5), (0.03125, 0.5)],
    );
    let (actual, report) = decoded_render(&automated, &directory.path("controlled.wav"), 100);
    assert_eq!(report.music_frames, 250);
    assert!(reference[250..].iter().any(|frame| *frame != [0.0; 2]));
    for (frame, (actual, source)) in actual.iter().zip(&reference).enumerate() {
        let control_frame = frame.min(249) as f32;
        let volume = 1.0 - 0.75 * control_frame / 250.0;
        let pan = -0.5 + control_frame / 250.0;
        close(actual[0], source[0] * volume * (1.0 - pan.max(0.0)));
        close(actual[1], source[1] * volume * (1.0 + pan.min(0.0)));
    }
}

fn assigned_plugin(enabled: bool, bypass: bool) -> PluginInstance {
    PluginInstance {
        id: 901,
        midi_ports: crate::plugin_midi_routing::PluginMidiPorts::default(),
        format: PluginFormat::Vst3,
        role: PluginRole::Instrument,
        path: PathBuf::from("not-loaded-by-this-offline-test.vst3"),
        uid: "workflow retained generator".into(),
        vendor: "test".into(),
        name: "retained source generator".into(),
        enabled,
        bypass,
        wet: 1.0,
        parameters: BTreeMap::new(),
        opaque_state: Vec::new(),
        runtime_status: PluginRuntimeStatus::Unloaded,
    }
}

#[test]
fn workflow_retained_disabled_or_bypassed_generator_cannot_export_native_substitute() {
    let directory = FixtureDirectory::new("retained-generator");
    let mut native = native_tail_project();
    native.patterns[0].channel_steps[0][0] = true;
    let (reference, _) = decoded_render(&native, &directory.path("native.wav"), 20);
    assert!(reference.iter().any(|frame| frame[0].abs() > 0.01));
    for (enabled, bypass) in [(false, false), (true, true)] {
        let mut retained = native.clone();
        retained
            .plugin_instances
            .push(assigned_plugin(enabled, bypass));
        retained.channels[0].instrument_plugin_instance_id = Some(901);
        let (silent, _) = decoded_render(&retained, &directory.path("silent.wav"), 20);
        assert!(
            silent.iter().all(|frame| *frame == [0.0; 2]),
            "assigned retained source cannot become native PCM"
        );

        let audio = audio_project(&directory);
        decoded_render(&audio, &directory.path("audio-reference.wav"), 0);
        let mut converted = audio.clone();
        converted.patterns[0].notes.push(note(1, 0.0, 0.125, 69));
        converted.patterns[0].channel_steps[0][0] = true;
        converted
            .plugin_instances
            .push(assigned_plugin(enabled, bypass));
        converted.channels[0].instrument_plugin_instance_id = Some(901);
        converted.clips.push(clip(1, ClipKind::Pattern, 0.0, 0.5));
        for muted in [false, true] {
            converted.clips.last_mut().unwrap().muted = muted;
            decoded_render(&converted, &directory.path("converted.wav"), 0);
            assert_eq!(
                std::fs::read(directory.path("converted.wav")).unwrap(),
                std::fs::read(directory.path("audio-reference.wav")).unwrap(),
                "converted audio survives while the retained MIDI source stays silent"
            );
        }
    }
}

#[test]
fn workflow_master_tail_cancellation_invalid_options_and_unsupported_processing_preserve_destination()
 {
    let directory = FixtureDirectory::new("atomic-errors");
    let mut project = audio_project(&directory);
    add_lane(
        &mut project,
        1,
        AutomationTarget::MasterVolume,
        AutomationCurve::Linear,
        &[(0.0, 1.0), (0.5, 0.2)],
    );
    let path = directory.path("existing.wav");
    let marker = b"original recoverable destination";
    for threshold in [6_500, 6_800, 9_800, 9_900] {
        std::fs::write(&path, marker).unwrap();
        let control = ExportControl::default();
        control.cancel_at(threshold);
        let error =
            render_project_wav_with_options(&project, &path, options(500), &control).unwrap_err();
        assert!(
            error
                .downcast_ref::<crate::export_job::ExportCancelled>()
                .is_some(),
            "{threshold}: {error:#}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), marker);
        assert_eq!(
            std::fs::read_dir(&directory.0).unwrap().count(),
            2,
            "staging must be cleaned"
        );
    }
    for invalid in [30_001, u32::MAX] {
        assert!(
            render_project_wav_with_options(
                &project,
                &path,
                options(invalid),
                &ExportControl::default()
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), marker);
    }
    let mut overflow = project.clone();
    overflow.song_length_beats = f32::MAX;
    assert!(
        render_project_wav_with_options(
            &overflow,
            &path,
            options(30_000),
            &ExportControl::default()
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), marker);
    let mut unsupported = project.clone();
    add_lane(
        &mut unsupported,
        2,
        AutomationTarget::ChannelVolume { channel: 1 },
        AutomationCurve::Hold,
        &[(0.0, 0.3)],
    );
    assert!(
        render_project_wav_with_options(
            &unsupported,
            &path,
            options(50),
            &ExportControl::default()
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), marker);
    let mut active = project.clone();
    active.channels[0].instrument_plugin_instance_id = Some(901);
    active.plugin_instances.push(assigned_plugin(true, false));
    let error =
        render_project_wav_with_options(&active, &path, options(50), &ExportControl::default())
            .unwrap_err();
    assert!(format!("{error:#}").contains("active instrument"));
    assert_eq!(std::fs::read(&path).unwrap(), marker);
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
}
