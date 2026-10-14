//! Editor view navigation never edits musical content or creates project history.
use super::*;

#[derive(Clone, Copy)]
struct ZoomGesture {
    owner: Id,
    vertical: bool,
    last_frame: u64,
}

fn zoom_gesture_key() -> Id {
    Id::new("editor-wheel-zoom-gesture")
}

/// Called before any widget consumes raw events. A fresh wheel belongs to no
/// editor until an eligible canvas claims it, including Ctrl+Alt expression
/// events that egui has already converted into a private zoom delta. New edits
/// terminate old view inertia before shortcuts are dispatched after UI drawing.
pub(super) fn begin_frame(ctx: &egui::Context) {
    let frame = ctx.cumulative_frame_nr();
    let invalidate = ctx.input(|input| {
        !input.focused
            || input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::MouseWheel { .. }
                        | egui::Event::Zoom(_)
                        | egui::Event::WindowFocused(false)
                        | egui::Event::Key { pressed: true, .. }
                        | egui::Event::PointerButton { .. }
                        | egui::Event::Text(_)
                )
            })
    });
    let key = zoom_gesture_key();
    ctx.data_mut(|data| {
        if invalidate
            || data
                .get_temp::<ZoomGesture>(key)
                .is_some_and(|gesture| frame.saturating_sub(gesture.last_frame) > 1)
        {
            data.remove::<ZoomGesture>(key);
        }
    });
}

pub(super) fn wheel(ui: &egui::Ui, viewport: &mut Viewport2D, canvas: Rect, owner: Id) {
    let (scroll, factor, modifiers, new_zoom) = ui.input(|input| {
        let new_zoom = input.events.iter().rev().find_map(|event| match event {
            egui::Event::MouseWheel { modifiers, .. } if modifiers.ctrl || modifiers.command => {
                Some(modifiers.shift)
            }
            egui::Event::Zoom(_) => Some(false),
            _ => None,
        });
        (
            input.smooth_scroll_delta,
            input.zoom_delta(),
            input.modifiers,
            new_zoom,
        )
    });
    let key = zoom_gesture_key();
    let frame = ui.ctx().cumulative_frame_nr();
    if let Some(vertical) = new_zoom {
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                key,
                ZoomGesture {
                    owner,
                    vertical,
                    last_frame: frame,
                },
            );
        });
    }
    if factor.is_finite() && factor > 0.0 && (factor - 1.0).abs() > f32::EPSILON {
        // egui 0.35 converts Ctrl-wheel to zoom_delta and clears smooth_scroll_delta.
        // Keep the axis on inertial frames after Ctrl/Shift is released.
        if let Some(mut gesture) = ui.ctx().data(|data| data.get_temp::<ZoomGesture>(key))
            && gesture.owner == owner
        {
            let pointer = ui.ctx().pointer_hover_pos().unwrap_or(canvas.center());
            let (axis, anchor) = if gesture.vertical {
                (
                    &mut viewport.y,
                    (pointer.y - canvas.top()).clamp(0.0, canvas.height()),
                )
            } else {
                (
                    &mut viewport.x,
                    (pointer.x - canvas.left()).clamp(0.0, canvas.width()),
                )
            };
            let _ = axis.zoom_at_pixel(f64::from(anchor), f64::from(factor));
            gesture.last_frame = frame;
            ui.ctx().data_mut(|data| data.insert_temp(key, gesture));
            return;
        }
    }
    // An unclaimed private zoom tail must not swallow a fresh ordinary scroll.
    // Expression-owned events already clear their exposed scroll delta.
    apply_editor_wheel(
        viewport,
        canvas,
        ui.ctx().pointer_hover_pos(),
        scroll,
        modifiers,
    );
}

