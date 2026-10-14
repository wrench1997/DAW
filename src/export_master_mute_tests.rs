//! Master Mute regressions inspect published WAV files using the existing
//! export fixtures. No audio device, plug-in SDK, or desktop is involved.

use super::tests::{
    arrangement_project, audio_project, test_plugin, workflow_test_directory, write_pcm16_wav,
};
use super::*;
use crate::{
    automation::{AutomationCurve, AutomationLane, AutomationPoint},
    export_options::WavLevelPolicy,
    mixer_graph::{MixerRoute as ProjectMixerRoute, MixerRouteDestination},
    model::{MixerInsertSlotRef, PianoNote, ProjectAutomation},
};

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    fn new(label: &str) -> Self {
        Self(workflow_test_directory(&format!("master-mute-{label}")))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let (Ok(path), Ok(temp)) = (self.0.canonicalize(), std::env::temp_dir().canonicalize())
        else {
            return;
        };
        if path.starts_with(temp)
            && path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("citrus-export-master-mute-")
            })
        {
            let _ = std::fs::remove_dir_all(path);
        }
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

fn native_project(beats: f32) -> Project {
    let mut project = arrangement_project();
    project.tempo = 60.0;
    project.swing = 0.0;
    project.song_length_beats = beats;
    project.clips[0].start = 0.0;
    project.clips[0].length = beats;
    project.patterns[0].channel_steps[0] = [false; 16];
    project.patterns[0].notes = vec![PianoNote {
        id: 11,
        channel_id: Some(project.channels[0].id),
        group_id: None,
        note: 69,
        start: 0.0,
        length: beats,
        velocity: 1.0,
        selected: false,
        muted: false,
    }];
    project
}

fn constant_audio_project(directory: &FixtureDirectory) -> Project {
    let source = directory.path("source.wav");
    write_pcm16_wav(&source, 8_000, 1, &vec![16_384; 4_000]);
    let mut project = audio_project(&source, 8_000, 1, 4_000);
    project.song_length_beats = 0.5;
    project.swing = 0.0;
    project.clips[0].length = 0.5;
    project
}

fn set_master_muted(project: &mut Project, muted: bool) {
    project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == MASTER_MIXER_TRACK_ID)
        .unwrap()
        .muted = muted;
}

fn add_lane(project: &mut Project, id: u64, curve: AutomationCurve, points: &[(f64, f64)]) {
    let mut lane = AutomationLane::new(AutomationTarget::MixerMute {
        track: MASTER_MIXER_TRACK_ID,
    });
    lane.set_curve(curve);
    lane.replace_points(
        points
            .iter()
            .map(|&(beat, value)| AutomationPoint::new(beat, value)),
    );
    project.automation_lanes.push(ProjectAutomation {
        id,
        name: format!("Master Mute {id}"),
        lane,
    });
}

fn add_placement(project: &mut Project, id: u32, lane_id: u64, start: f32, length: f32, slip: f32) {
    let mut placement = project.clips[0].clone();
    placement.id = id;
    placement.kind = ClipKind::Automation;
    placement.automation_id = Some(lane_id);
    placement.audio_asset_id = None;
    placement.audio_source_offset_frame = None;
    placement.start = start;
    placement.length = length;
    placement.source_offset = slip;
    project.clips.push(placement);
}

fn render(project: &Project, path: &Path, tail: u32) -> (Vec<[f32; 2]>, WavExportReport) {
    let report =
        render_project_wav_with_options(project, path, options(tail), &ExportControl::default())
            .unwrap();
    let decoded = wav::read_wav(path).unwrap();
    assert_eq!(decoded.metadata.sample_rate, 8_000);
    assert_eq!(decoded.metadata.channels, 2);
    assert_eq!(decoded.metadata.frames, report.frames as u64);
    assert_eq!(report.frames, report.music_frames + report.tail_frames);
    (decoded.samples.as_chunks::<2>().0.to_vec(), report)
}

