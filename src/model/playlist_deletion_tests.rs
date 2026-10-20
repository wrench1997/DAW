//! Model regressions for source ownership, routes, grouping, and persistence.
use super::*;
use crate::{
    automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
    model::{AudioAsset, AudioClipMixerDestination, Clip, ProjectAutomation},
};

fn lane(id: u64, target: AutomationTarget, first: f64, last: f64) -> ProjectAutomation {
    let mut lane = AutomationLane::new(target);
    lane.set_curve(AutomationCurve::Tension);
    lane.replace_points([
        AutomationPoint::with_tension(0.0, first, 0.3),
        AutomationPoint::new(32.0, last),
    ]);
    ProjectAutomation {
        id,
        name: format!("Keep complete source {id}"),
        lane,
    }
}

fn clip(id: u32, kind: ClipKind, automation_id: Option<u64>) -> Clip {
    let mut clip = Project::default().clips[0].clone();
    clip.id = id;
    clip.track = 0;
    clip.start = id as f32;
    clip.length = 4.0;
    clip.kind = kind;
    clip.automation_id = automation_id;
    clip.group_id = None;
    clip
}

fn project() -> Project {
    let mut project = Project::blank();
    project.automation_lanes = vec![
        lane(500, AutomationTarget::Tempo, 120.0, 90.0),
        lane(501, AutomationTarget::MasterVolume, 0.8, 0.2),
        lane(600, AutomationTarget::MasterPan, -0.5, 0.5),
        lane(601, AutomationTarget::Swing, 0.0, 0.3),
    ];
    project.clips = vec![
        clip(1, ClipKind::Pattern, None),
        clip(10, ClipKind::Automation, Some(600)),
        clip(11, ClipKind::Automation, Some(600)),
        clip(12, ClipKind::Automation, Some(601)),
    ];
    project
}

fn json(value: &(impl serde::Serialize + ?Sized)) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

#[test]
fn playlist_delete_pattern_preserves_all_global_and_placed_sources_exactly() {
    let mut project = project();
    let lanes = json(&project.automation_lanes);
    let patterns = json(&project.patterns);
    let surviving_clips = json(&project.clips[1..]);
    assert!(project.delete_playlist_clips(&HashSet::from([1])));
    assert_eq!(json(&project.automation_lanes), lanes);
    assert_eq!(json(&project.patterns), patterns);
    assert_eq!(json(&project.clips), surviving_clips);
}

#[test]
fn playlist_delete_shared_source_keeps_muted_placement_then_removes_final_source() {
    let mut project = project();
    project.clips[2].muted = true;
    let lanes = json(&project.automation_lanes);
    let expected = project
        .automation_lanes
        .iter()
        .filter(|lane| lane.id != 600)
        .cloned()
        .collect::<Vec<_>>();
    assert!(project.delete_playlist_clips(&HashSet::from([10])));
    assert_eq!(json(&project.automation_lanes), lanes);
    assert!(project.delete_playlist_clips(&HashSet::from([11])));
    assert_eq!(json(&project.automation_lanes), json(&expected));
    assert_eq!(
        project.clips.iter().map(|clip| clip.id).collect::<Vec<_>>(),
        [1, 12]
    );
}

#[test]
fn playlist_delete_batch_removes_only_newly_orphaned_sources() {
    let mut project = project();
    let globals = json(&project.automation_lanes[..2]);
    assert!(project.delete_playlist_clips(&HashSet::from([1, 10, 11, 12])));
    assert!(project.clips.is_empty());
    assert_eq!(json(&project.automation_lanes), globals);
}

#[test]
fn playlist_delete_empty_and_stale_selection_are_exact_noops() {
    let mut project = project();
    let before = json(&project);
    for ids in [HashSet::new(), HashSet::from([0, 999])] {
        assert!(!project.delete_playlist_clips(&ids));
        assert_eq!(json(&project), before);
    }
}

#[test]
fn playlist_delete_source_ownership_matches_automation_clip_kind() {
    let mut project = project();
    // Tolerate stale fields without treating a Pattern as an Automation placement.
    project.clips[0].automation_id = Some(500);
    let lanes = json(&project.automation_lanes);
    assert!(project.delete_playlist_clips(&HashSet::from([1])));
    assert_eq!(json(&project.automation_lanes), lanes);
    project.clips.push(clip(2, ClipKind::Pattern, Some(600)));
    assert!(project.delete_playlist_clips(&HashSet::from([10, 11])));
    assert!(!project.automation_lanes.iter().any(|lane| lane.id == 600));
    assert!(project.automation_lanes.iter().any(|lane| lane.id == 500));
}

#[test]
fn playlist_delete_audio_removes_only_owned_route_and_preserves_media_and_lanes() {
    let mut project = project();
    for id in [20, 21] {
        let mut clip = clip(id, ClipKind::Audio, None);
        clip.audio_asset_id = Some(700);
        clip.audio_source_offset_frame = Some(0);
        project.clips.push(clip);
        project
            .audio_clip_mixer_destinations
            .push(AudioClipMixerDestination {
                clip_id: id,
                mixer_track_id: project.mixer_tracks[1].id,
            });
    }
    project.audio_assets.push(AudioAsset {
        id: 700,
        name: "Shared imported source".into(),
        path: "missing-for-model-test.wav".into(),
        sample_rate: 48_000,
        channels: 1,
        bits_per_sample: 32,
        frames: 192_000,
        waveform_peaks: Vec::new(),
    });
    project.validate_mixer_graph().unwrap();
    let lanes = json(&project.automation_lanes);
    let assets = json(&project.audio_assets);
    let kept_route = json(&project.audio_clip_mixer_destinations[1..]);
    assert!(project.delete_playlist_clips(&HashSet::from([20])));
    assert_eq!(json(&project.automation_lanes), lanes);
    assert_eq!(json(&project.audio_assets), assets);
    assert_eq!(json(&project.audio_clip_mixer_destinations), kept_route);
    project.validate_mixer_graph().unwrap();
}

#[test]
fn playlist_delete_normalizes_surviving_group_without_expanding_selection() {
    let mut project = project();
    project.clips[1].group_id = Some(77);
    project.clips[2].group_id = Some(77);
    assert!(project.delete_playlist_clips(&HashSet::from([10])));
    let survivor = project.clips.iter().find(|clip| clip.id == 11).unwrap();
    assert_eq!(survivor.group_id, None);
    assert!(project.automation_lanes.iter().any(|lane| lane.id == 600));
}

#[test]
fn playlist_delete_save_reopen_preserves_global_payload_and_final_source_removal() {
    let mut project = project();
    let expected_globals = json(&project.automation_lanes[..2]);
    let path = std::env::temp_dir().join(format!(
        "citrus-playlist-delete-{}-{}.citrus",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    project.save(&path).unwrap();
    project = Project::load(&path).unwrap();
    assert_eq!(json(&project.automation_lanes[..2]), expected_globals);
    assert!(project.delete_playlist_clips(&HashSet::from([10, 11, 12])));
    project.save(&path).unwrap();
    let loaded = Project::load(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(json(&loaded.automation_lanes), expected_globals);
    assert_eq!(json(&loaded.clips), json(&project.clips));
    assert_eq!(loaded.clips.len(), 1);
}
