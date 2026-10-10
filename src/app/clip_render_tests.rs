use super::*;
use std::collections::BTreeMap;

fn context() -> OccurrenceContext {
    OccurrenceContext {
        project_session: 1,
        source_revision: 2,
        control_revision: 0,
    }
}
fn project() -> Project {
    let mut project = Project {
        tempo: 120.0,
        song_length_beats: 20.0,
        ..Project::default()
    };
    project.channels.truncate(1);
    project.channels[0].instrument_plugin_instance_id = Some(41);
    project.channels[0].pan = 0.0;
    project.channels[0].volume = 0.75;
    project.plugin_instances = vec![PluginInstance {
        id: 41,
        format: PluginFormat::Vst3,
        role: PluginRole::Instrument,
        path: "fake-only.vst3".into(),
        uid: "fixture".into(),
        vendor: "Fixture".into(),
        name: "Fixture".into(),
        enabled: true,
        bypass: false,
        wet: 1.0,
        parameters: BTreeMap::new(),
        opaque_state: vec![1, 2, 3],
        runtime_status: PluginRuntimeStatus::Loaded,
        midi_ports: Default::default(),
    }];
    project.automation_lanes.clear();
    project.patterns.truncate(1);
    project.patterns[0].length_steps = 4;
    project.patterns[0].notes.clear();
    project.patterns[0].channel_steps = vec![[false; 16]];
    project.patterns[0].channel_steps[0][0] = true;
    project.clips.truncate(1);
    project.clips[0].kind = ClipKind::Pattern;
    project.clips[0].muted = false;
    project.clips[0].start = 1.0;
    project.clips[0].length = 1.0;
    project.clips[0].source_offset = 0.0;
    project.clips[0].pattern_id = project.patterns[0].id;
    project
}
fn plan(project: &Project, tail: u32) -> OccurrencePlan {
    OccurrencePlan::prepare(project, context(), project.clips[0].id, 8000, tail).unwrap()
}
fn published_path() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\published\occurrence.wav")
    } else {
        PathBuf::from("/published/occurrence.wav")
    }
}
fn media(plan: &OccurrencePlan) -> RenderedOccurrenceMedia {
    let frames = plan.end_frame - plan.start_frame + u64::from(plan.tail_frames);
    let samples = (0..frames).flat_map(|_| [0.25, -0.5]).collect();
    RenderedOccurrenceMedia {
        snapshot: Arc::clone(&plan.snapshot),
        context: plan.context,
        clip_id: plan.clip_id,
        pattern_id: plan.pattern_id,
        channel_id: plan.channel_id,
        instance_id: plan.instance_id,
        publication_id: 9,
        path: published_path(),
        asset: wav::WavAsset {
            metadata: wav::WavMetadata {
                sample_rate: plan.sample_rate,
                channels: 2,
                encoding: wav::WavSampleEncoding::IeeeFloat,
                bits_per_sample: 32,
                valid_bits_per_sample: 32,
                channel_mask: None,
                block_align: 8,
                byte_rate: plan.sample_rate * 8,
                frames,
                duration_seconds: frames as f64 / f64::from(plan.sample_rate),
                data_bytes: frames * 8,
            },
            samples,
        },
        prewrite_peak: 0.5,
    }
}
fn on_frames(plan: &OccurrencePlan) -> Vec<u64> {
    plan.local_events
        .iter()
        .filter(|e| matches!(e.kind, TimelineEventKind::NoteOn { .. }))
        .map(|e| e.frame)
        .collect()
}

#[test]
fn occurrence_plan_preserves_compiler_identity_and_nonquantum_local_boundaries() {
    for start in [0, 1, 127, 128, 129] {
        for frames in [1, 127, 128, 129, 257] {
            let mut p = project();
            p.clips[0].start = start as f32 / 4000.0;
            p.clips[0].length = frames as f32 / 4000.0;
            let before = project_snapshot(&p).unwrap();
            let plan = plan(&p, 7);
            assert_eq!((plan.start_frame, plan.end_frame), (start, start + frames));
            assert_eq!(on_frames(&plan), vec![0]);
            assert_eq!(plan.local_events.last().unwrap().frame, frames);
            let original = CompiledTimeline::from_project(
                &p,
                &plan.tempo_map,
                TimelineCompileOptions::default(),
            )
            .unwrap();
            for (selected, original) in plan.local_events.iter().zip(original.events().iter()) {
                assert_eq!(selected.frame + start, original.frame);
                assert_eq!(selected.kind, original.kind);
            }
            assert_eq!(project_snapshot(&p).unwrap(), before);
        }
    }
}

