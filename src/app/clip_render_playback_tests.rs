//! Source-only projection tests using the actual device-free conversion owner.
//! Setup stops at exact source join, not the formerly failing final Timeline.
//! All workers and generated media remain under Fixture's bounded retained cleanup.
use super::*;
use crate::app::clip_render_glue::tests::authoritative_mock_restoration_factory;
use crate::app::clip_render_job::{ClipRenderPhase, tests::Fixture};
use crate::model::ProjectAutomation;
use crate::timeline::TimelineEventKind;

fn joined_conversion() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.begin();
    fixture.until(Fixture::original_source_joined);
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().pending_generator_chains.is_empty());
    assert!(fixture.app().clip_render_restorations.contains_key(&41));
    assert!(!fixture.app().project.plugin_instances[0].enabled);
    fixture
}

fn compile_playback(project: &Project, mode: TransportMode) -> CompiledTimeline {
    let map = TempoMap::from_project(project, 48_000).unwrap();
    CompiledTimeline::from_project(
        project,
        &map,
        transport_timeline_compile_options(project, mode),
    )
    .unwrap()
}

fn bytes<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn automation(id: u64, target: AutomationTarget) -> ProjectAutomation {
    let mut lane = AutomationLane::new(target);
    lane.insert_point(AutomationPoint::new(0.0, 0.25)).unwrap();
    lane.insert_point(AutomationPoint::new(1.0, 0.75)).unwrap();
    ProjectAutomation {
        id,
        name: format!("Projection lane {id}"),
        lane,
    }
}

fn note(id: u64, channel: u32, pitch: u8, start: f32, length: f32) -> PianoNote {
    PianoNote {
        id,
        channel_id: Some(channel),
        group_id: None,
        note: pitch,
        start,
        length,
        velocity: 0.8,
        selected: false,
        muted: false,
    }
}

fn add_unrelated_native_channel(project: &mut Project) -> u32 {
    let mut channel = project.channels[0].clone();
    channel.id = project
        .channels
        .iter()
        .map(|channel| channel.id)
        .max()
        .unwrap()
        + 1;
    channel.name = "Unrelated native source".into();
    channel.instrument_plugin_instance_id = None;
    channel.muted = false;
    channel.steps.fill(false);
    let id = channel.id;
    project.channels.push(channel);
    for pattern in &mut project.patterns {
        pattern.channel_steps.push([false; 16]);
    }
    id
}

fn note_edges(timeline: &CompiledTimeline, channel: u32) -> Vec<(u64, bool, u8)> {
    timeline
        .events()
        .iter()
        .filter_map(|event| match event.kind {
            TimelineEventKind::NoteOn {
                channel_id, note, ..
            } if channel_id == channel => Some((event.frame, true, note)),
            TimelineEventKind::NoteOff {
                channel_id, note, ..
            } if channel_id == channel => Some((event.frame, false, note)),
            _ => None,
        })
        .collect()
}

