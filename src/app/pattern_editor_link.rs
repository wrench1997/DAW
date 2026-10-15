//! Shared Pattern editing follows a Playlist occurrence without changing playback.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PatternEditorOrigin {
    project_session: u64,
    clip_id: u32,
    pattern_id: u32,
}

/// Mirrors Timeline's read-only Swing precedence and f64 placement/source clock.
/// The legacy UI automation bridge accepts f32 beats; using it here can round a
/// local pair across a placement edge. This query allocates nothing and never
/// dispatches automation or changes the compiler/callback scheduling contract.
fn pattern_editor_swing_at(project: &Project, beat: f64) -> f64 {
    project
        .automation_lanes
        .iter()
        .rev()
        .find_map(|automation| {
            let lane = &automation.lane;
            if !lane.is_enabled()
                || lane.points().is_empty()
                || !matches!(lane.target(), AutomationTarget::Swing)
            {
                return None;
            }
            let placements = || {
                project.clips.iter().filter(|clip| {
                    clip.kind == ClipKind::Automation && clip.automation_id == Some(automation.id)
                })
            };
            let source = if placements().next().is_none() {
                beat
            } else {
                let clip = placements().rev().find(|clip| {
                    !clip.muted
                        && clip.start.is_finite()
                        && clip.length.is_finite()
                        && clip.start >= 0.0
                        && clip.length > 0.0
                        && beat >= f64::from(clip.start)
                        && beat < f64::from(clip.start) + f64::from(clip.length)
                })?;
                f64::from(clip.source_offset) + beat - f64::from(clip.start)
            };
            lane.evaluate(source).filter(|value| value.is_finite())
        })
        .unwrap_or(f64::from(project.swing))
        .clamp(0.0, 1.0)
}

impl CitrusApp {
    pub(super) fn retain_pattern_editor_origin(&mut self) {
        self.pattern_editor_origin = self.pattern_editor_origin.filter(|origin| {
            origin.project_session == self.project_session
                && origin.pattern_id == self.project.active_pattern().id
                && self.project.clips.iter().any(|clip| {
                    clip.id == origin.clip_id
                        && clip.kind == ClipKind::Pattern
                        && clip.pattern_id == origin.pattern_id
                })
        });
    }

    /// Only explicit Pattern changes auto-locate a populated Channel. Idle frames
    /// and manual Rack/Piano target changes must never steal the user's target.
    pub(super) fn pattern_editor_selection_changed(&mut self) {
        self.pattern_editor_origin = None;
        self.piano_roll_state.clear_selection();
        if let Some(index) = self.first_pattern_note_channel() {
            self.selected_channel = index;
        }
        self.sync_piano_range_owner();
    }

    fn first_pattern_note_channel(&self) -> Option<usize> {
        let notes = &self.project.active_pattern().notes;
        self.project
            .channels
            .iter()
            .position(|channel| notes.iter().any(|note| note.channel_id == Some(channel.id)))
    }

    pub(super) fn open_playlist_pattern(&mut self, clip_id: u32, song_beat: f64, alternate: bool) {
        let Some(clip) = self
            .project
            .clips
            .iter()
            .find(|clip| clip.id == clip_id && clip.kind == ClipKind::Pattern)
        else {
            return;
        };
        let Some(index) = self
            .project
            .patterns
            .iter()
            .position(|p| p.id == clip.pattern_id)
        else {
            return;
        };
        let origin = PatternEditorOrigin {
            project_session: self.project_session,
            clip_id,
            pattern_id: clip.pattern_id,
        };
        let pattern = &self.project.patterns[index];
        let open_piano = !pattern.notes.is_empty() ^ alternate;
        // The Playlist thumbnail uses the SONG compiler's Piano period, even
        // when PAT is selected. Opening is navigation, not a mode/seek command.
        let period = clip.piano_period_beats(
            pattern,
            TimelineCompileOptions::default().legacy_piano_period_beats,
        );
        let source_beat =
            (f64::from(clip.source_offset) + song_beat - f64::from(clip.start)).rem_euclid(period);
        if self.project.active_pattern != index {
            self.project.active_pattern = index;
            self.active_pattern_selection_changed();
        } else {
            let selected = self
                .project
                .channels
                .get(self.selected_channel)
                .map(|c| c.id);
            if !self
                .project
                .active_pattern()
                .notes
                .iter()
                .any(|n| n.channel_id == selected)
                && let Some(index) = self.first_pattern_note_channel()
            {
                self.selected_channel = index;
                self.piano_roll_state.clear_selection();
                self.sync_piano_range_owner();
            }
        }
        self.pattern_editor_origin = Some(origin);
        self.focus_editor(if open_piano {
            StudioView::PianoRoll
        } else {
            StudioView::ChannelRack
        });
        if open_piano {
            let _ = self.piano_viewport.x.set_content_range(
                0.0,
                piano_pattern_content_end(self.project.active_pattern()),
            );
            let bar = (source_beat / 4.0).floor() * 4.0;
            let delta = (bar - self.piano_viewport.x.visible_range().0)
                * self.piano_viewport.x.pixels_per_unit();
            let _ = self.piano_viewport.x.scroll_by_pixels(delta);
            let channel = self
                .project
                .channels
                .get(self.selected_channel)
                .map(|c| c.id);
            let mut pitches = self
                .project
                .active_pattern()
                .notes
                .iter()
                .filter(|note| note.channel_id == channel)
                .map(|note| piano_pitch_row(note.note));
            if let Some(first) = pitches.next() {
                let (low, high) = pitches.fold((first, first), |(low, high), pitch| {
                    (low.min(pitch), high.max(pitch))
                });
                let _ = self.piano_viewport.y.reveal(low, high + 1.0, 12.0);
            }
        }
    }

