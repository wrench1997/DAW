//! Native-control entry points; the existing automation engine remains authoritative.
use super::*;

pub(super) fn control_menu(
    response: &Response,
    target: AutomationTarget,
    request: &mut Option<AutomationTarget>,
) {
    response.context_menu(|ui| {
        if ui.button("Create/open automation clip").clicked() {
            *request = Some(target);
            ui.close();
        }
    });
}

impl CitrusApp {
    pub(super) fn create_or_open_native_automation(&mut self, target: AutomationTarget) {
        let first_track = self.playlist_viewport.y.visible_range().0.floor().max(0.0) as usize;
        let result = native_automation_candidate(
            &self.project,
            target,
            self.beat_position,
            self.snap,
            first_track,
        );
        match result {
            Ok((candidate, clip_id)) => {
                let created = commit_explicit_project_history_transaction(
                    &mut self.project,
                    &mut self.history_snapshot,
                    &mut self.history_fingerprint,
                    &mut self.undo_stack,
                    &mut self.redo_stack,
                    &mut self.dirty,
                    candidate,
                );
                if created {
                    self.sync_history_observer();
                    self.automation_evaluator.reset();
                }
                self.select_playlist_clip_only(clip_id);
                self.tool_mode = ToolMode::Select;
                if let Some(clip) = self.project.clips.iter().find(|clip| clip.id == clip_id) {
                    let _ = self.playlist_viewport.x.reveal(
                        f64::from(clip.start),
                        f64::from(clip.start + clip.length),
                        0.0,
                    );
                    let _ = self.playlist_viewport.y.reveal(
                        clip.track as f64,
                        (clip.track + 1) as f64,
                        0.0,
                    );
                }
                self.focus_editor(StudioView::Playlist);
                self.notify(
                    if created {
                        "Created automation clip"
                    } else {
                        "Opened existing automation clip"
                    }
                    .into(),
                );
            }
            Err(error) => self.notify(format!("Automation unavailable: {error}")),
        }
    }
}