impl CitrusApp {
    pub(super) fn piano_view_controls(&mut self, ui: &mut egui::Ui) {
        let mut horizontal = self.piano_viewport.x.pixels_per_unit() as f32;
        let mut vertical = self.piano_viewport.y.pixels_per_unit() as f32;
        egui::Frame::NONE
            .fill(theme::BG)
            .inner_margin(egui::Margin::symmetric(4, 1))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("VIEW").size(8.0).color(theme::MUTED));
                    ui.label(RichText::new("H").size(9.0).color(theme::MUTED));
                    if ui
                        .add_sized(
                            [90.0, 24.0],
                            egui::Slider::new(&mut horizontal, 1.0..=512.0)
                                .logarithmic(true)
                                .show_value(false),
                        )
                        .on_hover_text("Time zoom — Ctrl + mouse wheel")
                        .changed()
                    {
                        let anchor = self.piano_viewport.x.viewport_pixels() * 0.5;
                        let factor =
                            f64::from(horizontal) / self.piano_viewport.x.pixels_per_unit();
                        let _ = self.piano_viewport.x.zoom_at_pixel(anchor, factor);
                    }
                    ui.label(RichText::new("V").size(9.0).color(theme::MUTED));
                    if ui
                        .add_sized(
                            [90.0, 24.0],
                            egui::Slider::new(&mut vertical, 8.0..=96.0)
                                .logarithmic(true)
                                .show_value(false),
                        )
                        .on_hover_text("Pitch zoom — Ctrl + Shift + mouse wheel")
                        .changed()
                    {
                        let anchor = self.piano_viewport.y.viewport_pixels() * 0.5;
                        let factor = f64::from(vertical) / self.piano_viewport.y.pixels_per_unit();
                        let _ = self.piano_viewport.y.zoom_at_pixel(anchor, factor);
                    }
                    if ui
                        .studio_button(RichText::new("FIT NOTES").size(8.0))
                        .clicked()
                    {
                        let notes = &self.project.active_pattern().notes;
                        let active_channel = self
                            .project
                            .channels
                            .get(self.selected_channel)
                            .map(|c| c.id);
                        let targets: Vec<_> = notes
                            .iter()
                            .filter(|n| n.channel_id == active_channel)
                            .collect();
                        let end = targets
                            .iter()
                            .map(|n| f64::from(n.start + n.length))
                            .fold(4.0, f64::max);
                        fit_axis(&mut self.piano_viewport.x, 0.0, end, 12.0);
                        if let (Some(high), Some(low)) = (
                            targets.iter().map(|n| n.note).max(),
                            targets.iter().map(|n| n.note).min(),
                        ) {
                            fit_axis(
                                &mut self.piano_viewport.y,
                                piano_pitch_row(high) - 1.0,
                                piano_pitch_row(low) + 2.0,
                                8.0,
                            );
                        }
                    }
                });
            });
    }

    pub(super) fn seek_playlist_pointer(&mut self, pointer: Pos2, canvas: Rect) {
        let beat = (self
            .playlist_viewport
            .x
            .content_at_pixel(f64::from(pointer.x - canvas.left())) as f32
            / self.snap)
            .floor()
            * self.snap;
        if !beat.is_finite() {
            return;
        }
        let loop_end = self.transport_loop_end_beat();
        let transport_beat = if self.transport_mode == TransportMode::Pattern {
            beat.max(0.0).rem_euclid(loop_end)
        } else {
            beat.clamp(0.0, loop_end)
        };
        self.beat_position = transport_beat;
        self.last_step = usize::MAX;
        self.pending_generator_note_offs.clear();
        self.active_audio_clips.clear();
        self.publish_transport_seek(f64::from(transport_beat));
    }
}

fn fit_axis(axis: &mut AxisViewport, start: f64, end: f64, margin: f64) {
    let pixels = (axis.viewport_pixels() - 2.0 * margin).max(1.0);
    let factor = pixels / (end - start).max(1.0) / axis.pixels_per_unit();
    let _ = axis.zoom_at_pixel(0.0, factor);
    let _ = axis.reveal(start, end, margin);
}
