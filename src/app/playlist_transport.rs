//! Playlist transport feedback and range controls share the actual playback scope.
use super::*;

/// Creating automation edits the arrangement, not a stale cached playback length.
/// Manual callers keep their explicit endpoint, even when it is shorter than snap.
pub(super) fn automation_creation_span(
    project: &Project,
    requested_start: f32,
    snap: f32,
) -> Option<(f32, f32)> {
    let end = if project.auto_song_length {
        4096.0
    } else {
        project.song_length_beats
    };
    if !end.is_finite()
        || end <= 0.0
        || !requested_start.is_finite()
        || !snap.is_finite()
        || snap <= 0.0
    {
        return None;
    }
    let max_start = f64::from((end - snap.min(end)).max(0.0));
    let start = ((f64::from(requested_start) / f64::from(snap)).floor() * f64::from(snap))
        .clamp(0.0, max_start) as f32;
    let length = 4.0_f32.min(end - start);
    (length.is_finite() && length > 0.0 && start + length > start).then_some((start, length))
}

fn bars_label(beats: f32) -> String {
    let bars = beats / 4.0;
    if (bars - bars.round()).abs() < 0.001 {
        format!(
            "{} {}",
            bars.round() as u32,
            if bars.round() == 1.0 { "bar" } else { "bars" }
        )
    } else {
        format!("{beats} beats")
    }
}

fn mode_button(ui: &mut egui::Ui, mode: TransportMode, selected: bool) -> Response {
    let (label, accent) = match mode {
        TransportMode::Pattern => ("PAT", theme::GREEN),
        TransportMode::Song => ("SONG", theme::ORANGE),
    };
    let face = ui.painter().add(egui::Shape::Noop);
    let response = ui.add_sized(
        [46.0, 19.0],
        egui::Button::new(RichText::new(label).strong().size(9.0).color(if selected {
            theme::WELL
        } else {
            theme::MUTED
        }))
        .selected(selected)
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE),
    );
    ui.painter().set(
        face,
        theme::button_face(
            response.rect,
            if selected {
                accent
            } else if response.hovered() {
                theme::HIGHLIGHT
            } else {
                theme::PANEL_ALT
            },
            selected || response.is_pointer_button_down_on(),
            response.has_focus(),
        ),
    );
    response
}

impl CitrusApp {
    pub(super) fn transport_mode_controls(&mut self, ui: &mut egui::Ui) {
        let mut requested_mode = None;
        let mut requested_view = None;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for (mode, hint, view) in [
                (
                    TransportMode::Pattern,
                    "Play only the selected Pattern (L)\nRight-click: Channel Rack",
                    StudioView::ChannelRack,
                ),
                (
                    TransportMode::Song,
                    "Play the Playlist arrangement (L)\nRight-click: Playlist",
                    StudioView::Playlist,
                ),
            ] {
                let response =
                    mode_button(ui, mode, self.transport_mode == mode).on_hover_text(hint);
                if response.clicked() {
                    requested_mode = Some(mode);
                }
                if response.secondary_clicked() {
                    requested_view = Some(view);
                }
            }
        });
        if let Some(mode) = requested_mode {
            self.set_transport_mode(mode);
        }
        if let Some(view) = requested_view {
            self.focus_editor(view);
        }
    }

    pub(super) fn transport_scope_caption(&self) -> String {
        let extent = bars_label(self.transport_loop_end_beat());
        match self.transport_mode {
            TransportMode::Pattern => format!("PAT · {extent} loop"),
            TransportMode::Song => format!(
                "SONG · {extent} · {}",
                if self.project.auto_song_length {
                    "AUTO"
                } else {
                    "MANUAL"
                }
            ),
        }
    }

    pub(super) fn playlist_scope_caption(&self) -> String {
        match self.transport_mode {
            TransportMode::Pattern => format!(
                "PAT · {} · {} loop — Playlist",
                self.project.active_pattern().name,
                bars_label(self.transport_loop_end_beat())
            ),
            TransportMode::Song => format!(
                "SONG · {} · {} — Draw: {}",
                bars_label(self.project.song_end_beat()),
                if self.project.auto_song_length {
                    "AUTO"
                } else {
                    "MANUAL"
                },
                self.project.active_pattern().name
            ),
        }
    }

    /// Range policy is music data; unlike PAT/SONG preference it gets its own undo step.
    fn set_automatic_song_length(&mut self, automatic: bool) {
        if automatic == self.project.auto_song_length || self.clip_render_reject_mutation() {
            return;
        }
        if self.recording || self.input_recorder.is_some() || !self.midi_recording.is_idle() {
            self.notify("Stop the active recording before changing the song range".into());
            return;
        }
        let mut candidate = self.project.clone();
        if !automatic {
            candidate.song_length_beats = self.project.song_end_beat();
        }
        candidate.auto_song_length = automatic;
        if commit_explicit_project_history_transaction(
            &mut self.project,
            &mut self.history_snapshot,
            &mut self.history_fingerprint,
            &mut self.undo_stack,
            &mut self.redo_stack,
            &mut self.dirty,
            candidate,
        ) {
            self.last_history_check = Instant::now();
            self.last_history_capture = self.last_history_check;
            self.rebuild_tempo_map();
            self.request_immediate_timeline_compile();
            self.transport_loop_dirty = true;
        }
    }

    pub(super) fn playlist_song_range_controls(&mut self, ui: &mut egui::Ui) {
        if ui
            .add(theme::Button::new(RichText::new("AUTO").size(8.0)).selected(self.project.auto_song_length))
            .on_hover_text("Song end follows Playlist clips and unplaced automation. View zoom does not change it.")
            .clicked()
        {
            self.set_automatic_song_length(!self.project.auto_song_length);
        }
        if self.project.auto_song_length {
            ui.label(
                RichText::new(bars_label(self.project.song_end_beat()))
                    .size(9.0)
                    .color(theme::TEXT),
            );
        } else {
            ui.add_enabled(
                !self.recording && self.input_recorder.is_none() && self.midi_recording.is_idle(),
                egui::DragValue::new(&mut self.project.song_length_beats)
                    .range(0.25..=4096.0)
                    .clamp_existing_to_range(false)
                    .speed(0.25)
                    .suffix(" beats"),
            )
            .on_hover_text("Manual playback/export end. Existing Draw, import and recording commands may extend it; clips outside it stay in the project.");
            ui.label(RichText::new("MANUAL").size(8.0).color(theme::MUTED));
        }
    }
}