#[test]
fn final_wav_initial_master_mute_can_unmute_retained_native_notes_and_audio_assets() {
    let directory = FixtureDirectory::new("retained-sources");
    for (name, project) in [
        ("native", native_project(0.5)),
        ("asset", constant_audio_project(&directory)),
    ] {
        let (reference, _) = render(
            &project,
            &directory.path(&format!("{name}-reference.wav")),
            0,
        );
        assert!(
            reference[1_000..3_000]
                .iter()
                .any(|frame| frame[0].abs() > 0.001)
        );
        let mut automated = project.clone();
        set_master_muted(&mut automated, true);
        add_lane(&mut automated, 101, AutomationCurve::Hold, &[(0.0, 0.0)]);
        add_placement(&mut automated, 101, 101, 0.125, 0.25, 0.0);
        ensure_supported_project(&automated).unwrap();
        let (actual, report) = render(
            &automated,
            &directory.path(&format!("{name}-automated.wav")),
            0,
        );
        assert_eq!(report.music_frames, 4_000);
        for (index, (actual, source)) in actual.iter().zip(reference).enumerate() {
            assert_eq!(
                *actual,
                if (1_000..3_000).contains(&index) {
                    source
                } else {
                    [0.0; 2]
                },
                "{name} output at frame {index}"
            );
        }
        assert!(
            automated
                .mixer_track_by_id(MASTER_MIXER_TRACK_ID)
                .unwrap()
                .muted
        );
    }
}