    /// A single editor follows the explicitly opened occurrence while it is
    /// active. Otherwise use the latest-starting active occurrence (ID tie-break),
    /// so repeated placements follow naturally and overlaps stay deterministic.
    fn pattern_editor_source_position(&self) -> Option<(f64, f64)> {
        let beat = f64::from(self.beat_position);
        if !beat.is_finite() || beat < 0.0 {
            return None;
        }
        if self.playing
            && self.audio.is_some()
            && (!self.transport_clock_playing || !self.timeline_audio_matches_desired())
        {
            return None;
        }
        if self.transport_mode == TransportMode::Pattern {
            return Some((beat, pattern_playback_length_beats(&self.project).into()));
        }
        if beat >= f64::from(self.project.song_end_beat()) {
            return None;
        }
        let pattern_id = self.project.active_pattern().id;
        let active = |clip: &&Clip| {
            let length = self.project.clip_length_beats(clip);
            clip.kind == ClipKind::Pattern
                && clip.pattern_id == pattern_id
                && !clip.muted
                && clip.start.is_finite()
                && length.is_finite()
                && clip.source_offset.is_finite()
                && clip.source_offset >= 0.0
                && beat >= f64::from(clip.start)
                && beat < f64::from(clip.start) + f64::from(length)
        };
        let preferred = self.pattern_editor_origin.filter(|origin| {
            origin.project_session == self.project_session && origin.pattern_id == pattern_id
        });
        let clip = self.project.clips.iter().filter(active).max_by(|a, b| {
            let is_preferred = |clip: &Clip| preferred.is_some_and(|o| o.clip_id == clip.id);
            is_preferred(a)
                .cmp(&is_preferred(b))
                .then_with(|| a.start.total_cmp(&b.start))
                .then_with(|| a.id.cmp(&b.id))
        })?;
        Some((
            f64::from(clip.source_offset) + beat - f64::from(clip.start),
            clip.piano_period_beats(
                self.project.active_pattern(),
                TimelineCompileOptions::default().legacy_piano_period_beats,
            ),
        ))
    }

    pub(super) fn piano_editor_playhead(&self) -> Option<f32> {
        let (source, period) = self.pattern_editor_source_position()?;
        Some(source.rem_euclid(period) as f32)
    }

    pub(super) fn rack_editor_playing_step(&self) -> Option<usize> {
        if !self.playing || !self.transport_clock_playing {
            return None;
        }
        let (source, _) = self.pattern_editor_source_position()?;
        let steps = self.project.active_pattern().length_steps.clamp(1, 16);
        let phase = source.rem_euclid(steps as f64 / 4.0);
        // Timeline samples Swing at the start of the local two-step pair, not
        // at the current cursor. Keep its f64 source/placement boundary precision.
        let pair_phase = (phase / 0.5).floor() * 0.5;
        let pair_beat = f64::from(self.beat_position) - (phase - pair_phase);
        let swing = if self.transport_mode == TransportMode::Song {
            pattern_editor_swing_at(&self.project, pair_beat)
        } else {
            f64::from(self.project.swing).clamp(0.0, 1.0)
        };
        let odd_boundary = 0.25 * (1.0 + swing * 0.75);
        Some(
            ((pair_phase * 4.0) as usize + usize::from(phase - pair_phase >= odd_boundary)) % steps,
        )
    }
}
