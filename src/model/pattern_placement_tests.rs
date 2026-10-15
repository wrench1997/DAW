use super::*;
use crate::{
    tempo_map::TempoMap,
    timeline::{
        CompiledTimeline, NoteSourceDescriptor, TimelineCompileOptions, TimelineDiagnostic,
        TimelineDiagnosticKind, TimelineEntity, TimelineEventKind,
    },
};

struct PrivateDirectory(PathBuf);

impl PrivateDirectory {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "citrus-pattern-placement-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        // Only the unique, absolute directory successfully created by this owner.
        if self.0.is_absolute()
            && self.0.parent() == Some(std::env::temp_dir().as_path())
            && self.0.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("citrus-pattern-placement-")
            })
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn note(id: u64, start: f32, length: f32) -> PianoNote {
    PianoNote {
        id,
        channel_id: Some(1),
        group_id: None,
        note: 60,
        start,
        length,
        velocity: 0.8,
        selected: false,
        muted: false,
    }
}

fn placement(id: u32, start: f32, length: f32, policy: Option<PatternLengthPolicy>) -> Clip {
    let mut clip = Project::default()
        .clips
        .into_iter()
        .find(|clip| clip.kind == ClipKind::Pattern)
        .unwrap();
    assert_eq!(clip.pattern_length_policy, None);
    clip.id = id;
    clip.track = id as usize - 1;
    clip.start = start;
    clip.length = length;
    clip.pattern_id = 1;
    clip.pattern_length_policy = policy;
    clip.group_id = None;
    clip.source_offset = 0.0;
    clip.gain = 1.0;
    clip.muted = false;
    clip
}

fn musical_project() -> Project {
    let mut project = Project::blank();
    project.tempo = 120.0;
    project.song_length_beats = 64.0;
    project.patterns[0].notes.push(note(7, 0.0, 0.5));
    project
}

fn compile_timeline(project: &Project) -> CompiledTimeline {
    let map = TempoMap::from_project(project, 8_000).unwrap();
    CompiledTimeline::from_project(project, &map, TimelineCompileOptions::default()).unwrap()
}

fn timeline(project: &Project) -> CompiledTimeline {
    let timeline = compile_timeline(project);
    assert!(
        timeline.diagnostics().is_empty(),
        "{:?}",
        timeline.diagnostics()
    );
    timeline
}

fn timeline_with_manual_cutoff(
    project: &Project,
    clip_id: u32,
    requested_end_beat: f64,
) -> CompiledTimeline {
    let timeline = compile_timeline(project);
    assert_eq!(
        timeline.diagnostics(),
        &[TimelineDiagnostic {
            entity: TimelineEntity::Clip(clip_id),
            kind: TimelineDiagnosticKind::ClipEndTruncated {
                requested_end_beat,
                map_end_beat: f64::from(project.song_length_beats),
            },
        }]
    );
    timeline
}

fn piano_edges(timeline: &CompiledTimeline, clip_id: u32) -> Vec<(u64, bool)> {
    timeline
        .events()
        .iter()
        .filter_map(|event| {
            let (source, on) = match event.kind {
                TimelineEventKind::NoteOn { source, .. } => (source, true),
                TimelineEventKind::NoteOff { source, .. } => (source, false),
                _ => return None,
            };
            matches!(source, NoteSourceDescriptor::LegacyPiano { clip_id: id, .. } if id == clip_id)
                .then_some((event.frame, on))
        })
        .collect()
}

