//! Explicit placement policy changes share one Playlist music-history transaction.
use super::*;

impl CitrusApp {
    pub(super) fn set_pattern_follow(&mut self, clip_id: u32, follow: bool) {
        if self.clip_render_reject_mutation() {
            return;
        }
        let Some(index) = self
            .project
            .clips
            .iter()
            .position(|clip| clip.id == clip_id && clip.kind == ClipKind::Pattern)
        else {
            return;
        };
        if self.project.clips[index].follows_pattern() == follow {
            return;
        }
        self.finish_playlist_gesture();
        let mut candidate = self.project.clone();
        let length = if follow {
            let clip = &candidate.clips[index];
            let Some(pattern) = candidate.patterns.iter().find(|p| p.id == clip.pattern_id) else {
                self.notify("Follow Pattern requires an available source Pattern".into());
                return;
            };
            pattern.content_period_beats() as f32
        } else {
            candidate.clip_length_beats(&candidate.clips[index])
        };
        candidate.clips[index].length = length;
        candidate.clips[index].pattern_length_policy = Some(if follow {
            PatternLengthPolicy::Follow
        } else {
            PatternLengthPolicy::Manual
        });
        self.commit_editor_project(candidate);
        self.rebuild_tempo_map();
        self.request_immediate_timeline_compile();
    }

    pub(super) fn edit_pattern_clip_length(&mut self, clip_id: u32, length: f32) {
        if self.clip_render_reject_mutation() {
            return;
        }
        let Some(index) = self
            .project
            .clips
            .iter()
            .position(|clip| clip.id == clip_id && clip.kind == ClipKind::Pattern)
        else {
            return;
        };
        if self.project.clip_length_beats(&self.project.clips[index]) == length {
            return;
        }
        if self.playlist_gesture_before.is_none() {
            self.flush_pending_editor_history();
            self.playlist_gesture_before = Some(self.project.clone());
        }
        let original_timing = self.playlist_gesture_before.as_ref().and_then(|before| {
            let clip = before.clips.iter().find(|clip| clip.id == clip_id)?;
            (before.clip_length_beats(clip) == length)
                .then_some((clip.length, clip.pattern_length_policy))
        });
        if let Some((stored_length, policy)) = original_timing {
            self.project.clips[index].length = stored_length;
            self.project.clips[index].pattern_length_policy = policy;
        } else {
            self.project.clips[index].set_manual_pattern_length(length);
        }
        self.rebuild_tempo_map();
        self.request_immediate_timeline_compile();
    }

    pub(super) fn cancel_pattern_length_gesture(&mut self, ctx: &egui::Context) {
        let timing_changed = self.playlist_gesture_before.as_ref().is_some_and(|before| {
            before
                .clips
                .iter()
                .filter(|clip| clip.kind == ClipKind::Pattern)
                .any(|old| {
                    self.project
                        .clips
                        .iter()
                        .find(|clip| clip.id == old.id)
                        .is_some_and(|new| {
                            new.length != old.length
                                || new.pattern_length_policy != old.pattern_length_policy
                        })
                })
        });
        if timing_changed
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.project = self
                .playlist_gesture_before
                .take()
                .expect("timing before-image");
            ctx.stop_dragging();
            self.cancel_editor_pointer_until_release();
            self.rebuild_tempo_map();
            self.request_immediate_timeline_compile();
        }
    }
}
