//! Playlist-only automation gestures. No persisted format or evaluation changes.
use super::*;

#[derive(Clone, Debug)]
pub(super) struct PointReference {
    pub clip_id: u32,
    pub lane_id: u64,
    pub point: AutomationPoint,
    pub target: AutomationTarget,
    pub range: crate::automation::AutomationValueRange,
}

#[derive(Clone)]
pub(super) enum PointAction {
    Value(PointReference, f64),
    Delete(PointReference),
    Tension(TensionReference, f64),
    Insert {
        clip_id: u32,
        lane_id: u64,
        point: AutomationPoint,
    },
}

pub(super) fn valid_clip_span(clip: &Clip) -> bool {
    clip.start.is_finite()
        && clip.start >= 0.0
        && clip.source_offset.is_finite()
        && clip.source_offset >= 0.0
        && clip.length.is_finite()
        && clip.length > 0.0
        && (clip.start + clip.length).is_finite()
        && (clip.source_offset + clip.length).is_finite()
}

/// A context action resolves the captured value, never a potentially reordered index.
pub(super) fn candidate(project: &Project, action: PointAction) -> Option<Project> {
    let (clip_id, lane_id) = match &action {
        PointAction::Value(reference, _) | PointAction::Delete(reference) => {
            (reference.clip_id, reference.lane_id)
        }
        PointAction::Insert {
            clip_id, lane_id, ..
        } => (*clip_id, *lane_id),
        PointAction::Tension(reference, _) => (reference.left.clip_id, reference.left.lane_id),
    };
    if project
        .clips
        .iter()
        .filter(|clip| clip.id == clip_id)
        .count()
        != 1
        || project
            .automation_lanes
            .iter()
            .filter(|lane| lane.id == lane_id)
            .count()
            != 1
    {
        return None;
    }
    let clip = project.clips.iter().find(|clip| {
        clip.id == clip_id
            && clip.kind == ClipKind::Automation
            && clip.automation_id == Some(lane_id)
    })?;
    if !valid_clip_span(clip) {
        return None;
    }
    if let PointAction::Tension(reference, _) = &action
        && !reference.matches_clip(clip)
    {
        return None;
    }
    let mut result = project.clone();
    let lane = &mut result
        .automation_lanes
        .iter_mut()
        .find(|lane| lane.id == lane_id)?
        .lane;
    match action {
        PointAction::Tension(reference, tension) => {
            if !tension.is_finite() || !(-1.0..=1.0).contains(&tension) {
                return None;
            }
            let index = reference.resolve(lane)?;
            let mut point = reference.left.point;
            point.tension = tension;
            lane.update_point(index, point).ok()?;
        }
        PointAction::Value(reference, normalized) => {
            if !normalized.is_finite() || !(0.0..=1.0).contains(&normalized) {
                return None;
            }
            if lane.value_range() != reference.range || lane.target() != &reference.target {
                return None;
            }
            let index = lane
                .points()
                .iter()
                .position(|point| *point == reference.point)?;
            let mut point = reference.point;
            point.value = lane.value_range().denormalize(normalized);
            lane.update_point(index, point).ok()?;
        }
        PointAction::Delete(reference) => {
            if lane.value_range() != reference.range || lane.target() != &reference.target {
                return None;
            }
            let index = lane
                .points()
                .iter()
                .position(|point| *point == reference.point)?;
            lane.delete_point(index).ok()?;
        }
        PointAction::Insert { point, .. } => {
            if !point.position.is_finite()
                || !point.value.is_finite()
                || !point.tension.is_finite()
                || point.position < f64::from(clip.source_offset)
                || point.position > f64::from(clip.source_offset + clip.length)
                || lane.points().iter().any(|existing| {
                    (existing.position - point.position).abs()
                        <= crate::automation::AUTOMATION_POSITION_EPSILON
                })
            {
                return None;
            }
            lane.insert_point(point).ok()?;
        }
    }
    Some(result)
}