#[test]
fn follow_placements_grow_and_shrink_together_without_rewriting_stored_trims() {
    let mut project = musical_project();
    project.clips = vec![
        placement(1, 0.0, 1.25, Some(PatternLengthPolicy::Follow)),
        placement(2, 12.0, 2.5, Some(PatternLengthPolicy::Follow)),
        placement(3, 24.0, 1.5, None),
        placement(4, 28.0, 1.75, Some(PatternLengthPolicy::Manual)),
    ];
    let stored_clips = serde_json::to_value(&project.clips).unwrap();
    let lengths = |project: &Project| {
        project
            .clips
            .iter()
            .map(|clip| project.clip_length_beats(clip))
            .collect::<Vec<_>>()
    };
    assert_eq!(lengths(&project), vec![4.0, 4.0, 1.5, 1.75]);
    project.patterns[0].notes.push(note(8, 4.25, 0.5));
    assert_eq!(lengths(&project), vec![8.0, 8.0, 1.5, 1.75]);
    let grown = timeline(&project);
    assert_eq!(
        piano_edges(&grown, 1),
        vec![(0, true), (2_000, false), (17_000, true), (19_000, false)]
    );
    assert_eq!(
        piano_edges(&grown, 2),
        vec![
            (48_000, true),
            (50_000, false),
            (65_000, true),
            (67_000, false)
        ]
    );
    project.patterns[0].notes.push(note(9, 16.0, 0.25));
    assert_eq!(lengths(&project), vec![32.0, 32.0, 1.5, 1.75]);
    let long = timeline(&project);
    assert!(piano_edges(&long, 1).contains(&(64_000, true)));
    assert!(piano_edges(&long, 2).contains(&(112_000, true)));
    project.patterns[0].notes.retain(|note| note.id != 9);
    assert_eq!(lengths(&project), vec![8.0, 8.0, 1.5, 1.75]);
    assert_eq!(piano_edges(&timeline(&project), 1), piano_edges(&grown, 1));
    project.patterns[0].notes.retain(|note| note.id != 8);
    assert_eq!(lengths(&project), vec![4.0, 4.0, 1.5, 1.75]);
    assert_eq!(
        piano_edges(&timeline(&project), 1),
        vec![(0, true), (2_000, false)]
    );
    project.patterns[0].notes.clear();
    assert_eq!(lengths(&project), vec![4.0, 4.0, 1.5, 1.75]);
    assert_eq!(serde_json::to_value(&project.clips).unwrap(), stored_clips);
}

#[test]
fn explicit_manual_trim_keeps_the_modern_source_clock_and_legacy_absence() {
    let mut project = musical_project();
    let mut legacy = placement(1, 0.0, 8.0, None);
    let mut modern = placement(2, 0.0, 1.0, Some(PatternLengthPolicy::Follow));
    legacy.set_manual_pattern_length(2.25);
    modern.set_manual_pattern_length(5.5);
    assert_eq!(legacy.pattern_length_policy, None);
    assert_eq!(
        modern.pattern_length_policy,
        Some(PatternLengthPolicy::Manual)
    );
    assert_eq!(project.clip_length_beats(&legacy), 2.25);
    assert_eq!(project.clip_length_beats(&modern), 5.5);
    assert_eq!(legacy.piano_period_beats(&project.patterns[0], 16.0), 16.0);
    assert_eq!(modern.piano_period_beats(&project.patterns[0], 16.0), 4.0);
    project.patterns[0].notes.push(note(8, 4.25, 0.5));
    assert_eq!(project.clip_length_beats(&modern), 5.5);
    assert_eq!(modern.piano_period_beats(&project.patterns[0], 16.0), 8.0);
    assert_eq!(legacy.piano_period_beats(&project.patterns[0], 16.0), 16.0);
}

#[test]
fn resolved_geometry_is_a_transient_image_with_original_placement_payload() {
    let mut project = musical_project();
    project.patterns[0].notes.push(note(8, 4.25, 0.5));
    let mut clip = placement(1, 12.0, 1.25, Some(PatternLengthPolicy::Follow));
    clip.source_offset = 1.5;
    clip.gain = 0.625;
    clip.muted = true;
    let original = serde_json::to_value(&clip).unwrap();
    let resolved = project.resolved_clip_geometry(&clip);
    assert_eq!(resolved.length, 8.0);
    let mut expected = original.clone();
    expected["length"] = serde_json::json!(8.0);
    assert_eq!(serde_json::to_value(resolved).unwrap(), expected);
    assert_eq!(serde_json::to_value(&clip).unwrap(), original);
}

