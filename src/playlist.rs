use std::{collections::HashSet, fmt};

use crate::model::{
    AudioClipMixerDestination, Clip, ClipKind, Project, ProjectAutomation,
    normalize_playlist_clip_groups,
};

/// The data a Playlist gesture can mutate, captured before its first frame.
/// Keeping related lanes and routes together prevents undo from restoring orphaned clips.
pub struct PlaylistGestureSnapshot {
    pub clips: Vec<Clip>,
    pub automation_lanes: Vec<ProjectAutomation>,
    audio_clip_mixer_destinations: Vec<AudioClipMixerDestination>,
}

impl PlaylistGestureSnapshot {
    pub fn capture(project: &Project) -> Self {
        Self {
            clips: project.clips.clone(),
            automation_lanes: project.automation_lanes.clone(),
            audio_clip_mixer_destinations: project.audio_clip_mixer_destinations.clone(),
        }
    }

    pub fn restore_into(self, mut project: Project) -> Project {
        project.clips = self.clips;
        project.automation_lanes = self.automation_lanes;
        project.audio_clip_mixer_destinations = self.audio_clip_mixer_destinations;
        project
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistEditError {
    InvalidTarget,
    InvalidNumber,
    InvalidSnap,
    InvalidBounds,
    GroupIdExhausted,
    CrossfadeSelection,
    CrossfadeAudioOnly,
    CrossfadeGeometry,
}

impl fmt::Display for PlaylistEditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTarget => "the Playlist selection no longer matches stable Clip IDs",
            Self::InvalidNumber => "a Playlist Clip or edit delta is not finite",
            Self::InvalidSnap => "the Playlist snap must be a positive finite value",
            Self::InvalidBounds => "the Playlist edit bounds are invalid",
            Self::GroupIdExhausted => "no stable Playlist Clip-group ID remains",
            Self::CrossfadeSelection => "select exactly two Playlist Clips to crossfade",
            Self::CrossfadeAudioOnly => "crossfades require two Audio Clips on the same track",
            Self::CrossfadeGeometry => {
                "the selected Audio Clips need a non-nested overlap to crossfade"
            }
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistFadeSide {
    In,
    Out,
}

#[derive(Clone, Debug)]
pub struct PlaylistCrossfadeEditResult {
    pub clips: Vec<Clip>,
    pub left_clip_id: u32,
    pub right_clip_id: u32,
    pub overlap_beats: f32,
}

#[derive(Clone, Debug)]
pub struct PlaylistGroupEditResult {
    pub clips: Vec<Clip>,
    pub selection_ids: HashSet<u32>,
    pub affected_clips: usize,
    pub group_id: Option<u64>,
}

struct ClipGroupIdAllocator {
    used: HashSet<u64>,
    next: u64,
}

impl ClipGroupIdAllocator {
    fn new(clips: &[Clip]) -> Result<Self, PlaylistEditError> {
        let used = clips
            .iter()
            .filter_map(|clip| clip.group_id)
            .collect::<HashSet<_>>();
        if used.contains(&0) {
            return Err(PlaylistEditError::InvalidTarget);
        }
        let next = used.iter().copied().max().unwrap_or(0);
        Ok(Self { used, next })
    }

    fn allocate(&mut self) -> Result<u64, PlaylistEditError> {
        loop {
            self.next = self
                .next
                .checked_add(1)
                .ok_or(PlaylistEditError::GroupIdExhausted)?;
            if self.next != 0 && self.used.insert(self.next) {
                return Ok(self.next);
            }
        }
    }
}

pub fn clip_group_members(clips: &[Clip], clip_id: u32, grouping_enabled: bool) -> HashSet<u32> {
    let Some(anchor) = clips.iter().find(|clip| clip.id == clip_id) else {
        return HashSet::new();
    };
    let Some(group_id) = anchor.group_id.filter(|_| grouping_enabled) else {
        return HashSet::from([clip_id]);
    };
    clips
        .iter()
        .filter(|clip| clip.group_id == Some(group_id))
        .map(|clip| clip.id)
        .collect()
}

pub fn expand_clip_group_selection(
    clips: &[Clip],
    selection_ids: &HashSet<u32>,
    grouping_enabled: bool,
) -> HashSet<u32> {
    selection_ids
        .iter()
        .flat_map(|clip_id| clip_group_members(clips, *clip_id, grouping_enabled))
        .collect()
}

pub fn toggle_clip_group_selection(
    clips: &[Clip],
    selection_ids: &mut HashSet<u32>,
    clip_id: u32,
    grouping_enabled: bool,
) {
    let members = clip_group_members(clips, clip_id, grouping_enabled);
    let remove = !members.is_empty() && members.iter().all(|id| selection_ids.contains(id));
    if remove {
        selection_ids.retain(|id| !members.contains(id));
    } else {
        selection_ids.extend(members);
    }
}

pub fn select_clip_group_members(
    clips: &[Clip],
    selection_ids: &mut HashSet<u32>,
    clip_id: u32,
    grouping_enabled: bool,
    additive: bool,
) {
    if !additive {
        selection_ids.clear();
    }
    selection_ids.extend(clip_group_members(clips, clip_id, grouping_enabled));
}

pub fn ensure_clip_group_selected(
    clips: &[Clip],
    selection_ids: &mut HashSet<u32>,
    clip_id: u32,
    grouping_enabled: bool,
) {
    let expanded = expand_clip_group_selection(clips, selection_ids, grouping_enabled);
    if expanded.contains(&clip_id) {
        *selection_ids = expanded;
    } else {
        select_clip_group_members(clips, selection_ids, clip_id, grouping_enabled, false);
    }
}

pub fn group_selected_clips(
    clips: &[Clip],
    selection_ids: &HashSet<u32>,
) -> Result<PlaylistGroupEditResult, PlaylistEditError> {
    validate_group_edit_target(clips, selection_ids)?;
    if selection_ids.len() < 2 {
        return Ok(PlaylistGroupEditResult {
            clips: clips.to_vec(),
            selection_ids: selection_ids.clone(),
            affected_clips: 0,
            group_id: None,
        });
    }
    let group_id = ClipGroupIdAllocator::new(clips)?.allocate()?;
    let mut result = clips.to_vec();
    for clip in &mut result {
        if selection_ids.contains(&clip.id) {
            clip.group_id = Some(group_id);
        }
    }
    normalize_playlist_clip_groups(&mut result);
    Ok(PlaylistGroupEditResult {
        clips: result,
        selection_ids: selection_ids.clone(),
        affected_clips: selection_ids.len(),
        group_id: Some(group_id),
    })
}

pub fn ungroup_selected_clips(
    clips: &[Clip],
    selection_ids: &HashSet<u32>,
) -> Result<PlaylistGroupEditResult, PlaylistEditError> {
    validate_group_edit_target(clips, selection_ids)?;
    let mut result = clips.to_vec();
    let mut affected_clips = 0;
    for clip in &mut result {
        if selection_ids.contains(&clip.id) && clip.group_id.take().is_some() {
            affected_clips += 1;
        }
    }
    normalize_playlist_clip_groups(&mut result);
    Ok(PlaylistGroupEditResult {
        clips: result,
        selection_ids: selection_ids.clone(),
        affected_clips,
        group_id: None,
    })
}

fn validate_group_edit_target(
    clips: &[Clip],
    selection_ids: &HashSet<u32>,
) -> Result<(), PlaylistEditError> {
    let mut seen = HashSet::with_capacity(clips.len());
    let mut matched = 0;
    for clip in clips {
        if clip.id == 0 || !seen.insert(clip.id) {
            return Err(PlaylistEditError::InvalidTarget);
        }
        matched += usize::from(selection_ids.contains(&clip.id));
    }
    if matched != selection_ids.len() {
        return Err(PlaylistEditError::InvalidTarget);
    }
    Ok(())
}

pub fn clamp_group_move_delta(
    clips: &[Clip],
    requested_time_delta: f32,
    requested_track_delta: isize,
    song_length_beats: f32,
    track_count: usize,
) -> Result<(f32, isize), PlaylistEditError> {
    if !requested_time_delta.is_finite()
        || !song_length_beats.is_finite()
        || song_length_beats <= 0.0
        || track_count == 0
    {
        return Err(PlaylistEditError::InvalidBounds);
    }
    if clips.is_empty() {
        return Ok((requested_time_delta, requested_track_delta));
    }
    if clips.iter().any(|clip| {
        clip.id == 0
            || !clip.start.is_finite()
            || !clip.length.is_finite()
            || clip.start < 0.0
            || clip.length <= 0.0
            || clip.start + clip.length > song_length_beats
            || clip.track >= track_count
    }) {
        return Err(PlaylistEditError::InvalidTarget);
    }
    let minimum_start = clips
        .iter()
        .map(|clip| clip.start)
        .reduce(f32::min)
        .unwrap();
    let maximum_end = clips
        .iter()
        .map(|clip| clip.start + clip.length)
        .reduce(f32::max)
        .unwrap();
    let minimum_track = clips.iter().map(|clip| clip.track).min().unwrap();
    let maximum_track = clips.iter().map(|clip| clip.track).max().unwrap();
    Ok((
        requested_time_delta.clamp(-minimum_start, song_length_beats - maximum_end),
        requested_track_delta.clamp(
            -(minimum_track as isize),
            (track_count - 1 - maximum_track) as isize,
        ),
    ))
}

pub fn clamp_group_resize_delta(
    clips: &[Clip],
    requested_length_delta: f32,
    minimum_length: f32,
    song_length_beats: f32,
) -> Result<f32, PlaylistEditError> {
    if !requested_length_delta.is_finite()
        || !minimum_length.is_finite()
        || minimum_length <= 0.0
        || !song_length_beats.is_finite()
        || song_length_beats <= 0.0
    {
        return Err(PlaylistEditError::InvalidBounds);
    }
    if clips.is_empty() {
        return Ok(requested_length_delta);
    }
    if clips.iter().any(|clip| {
        clip.id == 0
            || !clip.start.is_finite()
            || !clip.length.is_finite()
            || clip.start < 0.0
            || clip.length <= 0.0
            || clip.start + clip.length > song_length_beats
    }) {
        return Err(PlaylistEditError::InvalidTarget);
    }
    let lower = clips
        .iter()
        .map(|clip| minimum_length - clip.length)
        .reduce(f32::max)
        .unwrap();
    let upper = clips
        .iter()
        .map(|clip| song_length_beats - clip.start - clip.length)
        .reduce(f32::min)
        .unwrap();
    if lower > upper {
        return Err(PlaylistEditError::InvalidBounds);
    }
    Ok(requested_length_delta.clamp(lower, upper))
}

pub fn quantized_slip_delta(
    pointer_delta_beats: f32,
    snap_beats: f32,
    bypass_snap: bool,
) -> Result<f32, PlaylistEditError> {
    if !pointer_delta_beats.is_finite() {
        return Err(PlaylistEditError::InvalidNumber);
    }
    if bypass_snap {
        return Ok(pointer_delta_beats);
    }
    if !snap_beats.is_finite() || snap_beats <= 0.0 {
        return Err(PlaylistEditError::InvalidSnap);
    }
    // Compute in a wider domain: finite f32 inputs can overflow their ratio.
    let delta = ((f64::from(pointer_delta_beats) / f64::from(snap_beats)).round()
        * f64::from(snap_beats)) as f32;
    if !delta.is_finite() {
        return Err(PlaylistEditError::InvalidNumber);
    }
    Ok(delta)
}

pub fn slipped_looping_source_offset(
    origin: f32,
    pointer_delta_beats: f32,
    period_beats: f32,
    snap_beats: f32,
    bypass_snap: bool,
) -> Result<f32, PlaylistEditError> {
    if !origin.is_finite() || !period_beats.is_finite() || period_beats <= 0.0 {
        return Err(PlaylistEditError::InvalidBounds);
    }
    let delta = quantized_slip_delta(pointer_delta_beats, snap_beats, bypass_snap)?;
    let offset = (f64::from(origin) - f64::from(delta)).rem_euclid(f64::from(period_beats)) as f32;
    // Rounding at the wrap boundary must not produce the excluded endpoint.
    Ok(if offset >= period_beats { 0.0 } else { offset })
}

pub fn slipped_bounded_source_offset(
    origin: f32,
    pointer_delta_beats: f32,
    minimum: f32,
    maximum: f32,
    snap_beats: f32,
    bypass_snap: bool,
) -> Result<f32, PlaylistEditError> {
    if !origin.is_finite() || !minimum.is_finite() || !maximum.is_finite() || minimum > maximum {
        return Err(PlaylistEditError::InvalidBounds);
    }
    let delta = quantized_slip_delta(pointer_delta_beats, snap_beats, bypass_snap)?;
    Ok((f64::from(origin) - f64::from(delta)).clamp(f64::from(minimum), f64::from(maximum)) as f32)
}

pub fn slipped_audio_source_offset(
    origin_frame: u64,
    pointer_delta_frames: i128,
    maximum_frame: u64,
) -> u64 {
    i128::from(origin_frame)
        .saturating_sub(pointer_delta_frames)
        .clamp(0, i128::from(maximum_frame)) as u64
}

pub fn dragged_fade_fraction(
    clip: &Clip,
    side: PlaylistFadeSide,
    origin_fraction: f32,
    pointer_delta_beats: f32,
    snap_beats: f32,
    bypass_snap: bool,
) -> Result<f32, PlaylistEditError> {
    if clip.id == 0
        || !clip.start.is_finite()
        || !clip.length.is_finite()
        || clip.start < 0.0
        || clip.length <= 0.0
        || !origin_fraction.is_finite()
        || !pointer_delta_beats.is_finite()
    {
        return Err(PlaylistEditError::InvalidTarget);
    }
    if !bypass_snap && (!snap_beats.is_finite() || snap_beats <= 0.0) {
        return Err(PlaylistEditError::InvalidSnap);
    }

    let clip_end = clip.start + clip.length;
    if !clip_end.is_finite() {
        return Err(PlaylistEditError::InvalidTarget);
    }
    let origin_handle = match side {
        PlaylistFadeSide::In => clip.start + origin_fraction.clamp(0.0, 1.0) * clip.length,
        PlaylistFadeSide::Out => clip_end - origin_fraction.clamp(0.0, 1.0) * clip.length,
    };
    let raw_handle = f64::from(origin_handle) + f64::from(pointer_delta_beats);
    let handle = if bypass_snap {
        raw_handle
    } else {
        (raw_handle / f64::from(snap_beats)).round() * f64::from(snap_beats)
    }
    .clamp(f64::from(clip.start), f64::from(clip_end)) as f32;

    Ok(match side {
        PlaylistFadeSide::In => (handle - clip.start) / clip.length,
        PlaylistFadeSide::Out => (clip_end - handle) / clip.length,
    }
    .clamp(0.0, 1.0))
}

pub fn create_audio_crossfade(
    clips: &[Clip],
    selection_ids: &HashSet<u32>,
) -> Result<PlaylistCrossfadeEditResult, PlaylistEditError> {
    validate_group_edit_target(clips, selection_ids)?;
    if selection_ids.len() != 2 {
        return Err(PlaylistEditError::CrossfadeSelection);
    }
    let mut selected = clips
        .iter()
        .filter(|clip| selection_ids.contains(&clip.id))
        .collect::<Vec<_>>();
    if selected.len() != 2
        || selected.iter().any(|clip| clip.kind != ClipKind::Audio)
        || selected[0].track != selected[1].track
    {
        return Err(PlaylistEditError::CrossfadeAudioOnly);
    }
    if selected.iter().any(|clip| {
        !clip.start.is_finite()
            || !clip.length.is_finite()
            || clip.start < 0.0
            || clip.length <= 0.0
    }) {
        return Err(PlaylistEditError::InvalidTarget);
    }
    selected.sort_by(|left, right| {
        left.start
            .total_cmp(&right.start)
            .then_with(|| left.id.cmp(&right.id))
    });
    let left = selected[0];
    let right = selected[1];
    let left_end = left.start + left.length;
    let right_end = right.start + right.length;
    if !left_end.is_finite() || !right_end.is_finite() {
        return Err(PlaylistEditError::InvalidTarget);
    }
    if left.start >= right.start || right.start >= left_end || left_end >= right_end {
        return Err(PlaylistEditError::CrossfadeGeometry);
    }
    let overlap_beats = left_end - right.start;
    let mut result = clips.to_vec();
    for clip in &mut result {
        if clip.id == left.id {
            clip.fade_out = (overlap_beats / clip.length).clamp(0.0, 1.0);
        } else if clip.id == right.id {
            clip.fade_in = (overlap_beats / clip.length).clamp(0.0, 1.0);
        }
    }
    Ok(PlaylistCrossfadeEditResult {
        clips: result,
        left_clip_id: left.id,
        right_clip_id: right.id,
        overlap_beats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(id: u32, track: usize, start: f32, length: f32, group_id: Option<u64>) -> Clip {
        Clip {
            id,
            track,
            start,
            length,
            name: format!("Clip {id}"),
            color: [1, 2, 3],
            kind: ClipKind::Pattern,
            group_id,
            pattern_id: 1,
            automation_id: None,
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            muted: false,
        }
    }

    #[test]
    fn gesture_snapshot_restores_clips_automation_and_audio_routes_together() {
        use crate::automation::{AutomationLane, AutomationPoint, AutomationTarget};

        let mut project = Project::default();
        let mut lane = AutomationLane::new(AutomationTarget::MasterPan);
        lane.replace_points([AutomationPoint::new(0.0, 0.25)]);
        project.automation_lanes = vec![ProjectAutomation {
            id: 42,
            name: "Pan".into(),
            lane,
        }];
        project.audio_clip_mixer_destinations = vec![AudioClipMixerDestination {
            clip_id: 1,
            mixer_track_id: 1,
        }];
        let original = serde_json::to_value(&project).unwrap();
        let snapshot = PlaylistGestureSnapshot::capture(&project);
        project.clips.clear();
        project.automation_lanes.clear();
        project.audio_clip_mixer_destinations.clear();
        assert_eq!(
            serde_json::to_value(snapshot.restore_into(project)).unwrap(),
            original
        );
    }

    #[test]
    fn gesture_snapshot_restores_point_edits_and_removes_new_split_routes() {
        use crate::automation::{AutomationLane, AutomationPoint, AutomationTarget};

        let mut project = Project::default();
        let mut lane = AutomationLane::new(AutomationTarget::MasterPan);
        lane.replace_points([AutomationPoint::new(0.0, 0.25)]);
        project.automation_lanes = vec![ProjectAutomation {
            id: 42,
            name: "Pan".into(),
            lane,
        }];
        let original = serde_json::to_value(&project).unwrap();
        let snapshot = PlaylistGestureSnapshot::capture(&project);
        project.automation_lanes[0]
            .lane
            .replace_points([AutomationPoint::new(0.0, 0.75)]);
        project
            .audio_clip_mixer_destinations
            .push(AudioClipMixerDestination {
                clip_id: 999,
                mixer_track_id: 1,
            });
        assert_eq!(
            serde_json::to_value(snapshot.restore_into(project)).unwrap(),
            original
        );
    }

    #[test]
    fn impossible_group_resize_returns_error_instead_of_panicking() {
        let clips = vec![clip(1, 0, 3.0, 1.0, None)];
        assert_eq!(
            clamp_group_resize_delta(&clips, 0.0, 2.0, 4.0),
            Err(PlaylistEditError::InvalidBounds)
        );
    }

    #[test]
    fn finite_slip_inputs_do_not_overflow_intermediate_arithmetic() {
        assert_eq!(
            quantized_slip_delta(f32::MAX, f32::MIN_POSITIVE, false),
            Ok(f32::MAX)
        );
        assert_eq!(
            quantized_slip_delta(f32::MAX, f32::MAX * 0.75, false),
            Ok(f32::MAX * 0.75)
        );
        let offset = slipped_looping_source_offset(f32::MAX, -f32::MAX, 4.0, 1.0, true).unwrap();
        assert!(offset.is_finite() && (0.0..4.0).contains(&offset));
        let offset = slipped_looping_source_offset(0.0, f32::MIN_POSITIVE, 4.0, 1.0, true).unwrap();
        assert!((0.0..4.0).contains(&offset));
        assert_eq!(
            slipped_bounded_source_offset(f32::MAX, -f32::MAX, 0.0, 4.0, 1.0, true),
            Ok(4.0)
        );
        assert_eq!(slipped_audio_source_offset(1, i128::MIN, 100), 100);
        assert_eq!(slipped_audio_source_offset(1, i128::MAX, 100), 0);
        assert_eq!(
            slipped_audio_source_offset(u64::MAX, i128::MIN, u64::MAX),
            u64::MAX
        );
    }

    #[test]
    fn quantized_slip_rejects_an_unrepresentable_result() {
        assert_eq!(
            quantized_slip_delta(f32::MAX, f32::MAX * 0.6, false),
            Err(PlaylistEditError::InvalidNumber)
        );
    }

    #[test]
    fn fades_reject_overflowing_clip_ends_and_handle_tiny_snap() {
        let invalid = clip(1, 0, f32::MAX, f32::MAX, None);
        assert_eq!(
            dragged_fade_fraction(&invalid, PlaylistFadeSide::In, 0.0, 0.0, 1.0, true),
            Err(PlaylistEditError::InvalidTarget)
        );
        let valid = clip(1, 0, 1.0, 4.0, None);
        assert_eq!(
            dragged_fade_fraction(
                &valid,
                PlaylistFadeSide::In,
                0.0,
                1.0,
                f32::MIN_POSITIVE,
                false
            ),
            Ok(0.25)
        );
        let mut left = clip(1, 0, 0.0, f32::MAX, None);
        let mut right = invalid;
        right.id = 2;
        right.start = f32::MAX * 0.5;
        left.kind = ClipKind::Audio;
        right.kind = ClipKind::Audio;
        assert_eq!(
            create_audio_crossfade(&[left, right], &HashSet::from([1, 2])).unwrap_err(),
            PlaylistEditError::InvalidTarget
        );
    }

    #[test]
    fn group_selection_expands_toggles_and_can_be_suspended() {
        let clips = vec![
            clip(1, 0, 0.0, 4.0, Some(7)),
            clip(2, 2, 8.0, 2.0, Some(7)),
            clip(3, 1, 4.0, 1.0, None),
        ];
        assert_eq!(clip_group_members(&clips, 1, true), HashSet::from([1, 2]));
        assert_eq!(clip_group_members(&clips, 1, false), HashSet::from([1]));
        let mut selection = HashSet::new();
        toggle_clip_group_selection(&clips, &mut selection, 1, true);
        assert_eq!(selection, HashSet::from([1, 2]));
        toggle_clip_group_selection(&clips, &mut selection, 2, true);
        assert!(selection.is_empty());
    }

    #[test]
    fn grouping_and_ungrouping_allocate_stable_identity() {
        let clips = vec![
            clip(1, 0, 0.0, 4.0, Some(9)),
            clip(2, 1, 4.0, 4.0, Some(9)),
            clip(3, 2, 8.0, 4.0, None),
            clip(4, 3, 12.0, 4.0, None),
        ];
        let grouped = group_selected_clips(&clips, &HashSet::from([3, 4])).unwrap();
        assert_eq!(grouped.group_id, Some(10));
        assert_eq!(grouped.affected_clips, 2);
        assert!(
            grouped.clips[2..]
                .iter()
                .all(|clip| clip.group_id == Some(10))
        );
        let ungrouped = ungroup_selected_clips(&grouped.clips, &HashSet::from([3, 4])).unwrap();
        assert_eq!(ungrouped.affected_clips, 2);
        assert!(
            ungrouped.clips[2..]
                .iter()
                .all(|clip| clip.group_id.is_none())
        );
    }

    #[test]
    fn group_move_and_resize_preserve_relations_at_boundaries() {
        let clips = vec![clip(1, 1, 2.0, 3.0, Some(1)), clip(2, 3, 8.0, 5.0, Some(1))];
        assert_eq!(
            clamp_group_move_delta(&clips, -7.0, -9, 16.0, 8).unwrap(),
            (-2.0, -1)
        );
        assert_eq!(
            clamp_group_move_delta(&clips, 9.0, 9, 16.0, 8).unwrap(),
            (3.0, 4)
        );
        assert_eq!(
            clamp_group_resize_delta(&clips, -9.0, 0.25, 16.0).unwrap(),
            -2.75
        );
        assert_eq!(
            clamp_group_resize_delta(&clips, 9.0, 0.25, 16.0).unwrap(),
            3.0
        );
    }

    #[test]
    fn slip_direction_snap_loop_and_audio_bounds_are_explicit() {
        assert_eq!(quantized_slip_delta(0.62, 0.25, false), Ok(0.5));
        assert_eq!(quantized_slip_delta(0.62, 0.25, true), Ok(0.62));
        assert_eq!(
            slipped_looping_source_offset(0.25, 0.5, 4.0, 0.25, false),
            Ok(3.75)
        );
        assert_eq!(
            slipped_bounded_source_offset(2.0, -1.2, 0.0, 3.0, 0.25, false),
            Ok(3.0)
        );
        assert_eq!(slipped_audio_source_offset(1_000, 250, 2_000), 750);
        assert_eq!(slipped_audio_source_offset(1_000, -2_000, 2_000), 2_000);
    }

    #[test]
    fn fade_handle_drag_uses_absolute_grid_and_alt_bypasses_snap() {
        let clip = clip(1, 0, 1.0, 4.0, None);
        assert_eq!(
            dragged_fade_fraction(&clip, PlaylistFadeSide::In, 0.0, 0.62, 0.25, false,),
            Ok(0.125)
        );
        assert_eq!(
            dragged_fade_fraction(&clip, PlaylistFadeSide::Out, 0.25, 0.3, 0.25, false,),
            Ok(0.1875)
        );
        assert_eq!(
            dragged_fade_fraction(&clip, PlaylistFadeSide::In, 0.0, 0.62, 0.25, true,),
            Ok(0.155)
        );
    }

    #[test]
    fn crossfade_sets_matching_overlap_lengths_and_preserves_other_fades() {
        let mut left = clip(1, 3, 2.0, 5.0, None);
        left.kind = ClipKind::Audio;
        left.fade_in = 0.2;
        let mut right = clip(2, 3, 5.0, 4.0, None);
        right.kind = ClipKind::Audio;
        right.fade_out = 0.3;
        let edit = create_audio_crossfade(&[left, right], &HashSet::from([1, 2])).unwrap();
        assert_eq!(edit.left_clip_id, 1);
        assert_eq!(edit.right_clip_id, 2);
        assert_eq!(edit.overlap_beats, 2.0);
        assert_eq!(edit.clips[0].fade_in, 0.2);
        assert_eq!(edit.clips[0].fade_out, 0.4);
        assert_eq!(edit.clips[1].fade_in, 0.5);
        assert_eq!(edit.clips[1].fade_out, 0.3);
    }

    #[test]
    fn crossfade_rejects_non_audio_cross_track_and_nested_pairs() {
        let pattern = clip(1, 0, 0.0, 4.0, None);
        let mut audio = clip(2, 0, 2.0, 4.0, None);
        audio.kind = ClipKind::Audio;
        assert_eq!(
            create_audio_crossfade(&[pattern, audio.clone()], &HashSet::from([1, 2])).unwrap_err(),
            PlaylistEditError::CrossfadeAudioOnly
        );
        let mut other_track = audio.clone();
        other_track.id = 3;
        other_track.track = 1;
        assert_eq!(
            create_audio_crossfade(&[audio.clone(), other_track], &HashSet::from([2, 3]))
                .unwrap_err(),
            PlaylistEditError::CrossfadeAudioOnly
        );
        let mut outer = audio;
        outer.id = 4;
        outer.start = 0.0;
        outer.length = 8.0;
        let mut inner = outer.clone();
        inner.id = 5;
        inner.start = 2.0;
        inner.length = 2.0;
        assert_eq!(
            create_audio_crossfade(&[outer, inner], &HashSet::from([4, 5])).unwrap_err(),
            PlaylistEditError::CrossfadeGeometry
        );
    }
}