#[test]
fn exact_disabled_projection_preserves_persisted_project_and_audio_route_gain() {
    let mut fixture = joined_conversion();
    let channel = fixture.app().project.channels[0].id;
    let other = add_unrelated_native_channel(&mut fixture.app_mut().project);
    fixture.app_mut().project.automation_lanes = vec![
        automation(
            101,
            AutomationTarget::PluginParameter {
                instance: 41,
                parameter: 17,
            },
        ),
        automation(102, AutomationTarget::ChannelVolume { channel }),
        automation(103, AutomationTarget::ChannelPan { channel }),
        automation(104, AutomationTarget::ChannelMute { channel }),
        automation(105, AutomationTarget::ChannelMute { channel: other }),
        automation(106, AutomationTarget::MasterVolume),
    ];
    let before = fixture.bytes();
    let source = fixture.app().project.clone();
    let playback = fixture.app().clip_timeline_playback_project().unwrap();
    assert_eq!(fixture.bytes(), before);
    assert_eq!(source.channels[0].instrument_plugin_instance_id, Some(41));
    assert_eq!(playback.channels[0].instrument_plugin_instance_id, None);
    assert!(playback.channels[0].muted);
    assert_eq!(
        playback.channels[0].volume.to_bits(),
        source.channels[0].volume.to_bits()
    );
    assert_eq!(
        playback.channels[0].pan.to_bits(),
        source.channels[0].pan.to_bits()
    );
    assert_eq!(
        playback.channels[0].mixer_track,
        source.channels[0].mixer_track
    );
    assert_eq!(bytes(&playback.channels[1]), bytes(&source.channels[1]));
    assert_eq!(
        bytes(&playback.plugin_instances),
        bytes(&source.plugin_instances)
    );
    assert_eq!(bytes(&playback.clips), bytes(&source.clips));
    assert_eq!(bytes(&playback.audio_assets), bytes(&source.audio_assets));
    assert_eq!(
        playback.audio_clip_mixer_destinations,
        source.audio_clip_mixer_destinations
    );
    for id in 101..=104 {
        assert!(
            !playback
                .automation_lanes
                .iter()
                .find(|lane| lane.id == id)
                .unwrap()
                .lane
                .is_enabled()
        );
        assert!(
            source
                .automation_lanes
                .iter()
                .find(|lane| lane.id == id)
                .unwrap()
                .lane
                .is_enabled()
        );
    }
    for id in [105, 106] {
        assert_eq!(
            bytes(
                playback
                    .automation_lanes
                    .iter()
                    .find(|lane| lane.id == id)
                    .unwrap()
            ),
            bytes(
                source
                    .automation_lanes
                    .iter()
                    .find(|lane| lane.id == id)
                    .unwrap()
            )
        );
    }
    let original_timeline = compile_playback(&source, TransportMode::Song);
    assert!(
        original_timeline
            .plugin_routes()
            .iter()
            .any(|route| route.instance_id == 41),
        "control proves the ordinary compiler still emits this disabled route"
    );
    let projected_timeline = compile_playback(&playback, TransportMode::Song);
    assert!(
        !projected_timeline
            .plugin_routes()
            .iter()
            .any(|route| route.instance_id == 41)
    );
    assert_eq!(
        projected_timeline.audio_clips(),
        original_timeline.audio_clips()
    );
    assert_eq!(projected_timeline.audio_clips().len(), 1);
    assert!(note_edges(&projected_timeline, channel).is_empty());
    assert!(
        !projected_timeline
            .driven_automation_targets()
            .iter()
            .any(|target| match target {
                CompiledAutomationTarget::PluginParameter { instance_id, .. } => *instance_id == 41,
                CompiledAutomationTarget::ChannelVolume { channel_id }
                | CompiledAutomationTarget::ChannelPan { channel_id }
                | CompiledAutomationTarget::ChannelMute { channel_id } => *channel_id == channel,
                _ => false,
            })
    );
}

#[test]
fn ordinary_unbound_disabled_source_keeps_existing_compiler_semantics() {
    let mut fixture = Fixture::new();
    fixture.app_mut().project.plugin_instances[0].enabled = false;
    assert!(fixture.app().clip_render_restorations.is_empty());
    let ordinary = transport_playback_project(&fixture.app().project, fixture.app().transport_mode);
    let playback = fixture.app().clip_timeline_playback_project().unwrap();
    assert_eq!(bytes(&playback), bytes(&ordinary));
    assert_eq!(playback.channels[0].instrument_plugin_instance_id, Some(41));
    assert!(
        compile_playback(&playback, TransportMode::Song)
            .plugin_routes()
            .iter()
            .any(|route| route.instance_id == 41)
    );
    assert_eq!(
        fixture.app().clip_timeline_fingerprint(48_000),
        timeline_playback_fingerprint(&fixture.app().project, fixture.app().transport_mode, 48_000)
    );
}

