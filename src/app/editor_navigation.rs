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
                        let active_channel = self
                            .project
                            .channels
                            .get(self.selected_channel)
                            .map(|c| c.id);
                        fit_piano_notes(
                            &mut self.piano_viewport,
                            &self.project.active_pattern().notes,
                            active_channel,
                        );
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

/// Fit only the editable channel, never ghost notes. Keeping this operation's
/// mutable input limited to the viewport also keeps it out of project history.
fn fit_piano_notes(viewport: &mut Viewport2D, notes: &[PianoNote], channel: Option<u32>) {
    let targets: Vec<_> = notes
        .iter()
        .filter(|note| note.channel_id == channel)
        .collect();
    let bounds = targets.iter().fold(None, |bounds, note| {
        let start = f64::from(note.start);
        let end = start + f64::from(note.length);
        Some(match bounds {
            Some((first, last)) => (f64::min(first, start), f64::max(last, end)),
            None => (start, end),
        })
    });
    // Empty channels retain the useful first-bar view. A populated channel
    // frames its phrase even when it begins many bars into the pattern.
    let (start, end) = bounds.unwrap_or((0.0, 4.0));
    fit_axis(&mut viewport.x, start, end, 12.0);
    if let (Some(high), Some(low)) = (
        targets.iter().map(|note| note.note).max(),
        targets.iter().map(|note| note.note).min(),
    ) {
        fit_axis(
            &mut viewport.y,
            piano_pitch_row(high) - 1.0,
            piano_pitch_row(low) + 2.0,
            8.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(channel: Option<u32>, start: f32, length: f32, pitch: u8) -> PianoNote {
        PianoNote {
            id: 1,
            channel_id: channel,
            group_id: None,
            note: pitch,
            start,
            length,
            velocity: 0.8,
            selected: false,
            muted: false,
        }
    }

    fn viewport() -> Viewport2D {
        let mut viewport = default_piano_viewport();
        viewport.x.set_content_range(0.0, 8192.0).unwrap();
        viewport.x.set_viewport_pixels(720.0).unwrap();
        viewport
    }

    fn assert_visible(axis: AxisViewport, start: f64, end: f64) {
        let (left, right) = axis.visible_range();
        assert!(left <= start + 1.0e-9, "{left} > {start}");
        assert!(right >= end - 1.0e-9, "{right} < {end}");
    }

    #[test]
    fn fit_notes_frames_late_phrase_instead_of_leading_silence() {
        let mut viewport = viewport();
        let notes = [note(Some(7), 128.0, 1.0, 60), note(Some(7), 131.0, 1.0, 67)];
        fit_piano_notes(&mut viewport, &notes, Some(7));
        assert_visible(viewport.x, 128.0, 132.0);
        assert!((viewport.x.pixels_per_unit() - 174.0).abs() < 1.0e-9);
        assert!((viewport.x.pixel_for_content(128.0) - 12.0).abs() < 1.0e-9);
        assert!((viewport.x.pixel_for_content(132.0) - 708.0).abs() < 1.0e-9);
        assert_visible(viewport.y, piano_pitch_row(67), piano_pitch_row(60) + 1.0);
        let fitted = viewport;
        fit_piano_notes(&mut viewport, &notes, Some(7));
        assert_eq!(viewport, fitted, "repeated fitting should be stable");
    }

    #[test]
    fn fit_notes_empty_pattern_returns_to_first_bar_without_changing_pitch() {
        let mut viewport = viewport();
        viewport.x.set_normalized_scroll(1.0).unwrap();
        let pitch_before = viewport.y;
        fit_piano_notes(&mut viewport, &[], Some(7));
        assert_visible(viewport.x, 0.0, 4.0);
        assert_eq!(viewport.x.origin(), 0.0);
        assert_eq!(viewport.y, pitch_before);
    }

    #[test]
    fn fit_notes_single_short_note_respects_maximum_zoom() {
        let mut viewport = viewport();
        fit_piano_notes(&mut viewport, &[note(Some(7), 128.0, 0.25, 60)], Some(7));
        assert_visible(viewport.x, 128.0, 128.25);
        assert_eq!(viewport.x.pixels_per_unit(), 512.0);
        assert_eq!(viewport.y.pixels_per_unit(), 96.0);
    }

    #[test]
    fn fit_notes_long_note_includes_its_full_tail() {
        let mut viewport = viewport();
        fit_piano_notes(&mut viewport, &[note(Some(7), 128.0, 256.0, 60)], Some(7));
        assert_visible(viewport.x, 128.0, 384.0);
        assert!((viewport.x.pixels_per_unit() - 696.0 / 256.0).abs() < 1.0e-9);
    }

    #[test]
    fn fit_notes_oversized_phrase_respects_minimum_zoom_and_reveals_start() {
        let mut viewport = viewport();
        fit_piano_notes(&mut viewport, &[note(Some(7), 128.0, 4096.0, 60)], Some(7));
        assert_eq!(viewport.x.pixels_per_unit(), 1.0);
        assert!((viewport.x.pixel_for_content(128.0) - 12.0).abs() < 1.0e-9);
    }

    #[test]
    fn fit_notes_ignores_other_channels_and_unassigned_ghost_notes() {
        let notes = [
            note(Some(7), 128.0, 4.0, 60),
            note(Some(8), 0.0, 4096.0, 127),
            note(None, 4096.0, 8.0, 0),
        ];
        let mut actual = viewport();
        let mut expected = actual;
        fit_piano_notes(&mut expected, &notes[..1], Some(7));
        fit_piano_notes(&mut actual, &notes, Some(7));
        assert_eq!(actual, expected);
    }

    #[test]
    fn fit_notes_empty_selected_channel_does_not_fit_ghosts() {
        let mut actual = viewport();
        let mut expected = actual;
        fit_piano_notes(&mut expected, &[], Some(7));
        fit_piano_notes(&mut actual, &[note(Some(8), 128.0, 4.0, 127)], Some(7));
        assert_eq!(actual, expected);
    }

    #[test]
    fn fit_notes_unassigned_target_preserves_existing_channel_matching() {
        let notes = [note(None, 128.0, 4.0, 60), note(Some(8), 0.0, 4096.0, 127)];
        let mut actual = viewport();
        let mut expected = actual;
        fit_piano_notes(&mut expected, &notes[..1], None);
        fit_piano_notes(&mut actual, &notes, None);
        assert_eq!(actual, expected);
        assert_visible(actual.x, 128.0, 132.0);
    }

    #[test]
    fn fit_notes_at_time_and_pitch_edges_stays_in_content_bounds() {
        let mut viewport = viewport();
        let notes = [note(Some(7), 0.0, 1.0, 0), note(Some(7), 3.0, 1.0, 127)];
        fit_piano_notes(&mut viewport, &notes, Some(7));
        assert_eq!(viewport.x.origin(), 0.0);
        assert_visible(viewport.x, 0.0, 4.0);
        assert_eq!(viewport.y.origin(), 0.0);
        assert_eq!(viewport.y.pixels_per_unit(), 8.0);
    }
}