/// Captures the outgoing segment, including the placement that exposes it.
/// Never infer a new neighbour or silently convert a lane-wide curve mode.
#[derive(Clone, Debug)]
pub(super) struct TensionReference {
    pub left: PointReference,
    pub right: AutomationPoint,
    pub clip_start: f32,
    pub source_offset: f32,
    pub length: f32,
}

impl TensionReference {
    pub fn matches_clip(&self, clip: &Clip) -> bool {
        valid_clip_span(clip)
            && clip.id == self.left.clip_id
            && clip.automation_id == Some(self.left.lane_id)
            && clip.kind == ClipKind::Automation
            && clip.start == self.clip_start
            && clip.source_offset == self.source_offset
            && clip.length == self.length
    }

    pub fn resolve(&self, lane: &AutomationLane) -> Option<usize> {
        if !lane.is_enabled()
            || lane.curve() != AutomationCurve::Tension
            || lane.target() != &self.left.target
            || lane.value_range() != self.left.range
        {
            return None;
        }
        lane.points()
            .windows(2)
            .position(|pair| pair[0] == self.left.point && pair[1] == self.right)
    }
}

/// The midpoint of the segment's *clip* intersection, never its viewport
/// intersection. Scrolling may hide a handle but cannot move its source time.
pub(super) fn tension_handle(
    lane: &AutomationLane,
    index: usize,
    clip: &Clip,
    pixels_per_beat: f32,
) -> Option<(f64, f64)> {
    if !valid_clip_span(clip)
        || !lane.is_enabled()
        || lane.curve() != AutomationCurve::Tension
        || !pixels_per_beat.is_finite()
        || pixels_per_beat <= 0.0
    {
        return None;
    }
    let left = *lane.points().get(index)?;
    let right = *lane.points().get(index + 1)?;
    if ![
        left.position,
        right.position,
        left.value,
        right.value,
        left.tension,
    ]
    .iter()
    .all(|value| value.is_finite())
        || right.position - left.position <= crate::automation::AUTOMATION_POSITION_EPSILON
        || left.value == right.value
    {
        return None;
    }
    let start = left.position.max(f64::from(clip.source_offset));
    let end = right
        .position
        .min(f64::from(clip.source_offset + clip.length));
    if (end - start) * f64::from(pixels_per_beat) < 24.0 {
        return None;
    }
    let source = start + (end - start) * 0.5;
    let progress = (source - left.position) / (right.position - left.position);
    let value = left.value
        + (right.value - left.value)
            * crate::automation::sample_curve_progress(
                AutomationCurve::Tension,
                progress,
                left.tension,
            );
    (source.is_finite() && value.is_finite()).then_some((source, value))
}

#[derive(Clone, Debug)]
pub(super) struct TensionDrag {
    pub reference: TensionReference,
    pub last_delta_y: f32,
}

impl TensionDrag {
    /// Integrate motion, rather than rescaling the total on Ctrl changes.
    /// Clamp every increment so reversing away from a limit responds immediately.
    pub fn advance(
        &mut self,
        lane: &AutomationLane,
        total_delta_y: f32,
        fine: bool,
    ) -> Option<(usize, AutomationPoint)> {
        let index = self.reference.resolve(lane)?;
        if !total_delta_y.is_finite() || !self.last_delta_y.is_finite() {
            return None;
        }
        let mut point = self.reference.left.point;
        let direction = (self.reference.right.value - point.value).signum();
        let delta = f64::from(total_delta_y) - f64::from(self.last_delta_y);
        point.tension =
            (point.tension + direction * delta * if fine { 0.001 } else { 0.01 }).clamp(-1.0, 1.0);
        self.last_delta_y = total_delta_y;
        self.reference.left.point = point;
        Some((index, point))
    }
}