#[test]
fn final_wav_master_mute_threshold_and_manual_mute_preserve_exact_gate_semantics() {
    let directory = FixtureDirectory::new("threshold-static");
    let mut project = constant_audio_project(&directory);
    let (reference, _) = render(&project, &directory.path("reference.wav"), 0);
    add_lane(
        &mut project,
        101,
        AutomationCurve::Hold,
        &[(0.0, 0.499), (0.125, 0.5), (0.25, 0.501), (0.375, 0.0)],
    );
    let (actual, _) = render(&project, &directory.path("threshold.wav"), 0);
    for (index, (actual, source)) in actual.iter().zip(reference).enumerate() {
        assert_eq!(
            *actual,
            if (1_000..3_000).contains(&index) {
                [0.0; 2]
            } else {
                source
            }
        );
    }
    set_master_muted(&mut project, true);
    for inactive in 0..4 {
        let mut static_project = project.clone();
        match inactive {
            0 => static_project.automation_lanes.clear(),
            1 => static_project.automation_lanes[0].lane.set_enabled(false),
            2 => {
                static_project.automation_lanes[0].lane.replace_points([]);
            }
            _ => {
                add_placement(&mut static_project, 101, 101, 0.0, 0.5, 0.0);
                static_project.clips.last_mut().unwrap().muted = true;
            }
        }
        let (silent, report) = render(
            &static_project,
            &directory.path(&format!("static-{inactive}.wav")),
            0,
        );
        assert!(silent.iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(report.sample_peak, 0.0);
    }
}

#[test]
fn final_wav_master_unmute_keeps_nonmaster_track_mute_and_solo_gates() {
    let directory = FixtureDirectory::new("nonmaster-gates");
    let mut project = native_project(0.5);
    set_master_muted(&mut project, true);
    add_lane(&mut project, 101, AutomationCurve::Hold, &[(0.0, 0.0)]);
    let (reference, _) = render(&project, &directory.path("reference.wav"), 0);
    assert!(reference.iter().any(|frame| frame[0].abs() > 0.01));
    for gate in 0..3 {
        let mut gated = project.clone();
        match gate {
            0 => gated.mixer_tracks[1].muted = true,
            1 => gated.mixer_tracks[2].solo = true,
            _ => gated.channels[0].muted = true,
        }
        let (silent, _) = render(&gated, &directory.path(&format!("gate-{gate}.wav")), 0);
        assert!(silent.iter().all(|frame| *frame == [0.0; 2]));
    }
    for solo_slot in [0, 1] {
        let mut soloed = project.clone();
        soloed.mixer_tracks[solo_slot].solo = true;
        let (actual, _) = render(
            &soloed,
            &directory.path(&format!("solo-{solo_slot}.wav")),
            0,
        );
        assert_eq!(actual, reference);
    }
}

#[test]
fn final_wav_native_tail_holds_last_master_mute_without_base_revival_or_new_events() {
    let directory = FixtureDirectory::new("native-tail");
    let mut source = native_project(0.03125);
    source.patterns[0].notes[0].start = 0.015625;
    source.patterns[0].notes[0].length = 0.015625;
    let (reference, reference_report) = render(&source, &directory.path("reference.wav"), 100);
    assert_eq!(reference_report.music_frames, 250);
    assert_eq!(reference_report.tail_frames, 800);
    assert!(reference[250..].iter().any(|frame| *frame != [0.0; 2]));
    for final_muted in [false, true] {
        let mut project = source.clone();
        set_master_muted(&mut project, !final_muted);
        add_lane(
            &mut project,
            101,
            AutomationCurve::Hold,
            &[(0.0, 0.0), (0.0234375, f64::from(u8::from(final_muted)))],
        );
        add_placement(&mut project, 101, 101, 0.0, 0.03125, 0.0);
        // These events fall exactly at the exclusive musical end. Neither
        // the mute point nor a new note may be admitted into the fixed tail.
        project.automation_lanes[0]
            .lane
            .insert_point(AutomationPoint::new(
                0.03125,
                f64::from(u8::from(!final_muted)),
            ))
            .unwrap();
        let mut end_note = project.patterns[0].notes[0].clone();
        end_note.id = 12;
        end_note.start = 0.03125;
        end_note.note = 96;
        project.patterns[0].notes.push(end_note);
        let (actual, report) = render(
            &project,
            &directory.path(&format!("tail-{final_muted}.wav")),
            100,
        );
        assert_eq!(report.frames, reference_report.frames);
        assert_eq!(
            report.tail_limit_reached,
            reference_report.tail_limit_reached
        );
        for (index, (actual, source)) in actual.iter().zip(&reference).enumerate() {
            // .0234375 beat rounds to output frame 188 at this rate.
            assert_eq!(
                *actual,
                if final_muted && index >= 188 {
                    [0.0; 2]
                } else {
                    *source
                }
            );
        }
    }
}

#[test]
fn final_wav_saved_cold_master_mute_keeps_stable_target_base_placements_and_bytes() {
    let directory = FixtureDirectory::new("save-cold");
    let mut project = constant_audio_project(&directory);
    project.song_length_beats = 16.0;
    set_master_muted(&mut project, true);
    add_lane(
        &mut project,
        101,
        AutomationCurve::Hold,
        &[(0.0, 1.0), (0.125, 0.0), (0.25, 1.0)],
    );
    add_lane(&mut project, 102, AutomationCurve::Hold, &[(0.0, 0.0)]);
    add_lane(
        &mut project,
        103,
        AutomationCurve::Linear,
        &[(0.0, 0.499), (0.5, 0.5)],
    );
    project
        .automation_lanes
        .last_mut()
        .unwrap()
        .lane
        .set_enabled(false);
    add_placement(&mut project, 101, 101, 0.125, 0.25, 0.125);
    add_placement(&mut project, 102, 102, 0.25, 0.0625, 0.0);
    add_placement(&mut project, 103, 102, 0.0, 0.125, 0.0);
    project.clips.last_mut().unwrap().muted = true;
    let expected_lanes = serde_json::to_value(&project.automation_lanes).unwrap();
    let expected_clips = serde_json::to_value(&project.clips).unwrap();
    let expected_routes = serde_json::to_value(&project.audio_clip_mixer_destinations).unwrap();
    let saved = directory.path("own-test.citrus");
    project.save(&saved).unwrap();
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    assert_eq!(
        persisted["automation_lanes"][0]["lane"]["target"]["kind"],
        "mixer_mute"
    );
    assert_eq!(
        persisted["automation_lanes"][0]["lane"]["target"]["mixer_track_id"],
        serde_json::json!(0xffff_ffff_u32)
    );
    let (before, before_report) = render(&project, &directory.path("before.wav"), 37);
    assert!(before[1_000..2_500].iter().all(|frame| *frame != [0.0; 2]));
    assert!(
        before[..1_000]
            .iter()
            .chain(&before[2_500..])
            .all(|frame| *frame == [0.0; 2])
    );
    drop(project);
    let cold = Project::load(&saved).unwrap();
    assert_eq!(cold.tempo, 60.0);
    assert_eq!(cold.song_length_beats, 16.0);
    assert!(cold.mixer_track_by_id(MASTER_MIXER_TRACK_ID).unwrap().muted);
    assert_eq!(
        serde_json::to_value(&cold.automation_lanes).unwrap(),
        expected_lanes
    );
    assert_eq!(serde_json::to_value(&cold.clips).unwrap(), expected_clips);
    assert_eq!(
        serde_json::to_value(&cold.audio_clip_mixer_destinations).unwrap(),
        expected_routes
    );
    let (after, after_report) = render(&cold, &directory.path("cold.wav"), 37);
    assert_eq!(after_report, before_report);
    assert_eq!(after, before);
    assert_eq!(
        std::fs::read(directory.path("cold.wav")).unwrap(),
        std::fs::read(directory.path("before.wav")).unwrap()
    );
    // Saving onto a directory must fail and leave existing user bytes intact.
    let blocked = directory.path("save-blocked.citrus");
    std::fs::create_dir(&blocked).unwrap();
    let marker = blocked.join("keep");
    std::fs::write(&marker, b"existing user bytes").unwrap();
    let error = cold.save(&blocked).unwrap_err();
    assert!(format!("{error:#}").contains("Unable to atomically commit"));
    assert_eq!(std::fs::read(marker).unwrap(), b"existing user bytes");
    assert!(std::fs::read_dir(&directory.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".citrus-save-")
    }));
}