#[test]
fn occurrence_plan_uses_real_source_offsets_swing_and_separate_repeat_periods() {
    let mut swing = project();
    swing.clips[0].start = 0.0;
    swing.clips[0].length = 0.5;
    swing.swing = 1.0;
    swing.patterns[0].channel_steps[0][1] = true;
    assert_eq!(on_frames(&plan(&swing, 0)), vec![0, 1750]);
    let mut slipped = project();
    slipped.clips[0].source_offset = 0.25;
    slipped.clips[0].length = 17.375;
    slipped.patterns[0].notes.push(PianoNote {
        id: 91,
        channel_id: Some(slipped.channels[0].id),
        group_id: None,
        note: 64,
        start: 0.5,
        length: 2.0,
        velocity: 0.8,
        selected: false,
        muted: false,
    });
    let p = plan(&slipped, 0);
    let piano_ons: Vec<_> = p
        .local_events
        .iter()
        .filter_map(|e| match e.kind {
            TimelineEventKind::NoteOn {
                source:
                    NoteSourceDescriptor::LegacyPiano {
                        persistent_note_id: 91,
                        repetition,
                        ..
                    },
                ..
            } => Some((e.frame, repetition)),
            _ => None,
        })
        .collect();
    assert_eq!(piano_ons, vec![(1000, 0), (65000, 1)]);
    assert!(p.local_events.iter().any(|e| matches!(
        e.kind,
        TimelineEventKind::NoteOn {
            source: NoteSourceDescriptor::ChannelStep { repetition: 1, .. },
            ..
        }
    ) && e.frame == 3000));
    assert_eq!(
        p.local_events.last().unwrap().frame,
        p.end_frame - p.start_frame
    );
}

#[test]
fn occurrence_plan_rejects_exact_unsupported_source_cases_without_mutation() {
    let good = project();
    assert!(OccurrencePlan::prepare(&good, context(), good.clips[0].id, 8000, 0).is_ok());
    for case in 0..12 {
        let mut p = project();
        let (kind, reason) = match case {
            0 => {
                p.clips.push(p.clips[0].clone());
                (OccurrenceErrorKind::Source, "Clip is missing or duplicated")
            }
            1 => {
                p.patterns.push(p.patterns[0].clone());
                (
                    OccurrenceErrorKind::Source,
                    "Pattern is missing or duplicated",
                )
            }
            2 => {
                p.channels[0].pan = 0.5;
                (OccurrenceErrorKind::Unsupported, "centered Channel pan")
            }
            3 => {
                p.channels[0].muted = true;
                (
                    OccurrenceErrorKind::Unsupported,
                    "muted or excluded by Solo",
                )
            }
            4 => {
                p.clips[0].muted = true;
                (OccurrenceErrorKind::Source, "unmuted Pattern Clip")
            }
            5 => {
                p.plugin_instances[0].wet = 0.5;
                (OccurrenceErrorKind::Unsupported, "fully-wet")
            }
            6 => {
                p.patterns[0].notes.push(PianoNote {
                    id: 9,
                    channel_id: None,
                    group_id: None,
                    note: 64,
                    start: 0.0,
                    length: 0.25,
                    velocity: 1.0,
                    selected: false,
                    muted: false,
                });
                (OccurrenceErrorKind::Source, "unassigned Piano note")
            }
            7 => {
                let mut c = p.channels[0].clone();
                c.id = 999;
                c.instrument_plugin_instance_id = None;
                p.channels.push(c);
                p.patterns[0].channel_steps.push([true; 16]);
                (
                    OccurrenceErrorKind::Unsupported,
                    "exactly one musical Channel",
                )
            }
            8 => {
                p.clips[0].source_offset = 0.5;
                p.clips[0].length = 0.25;
                (
                    OccurrenceErrorKind::Unsupported,
                    "contains no compiled notes",
                )
            }
            9 => {
                p.clips[0].start = 19.9;
                p.clips[0].length = 1.0;
                (OccurrenceErrorKind::Timeline, "Timeline diagnostics")
            }
            10 => {
                p.channels[0].volume = f32::NAN;
                (OccurrenceErrorKind::Source, "snapshot is not persistable")
            }
            _ => {
                let mut lane = AutomationLane::new(AutomationTarget::Tempo);
                lane.replace_points([crate::automation::AutomationPoint::new(0.0, 120.0)]);
                p.automation_lanes.push(ProjectAutomation {
                    id: 77,
                    name: "Tempo".into(),
                    lane,
                });
                (OccurrenceErrorKind::Unsupported, "Active automation")
            }
        };
        let before = serde_json::to_vec(&p).unwrap();
        let e = OccurrencePlan::prepare(&p, context(), p.clips[0].id, 8000, 0)
            .err()
            .unwrap();
        assert_eq!(e.kind, kind, "case {case}: {e:?}");
        assert!(e.reason.contains(reason), "case {case}: {e:?}");
        assert_eq!(serde_json::to_vec(&p).unwrap(), before);
    }
}