/// Points cannot cross/overwrite their neighbours. A colliding time stays at its
/// current position while a simultaneous value edit is still allowed.
#[allow(clippy::too_many_arguments)]
pub(super) fn dragged_point(
    lane: &AutomationLane,
    index: usize,
    origin: AutomationPoint,
    delta: Vec2,
    beat_width: f32,
    height: f32,
    source_start: f32,
    clip_start: f32,
    length: f32,
    snap: f32,
    modifiers: egui::Modifiers,
) -> Option<AutomationPoint> {
    let current = *lane.points().get(index)?;
    if !origin.position.is_finite()
        || !origin.value.is_finite()
        || !origin.tension.is_finite()
        || !delta.x.is_finite()
        || !delta.y.is_finite()
        || !source_start.is_finite()
        || source_start < 0.0
        || !clip_start.is_finite()
        || clip_start < 0.0
        || !length.is_finite()
        || length <= 0.0
        || !(source_start + length).is_finite()
        || !(clip_start + length).is_finite()
        || !beat_width.is_finite()
        || beat_width <= 0.0
        || !height.is_finite()
        || height <= 0.0
        || !snap.is_finite()
        || snap <= 0.0
    {
        return None;
    }
    let mut position = origin.position;
    if !modifiers.ctrl {
        position += f64::from(delta.x / beat_width);
        if !modifiers.alt {
            let timeline = position - f64::from(source_start) + f64::from(clip_start);
            position = (timeline / f64::from(snap)).round() * f64::from(snap)
                - f64::from(clip_start)
                + f64::from(source_start);
        }
        position = position.clamp(f64::from(source_start), f64::from(source_start + length));
        if index > 0
            && position
                <= lane.points()[index - 1].position
                    + crate::automation::AUTOMATION_POSITION_EPSILON
            || lane.points().get(index + 1).is_some_and(|next| {
                position >= next.position - crate::automation::AUTOMATION_POSITION_EPSILON
            })
        {
            position = current.position;
        }
    }
    let range = lane.value_range();
    let value = if modifiers.shift {
        origin.value
    } else {
        range.denormalize(range.normalize(origin.value) - f64::from(delta.y / height))
    };
    (position.is_finite() && value.is_finite()).then_some(AutomationPoint::with_tension(
        position,
        value,
        origin.tension,
    ))
}