#[test]
fn final_wav_master_mute_is_measured_before_level_policy_and_actual_file_limits() {
    let directory = FixtureDirectory::new("level-policy");
    let mut project = constant_audio_project(&directory);
    project.clips[0].gain = 4.0;
    add_lane(
        &mut project,
        101,
        AutomationCurve::Hold,
        &[(0.0, 1.0), (0.25, 0.0)],
    );
    for sample_format in WavSampleFormat::ALL {
        let attenuated_path = directory.path(&format!("attenuated-{sample_format:?}.wav"));
        let opts = WavExportOptions {
            sample_format,
            level_policy: WavLevelPolicy::AttenuatePeaks,
            ..options(0)
        };
        let report = render_project_wav_with_options(
            &project,
            &attenuated_path,
            opts,
            &ExportControl::default(),
        )
        .unwrap();
        assert_eq!(report.sample_peak, 2.0);
        assert_eq!(report.applied_gain, 0.475);
        let decoded = wav::read_wav(&attenuated_path).unwrap();
        assert!(decoded.samples[..4_000].iter().all(|value| *value == 0.0));
        assert!(
            decoded.samples[4_000..]
                .iter()
                .all(|value| (*value - 0.95).abs() < 4.0e-5)
        );
        let preserved_path = directory.path(&format!("preserved-{sample_format:?}.wav"));
        std::fs::write(&preserved_path, b"original destination").unwrap();
        let preserve = WavExportOptions {
            level_policy: WavLevelPolicy::PreserveLevel,
            ..opts
        };
        if sample_format == WavSampleFormat::Float32 {
            let report = render_project_wav_with_options(
                &project,
                &preserved_path,
                preserve,
                &ExportControl::default(),
            )
            .unwrap();
            assert_eq!(report.sample_peak, 2.0);
            assert_eq!(report.applied_gain, 1.0);
            let decoded = wav::read_wav(&preserved_path).unwrap();
            assert!(decoded.samples[..4_000].iter().all(|value| *value == 0.0));
            // Asset reimport deliberately clamps over-unity Float32. Inspect
            // the actual writer's RIFF payload to prove PreserveLevel instead.
            let bytes = std::fs::read(&preserved_path).unwrap();
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..16], b"WAVEfmt ");
            assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 18);
            assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 3);
            assert_eq!(&bytes[38..42], b"fact");
            assert_eq!(u32::from_le_bytes(bytes[46..50].try_into().unwrap()), 4_000);
            assert_eq!(&bytes[50..54], b"data");
            let data_bytes = u32::from_le_bytes(bytes[54..58].try_into().unwrap()) as usize;
            assert_eq!(data_bytes, 4_000 * 2 * 4);
            assert_eq!(bytes.len(), 58 + data_bytes);
            for (sample, word) in bytes[58..].as_chunks::<4>().0.iter().enumerate() {
                let raw = f32::from_le_bytes(*word);
                let expected = if sample < 4_000 { 0.0_f32 } else { 2.0 };
                assert_eq!(
                    raw.to_bits(),
                    expected.to_bits(),
                    "raw Float32 sample {sample}"
                );
            }
            assert!(decoded.samples[4_000..].iter().all(|value| *value == 1.0));
        } else {
            let error = render_project_wav_with_options(
                &project,
                &preserved_path,
                preserve,
                &ExportControl::default(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("would clip"));
            assert_eq!(
                std::fs::read(&preserved_path).unwrap(),
                b"original destination"
            );
        }
        project.automation_lanes[0]
            .lane
            .replace_points([AutomationPoint::new(0.0, 1.0)]);
        let silent_path = directory.path(&format!("silent-{sample_format:?}.wav"));
        let report = render_project_wav_with_options(
            &project,
            &silent_path,
            preserve,
            &ExportControl::default(),
        )
        .unwrap();
        assert_eq!(report.sample_peak, 0.0);
        assert_eq!(report.applied_gain, 1.0);
        assert!(
            wav::read_wav(&silent_path)
                .unwrap()
                .samples
                .iter()
                .all(|value| *value == 0.0)
        );
        project.automation_lanes[0].lane.replace_points([
            AutomationPoint::new(0.0, 1.0),
            AutomationPoint::new(0.25, 0.0),
        ]);
    }
}

