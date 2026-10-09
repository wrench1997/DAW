//! Note expression uses committed raw-event edits and a separate, draft-only dialog.
//! No wheel burst retains a Project snapshot that a later save/import could overwrite.
use super::*;

const COARSE: f32 = 0.05;
const FINE: f32 = 0.01;
const MAX_WHEEL_UNITS: f32 = 8.0;

#[derive(Clone)]
pub(super) struct NoteProperties {
    session: u64,
    pattern: usize,
    pattern_id: u32,
    channel: u32,
    origins: Vec<PianoNote>,
    edited: PianoNote,
    transpose: i16,
    velocity_change: f32,
    mute: Option<bool>,
    assignment: Option<Option<u32>>,
    error: Option<String>,
    numeric_focus: bool,
}

// Only these exact modifiers own velocity. Shift+Alt is reserved for time nudge.
fn velocity_modifiers(m: egui::Modifiers) -> bool {
    m.alt && !m.shift && !m.mac_cmd
}

fn wheel_delta(unit: egui::MouseWheelUnit, delta: Vec2, modifiers: egui::Modifiers) -> Option<f32> {
    if !velocity_modifiers(modifiers)
        || !delta.x.is_finite()
        || !delta.y.is_finite()
        || delta.y == 0.0
    {
        return None;
    }
    let units = match unit {
        egui::MouseWheelUnit::Line => delta.y,
        // An explicit convention for trackpads and page wheels, not FL parity.
        egui::MouseWheelUnit::Point => delta.y / 40.0,
        egui::MouseWheelUnit::Page => delta.y,
    }
    .clamp(-MAX_WHEEL_UNITS, MAX_WHEEL_UNITS);
    Some(units * if modifiers.ctrl { FINE } else { COARSE })
}

fn valid_origins(project: &Project, origins: &[PianoNote]) -> bool {
    let ids: HashSet<_> = origins.iter().map(|n| n.id).collect();
    !origins.is_empty()
        && origins.len() <= crate::piano_roll::MAX_TRANSFORM_NOTES
        && ids.len() == origins.len()
        && !ids.contains(&0)
        && project
            .patterns
            .iter()
            .flat_map(|p| &p.notes)
            .filter(|n| ids.contains(&n.id))
            .count()
            == ids.len()
        && origins.iter().all(|n| {
            n.note <= 127
                && n.start.is_finite()
                && n.start >= 0.0
                && n.length.is_finite()
                && n.length > 0.0
                && (n.start + n.length).is_finite()
                && n.velocity.is_finite()
                && (0.0..=1.0).contains(&n.velocity)
        })
}

fn same_note(a: &PianoNote, b: &PianoNote) -> bool {
    a.id == b.id
        && a.channel_id == b.channel_id
        && a.group_id == b.group_id
        && a.note == b.note
        && a.start == b.start
        && a.length == b.length
        && a.velocity == b.velocity
        && a.muted == b.muted
}

fn wheel_residue_key() -> Id {
    Id::new("piano-expression-wheel-residue")
}

/// While egui drains an owned wheel tail, expose only fresh raw navigation input.
/// Releasing Alt and immediately reversing a normal wheel must not reuse the old
/// direction. The temporary unsmoothed navigation mode ends once egui is idle.
pub(super) fn suppress_owned_wheel_tail(ctx: &egui::Context) {
    if !ctx.data(|d| d.get_temp::<bool>(wheel_residue_key()).unwrap_or(false)) {
        return;
    }
    if !ctx.input(|i| i.is_scrolling()) {
        ctx.data_mut(|d| d.remove::<bool>(wheel_residue_key()));
        return;
    }
    let options = ctx.options(|o| o.input_options);
    let height = ctx.content_rect().height();
    ctx.input_mut(|input| {
        input.smooth_scroll_delta = input
            .events
            .iter()
            .filter_map(|event| {
                let egui::Event::MouseWheel {
                    unit,
                    delta,
                    phase: egui::TouchPhase::Move,
                    modifiers,
                } = event
                else {
                    return None;
                };
                if !delta.is_finite() {
                    return None;
                }
                let mut delta = match unit {
                    egui::MouseWheelUnit::Point => *delta,
                    egui::MouseWheelUnit::Line => *delta * options.line_scroll_speed,
                    egui::MouseWheelUnit::Page => *delta * height,
                };
                let horizontal = modifiers.matches_any(options.horizontal_scroll_modifier);
                let vertical = modifiers.matches_any(options.vertical_scroll_modifier);
                if horizontal && !vertical {
                    delta = Vec2::new(delta.x + delta.y, 0.0);
                }
                if vertical && !horizontal {
                    delta = Vec2::new(0.0, delta.x + delta.y);
                }
                Some(delta)
            })
            .fold(Vec2::ZERO, |sum, delta| sum + delta);
    });
}