/// Returns an all-or-nothing candidate. Opening an existing placement returns an unchanged
/// project, so the explicit history transaction produces no spurious undo entry.
fn native_automation_candidate(
    project: &Project,
    target: AutomationTarget,
    requested_start: f32,
    snap: f32,
    first_track: usize,
) -> Result<(Project, u32), String> {
    let target = canonicalize_automation_target(target);
    let (value, name, color) = match target {
        AutomationTarget::MasterVolume
        | AutomationTarget::MasterPan
        | AutomationTarget::MixerVolume { .. }
        | AutomationTarget::MixerPan { .. } => {
            let (id, pan) = match target {
                AutomationTarget::MasterVolume => (MASTER_MIXER_TRACK_ID, false),
                AutomationTarget::MasterPan => (MASTER_MIXER_TRACK_ID, true),
                AutomationTarget::MixerVolume { track } => (track, false),
                AutomationTarget::MixerPan { track } => (track, true),
                _ => unreachable!(),
            };
            let mut tracks = project.mixer_tracks.iter().filter(|track| track.id == id);
            let track = tracks.next().ok_or("The Mixer track no longer exists")?;
            if tracks.next().is_some() {
                return Err("The Mixer identity is ambiguous".into());
            }
            (
                if pan { track.pan } else { track.volume },
                format!("{} · {}", track.name, if pan { "Pan" } else { "Volume" }),
                track.color,
            )
        }
        AutomationTarget::ChannelVolume { channel } | AutomationTarget::ChannelPan { channel } => {
            let mut channels = project.channels.iter().filter(|item| item.id == channel);
            let item = channels.next().ok_or("The Channel no longer exists")?;
            if channels.next().is_some() {
                return Err("The Channel identity is ambiguous".into());
            }
            let pan = matches!(target, AutomationTarget::ChannelPan { .. });
            (
                if pan { item.pan } else { item.volume },
                format!("{} · {}", item.name, if pan { "Pan" } else { "Volume" }),
                item.color,
            )
        }
        _ => return Err("Use the parameter catalog for plug-in automation".into()),
    };
    let mut matching = project
        .automation_lanes
        .iter()
        .filter(|lane| canonicalize_automation_target(lane.lane.target().clone()) == target);
    if let Some(lane) = matching.next() {
        if matching.next().is_some() {
            return Err("Multiple automation lanes target this control; select the intended clip in the Playlist".into());
        }
        if lane.id == 0
            || project
                .automation_lanes
                .iter()
                .filter(|other| other.id == lane.id)
                .count()
                != 1
        {
            return Err("The automation lane identity is ambiguous".into());
        }
        let clip = project.clips.iter().filter(|clip| clip.kind == ClipKind::Automation && clip.automation_id == Some(lane.id))
            .min_by(|a, b| a.start.total_cmp(&b.start).then_with(|| a.id.cmp(&b.id)))
            .ok_or("This control has an unplaced global lane; placing it would change playback. Use the existing automation workflow")?;
        if clip.id == 0
            || project
                .clips
                .iter()
                .filter(|other| other.id == clip.id)
                .count()
                != 1
            || !clip.start.is_finite()
            || !clip.length.is_finite()
            || clip.start < 0.0
            || clip.length <= 0.0
            || !(clip.start + clip.length).is_finite()
            || clip.track >= PLAYLIST_TRACK_COUNT
        {
            return Err("The existing automation placement is invalid or ambiguous".into());
        }
        return Ok((project.clone(), clip.id));
    }
    let range = target.default_value_range();
    if !value.is_finite() || f64::from(value) < range.min || f64::from(value) > range.max {
        return Err("The control has an invalid value".into());
    }
    let song_length = project.song_length_beats;
    if !song_length.is_finite()
        || song_length <= 0.0
        || !requested_start.is_finite()
        || !snap.is_finite()
        || snap <= 0.0
    {
        return Err("The Playlist timing or snap is invalid".into());
    }
    let start = ((f64::from(requested_start) / f64::from(snap)).floor() * f64::from(snap)).clamp(
        0.0,
        f64::from((song_length - snap.min(song_length)).max(0.0)),
    ) as f32;
    let length = 4.0_f32.min(song_length - start);
    if length <= 0.0 || start + length <= start {
        return Err("No representable Playlist duration remains".into());
    }
    let first = first_track.min(PLAYLIST_TRACK_COUNT - 1);
    let track = (first..PLAYLIST_TRACK_COUNT)
        .chain(0..first)
        .find(|track| {
            !project.clips.iter().any(|clip| {
                clip.track == *track
                    && (!clip.start.is_finite()
                        || !clip.length.is_finite()
                        || clip.length < 0.0
                        || (clip.start < start + length && clip.start + clip.length > start))
            })
        })
        .ok_or(
            "All Playlist tracks are occupied at the playhead; move the playhead or free a track",
        )?;
    let automation_id =
        next_available_automation_id(project).ok_or("No automation identity is available")?;
    let clip_id = next_available_clip_id(project).ok_or("No clip identity is available")?;
    let pattern_id = project
        .patterns
        .get(project.active_pattern)
        .ok_or("No active Pattern is available")?
        .id;
    let mut lane = AutomationLane::new(target);
    lane.set_curve(AutomationCurve::Tension);
    lane.replace_points([
        AutomationPoint::new(0.0, f64::from(value)),
        AutomationPoint::new(f64::from(length), f64::from(value)),
    ]);
    let mut candidate = project.clone();
    candidate.automation_lanes.push(ProjectAutomation {
        id: automation_id,
        name: name.clone(),
        lane,
    });
    candidate.clips.push(Clip {
        id: clip_id,
        track,
        start,
        length,
        name,
        color,
        kind: ClipKind::Automation,
        group_id: None,
        pattern_id,
        automation_id: Some(automation_id),
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
    Ok((candidate, clip_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(project: &Project, target: AutomationTarget) -> Result<(Project, u32), String> {
        native_automation_candidate(project, target, 2.37, 0.25, 3)
    }

    #[test]
    fn native_creation_seeds_value_and_preserves_audio_routes() {
        let project = Project::blank();
        let channel = project.channels[0].id;
        let mixer = project.mixer_tracks[1].id;
        for target in [
            AutomationTarget::MasterVolume,
            AutomationTarget::MasterPan,
            AutomationTarget::MixerVolume { track: mixer },
            AutomationTarget::MixerPan { track: mixer },
            AutomationTarget::ChannelVolume { channel },
            AutomationTarget::ChannelPan { channel },
        ] {
            let before = project_fingerprint(&project);
            let (candidate, id) = create(&project, target.clone()).unwrap();
            assert_eq!(project_fingerprint(&project), before);
            let clip = candidate.clips.iter().find(|clip| clip.id == id).unwrap();
            assert_eq!((clip.start, clip.length, clip.track), (2.25, 4.0, 3));
            assert_eq!(
                candidate.audio_clip_mixer_destinations,
                project.audio_clip_mixer_destinations
            );
            let lane = candidate.automation_lanes.last().unwrap();
            assert_eq!(lane.lane.target(), &target);
            assert_eq!(lane.lane.points().len(), 2);
            assert_eq!(lane.lane.points()[0].value, lane.lane.points()[1].value);
            let restored: Project =
                serde_json::from_slice(&serde_json::to_vec(&candidate).unwrap()).unwrap();
            assert_eq!(
                project_fingerprint(&restored),
                project_fingerprint(&candidate)
            );
        }
    }

    #[test]
    fn native_pan_extremes_and_sparse_reordered_mixer_use_stable_identity() {
        for value in [-1.0, 0.0, 1.0] {
            let mut project = Project::blank();
            project.channels[0].pan = value;
            let (candidate, _) = create(
                &project,
                AutomationTarget::ChannelPan {
                    channel: project.channels[0].id,
                },
            )
            .unwrap();
            assert_eq!(
                candidate.automation_lanes.last().unwrap().lane.points()[0].value,
                f64::from(value)
            );
        }
        let mut project = Project::blank();
        project.mixer_tracks[1].id = 912;
        project.mixer_tracks[1].pan = -0.625;
        project.mixer_tracks.swap(1, 2);
        let (candidate, _) = create(&project, AutomationTarget::MixerPan { track: 912 }).unwrap();
        assert_eq!(
            candidate.automation_lanes.last().unwrap().lane.points()[0].value,
            -0.625
        );
    }

    #[test]
    fn native_open_is_idempotent_alias_aware_and_deterministic() {
        let (mut project, id) = create(&Project::blank(), AutomationTarget::MasterPan).unwrap();
        let mut second = project
            .clips
            .iter()
            .find(|clip| clip.id == id)
            .unwrap()
            .clone();
        second.id = id + 8;
        second.start = 1.0;
        project.clips.push(second.clone());
        second.id += 1;
        project.clips.push(second);
        let before = project_fingerprint(&project);
        let (candidate, opened) = create(
            &project,
            AutomationTarget::MixerPan {
                track: MASTER_MIXER_TRACK_ID,
            },
        )
        .unwrap();
        assert_eq!(opened, id + 8);
        assert_eq!(project_fingerprint(&candidate), before);
    }

    #[test]
    fn native_creation_rejects_ambiguous_unplaced_and_invalid_inputs() {
        let (mut project, _) = create(&Project::blank(), AutomationTarget::MasterVolume).unwrap();
        project.clips.clear();
        assert!(
            create(&project, AutomationTarget::MasterVolume)
                .unwrap_err()
                .contains("unplaced")
        );
        project
            .automation_lanes
            .push(project.automation_lanes[0].clone());
        assert!(
            create(&project, AutomationTarget::MasterVolume)
                .unwrap_err()
                .contains("Multiple")
        );
        let project = Project::blank();
        assert!(create(&project, AutomationTarget::MixerVolume { track: u64::MAX }).is_err());
        for (beat, snap) in [(f32::NAN, 0.25), (0.0, 0.0), (0.0, f32::INFINITY)] {
            assert!(
                native_automation_candidate(&project, AutomationTarget::MasterPan, beat, snap, 0)
                    .is_err()
            );
        }
        let mut project = project;
        project.channels[0].pan = f32::NAN;
        assert!(
            create(
                &project,
                AutomationTarget::ChannelPan {
                    channel: project.channels[0].id
                }
            )
            .is_err()
        );
    }

    #[test]
    fn native_creation_finds_space_truncates_and_avoids_reserved_ids() {
        let (mut project, _) = create(&Project::blank(), AutomationTarget::MasterVolume).unwrap();
        let (candidate, id) = create(&project, AutomationTarget::MasterPan).unwrap();
        assert_eq!(
            candidate
                .clips
                .iter()
                .find(|clip| clip.id == id)
                .unwrap()
                .track,
            4
        );
        let template = project.clips[0].clone();
        project.clips = (0..PLAYLIST_TRACK_COUNT)
            .map(|track| {
                let mut clip = template.clone();
                clip.track = track;
                clip.id = track as u32 + 1;
                clip
            })
            .collect();
        assert!(
            create(&project, AutomationTarget::MasterPan)
                .unwrap_err()
                .contains("occupied")
        );
        project.clips.clear();
        project.automation_lanes[0].id = u64::MAX;
        project.song_length_beats = 3.0;
        let (candidate, id) = create(&project, AutomationTarget::MasterPan).unwrap();
        let clip = candidate.clips.iter().find(|clip| clip.id == id).unwrap();
        assert_eq!(clip.length, 0.75);
        assert_ne!(clip.automation_id, Some(u64::MAX));
    }

    #[test]
    fn native_creation_history_preserves_preceding_edit_and_open_has_no_entry() {
        let mut project = Project::blank();
        let original = project.clone();
        let mut history = project.clone();
        let mut fingerprint = project_fingerprint(&project);
        let mut undo = Vec::new();
        let mut redo = vec![project.clone()];
        let mut dirty = false;
        project.channels[0].volume = 0.42;
        let preceding = project.clone();
        let (candidate, _) = create(&project, AutomationTarget::MasterPan).unwrap();
        assert!(commit_explicit_project_history_transaction(
            &mut project,
            &mut history,
            &mut fingerprint,
            &mut undo,
            &mut redo,
            &mut dirty,
            candidate
        ));
        assert_eq!(undo.len(), 2);
        assert_eq!(
            project_fingerprint(&undo[0]),
            project_fingerprint(&original)
        );
        assert_eq!(
            project_fingerprint(&undo[1]),
            project_fingerprint(&preceding)
        );
        assert!(redo.is_empty() && dirty);
        let (candidate, _) = create(&project, AutomationTarget::MasterPan).unwrap();
        assert!(!commit_explicit_project_history_transaction(
            &mut project,
            &mut history,
            &mut fingerprint,
            &mut undo,
            &mut redo,
            &mut dirty,
            candidate
        ));
        assert_eq!(undo.len(), 2);
    }
}
