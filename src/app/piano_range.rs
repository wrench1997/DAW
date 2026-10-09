//! Independent, session-local edit/repeat range. This never configures transport looping.
use super::*;
use crate::piano_roll::{MAX_PIANO_BEAT, PianoTimeRange, piano_keyboard_targets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RangeAction {
    FromSelection,
    Move(i8),
    Clear,
}

#[derive(Clone, Debug)]
struct RangeGesture {
    owner: (u64, usize, u32, Option<u32>),
    channel: Option<u32>,
    ruler: Rect,
    axis: [f64; 2],
    pointer: Pos2,
    start: f64,
    snap: PianoSnap,
    before_range: Option<PianoTimeRange>,
    before_selection: HashSet<u64>,
    additive: bool,
    armed: bool,
    moved: bool,
    modifiers: egui::Modifiers,
}

#[derive(Clone, Debug)]
struct RulerClick {
    owner: (u64, usize, u32, Option<u32>),
    ruler: Rect,
    axis: [f64; 2],
    point: Pos2,
    time: f64,
}

fn click_key() -> Id {
    Id::new("piano-ruler-last-click")
}

fn key() -> Id {
    Id::new("piano-range-gesture")
}
pub(super) fn active(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<RangeGesture>(key()).is_some())
}

impl CitrusApp {
    pub(super) fn sync_piano_range_owner(&mut self) {
        let owner = self
            .project
            .patterns
            .get(self.project.active_pattern)
            .map(|pattern| {
                (
                    self.project_session,
                    self.project.active_pattern,
                    pattern.id,
                    self.project
                        .channels
                        .get(self.selected_channel)
                        .map(|channel| channel.id),
                )
            });
        if self.piano_roll_state.range_owner != owner {
            self.piano_roll_state.repeat_range = None;
            self.piano_roll_state.range_owner = owner;
        }
        self.piano_roll_state.repeat_range = self
            .piano_roll_state
            .repeat_range
            .and_then(|range| PianoTimeRange::new(range.start, range.end));
    }

    pub(super) fn cancel_piano_range_gesture(&mut self, ctx: &egui::Context) {
        let gesture = ctx.data_mut(|data| {
            let gesture = data.get_temp::<RangeGesture>(key());
            data.remove::<RangeGesture>(key());
            data.remove::<RulerClick>(click_key());
            gesture
        });
        if let Some(gesture) = gesture {
            self.sync_piano_range_owner();
            if self.piano_roll_state.range_owner == Some(gesture.owner) {
                self.piano_roll_state.repeat_range = gesture.before_range;
                self.piano_roll_state.selection_ids = gesture.before_selection;
            }
            self.cancel_piano_mouse_pointer(ctx);
        }
    }

    pub(super) fn apply_piano_range_action(&mut self, action: RangeAction) {
        self.sync_piano_range_owner();
        match action {
            RangeAction::Clear => self.piano_roll_state.repeat_range = None,
            RangeAction::Move(direction) => {
                if let Some(range) = self.piano_roll_state.repeat_range {
                    let width = range.end - range.start;
                    let delta = (width * f64::from(direction))
                        .clamp(-range.start, MAX_PIANO_BEAT - range.end);
                    self.piano_roll_state.repeat_range =
                        PianoTimeRange::new(range.start + delta, range.end + delta);
                }
            }
            RangeAction::FromSelection => {
                let Some(channel) = self
                    .project
                    .channels
                    .get(self.selected_channel)
                    .map(|c| c.id)
                else {
                    return;
                };
                let notes = &self.project.active_pattern().notes;
                // This action intentionally has no selected-or-all fallback.
                if !notes.iter().any(|n| {
                    n.channel_id == Some(channel)
                        && self.piano_roll_state.selection_ids.contains(&n.id)
                }) {
                    return;
                }
                let targets = piano_keyboard_targets(
                    notes,
                    &self.piano_roll_state.selection_ids,
                    channel,
                    self.piano_roll_state.grouping_enabled,
                );
                let selected: Vec<_> = notes.iter().filter(|n| targets.contains(&n.id)).collect();
                if selected.iter().any(|n| {
                    !n.start.is_finite()
                        || !n.length.is_finite()
                        || n.start < 0.0
                        || n.length <= 0.0
                        || f64::from(n.start) + f64::from(n.length) > MAX_PIANO_BEAT
                }) {
                    self.notify("Range needs valid selected notes within 0–4096 beats".into());
                    return;
                }
                let start = selected
                    .iter()
                    .map(|n| f64::from(n.start))
                    .fold(f64::INFINITY, f64::min);
                let end = selected
                    .iter()
                    .map(|n| f64::from(n.start) + f64::from(n.length))
                    .fold(0.0, f64::max);
                self.piano_roll_state.repeat_range = PianoTimeRange::new(start, end);
            }
        }
    }