impl CitrusApp {
    fn expression_targets(&self, anchor: u64) -> Option<Vec<PianoNote>> {
        let channel = self.project.channels.get(self.selected_channel)?.id;
        if channel == 0
            || self
                .project
                .channels
                .iter()
                .filter(|c| c.id == channel)
                .count()
                != 1
        {
            return None;
        }
        let notes = &self.project.active_pattern().notes;
        let anchor = notes
            .iter()
            .find(|n| n.id == anchor && n.channel_id == Some(channel))?;
        let selection = if self.piano_roll_state.selection_ids.contains(&anchor.id) {
            self.piano_roll_state.selection_ids.clone()
        } else {
            HashSet::from([anchor.id])
        };
        let targets = crate::piano_roll::piano_keyboard_targets(
            notes,
            &selection,
            channel,
            self.piano_roll_state.grouping_enabled,
        );
        let origins: Vec<_> = notes
            .iter()
            .filter(|n| n.channel_id == Some(channel) && targets.contains(&n.id))
            .cloned()
            .collect();
        valid_origins(&self.project, &origins).then_some(origins)
    }

    pub(super) fn piano_velocity_wheel(&mut self, ui: &egui::Ui, grid: Rect) -> bool {
        if ui.is_sizing_pass() || !ui.is_enabled() {
            return false;
        }
        let ctx = ui.ctx();
        let Some(pointer) = ctx.pointer_hover_pos() else {
            return false;
        };
        if !grid.intersect(ui.clip_rect()).contains(pointer)
            || ctx.layer_id_at(pointer) != Some(ui.layer_id())
        {
            return false;
        }
        let owns = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { modifiers, .. } if velocity_modifiers(*modifiers))));
        if !owns {
            return false;
        }
        ctx.data_mut(|d| d.insert_temp(wheel_residue_key(), true));
        let ready = self.piano_editor_command_ready(ctx, false)
            && self.native_editor_topology_guard().is_ok()
            && ctx.input(|i| {
                i.focused
                    && !i
                        .events
                        .iter()
                        .any(|e| matches!(e, egui::Event::WindowFocused(false)))
                    && !i.pointer.any_down()
                    && !i
                        .events
                        .iter()
                        .any(|e| matches!(e, egui::Event::PointerButton { .. }))
            });
        let deltas: Vec<_> = ctx.input_mut(|input| {
            let deltas = input.events.iter().filter_map(|event| match event {
                egui::Event::MouseWheel { unit, delta, modifiers, phase: egui::TouchPhase::Move } => wheel_delta(*unit, *delta, *modifiers),
                _ => None,
            }).collect();
            // egui's smoothed residue must never be replayed as a project edit or zoom.
            input.smooth_scroll_delta = Vec2::ZERO;
            input.events.retain(|e| !matches!(e, egui::Event::MouseWheel { modifiers, .. } if velocity_modifiers(*modifiers)));
            deltas
        });
        if !ready {
            return true;
        }
        let channel = self
            .project
            .channels
            .get(self.selected_channel)
            .map(|c| c.id);
        let anchor = self
            .project
            .active_pattern()
            .notes
            .iter()
            .rev()
            .filter(|n| n.channel_id == channel && n.note <= 127)
            .find(|n| {
                piano_mouse::note_rects(grid, self.piano_viewport, n)
                    .1
                    .contains(pointer)
            })
            .map(|n| n.id);
        let Some(anchor) = anchor else {
            return true;
        };
        // A committed transaction for each actual raw event. No timer, origins replay,
        // gesture coalescing, or smooth-scroll synthesis can revive this edit later.
        for requested in deltas {
            let Some(origins) = self.expression_targets(anchor) else {
                break;
            };
            let Ok(delta) = clamp_group_velocity_delta(&origins, requested) else {
                break;
            };
            if delta == 0.0 {
                continue;
            }
            let ids: HashSet<_> = origins.iter().map(|n| n.id).collect();
            let mut candidate = self.project.clone();
            for note in &mut candidate.active_pattern_mut().notes {
                if ids.contains(&note.id) {
                    note.velocity = (note.velocity + delta).clamp(0.0, 1.0);
                }
            }
            self.commit_editor_project(candidate);
        }
        true
    }

    pub(super) fn begin_note_properties(&mut self, ctx: &egui::Context, anchor: u64) {
        if !self.piano_editor_command_ready(ctx, false)
            || ctx.input(|i| !i.focused || i.pointer.any_down())
        {
            return;
        }
        self.poll_native_editor_snapshots();
        if let Err(error) = self.native_editor_topology_guard() {
            self.notify(error);
            return;
        }
        let Some(origins) = self.expression_targets(anchor) else {
            return;
        };
        let edited = origins.iter().find(|n| n.id == anchor).unwrap().clone();
        self.piano_note_properties = Some(NoteProperties {
            session: self.project_session,
            pattern: self.project.active_pattern,
            pattern_id: self.project.active_pattern().id,
            channel: edited.channel_id.unwrap(),
            origins,
            edited,
            transpose: 0,
            velocity_change: 0.0,
            mute: None,
            assignment: None,
            error: None,
            numeric_focus: false,
        });
    }

    pub(super) fn accept_note_properties(&mut self) {
        let Some(mut draft) = self.piano_note_properties.take() else {
            return;
        };
        if self.project_snapshot_transition_pending()
            || self.project_lifecycle_barriers_active()
            || self.editor_pointer_gesture_active()
            || self.piano_roll_gesture_before.is_some()
            || self.playlist_gesture_before.is_some()
        {
            draft.error =
                Some("Wait for the project change or save to finish, then Apply again".into());
            self.piano_note_properties = Some(draft);
            return;
        }
        self.poll_native_editor_snapshots();
        if let Err(error) = self.native_editor_topology_guard() {
            self.notify(error);
            return;
        }
        match draft.candidate(self) {
            Ok(candidate) => self.commit_editor_project(candidate),
            Err(error) => {
                draft.error = Some(error.into());
                self.piano_note_properties = Some(draft);
            }
        }
    }

    pub(super) fn note_properties_dialog(&mut self, ctx: &egui::Context) {
        let Some(draft) = self.piano_note_properties.as_mut() else {
            return;
        };
        // Navigation/session changes can arrive from external/native sources. Drop the
        // draft without restoring any snapshot; unrelated committed data stays intact.
        if draft.session != self.project_session
            || draft.pattern != self.project.active_pattern
            || self
                .project
                .channels
                .get(self.selected_channel)
                .map(|c| c.id)
                != Some(draft.channel)
        {
            self.piano_note_properties = None;
            return;
        }
        let text_was_focused = ctx.text_edit_focused() || draft.numeric_focus;
        let mut apply = false;
        let mut cancel = false;
        let mut reset = false;
        let channels = &self.project.channels;
        egui::Modal::new(Id::new("piano-note-properties")).show(ctx, |ui| {
            ui.set_width(360.0_f32.min((ctx.content_rect().width() - 48.0).max(240.0)));
            ui.heading("Note properties");
            ui.label(format!("{} active-Channel note(s) · changes apply only on Apply", draft.origins.len()));
            ui.separator();
            egui::Grid::new("note-properties-fields").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                if draft.origins.len() == 1 {
                    property_control(ui, "Pitch (MIDI)", egui::DragValue::new(&mut draft.edited.note).range(0..=127));
                    property_control(ui, "Start (beats)", egui::DragValue::new(&mut draft.edited.start).range(0.0..=4095.95).clamp_existing_to_range(false).speed(0.05).max_decimals(3));
                    property_control(ui, "Length (beats)", egui::DragValue::new(&mut draft.edited.length).range(0.05..=4096.0).clamp_existing_to_range(false).speed(0.05).max_decimals(3));
                    property_control(ui, "Velocity", egui::Slider::new(&mut draft.edited.velocity, 0.0..=1.0).fixed_decimals(3));
                } else {
                    property_control(ui, "Transpose (semitones)", egui::DragValue::new(&mut draft.transpose).range(-127..=127));
                    property_control(ui, "Velocity change", egui::Slider::new(&mut draft.velocity_change, -1.0..=1.0).fixed_decimals(3));
                    ui.label("Start / length"); ui.label("Individual notes only"); ui.end_row();
                }
                ui.label("Mute");
                let current_mute = if draft.origins.iter().all(|n| n.muted) { "Keep current: muted" }
                    else if draft.origins.iter().all(|n| !n.muted) { "Keep current: unmuted" } else { "Keep mixed values" };
                let response = egui::ComboBox::from_id_salt("properties-mute").width(180.0).truncate().selected_text(match draft.mute { None => current_mute, Some(true) => "Muted", Some(false) => "Unmuted" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut draft.mute, None, "Keep current");
                        ui.selectable_value(&mut draft.mute, Some(true), "Muted");
                        ui.selectable_value(&mut draft.mute, Some(false), "Unmuted");
                    });
                remember_control(ui, "Mute", &response.response); ui.end_row();
                ui.label("Channel");
                let name = match draft.assignment {
                    None => channels.iter().find(|c| c.id == draft.channel).map_or("Keep current", |c| c.name.as_str()), Some(None) => "Unassigned",
                    Some(Some(id)) => channels.iter().find(|c| c.id == id).map_or("Missing channel", |c| c.name.as_str()),
                };
                let response = egui::ComboBox::from_id_salt("properties-channel").width(180.0).truncate().selected_text(name).show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.assignment, None, "Keep current");
                    ui.selectable_value(&mut draft.assignment, Some(None), "Unassigned");
                    for channel in channels { ui.selectable_value(&mut draft.assignment, Some(Some(channel.id)), &channel.name); }
                }); remember_control(ui, "Channel", &response.response); ui.end_row();
            });
            if draft.origins.len() > 1 {
                ui.add_space(8.0);
                ui.label("Pitch and velocity change together. The whole selection stops at the first limit, preserving its differences.");
            }
            if let Some(error) = &draft.error { ui.colored_label(theme::AMBER, error); }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                apply = ui.button("Apply").clicked();
                cancel = ui.button("Cancel").clicked();
                reset = ui.button("Reset").clicked();
            });
        });
        draft.numeric_focus = ctx.text_edit_focused();
        if text_was_focused {
            ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
        }
        if reset {
            draft.edited = draft
                .origins
                .iter()
                .find(|n| n.id == draft.edited.id)
                .unwrap()
                .clone();
            draft.transpose = 0;
            draft.velocity_change = 0.0;
            draft.mute = None;
            draft.assignment = None;
            draft.error = None;
        }
        if cancel {
            self.piano_note_properties = None;
        } else if apply {
            self.accept_note_properties();
        }
    }
}