#[test]
fn occurrence_plan_rejects_other_generator_overlap_and_tail_but_allows_touching() {
    // Selected interval is [4000,8000), with a 1000-frame explicit tail.
    for (start, length, tail, allowed) in [
        (2.0, 0.25, 0, true),
        (2.0, 0.25, 1000, false),
        (2.25, 0.25, 1000, true),
        (0.75, 0.25, 0, true),
        (0.75, 0.5, 0, false),
        (1.5, 0.25, 0, false),
    ] {
        let mut p = project();
        // Use a long Piano note so outside identities can genuinely cross boundaries.
        p.patterns[0].channel_steps[0] = [false; 16];
        p.patterns[0].notes = vec![PianoNote {
            id: 91,
            channel_id: Some(p.channels[0].id),
            group_id: None,
            note: 64,
            start: 0.0,
            length: 4.0,
            velocity: 1.0,
            selected: false,
            muted: false,
        }];
        let mut other = p.clips[0].clone();
        other.id = 55;
        other.start = start;
        other.length = length;
        p.clips.push(other);
        let result = OccurrencePlan::prepare(&p, context(), p.clips[0].id, 8000, tail);
        if allowed {
            assert!(
                result.is_ok(),
                "{start}/{length}/{tail}: {:?}",
                result.err()
            );
        } else {
            let e = result.err().unwrap();
            assert_eq!(e.kind, OccurrenceErrorKind::Unsupported);
            assert!(e.reason.contains("same generator overlaps or crosses"));
        }
    }
}

#[test]
fn occurrence_snapshot_and_context_reject_restored_values_after_revision_changes() {
    let mut p = project();
    let plan = plan(&p, 0);
    plan.check_current(&p, context()).unwrap();
    p.name.push_str(" changed");
    let e = plan.check_current(&p, context()).unwrap_err();
    assert_eq!(e.kind, OccurrenceErrorKind::Stale);
    assert!(e.reason.contains("Project changed"));
    p = project();
    for changed in [
        OccurrenceContext {
            project_session: 2,
            ..context()
        },
        OccurrenceContext {
            source_revision: 3,
            ..context()
        },
        OccurrenceContext {
            control_revision: 1,
            ..context()
        },
    ] {
        let e = plan.check_current(&p, changed).unwrap_err();
        assert_eq!(e.kind, OccurrenceErrorKind::Stale);
        assert!(e.reason.contains("source/control revision"));
    }
    let mut unpersistable = project();
    unpersistable.patterns[0].notes.push(PianoNote {
        id: 1,
        channel_id: Some(unpersistable.channels[0].id),
        group_id: None,
        note: 64,
        start: f32::INFINITY,
        length: 0.1,
        velocity: 1.0,
        selected: false,
        muted: true,
    });
    assert_eq!(
        project_snapshot(&unpersistable).unwrap_err().kind,
        OccurrenceErrorKind::Source
    );
}