#[test]
fn wrong_bound_descriptor_path_or_session_fails_without_rewriting_project() {
    let mut fixture = joined_conversion();
    let original = fixture.app().project.clone();
    let session = fixture.app().project_session;
    let fingerprint = fixture.app().clip_timeline_fingerprint(48_000);
    for mismatch in ["descriptor", "path", "session"] {
        fixture.app_mut().project = original.clone();
        fixture.app_mut().project_session = session;
        match mismatch {
            "descriptor" => fixture.app_mut().project.plugin_instances[0]
                .uid
                .push_str("-changed"),
            "path" => {
                fixture.app_mut().project.plugin_instances[0].path =
                    "/fixture-only/wrong.vst3".into()
            }
            "session" => fixture.app_mut().project_session = session + 1,
            _ => unreachable!(),
        }
        let before = fixture.bytes();
        assert_ne!(
            fixture.app().clip_timeline_fingerprint(48_000),
            fingerprint,
            "{mismatch} must invalidate previously compiled source identity"
        );
        let error = fixture.app().clip_timeline_playback_project().unwrap_err();
        assert!(
            error.starts_with("Converted playback projection"),
            "{mismatch}: {error}"
        );
        assert_eq!(fixture.bytes(), before);
    }
}

#[test]
fn exact_binding_readmission_changes_projection_revision_without_changing_project_bytes() {
    let mut fixture = joined_conversion();
    let before = fixture.bytes();
    let binding = fixture.app().clip_render_restorations[&41].clone();
    let revision = fixture.app().clip_timeline_projection_revision;
    fixture
        .app_mut()
        .refresh_timeline_fingerprint(Instant::now(), true);
    let generation = fixture.app().timeline_desired_generation;
    let fingerprint = fixture.app().clip_timeline_fingerprint(48_000);
    // The already-owned exact binding is admitted again under its existing key;
    // no candidate worker or Project mutation is manufactured for this check.
    fixture.app_mut().install_clip_render_restoration(binding);
    assert_eq!(
        fixture.app().clip_timeline_projection_revision,
        revision + 1
    );
    assert_eq!(fixture.bytes(), before);
    assert_ne!(fixture.app().clip_timeline_fingerprint(48_000), fingerprint);
    fixture
        .app_mut()
        .refresh_timeline_fingerprint(Instant::now(), true);
    assert_eq!(fixture.app().timeline_desired_generation, generation + 1);
    assert_eq!(
        fixture.app().timeline_observed_fingerprint,
        fixture.app().clip_timeline_fingerprint(48_000)
    );
}

#[test]
fn ambiguous_duplicate_or_shared_bound_source_ownership_fails_closed() {
    let mut fixture = joined_conversion();
    let original = fixture.app().project.clone();
    for ambiguity in [
        "duplicate instance",
        "shared channel",
        "duplicate channel",
        "mixer insert",
    ] {
        fixture.app_mut().project = original.clone();
        let project = &mut fixture.app_mut().project;
        match ambiguity {
            "duplicate instance" => project
                .plugin_instances
                .push(project.plugin_instances[0].clone()),
            "shared channel" => {
                let mut shared = project.channels[0].clone();
                shared.id += 1;
                project.channels.push(shared);
            }
            "duplicate channel" => {
                let mut duplicate = project.channels[0].clone();
                duplicate.instrument_plugin_instance_id = None;
                project.channels.push(duplicate);
            }
            "mixer insert" => project
                .mixer_insert_slots
                .push(crate::model::MixerInsertSlotRef {
                    track: project.channels[0].mixer_track,
                    slot: 0,
                    plugin_instance_id: 41,
                }),
            _ => unreachable!(),
        }
        let before = fixture.bytes();
        let error = fixture.app().clip_timeline_playback_project().unwrap_err();
        assert!(
            error.starts_with("Converted playback projection"),
            "{ambiguity}: {error}"
        );
        assert_eq!(fixture.bytes(), before);
    }
}

