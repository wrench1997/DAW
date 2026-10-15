use super::*;
use std::collections::BTreeMap;

fn context() -> OccurrenceContext {
    OccurrenceContext {
        project_session: 1,
        project_revision: 2,
        timeline_revision: 3,
        epoch: 4,
        engine_session: 5,
        endpoint_id: 6,
        channel_id: 1,
        instance_id: 41,
        mixer_track_id: 2,
        native_generation: 0,
        app_native_dirty_revision: 0,
        native_capture_serial: 0,
        parameter_capture_serial: 0,
        native_control_request: 0,
        parameter_control_request: 0,
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
fn acknowledgment(
    project: &Project,
    context: OccurrenceContext,
    clip_id: u32,
    sample_rate: u32,
    tail_frames: u32,
) -> Arc<FixedTailAcknowledgment> {
    Arc::new(
        FixedTailAcknowledgment::review(
            serde_json::to_vec(project).unwrap().into(),
            context,
            clip_id,
            sample_rate,
            tail_frames,
        )
        .unwrap(),
    )
}
fn admitted_plan(
    project: &Project,
    context: OccurrenceContext,
    clip_id: u32,
    sample_rate: u32,
    tail_frames: u32,
) -> Result<OccurrencePlan, OccurrenceError> {
    context
        .validate()
        .map_err(|reason| failure(OccurrenceErrorKind::Context, reason))?;
    if !(8000..=192000).contains(&sample_rate) || tail_frames > sample_rate.saturating_mul(30) {
        return Err(failure(
            OccurrenceErrorKind::Range,
            "Occurrence render requires 8000–192000 Hz and a fixed tail of at most 30 seconds",
        ));
    }
    OccurrencePlan::prepare(
        project,
        context,
        clip_id,
        sample_rate,
        tail_frames,
        Some(acknowledgment(
            project,
            context,
            clip_id,
            sample_rate,
            tail_frames,
        )),
    )
}
fn plan(project: &Project, tail: u32) -> OccurrencePlan {
    admitted_plan(project, context(), project.clips[0].id, 8000, tail).unwrap()
}
fn on_frames(plan: &OccurrencePlan) -> Vec<u64> {
    plan.local_events
        .iter()
        .filter(|e| matches!(e.kind, TimelineEventKind::NoteOn { .. }))
        .map(|e| e.frame)
        .collect()
}

#[test]
fn occurrence_follow_extent_grows_and_shrinks_without_materializing_or_losing_source_events() {
    let mut p = project();
    p.clips[0].pattern_length_policy = Some(crate::model::PatternLengthPolicy::Follow);
    p.patterns[0].channel_steps.fill([false; 16]);
    p.patterns[0].notes = vec![PianoNote {
        id: 91,
        channel_id: Some(p.channels[0].id),
        group_id: None,
        note: 64,
        start: 0.5,
        length: 0.25,
        velocity: 0.8,
        selected: false,
        muted: false,
    }];
    let initial = plan(&p, 0);
    assert_eq!(
        (
            initial.length_beats,
            initial.end_frame - initial.start_frame
        ),
        (4.0, 16_000)
    );
    let mut later = p.patterns[0].notes[0].clone();
    later.id = 92;
    later.start = 6.0;
    p.patterns[0].notes.push(later);
    let before = project_snapshot(&p).unwrap();
    let grown = plan(&p, 7);
    assert_eq!(
        (grown.length_beats, grown.end_frame - grown.start_frame),
        (8.0, 32_000)
    );
    assert_eq!(on_frames(&grown), vec![2_000, 24_000]);
    assert_eq!(
        p.clips[0].length, 1.0,
        "Follow is resolved, not a new cache-writing edit"
    );
    assert_eq!(project_snapshot(&p).unwrap(), before);
    p.patterns[0].notes.pop();
    let shrunk = plan(&p, 0);
    assert_eq!(
        (shrunk.length_beats, shrunk.end_frame - shrunk.start_frame),
        (4.0, 16_000)
    );
    assert_eq!(on_frames(&shrunk), vec![2_000]);
}

#[test]
fn occurrence_policy_only_clock_change_invalidates_callback_fingerprint_and_tail_acknowledgment() {
    let mut p = project();
    p.clips[0].length = 9.0;
    p.patterns[0].channel_steps.fill([false; 16]);
    p.patterns[0].notes = vec![PianoNote {
        id: 91,
        channel_id: Some(p.channels[0].id),
        group_id: None,
        note: 64,
        start: 0.5,
        length: 0.25,
        velocity: 0.8,
        selected: false,
        muted: false,
    }];
    let old = plan(&p, 0);
    let old_ack = acknowledgment(&p, context(), p.clips[0].id, 8_000, 0);
    let fingerprint = timeline_compile_fingerprint(&p, 8_000);
    p.clips[0].pattern_length_policy = Some(crate::model::PatternLengthPolicy::Manual);
    assert_ne!(timeline_compile_fingerprint(&p, 8_000), fingerprint);
    assert_eq!(on_frames(&old), vec![2_000]);
    assert_eq!(on_frames(&plan(&p, 0)), vec![2_000, 18_000, 34_000]);
    assert_eq!(
        old.check_current(&p, context()).unwrap_err().kind,
        OccurrenceErrorKind::Stale
    );
    let rejected = OccurrencePlan::prepare(&p, context(), p.clips[0].id, 8_000, 0, Some(old_ack))
        .err()
        .expect("old policy acknowledgment must be rejected");
    assert_eq!(rejected.kind, OccurrenceErrorKind::Unsupported);
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
    assert!(admitted_plan(&good, context(), good.clips[0].id, 8000, 0).is_ok());
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
        let e = admitted_plan(&p, context(), p.clips[0].id, 8000, 0)
            .err()
            .unwrap();
        assert_eq!(e.kind, kind, "case {case}: {e:?}");
        assert!(e.reason.contains(reason), "case {case}: {e:?}");
        assert_eq!(serde_json::to_vec(&p).unwrap(), before);
    }
}

#[test]
fn occurrence_plan_rejects_every_other_active_or_latent_generator_use() {
    for (start, length, muted) in [
        (0.0, 0.25, false),
        (0.75, 0.25, false),
        (1.5, 0.25, false),
        (2.0, 0.25, false),
        (9.0, 0.25, false),
        (9.0, 0.25, true),
    ] {
        let mut p = project();
        let mut other = p.clips[0].clone();
        other.id = 55;
        other.start = start;
        other.length = length;
        other.muted = muted;
        p.clips.push(other);
        let error = admitted_plan(&p, context(), p.clips[0].id, 8000, 0)
            .err()
            .unwrap();
        assert_eq!(error.kind, OccurrenceErrorKind::Unsupported);
        assert!(error.reason.contains("sole-use conversion"));
    }
    for insert in [false, true] {
        let mut p = project();
        if insert {
            p.mixer_insert_slots.push(MixerInsertSlotRef {
                track: p.channels[0].mixer_track,
                slot: 0,
                plugin_instance_id: 41,
            });
        } else {
            let mut other = p.channels[0].clone();
            other.id = 99;
            p.channels.push(other);
            p.patterns[0].channel_steps.push([false; 16]);
        }
        let error = admitted_plan(&p, context(), p.clips[0].id, 8000, 0)
            .err()
            .unwrap();
        assert!(error.reason.contains(if insert {
            "Mixer insert"
        } else {
            "exactly one Channel assignment"
        }));
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
            project_revision: 3,
            ..context()
        },
        OccurrenceContext {
            parameter_control_request: 1,
            ..context()
        },
    ] {
        let e = plan.check_current(&p, changed).unwrap_err();
        assert_eq!(e.kind, OccurrenceErrorKind::Stale);
        assert!(e.reason.contains("source/runtime stamp"));
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
                p.channels[0].volume = 1.0001;
                (OccurrenceErrorKind::Unsupported, "Channel volume in 0..=1")
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
        let error = admitted_plan(&p, context(), p.clips[0].id, 8000, 0)
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
                (
                    OccurrenceErrorKind::Context,
                    "actual occurrence source/runtime stamp",
                )
            }
            4 => {
                now.project_revision = 0;
                (
                    OccurrenceErrorKind::Context,
                    "actual occurrence source/runtime stamp",
                )
            }
            _ => {
                rate = 192000;
                p.song_length_beats = 512.0;
                p.clips[0].length = 256.0;
                (OccurrenceErrorKind::Capacity, "24-million-frame")
            }
        };
        let before = project_snapshot(&p).unwrap();
        let error = admitted_plan(&p, now, p.clips[0].id, rate, tail)
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
    assert_eq!(
        select(timeline.events()).unwrap().as_slice(),
        plan.local_events.as_ref()
    );
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

