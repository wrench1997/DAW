//! Visual-only Piano Roll references. No widgets, note IDs, or edit targets are
//! registered by the painter; all editing continues to use the active Pattern.
use super::*;
use crate::piano_roll::PianoGhostSource;

// A single editable voice uses a stable teal face, independent of the Channel's
// arrangement color. References are cooler and deliberately lower contrast.
pub(super) const NOTE_FACE: Color32 = Color32::from_rgb(76, 190, 166);
pub(super) const NOTE_EDGE: Color32 = Color32::from_rgb(117, 219, 195);
pub(super) const GHOST_FACE: Color32 = Color32::from_rgb(58, 76, 101);
pub(super) const GHOST_EDGE: Color32 = Color32::from_rgb(83, 104, 133);
pub(super) const ACTIVITY: Color32 = Color32::from_rgb(242, 167, 64);

impl CitrusApp {
    /// Visual scroll extent only: reference tails never change the active Pattern,
    /// transport loop, Fit Notes target, or compiled playback timeline.
    pub(super) fn piano_ghost_content_end(&self) -> f64 {
        let active_end = piano_pattern_content_end(self.project.active_pattern());
        if !self.piano_roll_state.ghosts_visible {
            return active_end;
        }
        match self.piano_roll_state.ghost_source {
            PianoGhostSource::CurrentPattern => active_end,
            PianoGhostSource::ReferencePattern(id) => self
                .project
                .patterns
                .iter()
                .find(|pattern| pattern.id == id)
                .map_or(active_end, |pattern| {
                    active_end.max(piano_pattern_content_end(pattern))
                }),
        }
    }

    pub(super) fn piano_ghost_controls(&mut self, ui: &mut egui::Ui) {
        let state = &mut self.piano_roll_state;
        state.ghost_source = state.ghost_source.resolved(&self.project.patterns);
        let response = icons::icon_button(ui, StudioIcon::Group, state.ghosts_visible, 24.0);
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                ui.is_enabled(),
                state.ghosts_visible,
                "GHOST NOTES",
            )
        });
        if response.on_hover_text("Ghost notes · Alt+V. Read-only blue-gray references; source is retained while hidden.").clicked() {
            state.ghosts_visible = !state.ghosts_visible;
        }
        let selected = match state.ghost_source {
            PianoGhostSource::CurrentPattern => "Current Pattern".to_owned(),
            PianoGhostSource::ReferencePattern(id) => self
                .project
                .patterns
                .iter()
                .find(|p| p.id == id)
                .map(|p| {
                    let short = p.name.chars().take(22).collect::<String>();
                    format!(
                        "Ref #{id}: {short}{}",
                        if p.name.chars().count() > 22 {
                            "…"
                        } else {
                            ""
                        }
                    )
                })
                .unwrap_or_else(|| "Current Pattern".to_owned()),
        };
        let current_source = match state.ghost_source {
            PianoGhostSource::CurrentPattern => true,
            PianoGhostSource::ReferencePattern(id) => id == self.project.active_pattern().id,
        };
        let scope = if !state.ghosts_visible {
            "Ghosts off"
        } else if current_source {
            "Other Channels · read only"
        } else {
            "All · local beats · read only"
        };
        // Reserve the combo's width before its internal horizontal layout runs;
        // otherwise egui can grow a wrapped narrow toolbar instead of wrapping it.
        ui.allocate_ui_with_layout(
            Vec2::new(100.0, ui.spacing().interact_size.y),
            Layout::left_to_right(Align::Center),
            |ui| {
        egui::ComboBox::from_id_salt("piano-ghost-source")
            .width(100.0)
            .truncate()
            .selected_text(selected)
            .show_ui(ui, |ui| {
                ui.set_max_width(280.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                ui.label(RichText::new(scope).size(9.0).color(theme::MUTED));
                ui.label("Read-only references · Pattern-local beat 0 alignment");
                ui.separator();
                ui.selectable_value(&mut state.ghost_source, PianoGhostSource::CurrentPattern, "Current Pattern: other Channels");
                ui.separator();
                ui.label("Reference Pattern (all Channels)");
                for pattern in &self.project.patterns {
                    let suffix = if pattern.id == self.project.active_pattern().id { " · current" } else { "" };
                    ui.selectable_value(
                        &mut state.ghost_source,
                        PianoGhostSource::ReferencePattern(pattern.id),
                        format!("#{} {}{suffix}", pattern.id, pattern.name),
                    );
                }
            }).response.on_hover_text("Ghost source: current Pattern's other Channels, or one reference Pattern (all Channels). Read-only. Reference beat 0 aligns with current Pattern beat 0, once, without Playlist offsets or looping. Reference tails extend only visual scrolling. Fit Notes targets the editable Channel. No playback, project, or Undo changes.");
            },
        );
    }

    pub(super) fn paint_piano_ghosts(
        &self,
        painter: &egui::Painter,
        grid: Rect,
        target_channel: Option<u32>,
    ) {
        for note in self.piano_roll_state.ghost_source.notes(
            self.piano_roll_state.ghosts_visible,
            &self.project.patterns,
            self.project.active_pattern,
            target_channel,
        ) {
            let row = piano_pitch_row(note.note);
            if !content_span_is_visible(
                self.piano_viewport.x,
                f64::from(note.start),
                f64::from(note.start) + f64::from(note.length),
            ) || !content_span_is_visible(self.piano_viewport.y, row, row + 1.0)
            {
                continue;
            }
            let (_, rect, _, _) = piano_mouse::note_rects(grid, self.piano_viewport, note);
            // Filled blue-gray reference silhouettes have no handles or widgets.
            // Their muted face and single top highlight remain distinct from the
            // brighter teal editable voice, including same-Channel references.
            let fill = if note.muted {
                GHOST_FACE.gamma_multiply(0.65)
            } else {
                GHOST_FACE
            };
            painter.rect_filled(rect, 1.0, fill);
            painter.line_segment(
                [
                    rect.left_top() + Vec2::new(0.5, 0.5),
                    rect.right_top() + Vec2::new(-0.5, 0.5),
                ],
                Stroke::new(0.7, GHOST_EDGE),
            );
        }
    }
}