#[test]
fn pattern_mode_retained_midi_cannot_fall_back_to_native_notes() {
    let mut fixture = joined_conversion();
    let channel = fixture.app().project.channels[0].id;
    let other = add_unrelated_native_channel(&mut fixture.app_mut().project);
    let project = &mut fixture.app_mut().project;
    project.channels[0].steps[0] = true;
    project.patterns[0].channel_steps[0][0] = true;
    project.patterns[0].notes = vec![
        note(901, channel, 60, 0.0, 20.0),
        note(902, other, 72, 0.5, 0.25),
    ];
    project.piano_notes = vec![note(903, channel, 64, 0.0, 24.0)];
    fixture.app_mut().transport_mode = TransportMode::Pattern;
    let before = fixture.bytes();
    let legacy_before = bytes(&fixture.app().project.piano_notes);
    let ordinary = transport_playback_project(&fixture.app().project, TransportMode::Pattern);
    let playback = fixture.app().clip_timeline_playback_project().unwrap();
    assert_eq!(fixture.bytes(), before);
    assert_eq!(bytes(&fixture.app().project.piano_notes), legacy_before);
    assert_eq!(playback.song_length_beats, ordinary.song_length_beats);
    assert_eq!(bytes(&playback.clips), bytes(&ordinary.clips));
    assert!(playback.audio_assets.is_empty());
    assert!(playback.channels[0].steps.iter().all(|step| !step));
    assert!(
        playback.patterns[0].channel_steps[0]
            .iter()
            .all(|step| !step)
    );
    assert_eq!(
        playback.patterns[0].channel_steps.len(),
        ordinary.patterns[0].channel_steps.len()
    );
    for (original, projected) in ordinary.patterns[0]
        .notes
        .iter()
        .zip(&playback.patterns[0].notes)
    {
        assert_eq!(
            (projected.id, projected.start, projected.length),
            (original.id, original.start, original.length)
        );
        assert_eq!(
            projected.muted,
            original.channel_id == Some(channel) || original.muted
        );
    }
    assert_eq!(
        playback.patterns[0].notes.len(),
        ordinary.patterns[0].notes.len()
    );
    assert_eq!(playback.piano_notes[0].id, ordinary.piano_notes[0].id);
    assert_eq!(
        playback.piano_notes[0].length,
        ordinary.piano_notes[0].length
    );
    assert!(playback.piano_notes[0].muted);
    let ordinary_timeline = compile_playback(&ordinary, TransportMode::Pattern);
    let projected_timeline = compile_playback(&playback, TransportMode::Pattern);
    assert!(!note_edges(&ordinary_timeline, channel).is_empty());
    assert!(note_edges(&projected_timeline, channel).is_empty());
    assert!(projected_timeline.plugin_routes().is_empty());
    assert!(!note_edges(&projected_timeline, other).is_empty());
    assert_eq!(
        note_edges(&projected_timeline, other),
        note_edges(&ordinary_timeline, other)
    );
}