#[test]
fn mixed_legacy_manual_and_follow_placements_compile_their_own_piano_clocks() {
    let mut project = musical_project();
    project.clips = vec![
        placement(1, 0.0, 8.0, None),
        placement(2, 10.0, 8.0, Some(PatternLengthPolicy::Manual)),
        placement(3, 20.0, 1.0, Some(PatternLengthPolicy::Follow)),
    ];
    let compiled = timeline(&project);
    assert_eq!(piano_edges(&compiled, 1), vec![(0, true), (2_000, false)]);
    assert_eq!(
        piano_edges(&compiled, 2),
        vec![
            (40_000, true),
            (42_000, false),
            (56_000, true),
            (58_000, false)
        ]
    );
    assert_eq!(
        piano_edges(&compiled, 3),
        vec![(80_000, true), (82_000, false)]
    );
    assert_eq!(project.clips[2].length, 1.0);
    assert_eq!(project.clip_length_beats(&project.clips[2]), 4.0);
}

#[test]
fn follow_extent_and_manual_song_cutoff_preserve_slip_gain_and_mute() {
    let mut project = musical_project();
    project.auto_song_length = true;
    project.song_length_beats = 2.5;
    project.patterns[0].notes = vec![note(7, 0.5, 1.0)];
    let mut follow = placement(1, 2.0, 0.25, Some(PatternLengthPolicy::Follow));
    follow.source_offset = 0.25;
    follow.gain = 0.625;
    let mut muted = follow.clone();
    muted.id = 2;
    muted.start = 100.0;
    muted.muted = true;
    project.clips = vec![follow, muted];
    assert_eq!(project.song_end_beat(), 8.0);
    let auto = timeline(&project);
    assert_eq!(auto.duration_frames(), 32_000);
    assert_eq!(piano_edges(&auto, 1), vec![(9_000, true), (13_000, false)]);
    assert!(piano_edges(&auto, 2).is_empty());
    assert!(auto.events().iter().any(|event| matches!(
        event.kind,
        TimelineEventKind::NoteOn { velocity, gain, .. }
            if velocity == 0.8 && gain == 0.625
    )));
    project.auto_song_length = false;
    assert_eq!(project.song_end_beat(), 2.5);
    let manual = timeline_with_manual_cutoff(&project, 1, 6.0);
    assert_eq!(manual.duration_frames(), 10_000);
    assert_eq!(
        piano_edges(&manual, 1),
        vec![(9_000, true), (10_000, false)]
    );
    assert_eq!(project.clips[0].length, 0.25);
    project.auto_song_length = true;
    project.clips[0].muted = true;
    assert_eq!(project.song_end_beat(), 4.0);
    assert!(timeline(&project).events().iter().all(|event| !matches!(
        event.kind,
        TimelineEventKind::NoteOn { .. } | TimelineEventKind::NoteOff { .. }
    )));
}