#[test]
fn occurrence_requires_explicit_source_bound_acknowledgment_even_for_zero_tail() {
    let p = project();
    let id = p.clips[0].id;
    for tail in [0, 127] {
        let consent = acknowledgment(&p, context(), id, 8000, tail);
        OccurrencePlan::prepare(&p, context(), id, 8000, tail, Some(consent.clone())).unwrap();
        for case in 0..7 {
            let mut stamp = consent.stamp();
            let mut clip_id = consent.clip_id();
            let mut sample_rate = consent.sample_rate();
            let mut tail_frames = consent.tail_frames();
            let mut snapshot = Arc::clone(consent.snapshot());
            match case {
                0 => {}
                1 => tail_frames += 1,
                2 => clip_id += 1,
                3 => stamp.project_revision += 1,
                4 => snapshot = Arc::from(&b"different acknowledged source"[..]),
                5 => stamp.project_session += 1,
                _ => sample_rate += 1,
            }
            let changed = Arc::new(
                FixedTailAcknowledgment::review(snapshot, stamp, clip_id, sample_rate, tail_frames)
                    .unwrap(),
            );
            let before = project_snapshot(&p).unwrap();
            let error = OccurrencePlan::prepare(
                &p,
                context(),
                id,
                8000,
                tail,
                if case == 0 { None } else { Some(changed) },
            )
            .err()
            .unwrap();
            assert_eq!(error.kind, OccurrenceErrorKind::Unsupported);
            assert_eq!(
                error.reason,
                if case == 0 {
                    "Explicit fixed-tail truncation acknowledgment is required"
                } else {
                    "Fixed-tail acknowledgment does not match the exact source/context/tail"
                }
            );
            assert_eq!(project_snapshot(&p).unwrap(), before);
        }
    }
}