#[test]
fn muted_target_note_extent_preserves_unrelated_long_period_repetition_timing() {
    let mut fixture = joined_conversion();
    let channel = fixture.app().project.channels[0].id;
    let other = add_unrelated_native_channel(&mut fixture.app_mut().project);
    let project = &mut fixture.app_mut().project;
    project.song_length_beats = 48.0;
    project.swing = 0.0;
    project.patterns[0].notes = vec![
        note(911, channel, 60, 0.0, 20.0),
        note(912, other, 72, 0.5, 0.25),
    ];
    let retained = project
        .clips
        .iter_mut()
        .find(|clip| clip.kind == ClipKind::Pattern)
        .unwrap();
    retained.muted = false;
    retained.start = 0.0;
    retained.length = 45.0;
    let before = fixture.bytes();
    let ordinary = transport_playback_project(&fixture.app().project, TransportMode::Song);
    let playback = fixture.app().clip_timeline_playback_project().unwrap();
    let original_timeline = compile_playback(&ordinary, TransportMode::Song);
    let projected_timeline = compile_playback(&playback, TransportMode::Song);
    assert_eq!(fixture.bytes(), before);
    assert!(playback.patterns[0].notes[0].muted);
    assert_eq!(playback.patterns[0].notes[0].length, 20.0);
    assert!(note_edges(&projected_timeline, channel).is_empty());
    let edges = note_edges(&projected_timeline, other);
    assert_eq!(edges, note_edges(&original_timeline, other));
    let starts = edges
        .iter()
        .filter_map(|(frame, on, _)| on.then_some(*frame))
        .collect::<Vec<_>>();
    assert_eq!(starts, vec![12_000, 492_000, 972_000]);
}

#[test]
fn actual_undo_restores_enabled_route_and_redo_projects_disabled_route_away() {
    let mut fixture = joined_conversion();
    fixture.app_mut().clip_restoration_test_factory = Some(authoritative_mock_restoration_factory);
    assert!(
        compile_playback(
            &fixture.app().clip_timeline_playback_project().unwrap(),
            TransportMode::Song
        )
        .plugin_routes()
        .is_empty()
    );
    // Unlike fixture setup, this is an explicit assertion of corrected final
    // callback readiness, followed by the real history admission methods.
    fixture.until(|fixture| {
        fixture.app().clip_render_job.as_ref().unwrap().phase() == ClipRenderPhase::Complete
    });
    let committed = bytes(&fixture.app().project);
    fixture.app_mut().undo();
    assert!(fixture.app().project.plugin_instances[0].enabled);
    let playback = fixture.app().clip_timeline_playback_project().unwrap();
    assert_eq!(
        bytes(&playback),
        bytes(&transport_playback_project(
            &fixture.app().project,
            TransportMode::Song
        ))
    );
    assert!(
        compile_playback(&playback, TransportMode::Song)
            .plugin_routes()
            .iter()
            .any(|route| route.instance_id == 41)
    );
    fixture.until(|fixture| !fixture.app().clip_restoration_playback_pending);
    assert!(!fixture.app().clip_restoration_failed);
    fixture.app_mut().redo();
    assert!(!fixture.app().project.plugin_instances[0].enabled);
    assert_eq!(bytes(&fixture.app().project), committed);
    assert!(
        compile_playback(
            &fixture.app().clip_timeline_playback_project().unwrap(),
            TransportMode::Song
        )
        .plugin_routes()
        .is_empty()
    );
    fixture.until(|fixture| !fixture.app().clip_restoration_playback_pending);
    assert!(!fixture.app().clip_restoration_failed);
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().timeline_audio_matches_desired());
    assert!(!fixture.app().playing);
}