#[test]
fn rendered_media_requires_full_exact_stereo_float_metadata_and_pcm() {
    let p = project();
    let plan = plan(&p, 7);
    validate_rendered_media(&plan, &media(&plan)).unwrap();
    for case in 0..22 {
        let mut m = media(&plan);
        match case {
            0 => {
                m.asset.samples.pop().unwrap();
            }
            1 => m.asset.metadata.frames += 1,
            2 => m.asset.metadata.sample_rate += 1,
            3 => m.asset.metadata.channels = 1,
            4 => m.asset.metadata.bits_per_sample = 24,
            5 => m.asset.metadata.valid_bits_per_sample = 24,
            6 => m.asset.metadata.encoding = wav::WavSampleEncoding::PcmInteger,
            7 => m.asset.metadata.block_align = 4,
            8 => m.asset.metadata.byte_rate -= 1,
            9 => m.asset.metadata.data_bytes -= 8,
            10 => m.asset.metadata.duration_seconds += 0.1,
            11 => m.asset.metadata.channel_mask = Some(4),
            12 => m.asset.samples[0] = f32::NAN,
            13 => m.asset.samples[0] = 1.01,
            14 => m.prewrite_peak = 1.01,
            15 => m.prewrite_peak = f32::NAN,
            16 => m.prewrite_peak = 0.25,
            17 => m.path = PathBuf::from("relative.wav"),
            18 => m.publication_id = 0,
            19 => m.context.control_revision += 1,
            20 => m.clip_id += 1,
            _ => m.snapshot = Arc::from(&b"different source"[..]),
        }
        let e = validate_rendered_media(&plan, &m).unwrap_err();
        assert_eq!(e.kind, OccurrenceErrorKind::Media, "case {case}");
        let reason = match case {
            0..=11 => "stereo Float32",
            12 | 13 => "PCM is nonfinite",
            14 | 15 => "Dry render headroom",
            16 => "peak differs",
            17 | 18 => "published absolute",
            _ => "different source",
        };
        assert!(e.reason.contains(reason), "case {case}: {e:?}");
        assert_eq!(
            m.path,
            if case == 17 {
                PathBuf::from("relative.wav")
            } else {
                published_path()
            }
        );
    }
    let mut stereo_mask = media(&plan);
    stereo_mask.asset.metadata.channel_mask = Some(3);
    validate_rendered_media(&plan, &stereo_mask).unwrap();
}

#[test]
fn occurrence_admission_rejects_missing_duplicate_routes_and_unrepresentable_gain() {
    let good = project();
    let _ = plan(&good, 0);
    for case in 0..9 {
        let mut p = project();
        let (kind, reason) = match case {
            0 => {
                p.channels.push(p.channels[0].clone());
                p.patterns[0].channel_steps.push([false; 16]);
                (
                    OccurrenceErrorKind::Source,
                    "Channel is missing or duplicated",
                )
            }
            1 => {
                p.plugin_instances.push(p.plugin_instances[0].clone());
                (
                    OccurrenceErrorKind::Source,
                    "instrument is missing or duplicated",
                )
            }
            2 => {
                p.plugin_instances.clear();
                (
                    OccurrenceErrorKind::Source,
                    "instrument is missing or duplicated",
                )
            }
            3 => {
                p.plugin_instances[0].midi_ports.input = Some(1);
                (OccurrenceErrorKind::Unsupported, "Configured MIDI routing")
            }
            4 => {
                p.plugin_instances[0].midi_ports.output = Some(1);
                (OccurrenceErrorKind::Unsupported, "Configured MIDI routing")
            }
            5 => {
                p.channels[0].volume = 1.5001;
                (
                    OccurrenceErrorKind::Unsupported,
                    "Channel volume in 0..=1.5",
                )
            }
            6 => {
                p.plugin_instances[0].role = PluginRole::Effect;
                (
                    OccurrenceErrorKind::Unsupported,
                    "fully-wet unbypassed VST3",
                )
            }
            7 => {
                p.patterns[0].notes.push(PianoNote {
                    id: 2,
                    channel_id: Some(999),
                    group_id: None,
                    note: 60,
                    start: 0.0,
                    length: 0.25,
                    velocity: 1.0,
                    muted: false,
                    selected: false,
                });
                p.patterns[0].channel_steps[0] = [false; 16];
                (
                    OccurrenceErrorKind::Source,
                    "Channel is missing or duplicated",
                )
            }
            _ => {
                p.plugin_instances[0].midi_ports.audio_monitor_muted = true;
                (OccurrenceErrorKind::Unsupported, "monitoring enabled")
            }
        };
        let before = project_snapshot(&p).unwrap();
        let error = OccurrencePlan::prepare(&p, context(), p.clips[0].id, 8000, 0)
            .err()
            .unwrap();
        assert_eq!(error.kind, kind, "case {case}: {error:?}");
        assert!(error.reason.contains(reason), "case {case}: {error:?}");
        assert_eq!(project_snapshot(&p).unwrap(), before);
    }
}