#[test]
fn occurrence_snapshot_rejects_optional_nonfinite_dormant_fade_references() {
    let mut p = project();
    let reference = crate::model::AudioFadeReference {
        offset_beats: 0.0,
        length_beats: 1.0,
        end_limit_beats: Some(1.0),
        export_length_beats: Some(1.0),
    };
    p.clips[0].fade_in_reference = Some(reference);
    let _ = plan(&p, 0);
    for muted in [false, true] {
        for field in [0, 1] {
            for nonfinite in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let mut dormant = p.clips[0].clone();
                dormant.id = 701;
                dormant.muted = muted;
                dormant.start = 4.0;
                let mut fade = reference;
                if field == 0 {
                    fade.end_limit_beats = Some(nonfinite);
                } else {
                    fade.export_length_beats = Some(nonfinite);
                }
                dormant.fade_in_reference = Some(fade);
                let mut changed = p.clone();
                changed.clips.push(dormant);
                let before = serde_json::to_vec(&changed).unwrap();
                let error = project_snapshot(&changed).unwrap_err();
                assert_eq!(error.kind, OccurrenceErrorKind::Source);
                assert_eq!(
                    error.reason,
                    "Project snapshot changes during serialization round-trip"
                );
                assert_eq!(serde_json::to_vec(&changed).unwrap(), before);
                let saved = changed.clips.last().unwrap().fade_in_reference.unwrap();
                assert_eq!(
                    if field == 0 {
                        saved.end_limit_beats
                    } else {
                        saved.export_length_beats
                    }
                    .unwrap()
                    .to_bits(),
                    nonfinite.to_bits()
                );
            }
        }
    }
}

#[test]
fn occurrence_request_shares_one_snapshot_and_supports_a_short_clip_in_a_long_song() {
    let mut project = project();
    project.song_length_beats = 2000.0;
    let plan = plan(&project, 129);
    assert!(plan.tempo_map.duration_frames() > 8000 * 600);
    assert_eq!(
        plan.request.song_frames(),
        plan.end_frame - plan.start_frame
    );
    assert!(Arc::ptr_eq(&plan.snapshot, plan.request.snapshot()));
    assert!(Arc::ptr_eq(
        &plan.tail_acknowledgment,
        plan.request.consent()
    ));
    assert_eq!(plan.request.local_events(), plan.local_events.as_ref());
    assert_eq!(plan.request.start_frame(), 4000);
}

#[test]
fn every_actual_stamp_component_fences_restored_project_bytes() {
    let project = project();
    let plan = plan(&project, 0);
    for field in 0..15 {
        let mut changed = context();
        match field {
            0 => changed.project_session += 1,
            1 => changed.project_revision += 2,
            2 => changed.timeline_revision += 1,
            3 => changed.epoch += 1,
            4 => changed.engine_session += 1,
            5 => changed.endpoint_id += 1,
            6 => changed.channel_id += 1,
            7 => changed.instance_id += 1,
            8 => changed.mixer_track_id += 1,
            9 => changed.native_generation += 1,
            10 => changed.app_native_dirty_revision += 1,
            11 => changed.native_capture_serial += 1,
            12 => changed.parameter_capture_serial += 1,
            13 => changed.native_control_request += 1,
            _ => changed.parameter_control_request += 1,
        }
        assert_eq!(
            plan.check_current(&project, changed).unwrap_err().kind,
            OccurrenceErrorKind::Stale
        );
    }
}

#[test]
fn sole_use_refuses_unplaced_patterns_and_their_muted_notes() {
    for muted in [false, true] {
        for step in [false, true] {
            let mut project = project();
            let mut other = project.patterns[0].clone();
            other.id = 702;
            other.channel_steps[0] = [false; 16];
            if step {
                other.channel_steps[0][1] = true;
            } else {
                other.notes.push(PianoNote {
                    id: 888,
                    channel_id: Some(1),
                    group_id: None,
                    note: 60,
                    start: 0.0,
                    length: 0.25,
                    velocity: 0.75,
                    selected: false,
                    muted,
                });
            }
            project.patterns.push(other);
            let error = admitted_plan(&project, context(), project.clips[0].id, 8000, 0)
                .err()
                .unwrap();
            assert_eq!(error.kind, OccurrenceErrorKind::Unsupported);
            assert!(
                error
                    .reason
                    .contains("persisted Pattern uses the retained generator")
            );
        }
    }
}