impl NoteProperties {
    fn candidate(&self, app: &CitrusApp) -> Result<Project, &'static str> {
        let current: HashMap<_, _> = app
            .project
            .active_pattern()
            .notes
            .iter()
            .map(|n| (n.id, n))
            .collect();
        if self.session != app.project_session
            || self.pattern != app.project.active_pattern
            || app.project.active_pattern().id != self.pattern_id
            || app.project.channels.get(app.selected_channel).map(|c| c.id) != Some(self.channel)
            || !valid_origins(&app.project, &self.origins)
            || self.origins.iter().any(|origin| {
                current
                    .get(&origin.id)
                    .is_none_or(|note| !same_note(note, origin))
            })
        {
            return Err("Notes changed while properties were open; reopen properties to edit them");
        }
        if let Some(Some(id)) = self.assignment
            && (id == 0 || app.project.channels.iter().filter(|c| c.id == id).count() != 1)
        {
            return Err("The selected destination Channel is no longer available");
        }
        let anchor = self
            .origins
            .iter()
            .find(|n| n.id == self.edited.id)
            .ok_or("The note no longer exists")?;
        let single = self.origins.len() == 1;
        let mut edited = self.edited.clone();
        if single
            && (edited.start != anchor.start || edited.length != anchor.length)
            && (!edited.start.is_finite()
                || !edited.length.is_finite()
                || !(0.0..=4095.95).contains(&edited.start)
                || !(0.05..=4096.0).contains(&edited.length)
                || edited.start + edited.length > 4096.0
                || edited.note > 127)
        {
            return Err("Note time must fit within 0–4096 beats with a positive length");
        }
        if single
            && app.piano_roll_state.snap_to_scale
            && (edited.note != anchor.note || edited.start != anchor.start)
        {
            edited.note = snap_pitch_to_scale(
                edited.note,
                app.piano_roll_state.scale_root,
                app.piano_roll_state.scale,
            );
        }
        let requested_pitch = if single {
            i16::from(edited.note) - i16::from(anchor.note)
        } else {
            self.transpose
        };
        let min = self
            .origins
            .iter()
            .map(|n| i16::from(n.note))
            .min()
            .unwrap();
        let max = self
            .origins
            .iter()
            .map(|n| i16::from(n.note))
            .max()
            .unwrap();
        let pitch_delta = requested_pitch.clamp(-min, 127 - max);
        let requested_velocity = if single {
            edited.velocity - anchor.velocity
        } else {
            self.velocity_change
        };
        let delta = clamp_group_velocity_delta(&self.origins, requested_velocity)
            .map_err(|_| "Velocity must be a finite value between 0 and 1")?;
        let ids: HashSet<_> = self.origins.iter().map(|n| n.id).collect();
        let mut candidate = app.project.clone();
        for note in &mut candidate.active_pattern_mut().notes {
            if !ids.contains(&note.id) {
                continue;
            }
            note.note = (i16::from(note.note) + pitch_delta) as u8;
            note.velocity = (note.velocity + delta).clamp(0.0, 1.0);
            if single {
                note.start = edited.start;
                note.length = edited.length;
            }
            if let Some(mute) = self.mute {
                note.muted = mute;
            }
            if let Some(channel) = self.assignment {
                note.channel_id = channel;
            }
        }
        if self.assignment.is_some() {
            normalize_piano_note_groups(&mut candidate.active_pattern_mut().notes);
        }
        Ok(candidate)
    }
}