impl CitrusApp {
    pub(super) fn commit_automation_point_action(&mut self, action: PointAction) {
        if self.playlist_gesture_before.is_some() {
            return;
        }
        let Some(candidate) = candidate(&self.project, action) else {
            return;
        };
        if commit_explicit_project_history_transaction(
            &mut self.project,
            &mut self.history_snapshot,
            &mut self.history_fingerprint,
            &mut self.undo_stack,
            &mut self.redo_stack,
            &mut self.dirty,
            candidate,
        ) {
            self.sync_history_observer();
            self.automation_evaluator.reset();
            self.refresh_timeline_fingerprint(Instant::now(), true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Project, PointReference) {
        let mut project = Project::blank();
        let mut lane = AutomationLane::new(AutomationTarget::MasterPan);
        lane.replace_points([
            AutomationPoint::new(2.0, -1.0),
            AutomationPoint::new(4.0, 0.0),
            AutomationPoint::new(6.0, 1.0),
        ]);
        let reference = PointReference {
            clip_id: 1,
            lane_id: 1,
            point: lane.points()[1],
            target: lane.target().clone(),
            range: lane.value_range(),
        };
        project.automation_lanes.push(ProjectAutomation {
            id: 1,
            name: "Pan".into(),
            lane,
        });
        project.clips.push(Clip {
            id: 1,
            track: 0,
            start: 8.0,
            length: 4.0,
            source_offset: 2.0,
            automation_id: Some(1),
            kind: ClipKind::Automation,
            pattern_id: project.patterns[0].id,
            name: "Pan".into(),
            color: [1, 2, 3],
            group_id: None,
            audio_asset_id: None,
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
        (project, reference)
    }
    fn tension_fixture() -> (Project, TensionReference) {
        let (mut project, mut left) = fixture();
        project.automation_lanes[0]
            .lane
            .set_curve(AutomationCurve::Tension);
        left.point = project.automation_lanes[0].lane.points()[0];
        let reference = TensionReference {
            left,
            right: project.automation_lanes[0].lane.points()[1],
            clip_start: 8.0,
            source_offset: 2.0,
            length: 4.0,
        };
        (project, reference)
    }

    #[test]
    fn tension_midpoint_uses_original_curve_and_clipped_source_interval() {
        let (mut project, _) = tension_fixture();
        let lane = &mut project.automation_lanes[0].lane;
        lane.replace_points([
            AutomationPoint::with_tension(0.0, -1.0, 0.5),
            AutomationPoint::new(8.0, 1.0),
        ]);
        let clip = &project.clips[0]; // source 2..6, neither original endpoint visible
        let (source, native) = tension_handle(lane, 0, clip, 100.0).unwrap();
        assert_eq!(source, 4.0);
        assert_eq!(native, lane.evaluate_unlooped(source).unwrap());
        assert_eq!(native, -0.875); // p=.5, exponent=4, native pan range
        let mut clipped = clip.clone();
        clipped.source_offset = 3.0;
        clipped.length = 1.0;
        let (source, native) = tension_handle(lane, 0, &clipped, 100.0).unwrap();
        assert_eq!(source, 3.5);
        assert_eq!(native, lane.evaluate_unlooped(source).unwrap());
        // No viewport is an input: scroll must never relocate this midpoint.
        assert_eq!(tension_handle(lane, 0, &clipped, 24.0).unwrap().0, 3.5);
        assert!(tension_handle(lane, 0, &clipped, 23.0).is_none());
    }

    #[test]
    fn tension_visibility_rejects_ignored_modes_flat_invalid_and_missing_segments() {
        let (mut project, _) = tension_fixture();
        let clip = &project.clips[0];
        let lane = &mut project.automation_lanes[0].lane;
        for curve in [AutomationCurve::Linear, AutomationCurve::Hold] {
            lane.set_curve(curve);
            assert!(tension_handle(lane, 0, clip, 100.0).is_none());
        }
        lane.set_curve(AutomationCurve::Tension);
        lane.set_enabled(false);
        assert!(tension_handle(lane, 0, clip, 100.0).is_none());
        lane.set_enabled(true);
        assert!(tension_handle(lane, 2, clip, 100.0).is_none());
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(tension_handle(lane, 0, clip, scale).is_none());
        }
        let mut invalid = clip.clone();
        invalid.source_offset = f32::NAN;
        assert!(tension_handle(lane, 0, &invalid, 100.0).is_none());
        lane.replace_points([
            AutomationPoint::new(2.0, 0.0),
            AutomationPoint::new(4.0, 0.0),
        ]);
        assert!(tension_handle(lane, 0, clip, 100.0).is_none());
    }

    #[test]
    fn tension_drag_ctrl_transitions_and_clamp_reversal_are_incremental() {
        let (mut project, reference) = tension_fixture();
        let lane = &mut project.automation_lanes[0].lane;
        let original = lane.points().to_vec();
        let mut drag = TensionDrag {
            reference,
            last_delta_y: 0.0,
        };
        for (total, fine, expected) in [
            (20.0, false, 0.2),
            (20.0, true, 0.2), // modifier-only frame cannot jump
            (30.0, true, 0.21),
            (30.0, false, 0.21),
            (40.0, false, 0.31),
            (500.0, false, 1.0),
            (499.0, false, 0.99), // immediate response, no overshoot debt
            (-500.0, false, -1.0),
            (-499.0, true, -0.999),
        ] {
            let (index, point) = drag.advance(lane, total, fine).unwrap();
            assert!((point.tension - expected).abs() < 1e-12);
            assert_eq!(
                (point.position, point.value),
                (original[0].position, original[0].value)
            );
            lane.update_point(index, point).unwrap();
            assert_eq!(&lane.points()[1..], &original[1..]);
        }
        assert!(drag.advance(lane, f32::NAN, false).is_none());
        let saved = lane.points()[0];
        lane.update_point(
            0,
            AutomationPoint::with_tension(saved.position, saved.value + 0.1, saved.tension),
        )
        .unwrap();
        assert!(drag.advance(lane, -498.0, false).is_none());
    }

    #[test]
    fn tension_drag_follows_vertical_direction_for_falling_native_values() {
        let (mut project, mut reference) = tension_fixture();
        let lane = &mut project.automation_lanes[0].lane;
        lane.replace_points([
            AutomationPoint::new(2.0, 1.0),
            AutomationPoint::new(4.0, -1.0),
        ]);
        reference.left.point = lane.points()[0];
        reference.right = lane.points()[1];
        let mut drag = TensionDrag {
            reference,
            last_delta_y: 0.0,
        };
        let before = lane.evaluate_unlooped(3.0).unwrap();
        let (index, point) = drag.advance(lane, -20.0, false).unwrap();
        assert_eq!(point.tension, 0.2);
        lane.update_point(index, point).unwrap();
        assert!(lane.evaluate_unlooped(3.0).unwrap() > before);
    }

    #[test]
    fn tension_actions_revalidate_segment_mode_placement_and_persist() {
        let (project, reference) = tension_fixture();
        let edited = candidate(&project, PointAction::Tension(reference.clone(), 0.75)).unwrap();
        assert_eq!(edited.automation_lanes[0].lane.points()[0].tension, 0.75);
        assert_eq!(
            &edited.automation_lanes[0].lane.points()[1..],
            &project.automation_lanes[0].lane.points()[1..]
        );
        let restored: Project =
            serde_json::from_slice(&serde_json::to_vec(&edited).unwrap()).unwrap();
        assert_eq!(project_fingerprint(&edited), project_fingerprint(&restored));
        assert!(candidate(&edited, PointAction::Tension(reference.clone(), 0.0)).is_none());
        for value in [f64::NAN, f64::INFINITY, -1.01, 1.01] {
            assert!(candidate(&project, PointAction::Tension(reference.clone(), value)).is_none());
        }
        let mut changed = project.clone();
        changed.automation_lanes[0]
            .lane
            .insert_point(AutomationPoint::new(3.0, -0.5))
            .unwrap();
        assert!(candidate(&changed, PointAction::Tension(reference.clone(), 0.0)).is_none());
        changed = project.clone();
        changed.automation_lanes[0]
            .lane
            .set_curve(AutomationCurve::Linear);
        assert!(candidate(&changed, PointAction::Tension(reference.clone(), 0.0)).is_none());
        changed = project.clone();
        changed.clips[0].source_offset += 1.0;
        assert!(candidate(&changed, PointAction::Tension(reference, 0.0)).is_none());
    }

    #[test]
    fn point_actions_normalize_pan_reject_nonfinite_and_resolve_stale_index() {
        let (mut project, reference) = fixture();
        project.automation_lanes[0]
            .lane
            .insert_point(AutomationPoint::new(3.0, -0.5))
            .unwrap();
        let edited = candidate(&project, PointAction::Value(reference.clone(), 0.75)).unwrap();
        assert_eq!(edited.automation_lanes[0].lane.points()[2].value, 0.5);
        assert_eq!(edited.automation_lanes[0].lane.points()[1].value, -0.5);
        for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(candidate(&project, PointAction::Value(reference.clone(), value)).is_none());
        }
        assert!(
            candidate(&edited, PointAction::Delete(reference.clone())).is_none(),
            "stale captured point cannot delete a changed point"
        );
        let restored: Project =
            serde_json::from_slice(&serde_json::to_vec(&edited).unwrap()).unwrap();
        assert_eq!(project_fingerprint(&edited), project_fingerprint(&restored));
    }
    #[test]
    fn point_insert_checks_source_span_and_collision_without_replacing() {
        let (project, _) = fixture();
        for position in [0.0, 1.0, 4.0, 7.0, f64::NAN] {
            assert!(
                candidate(
                    &project,
                    PointAction::Insert {
                        clip_id: 1,
                        lane_id: 1,
                        point: AutomationPoint::new(position, 0.5)
                    }
                )
                .is_none()
            );
        }
        let edited = candidate(
            &project,
            PointAction::Insert {
                clip_id: 1,
                lane_id: 1,
                point: AutomationPoint::new(3.0, 0.5),
            },
        )
        .unwrap();
        assert_eq!(edited.automation_lanes[0].lane.points().len(), 4);
    }
    #[test]
    fn point_drag_locks_unsnaps_and_never_merges_or_reorders() {
        let (project, reference) = fixture();
        let lane = &project.automation_lanes[0].lane;
        let edit = |delta, modifiers| {
            dragged_point(
                lane,
                1,
                reference.point,
                delta,
                100.0,
                100.0,
                2.0,
                8.0,
                4.0,
                0.25,
                modifiers,
            )
            .unwrap()
        };
        let default = edit(Vec2::new(33.0, -25.0), egui::Modifiers::NONE);
        assert_eq!((default.position, default.value), (4.25, 0.5));
        let shift = edit(Vec2::new(33.0, -25.0), egui::Modifiers::SHIFT);
        assert_eq!((shift.position, shift.value), (4.25, 0.0));
        let ctrl = edit(Vec2::new(33.0, -25.0), egui::Modifiers::CTRL);
        assert_eq!((ctrl.position, ctrl.value), (4.0, 0.5));
        let alt = edit(Vec2::new(33.0, -25.0), egui::Modifiers::ALT);
        assert!((alt.position - 4.33).abs() < 0.00001);
        assert_eq!(
            edit(Vec2::new(200.0, -25.0), egui::Modifiers::NONE).position,
            4.0
        );
        assert_eq!(
            edit(Vec2::new(-300.0, -25.0), egui::Modifiers::NONE).position,
            4.0
        );
    }
    #[test]
    fn point_actions_reject_stale_targets_deleted_and_ambiguous_identity() {
        let (project, reference) = fixture();
        let mut changed = project.clone();
        changed.automation_lanes[0]
            .lane
            .set_target(AutomationTarget::ChannelPan { channel: 1 });
        assert!(candidate(&changed, PointAction::Delete(reference.clone())).is_none());
        let mut changed = project.clone();
        changed.clips.push(changed.clips[0].clone());
        assert!(candidate(&changed, PointAction::Delete(reference.clone())).is_none());
        let mut changed = project.clone();
        changed
            .automation_lanes
            .push(changed.automation_lanes[0].clone());
        assert!(candidate(&changed, PointAction::Delete(reference.clone())).is_none());
        let mut changed = project.clone();
        changed.clips.clear();
        assert!(candidate(&changed, PointAction::Delete(reference.clone())).is_none());
        let mut changed = project;
        changed.automation_lanes.clear();
        assert!(candidate(&changed, PointAction::Delete(reference)).is_none());
    }
    #[test]
    fn point_edits_reject_nonfinite_and_invalid_spans_without_panicking() {
        let (project, reference) = fixture();
        for (start, source, length) in [
            (f32::NAN, 2.0, 4.0),
            (8.0, f32::NAN, 4.0),
            (8.0, 2.0, f32::INFINITY),
            (8.0, 2.0, -1.0),
            (8.0, f32::MAX, f32::MAX),
        ] {
            let mut invalid = project.clone();
            invalid.clips[0].start = start;
            invalid.clips[0].source_offset = source;
            invalid.clips[0].length = length;
            assert!(
                candidate(
                    &invalid,
                    PointAction::Insert {
                        clip_id: 1,
                        lane_id: 1,
                        point: AutomationPoint::new(3.0, 0.5)
                    }
                )
                .is_none()
            );
            assert!(
                dragged_point(
                    &project.automation_lanes[0].lane,
                    1,
                    reference.point,
                    Vec2::ZERO,
                    100.0,
                    100.0,
                    source,
                    start,
                    length,
                    0.25,
                    egui::Modifiers::NONE
                )
                .is_none()
            );
        }
        for delta in [Vec2::new(f32::NAN, 0.0), Vec2::new(0.0, f32::INFINITY)] {
            assert!(
                dragged_point(
                    &project.automation_lanes[0].lane,
                    1,
                    reference.point,
                    delta,
                    100.0,
                    100.0,
                    2.0,
                    8.0,
                    4.0,
                    0.25,
                    egui::Modifiers::NONE
                )
                .is_none()
            );
        }
    }
}