#[test]
fn final_wav_stable_master_mute_does_not_admit_alias_unbound_plugins_fx_or_sidechain() {
    let directory = FixtureDirectory::new("processing-guards");
    let mut supported = native_project(0.5);
    add_lane(&mut supported, 101, AutomationCurve::Hold, &[(0.0, 0.0)]);
    let path = directory.path("original.wav");
    std::fs::write(&path, b"existing output").unwrap();
    let mut rejected = Vec::new();
    for track in [0, 1, 0xffff_fffe] {
        let mut project = supported.clone();
        project.automation_lanes[0]
            .lane
            .set_target(AutomationTarget::MixerMute { track });
        rejected.push((project, "Only Tempo"));
    }
    let mut missing_master = supported.clone();
    missing_master
        .mixer_tracks
        .retain(|track| track.id != MASTER_MIXER_TRACK_ID);
    rejected.push((missing_master, "master"));
    let mut instrument = supported.clone();
    instrument.plugin_instances.push(test_plugin(901));
    instrument.channels[0].instrument_plugin_instance_id = Some(901);
    rejected.push((instrument, "active instrument"));
    for track in [1, MASTER_MIXER_TRACK_ID] {
        let mut effect = supported.clone();
        effect.plugin_instances.push(test_plugin(901));
        effect.mixer_insert_slots.push(MixerInsertSlotRef {
            track,
            slot: 0,
            plugin_instance_id: 901,
        });
        rejected.push((effect, "active mixer"));
    }
    let mut sidechain = supported.clone();
    sidechain.mixer_routes.push(ProjectMixerRoute {
        id: 901,
        runtime_slot: 2,
        source_mixer_track_id: 1,
        destination: MixerRouteDestination::PluginSidechain {
            mixer_track_id: MASTER_MIXER_TRACK_ID,
            slot: 0,
            input_bus: 1,
        },
        tap: MixerRouteTap::PostFader,
        gain: 1.0,
        enabled: true,
    });
    rejected.push((sidechain, "sidechain"));
    for sample_format in WavSampleFormat::ALL {
        let opts = WavExportOptions {
            sample_format,
            ..options(0)
        };
        render_project_wav_with_options(
            &supported,
            &directory.path(&format!("supported-{sample_format:?}.wav")),
            opts,
            &ExportControl::default(),
        )
        .unwrap();
        for (project, reason) in &rejected {
            let entries_before = std::fs::read_dir(&directory.0).unwrap().count();
            let preflight = format!("{:#}", ensure_supported_project(project).unwrap_err());
            assert!(
                preflight.to_lowercase().contains(&reason.to_lowercase()),
                "{preflight}"
            );
            let error =
                render_project_wav_with_options(project, &path, opts, &ExportControl::default())
                    .unwrap_err();
            assert!(
                format!("{error:#}")
                    .to_lowercase()
                    .contains(&reason.to_lowercase())
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"existing output");
            assert_eq!(
                std::fs::read_dir(&directory.0).unwrap().count(),
                entries_before
            );
        }
    }
}