#[test]
fn format_fourteen_round_trip_preserves_policies_exact_audio_and_opaque_state() {
    let directory = PrivateDirectory::new();
    let source = directory.0.join("source.citrus");
    let copy = directory.0.join("copy.citrus");
    let mut project = musical_project();
    project.auto_song_length = true;
    project.song_length_beats = 2.25;
    project.patterns[0].notes.push(note(8, 4.25, 0.5));
    project.clips = vec![
        placement(1, 0.0, 1.25, Some(PatternLengthPolicy::Follow)),
        placement(2, 12.0, 3.5, Some(PatternLengthPolicy::Manual)),
        placement(3, 20.0, 2.0, None),
    ];
    project.clips[0].group_id = Some(7);
    project.clips[1].group_id = Some(7);
    project.clips[0].source_offset = 0.25;
    project.clips[0].gain = 0.625;
    let mut audio = placement(4, 24.0, 2.0, None);
    audio.kind = ClipKind::Audio;
    audio.audio_asset_id = Some(41);
    audio.audio_source_offset_frame = Some(137);
    audio.audio_source_reference = Some(AudioSourceReference {
        elapsed_spans_seconds: vec![
            AudioSourceSpan {
                start_seconds: 0.125,
                end_seconds: 0.375,
            },
            AudioSourceSpan {
                start_seconds: 0.3,
                end_seconds: 0.25,
            },
        ],
    });
    audio.audio_length_reference = Some(AudioLengthReference {
        stored_length_beats: 2.0,
        length_beats: 2.0 + 1.0e-9,
        export_length_beats: 2.0 - 1.0e-9,
    });
    audio.fade_in = 0.2;
    audio.fade_out = 0.3;
    audio.fade_in_reference = Some(AudioFadeReference {
        offset_beats: -0.5,
        length_beats: 3.5,
        end_limit_beats: Some(2.0),
        export_length_beats: Some(3.5),
    });
    audio.fade_out_reference = Some(AudioFadeReference {
        offset_beats: -1.0,
        length_beats: 4.0,
        end_limit_beats: Some(2.0),
        export_length_beats: Some(4.0),
    });
    project.clips.push(audio);
    project.audio_assets.push(AudioAsset {
        id: 41,
        name: "Private metadata fixture".into(),
        path: directory.0.join("not-opened.wav"),
        sample_rate: 48_000,
        channels: 2,
        bits_per_sample: 24,
        frames: 96_000,
        waveform_peaks: Vec::new(),
    });
    project
        .audio_clip_mixer_destinations
        .push(AudioClipMixerDestination {
            clip_id: 4,
            mixer_track_id: 1,
        });
    project.plugin_instances.push(PluginInstance {
        id: 99,
        format: PluginFormat::Vst3,
        role: PluginRole::Instrument,
        path: directory.0.join("not-opened.vst3"),
        uid: "opaque-fixture".into(),
        vendor: "Fixture".into(),
        name: "Retained source".into(),
        enabled: false,
        bypass: false,
        wet: 1.0,
        parameters: BTreeMap::from([(7, 0.125), (8, 0.875)]),
        opaque_state: vec![0, 1, 0x7f, 0x80, 0xfe, 0xff],
        runtime_status: PluginRuntimeStatus::Loaded,
        midi_ports: crate::plugin_midi_routing::PluginMidiPorts {
            input: None,
            output: Some(0),
            audio_monitor_muted: true,
        },
    });
    project.channels[0].instrument_plugin_instance_id = Some(99);
    let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
    lane.replace_points([
        AutomationPoint::new(0.0, 0.5),
        AutomationPoint::new(27.0, 0.8),
    ]);
    project.automation_lanes.push(ProjectAutomation {
        id: 5,
        name: "Master fade".into(),
        lane,
    });
    let expected = serde_json::to_value(&project).unwrap();
    project.save(&source).unwrap();
    let original_bytes = std::fs::read(&source).unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
    assert_eq!(stored["format_version"], 14);
    // The previous maximum-version admission alone rejects this new schema.
    assert!(stored["format_version"].as_u64().unwrap() > 13);
    assert_eq!(stored["clips"][0]["pattern_length_policy"], "follow");
    assert_eq!(stored["clips"][1]["pattern_length_policy"], "manual");
    assert!(stored["clips"][2].get("pattern_length_policy").is_none());
    let reopened = Project::load(&source).unwrap();
    assert_eq!(serde_json::to_value(&reopened).unwrap(), expected);
    assert_eq!(reopened.clip_length_beats(&reopened.clips[0]), 8.0);
    assert_eq!(reopened.clips[0].length, 1.25);
    assert_eq!(reopened.song_end_beat(), 28.0);
    assert_eq!(reopened.song_length_beats, 2.25);
    assert!(!reopened.plugin_instances[0].opaque_state.is_empty());
    reopened.save(&copy).unwrap();
    assert_eq!(
        serde_json::to_value(Project::load(&copy).unwrap()).unwrap(),
        expected
    );
    assert_eq!(std::fs::read(&source).unwrap(), original_bytes);
}