#[test]
fn same_bytes_reopen_clears_session_projection_and_forces_new_compile_identity() {
    let mut fixture = joined_conversion();
    fixture.until(|fixture| {
        fixture.app().clip_render_job.as_ref().unwrap().phase() == ClipRenderPhase::Complete
    });
    let project = fixture.app().project.clone();
    let persisted = bytes(&project);
    let session = fixture.app().project_session;
    let generation = fixture.app().timeline_desired_generation;
    let fingerprint = fixture.app().clip_timeline_fingerprint(48_000);
    assert_ne!(
        fingerprint,
        timeline_playback_fingerprint(&project, TransportMode::Song, 48_000)
    );
    // Actual degraded dispatch prevents reopening this deliberately nonexistent
    // plugin path from invoking any SDK/helper. The real session install and
    // compile identity transition still execute; plugin restart is not claimed.
    fixture
        .app_mut()
        .enter_audio_degraded("Device-free same-bytes reopen fixture".into());
    assert!(fixture.app_mut().install_project(project, None, false));
    assert_eq!(bytes(&fixture.app().project), persisted);
    assert_ne!(fixture.app().project_session, session);
    assert!(fixture.app().clip_render_restorations.is_empty());
    assert!(fixture.app().retained_clip_media.is_empty());
    assert_eq!(fixture.app().retained_clip_pcm_bytes, 0);
    assert!(fixture.app().timeline_desired_generation > generation);
    let reopened_fingerprint = fixture.app().clip_timeline_fingerprint(48_000);
    assert_ne!(reopened_fingerprint, fingerprint);
    assert_eq!(
        reopened_fingerprint,
        timeline_playback_fingerprint(&fixture.app().project, TransportMode::Song, 48_000)
    );
    assert_eq!(
        fixture.app().timeline_observed_fingerprint,
        reopened_fingerprint
    );
    let playback = fixture.app().clip_timeline_playback_project().unwrap();
    assert_eq!(playback.channels[0].instrument_plugin_instance_id, Some(41));
    assert_eq!(
        bytes(&playback),
        bytes(&transport_playback_project(
            &fixture.app().project,
            TransportMode::Song
        ))
    );
    assert!(fixture.app().running_generator_chains.is_empty());
    assert!(fixture.app().pending_generator_chains.is_empty());
}

fn fill_remaining_command_slots(fixture: &mut Fixture) -> usize {
    for admitted in 0..=64 {
        if !fixture
            .app_mut()
            .audio
            .as_mut()
            .unwrap()
            .command(AudioCommand::SetMaster(1.0))
        {
            return admitted;
        }
    }
    panic!("bounded real callback command ring did not fill");
}

#[test]
fn actual_dormant_audition_cannot_enqueue_native_note_but_unbound_disabled_keeps_existing_behavior()
{
    {
        let mut fixture = joined_conversion();
        fixture.until(|fixture| {
            fixture.app().clip_render_job.as_ref().unwrap().phase() == ClipRenderPhase::Complete
        });
        let channel = fixture.app().project.channels[0].id;
        assert!(!fixture.app().clip_operation_fence_active());
        assert!(fixture.app().clip_dormant_channel_audition_refused(channel));
        fixture.pump_callback_only();
        assert_eq!(fill_remaining_command_slots(&mut fixture), 64);
        fixture.pump_callback_only();
        let before = fixture.bytes();
        fixture
            .app_mut()
            .trigger_channel_note(channel, 60, 0.8, 0.0, 1.0, 1);
        assert_eq!(
            fill_remaining_command_slots(&mut fixture),
            64,
            "completed conversion must not enqueue a native fallback NoteOn"
        );
        assert_eq!(fixture.bytes(), before);
        assert!(fixture.app().pending_generator_note_offs.is_empty());
        assert!(fixture.app().toast.as_ref().unwrap().0.contains("Undo"));
        fixture.pump_callback_only();
        assert!(
            fixture
                .callback()
                .last_output()
                .iter()
                .flatten()
                .all(|sample| *sample == 0.0)
        );
    }
    {
        let mut fixture = Fixture::new();
        fixture.app_mut().project.plugin_instances[0].enabled = false;
        let channel = fixture.app().project.channels[0].id;
        assert!(fixture.app().clip_render_restorations.is_empty());
        assert!(!fixture.app().clip_dormant_channel_audition_refused(channel));
        assert_eq!(fixture.app().active_generator_instance(channel), None);
        fixture.pump_callback_only();
        assert_eq!(fill_remaining_command_slots(&mut fixture), 64);
        fixture.pump_callback_only();
        fixture
            .app_mut()
            .trigger_channel_note(channel, 60, 0.8, 0.0, 1.0, 1);
        assert_eq!(
            fill_remaining_command_slots(&mut fixture),
            63,
            "ordinary unbound disabled source retains its existing native NoteOn path"
        );
    }
}