fn property_control(ui: &mut egui::Ui, name: &'static str, widget: impl egui::Widget) {
    let label = ui.label(name);
    let response = ui.add(widget).labelled_by(label.id);
    remember_control(ui, name, &response);
    ui.end_row();
}

fn remember_control(ui: &egui::Ui, name: &'static str, response: &egui::Response) {
    // Exact production widget IDs, for inspecting real layout/input in regression tests.
    ui.ctx()
        .data_mut(|data| data.insert_temp(Id::new(("note-properties-control", name)), response.id));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Box<CitrusApp> {
        let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        app.selected_channel = 0;
        let channel = app.project.channels[0].id;
        app.project.active_pattern_mut().notes = vec![PianoNote {
            id: 100_000,
            channel_id: Some(channel),
            group_id: None,
            note: 60,
            start: 0.0,
            length: 1.0,
            velocity: 0.5,
            selected: false,
            muted: false,
        }];
        app
    }
    fn draft(app: &CitrusApp) -> NoteProperties {
        NoteProperties {
            session: app.project_session,
            pattern: app.project.active_pattern,
            pattern_id: app.project.active_pattern().id,
            channel: app.project.channels[0].id,
            origins: app.project.active_pattern().notes.clone(),
            edited: app.project.active_pattern().notes[0].clone(),
            transpose: 0,
            velocity_change: 0.0,
            mute: None,
            assignment: None,
            error: None,
            numeric_focus: false,
        }
    }

    #[test]
    fn piano_expression_candidate_rejects_nonfinite_and_stale_global_identity() {
        let mut app = fixture();
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut d = draft(&app);
            d.edited.velocity = value;
            assert!(d.candidate(&app).is_err());
            let mut d = draft(&app);
            d.edited.start = value;
            assert!(d.candidate(&app).is_err());
            let mut d = draft(&app);
            d.edited.length = value;
            assert!(d.candidate(&app).is_err());
        }
        let d = draft(&app);
        app.project_session += 1;
        assert!(d.candidate(&app).is_err());
        app.project_session -= 1;
        let mut another = app.project.active_pattern().clone();
        another.id += 500;
        app.project.patterns.push(another);
        assert!(
            d.candidate(&app).is_err(),
            "target ID duplicated across patterns must fail closed"
        );
    }

    #[test]
    fn piano_expression_candidate_relative_bounds_and_linear_maximum_selection() {
        let mut app = fixture();
        let template = app.project.active_pattern().notes[0].clone();
        app.project.active_pattern_mut().notes = (0..crate::piano_roll::MAX_TRANSFORM_NOTES)
            .map(|i| PianoNote {
                id: 100_000 + i as u64,
                note: if i % 2 == 0 { 60 } else { 126 },
                velocity: if i % 2 == 0 { 0.2 } else { 0.8 },
                group_id: Some(1),
                ..template.clone()
            })
            .collect();
        app.piano_roll_state.selection_ids = app
            .project
            .active_pattern()
            .notes
            .iter()
            .map(|n| n.id)
            .collect();
        let origins = app.expression_targets(100_000).unwrap();
        assert_eq!(origins.len(), crate::piano_roll::MAX_TRANSFORM_NOTES);
        let mut d = draft(&app);
        d.transpose = 127;
        d.velocity_change = 1.0;
        let candidate = d.candidate(&app).unwrap();
        let notes = &candidate.active_pattern().notes;
        assert_eq!((notes[0].note, notes[1].note), (61, 127));
        assert!((notes[0].velocity - 0.4).abs() < 0.000_01);
        assert_eq!(notes[1].velocity, 1.0);
        d.transpose = -127;
        d.velocity_change = -1.0;
        let candidate = d.candidate(&app).unwrap();
        let notes = &candidate.active_pattern().notes;
        assert_eq!((notes[0].note, notes[1].note), (0, 66));
        assert_eq!(notes[0].velocity, 0.0);
        assert!((notes[1].velocity - 0.6).abs() < 0.000_01);
    }

    #[test]
    fn piano_expression_candidate_preserves_unrelated_project_changes_and_frozen_scope() {
        let mut app = fixture();
        let mut d = draft(&app);
        d.edited.velocity = 0.7;
        app.project.name = "A newer unrelated change".into();
        app.piano_roll_state.selection_ids.clear();
        let candidate = d.candidate(&app).unwrap();
        assert_eq!(candidate.name, "A newer unrelated change");
        assert_eq!(candidate.active_pattern().notes[0].velocity, 0.7);
        assert_eq!(app.project.active_pattern().notes[0].velocity, 0.5);
        d.assignment = Some(Some(u32::MAX));
        assert!(d.candidate(&app).is_err());
    }
}