#[test]
fn missing_policy_in_format_thirteen_remains_legacy_manual_after_cold_open() {
    let directory = PrivateDirectory::new();
    let source = directory.0.join("legacy-thirteen.citrus");
    let mut project = musical_project();
    project.song_length_beats = 2.0;
    project.clips.push(placement(1, 0.0, 2.25, None));
    let mut stored = serde_json::to_value(&project).unwrap();
    stored["format_version"] = serde_json::json!(13);
    assert!(stored["clips"][0].get("pattern_length_policy").is_none());
    let original_bytes = serde_json::to_vec_pretty(&stored).unwrap();
    std::fs::write(&source, &original_bytes).unwrap();
    let reopened = Project::load(&source).unwrap();
    assert_eq!(reopened.format_version, 14);
    assert_eq!(reopened.clips[0].pattern_length_policy, None);
    assert_eq!(reopened.clip_length_beats(&reopened.clips[0]), 2.25);
    assert_eq!(
        reopened.clips[0].piano_period_beats(&reopened.patterns[0], 16.0),
        16.0
    );
    assert!(!reopened.auto_song_length);
    assert_eq!(reopened.song_end_beat(), 2.0);
    assert_eq!(
        timeline_with_manual_cutoff(&reopened, 1, 2.25).duration_frames(),
        8_000
    );
    assert_eq!(std::fs::read(&source).unwrap(), original_bytes);
}

#[test]
fn invalid_format_fourteen_policy_inputs_fail_before_normalization_or_rewrite() {
    let directory = PrivateDirectory::new();
    let mut project = musical_project();
    project
        .clips
        .push(placement(1, 0.0, 1.0, Some(PatternLengthPolicy::Follow)));
    let good = serde_json::to_value(project).unwrap();
    for (case, expected_error) in [
        ("unknown", "unknown variant"),
        ("audio", "not a Pattern Clip"),
        ("automation", "not a Pattern Clip"),
        ("missing", "unambiguous source Pattern"),
        ("duplicate", "unambiguous source Pattern"),
        ("old-version", "requires project format 14"),
    ] {
        let mut value = good.clone();
        match case {
            "unknown" => value["clips"][0]["pattern_length_policy"] = serde_json::json!("guess"),
            "audio" => value["clips"][0]["kind"] = serde_json::json!("Audio"),
            "automation" => value["clips"][0]["kind"] = serde_json::json!("Automation"),
            "missing" => value["clips"][0]["pattern_id"] = serde_json::json!(999),
            "duplicate" => {
                let duplicate = value["patterns"][0].clone();
                value["patterns"].as_array_mut().unwrap().push(duplicate);
            }
            "old-version" => value["format_version"] = serde_json::json!(13),
            _ => unreachable!(),
        }
        let source = directory.0.join(format!("{case}.citrus"));
        let original_bytes = serde_json::to_vec_pretty(&value).unwrap();
        std::fs::write(&source, &original_bytes).unwrap();
        let error = Project::load(&source).unwrap_err();
        assert!(
            format!("{error:#}").contains(expected_error),
            "{case}: {error:#}"
        );
        assert_eq!(std::fs::read(source).unwrap(), original_bytes, "{case}");
    }
}

#[test]
fn failed_policy_saves_preserve_existing_file_bytes_and_do_not_leave_staging_files() {
    let directory = PrivateDirectory::new();
    let target = directory.0.join("recoverable.citrus");
    let good = musical_project();
    good.save(&target).unwrap();
    let original_bytes = std::fs::read(&target).unwrap();
    for case in 0..5 {
        let mut invalid = good.clone();
        invalid
            .clips
            .push(placement(1, 0.0, 1.0, Some(PatternLengthPolicy::Follow)));
        let expected = match case {
            0 => {
                invalid.format_version = 13;
                "requires project format 14"
            }
            1 => {
                invalid.clips[0].kind = ClipKind::Audio;
                "not a Pattern Clip"
            }
            2 => {
                invalid.clips[0].kind = ClipKind::Automation;
                "not a Pattern Clip"
            }
            3 => {
                invalid.clips[0].pattern_id = 999;
                assert!(invalid.clip_length_beats(&invalid.clips[0]).is_nan());
                "unambiguous source Pattern"
            }
            _ => {
                invalid.patterns.push(invalid.patterns[0].clone());
                "unambiguous source Pattern"
            }
        };
        let before = serde_json::to_value(&invalid).unwrap();
        let error = invalid.save(&target).unwrap_err();
        assert!(
            format!("{error:#}").contains(expected),
            "case {case}: {error:#}"
        );
        assert_eq!(serde_json::to_value(invalid).unwrap(), before);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            original_bytes,
            "case {case}"
        );
        assert_eq!(
            std::fs::read_dir(&directory.0).unwrap().count(),
            1,
            "case {case}"
        );
    }
}