#[test]
fn occurrence_bounds_reject_tail_song_end_rate_context_and_processing_capacity() {
    let good = project();
    let _ = plan(&good, 0);
    for case in 0..6 {
        let mut p = project();
        let mut now = context();
        let mut rate = 8000;
        let mut tail = 0;
        let (kind, reason) = match case {
            0 => {
                p.song_length_beats = 2.0;
                tail = 1;
                (OccurrenceErrorKind::Range, "beyond song end")
            }
            1 => {
                rate = 7999;
                (OccurrenceErrorKind::Range, "8000–192000")
            }
            2 => {
                tail = 240001;
                (OccurrenceErrorKind::Range, "at most 30 seconds")
            }
            3 => {
                now.project_session = 0;
                (OccurrenceErrorKind::Context, "session and source revision")
            }
            4 => {
                now.source_revision = 0;
                (OccurrenceErrorKind::Context, "session and source revision")
            }
            _ => {
                rate = 192000;
                p.song_length_beats = 512.0;
                p.clips[0].length = 256.0;
                (OccurrenceErrorKind::Capacity, "24-million-frame")
            }
        };
        let before = project_snapshot(&p).unwrap();
        let error = OccurrencePlan::prepare(&p, now, p.clips[0].id, rate, tail)
            .err()
            .unwrap();
        assert_eq!(error.kind, kind, "case {case}: {error:?}");
        assert!(error.reason.contains(reason), "case {case}: {error:?}");
        assert_eq!(project_snapshot(&p).unwrap(), before);
    }
}

#[test]
fn occurrence_filter_rejects_full_noteoff_identity_and_selected_source_corruption() {
    let p = project();
    let plan = plan(&p, 0);
    let timeline =
        CompiledTimeline::from_project(&p, &plan.tempo_map, TimelineCompileOptions::default())
            .unwrap();
    let select = |events: &[TimelineEvent]| {
        select_occurrence_events(
            &p,
            events,
            plan.clip_id,
            plan.pattern_id,
            plan.channel_id,
            plan.instance_id,
            plan.start_frame,
            plan.end_frame,
            plan.end_frame,
        )
    };
    assert_eq!(select(timeline.events()).unwrap(), plan.local_events);
    for case in 0..6 {
        let mut events = timeline.events().to_vec();
        let off = events
            .iter_mut()
            .find(|event| matches!(event.kind, TimelineEventKind::NoteOff { .. }))
            .unwrap();
        if let TimelineEventKind::NoteOff {
            note_id,
            channel_id,
            note,
            mixer_track,
            source,
        } = &mut off.kind
        {
            match case {
                0 => *note_id += 100,
                1 => *channel_id += 100,
                2 => *note += 1,
                3 => *mixer_track += 1,
                4 => match source {
                    NoteSourceDescriptor::ChannelStep { pattern_id, .. }
                    | NoteSourceDescriptor::LegacyPiano { pattern_id, .. } => *pattern_id += 100,
                },
                _ => off.frame = plan.start_frame,
            }
        }
        let error = select(&events).unwrap_err();
        assert_eq!(error.kind, OccurrenceErrorKind::Timeline);
        assert!(
            error.reason.contains(match case {
                0 => "no admitted NoteOn",
                1 => "another Channel",
                _ => "complete NoteOn identity",
            }),
            "case {case}: {error:?}"
        );
    }
}