    pub(super) fn piano_range_toolbar(&mut self, ui: &mut egui::Ui, menu_command: bool) {
        let mut request = None;
        ui.horizontal_wrapped(|ui| {
            let text = match self.piano_roll_state.repeat_range {
                Some(range) => format!("REPEAT RANGE  {:.3}–{:.3} · {:.3} beats", range.start, range.end, range.end - range.start),
                None => "REPEAT RANGE  none".into(),
            };
            let (rect, response) = ui.allocate_exact_size(Vec2::new(302.0, 18.0), Sense::hover());
            ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, &text, FontId::proportional(9.0), theme::ORANGE);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), &text));
            response.on_hover_text("Edit/repeat range only; playback is unchanged. Ctrl/Cmd-drag or double-click-and-drag the ruler to set a range and select note starts in [start, end), including their groups. Ctrl+B repeats selected active-Channel notes by the range width, even outside it. With no selected active notes, Ctrl+B repeats all active-Channel notes. Ctrl+D only deselects notes.");
            if ui.small_button("Range from selection").on_hover_text("Ctrl/Cmd+Enter · Exact selected-note extent; no selection leaves range unchanged").clicked() { request = Some(RangeAction::FromSelection); }
            let has_range = self.piano_roll_state.repeat_range.is_some();
            if ui.add_enabled(has_range, egui::Button::new("Range left").small()).on_hover_text("Ctrl/Cmd+Left · Move range by its width, bounded at beat zero; notes stay put").clicked() { request = Some(RangeAction::Move(-1)); }
            if ui.add_enabled(has_range, egui::Button::new("Range right").small()).on_hover_text("Ctrl/Cmd+Right · Move range by its width, bounded at beat 4096; notes stay put").clicked() { request = Some(RangeAction::Move(1)); }
            if ui.add_enabled(has_range, egui::Button::new("Clear range").small()).on_hover_text("Remove repeat interval; preserve note selection").clicked() { request = Some(RangeAction::Clear); }
        });
        if let Some(action) = request {
            if menu_command {
                ui.close();
            }
            self.piano_menu_action(ui.ctx(), ShortcutAction::PianoRange(action));
        }
    }

    pub(super) fn piano_range_ruler(&mut self, ui: &egui::Ui, ruler: Rect, grid: Rect) {
        let ctx = ui.ctx();
        let response = ui.interact(ruler, Id::new("piano-range-ruler"), Sense::click_and_drag())
            .on_hover_text("Ctrl/Cmd-drag or double-click-and-drag: select edit/repeat range. Playback is unchanged. Use Clear range to remove it.");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Other,
                ui.is_enabled(),
                "Piano repeat range ruler",
            )
        });
        let channel = self
            .project
            .channels
            .get(self.selected_channel)
            .map(|c| c.id);
        let axis = [
            self.piano_viewport.x.origin(),
            self.piano_viewport.x.pixels_per_unit(),
        ];
        let enabled = ui.is_enabled()
            && !ui.is_sizing_pass()
            && ctx.input(|i| i.focused)
            && self.workspace.focused == StudioView::PianoRoll
            && !self.piano_pointer_blocked()
            && !ctx.any_popup_open()
            && self.top_shortcut_modal().is_none()
            && !self.project_snapshot_transition_pending()
            && !self.project_lifecycle_barriers_active();
        let existing = ctx.data(|data| data.get_temp::<RangeGesture>(key()));
        if !enabled
            || existing.as_ref().is_some_and(|g| {
                Some(g.owner) != self.piano_roll_state.range_owner
                    || g.channel != channel
                    || g.axis != axis
                    || g.ruler != ruler
            })
        {
            self.cancel_piano_range_gesture(ctx);
        } else {
            let mut gesture = existing;
            let previous_click = ctx.data(|data| data.get_temp::<RulerClick>(click_key()));
            let previous_click = previous_click.filter(|click| {
                Some(click.owner) == self.piano_roll_state.range_owner
                    && click.ruler == ruler
                    && click.axis == axis
            });
            if previous_click.is_none() {
                ctx.data_mut(|data| data.remove::<RulerClick>(click_key()));
            }
            if ctx.input(|i| i.pointer.any_pressed()) {
                ctx.data_mut(|data| data.remove::<RulerClick>(click_key()));
            }
            if let Some((point, modifiers)) = piano_mouse::primary_press(ctx)
                && response.interact_rect.contains(point)
                && ctx.layer_id_at(point) == Some(response.layer_id)
                && (piano_mouse::owns_primary_press(&response)
                    || ctx.input(|i| i.pointer.primary_pressed() && i.pointer.primary_released()))
                && let Some(owner) = self.piano_roll_state.range_owner
            {
                let double_press = modifiers == egui::Modifiers::NONE
                    && previous_click.is_some_and(|click| {
                        let options = ctx.options(|o| o.input_options);
                        click.owner == owner
                            && click.ruler == ruler
                            && click.axis == axis
                            && point.distance(click.point) <= options.max_click_dist
                            && ctx.input(|i| i.time - click.time) >= 0.0
                            && ctx.input(|i| i.time - click.time) <= options.max_double_click_delay
                    });
                let snap = if modifiers.alt {
                    PianoSnap::Off
                } else {
                    self.piano_roll_state.local_snap
                };
                let start = crate::piano_snap::quantize_nearest(
                    self.piano_viewport
                        .x
                        .content_at_pixel(f64::from(point.x - ruler.left())),
                    snap,
                )
                .clamp(0.0, MAX_PIANO_BEAT);
                gesture = Some(RangeGesture {
                    owner,
                    channel,
                    ruler,
                    axis,
                    pointer: point,
                    start,
                    snap,
                    before_range: self.piano_roll_state.repeat_range,
                    before_selection: self.piano_roll_state.selection_ids.clone(),
                    additive: modifiers.shift,
                    armed: modifiers.ctrl || modifiers.command || double_press,
                    moved: false,
                    modifiers,
                });
            }
            let (down, released, pointer) = ctx.input(|i| {
                (
                    i.pointer.primary_down(),
                    i.pointer.primary_released(),
                    i.pointer.interact_pos(),
                )
            });
            if let (Some(g), Some(pointer)) = (gesture.as_mut(), pointer) {
                g.moved |= (pointer.x - g.pointer.x).abs()
                    > ctx.options(|o| o.input_options.max_click_dist);
            }
            if let (Some(g), Some(pointer)) = (&gesture, pointer)
                && g.armed
                && g.moved
                && (down || released)
                && pointer.x.is_finite()
            {
                let end = crate::piano_snap::quantize_nearest(
                    self.piano_viewport
                        .x
                        .content_at_pixel(f64::from(pointer.x - ruler.left())),
                    g.snap,
                )
                .clamp(0.0, MAX_PIANO_BEAT);
                if let Some(range) = PianoTimeRange::new(g.start.min(end), g.start.max(end)) {
                    self.piano_roll_state.repeat_range = Some(range);
                    let notes = &self.project.active_pattern().notes;
                    let mut hits: HashSet<_> = notes
                        .iter()
                        .filter(|n| {
                            n.channel_id == channel
                                && n.start.is_finite()
                                && n.start >= range.start as f32
                                && n.start < range.end as f32
                        })
                        .map(|n| n.id)
                        .collect();
                    if !hits.is_empty()
                        && let Some(channel) = channel
                    {
                        hits = piano_keyboard_targets(
                            notes,
                            &hits,
                            channel,
                            self.piano_roll_state.grouping_enabled,
                        );
                    }
                    if g.additive {
                        hits.extend(&g.before_selection);
                    }
                    self.piano_roll_state.selection_ids = hits;
                } else {
                    self.piano_roll_state.repeat_range = g.before_range;
                    self.piano_roll_state.selection_ids = g.before_selection.clone();
                }
            }
            if released
                && response.clicked_by(egui::PointerButton::Primary)
                && let Some(g) = &gesture
                && g.modifiers == egui::Modifiers::NONE
                && let Some(point) = pointer
            {
                let time = ctx.input(|i| i.time);
                ctx.data_mut(|data| {
                    data.insert_temp(
                        click_key(),
                        RulerClick {
                            owner: g.owner,
                            ruler,
                            axis,
                            point,
                            time,
                        },
                    )
                });
            }
            ctx.data_mut(|data| {
                data.remove::<RangeGesture>(key());
                if down && let Some(gesture) = gesture {
                    data.insert_temp(key(), gesture);
                }
            });
        }
        if let Some(range) = self.piano_roll_state.repeat_range {
            let x1 = ruler.left() + self.piano_viewport.x.pixel_for_content(range.start) as f32;
            let x2 = ruler.left() + self.piano_viewport.x.pixel_for_content(range.end) as f32;
            let painter = ui.painter_at(ruler.intersect(ui.clip_rect()));
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x1, ruler.top()), Pos2::new(x2, ruler.bottom())),
                0.0,
                theme::ORANGE.gamma_multiply(0.28),
            );
            let painter = ui.painter_at(grid.intersect(ui.clip_rect()));
            for x in [x1, x2] {
                painter.line_segment(
                    [Pos2::new(x, grid.top()), Pos2::new(x, grid.bottom())],
                    Stroke::new(1.0, theme::ORANGE.gamma_multiply(0.65)),
                );
            }
        }
    }
}
