//! Piano key geometry stays tied to chromatic grid rows, while the white key bodies
//! join behind the shorter, raised black keys. Pointer state is physical UI state;
//! the caller supplies actual transport activity separately.

use std::sync::Arc;

use crate::{
    editor_viewport::AxisViewport,
    theme,
    timeline::{CompiledTimeline, TimelineEventKind},
    timeline_executor::MAX_ACTIVE_NOTES,
};
use eframe::egui::{
    self, Align2, Color32, FontId, Id, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Vec2,
};

const MIDI_PITCHES: usize = 128;
const BLACK_KEY_WIDTH: f32 = 0.60;
const BLACK_KEY_HEIGHT: f32 = 0.72;

/// Validate the accepted activation against the callback identity before
/// reconstructing any key activity. Loop counters survive a new seek, so a
/// nonzero counter alone cannot select the loop start as the note boundary.
#[derive(Clone, Copy, Debug)]
pub(super) struct KeyboardPlaybackAdmission {
    /// project session, sync session, generation, seek serial, activation epoch
    pub(super) activation: Option<(u64, u64, u64, u64, u64)>,
    /// current project session, sync session, generation, seek serial
    pub(super) current: (u64, u64, u64, u64),
    pub(super) confirmed_timeline: Option<(u64, u64)>,
    pub(super) transport_epoch: u64,
    pub(super) loop_count: u64,
    pub(super) seek_frame: u64,
    pub(super) loop_start_frame: u64,
}

impl KeyboardPlaybackAdmission {
    pub(super) fn discontinuity_frame(self) -> Option<u64> {
        let (project, sync, generation, seek, activation_epoch) = self.activation?;
        if self.current != (project, sync, generation, seek)
            || self.confirmed_timeline != Some((generation, self.transport_epoch))
            || self.transport_epoch == 0
            || activation_epoch == 0
        {
            return None;
        }
        if self.transport_epoch == activation_epoch {
            Some(self.seek_frame)
        } else if self.loop_count > 0
            && super::timeline_epoch_has_reached(self.transport_epoch, activation_epoch)
        {
            Some(self.loop_start_frame)
        } else {
            None
        }
    }
}

/// An exact callback position, distinct from the UI's estimated musical beat.
/// On a discontinuity, the caller supplies its real chase boundary. With no
/// confirmed boundary, old Note Ons are conservatively excluded.
#[derive(Clone, Copy)]
pub(super) struct KeyboardPlaybackPosition {
    pub(super) channel_id: u32,
    pub(super) transport_epoch: u64,
    pub(super) seek_serial: u64,
    pub(super) loop_count: u64,
    pub(super) frame_exclusive: u64,
    pub(super) discontinuity_frame: Option<u64>,
}

pub(super) struct KeyboardPlaybackCache {
    timeline: Option<Arc<CompiledTimeline>>,
    identity: Option<(u32, u64, u64, u64)>,
    frame_exclusive: u64,
    cursor: usize,
    active: Vec<(u64, u8)>,
    pitches: [bool; MIDI_PITCHES],
}

impl Default for KeyboardPlaybackCache {
    fn default() -> Self {
        Self {
            timeline: None,
            identity: None,
            frame_exclusive: 0,
            cursor: 0,
            active: Vec::with_capacity(MAX_ACTIVE_NOTES),
            pitches: [false; MIDI_PITCHES],
        }
    }
}

impl KeyboardPlaybackCache {
    pub(super) fn clear(&mut self) {
        self.timeline = None;
        self.identity = None;
        self.cursor = 0;
        self.active.clear();
        self.pitches.fill(false);
    }

    /// Consume each scheduled event once in normal forward playback. No
    /// automation chase, full-project clone, or per-frame event-vector build.
    /// The flags describe scheduler activity, not a claim of audible samples.
    pub(super) fn update(
        &mut self,
        timeline: &Arc<CompiledTimeline>,
        position: KeyboardPlaybackPosition,
    ) -> &[bool; MIDI_PITCHES] {
        let identity = (
            position.channel_id,
            position.transport_epoch,
            position.seek_serial,
            position.loop_count,
        );
        let source_changed = self
            .timeline
            .as_ref()
            .is_none_or(|old| !Arc::ptr_eq(old, timeline));
        let reset = source_changed
            || self.identity != Some(identity)
            || position.frame_exclusive < self.frame_exclusive;
        let events = timeline.events();
        if reset {
            self.active.clear();
            self.pitches.fill(false);
            // DoNotChase is the current audio runtime's long-note policy. A
            // seek may restore automation while leaving its old notes silent.
            let boundary = position
                .discontinuity_frame
                .unwrap_or(position.frame_exclusive);
            self.cursor = events.partition_point(|event| event.frame < boundary);
            self.timeline = Some(Arc::clone(timeline));
            self.identity = Some(identity);
        }
        while let Some(event) = events
            .get(self.cursor)
            .filter(|event| event.frame < position.frame_exclusive)
        {
            match event.kind {
                TimelineEventKind::NoteOn {
                    note_id,
                    channel_id,
                    note,
                    ..
                } if channel_id == position.channel_id && note < 128 => {
                    if let Some(active) = self.active.iter_mut().find(|active| active.0 == note_id)
                    {
                        active.1 = note;
                    } else if self.active.len() < MAX_ACTIVE_NOTES {
                        self.active.push((note_id, note));
                    }
                }
                TimelineEventKind::NoteOff {
                    note_id,
                    channel_id,
                    ..
                } if channel_id == position.channel_id => {
                    self.active.retain(|active| active.0 != note_id);
                }
                _ => {}
            }
            self.cursor += 1;
        }
        self.frame_exclusive = position.frame_exclusive;
        self.pitches.fill(false);
        for &(_, pitch) in &self.active {
            self.pitches[usize::from(pitch)] = true;
        }
        &self.pitches
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PianoKeyboardGeometry {
    pub(super) rect: Rect,
    pub(super) pitch_axis: AxisViewport,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PianoKeyGeometry {
    pub(super) pitch: u8,
    pub(super) black: bool,
    /// The exact chromatic row, also used by the note grid.
    pub(super) pitch_row: Rect,
    /// White keys span to adjacent white-key boundaries. Black keys occupy less
    /// than one chromatic row and never extend to the grid edge.
    pub(super) face: Rect,
}

fn is_black(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

impl PianoKeyboardGeometry {
    pub(super) fn key(self, pitch: u8) -> PianoKeyGeometry {
        let height = self.pitch_axis.pixels_per_unit() as f32;
        let row = f64::from(127_u8.saturating_sub(pitch));
        let y = self.rect.top() + self.pitch_axis.pixel_for_content(row) as f32;
        let pitch_row = Rect::from_min_size(
            Pos2::new(self.rect.left(), y),
            Vec2::new(self.rect.width(), height),
        );
        let black = is_black(pitch);
        let face = if black {
            Rect::from_min_size(
                Pos2::new(
                    self.rect.left(),
                    y + height * (1.0 - BLACK_KEY_HEIGHT) / 2.0,
                ),
                Vec2::new(
                    self.rect.width() * BLACK_KEY_WIDTH,
                    height * BLACK_KEY_HEIGHT,
                ),
            )
        } else {
            let above = pitch < 127 && is_black(pitch + 1);
            let below = pitch > 0 && is_black(pitch - 1);
            Rect::from_min_max(
                Pos2::new(self.rect.left(), y - if above { height * 0.5 } else { 0.0 }),
                Pos2::new(
                    self.rect.right(),
                    y + height + if below { height * 0.5 } else { 0.0 },
                ),
            )
        };
        PianoKeyGeometry {
            pitch,
            black,
            pitch_row,
            face,
        }
    }

    pub(super) fn visible_keys(self) -> impl Iterator<Item = PianoKeyGeometry> {
        (0..MIDI_PITCHES)
            .map(move |pitch| self.key(pitch as u8))
            .filter(move |key| key.face.intersects(self.rect))
    }

    /// The black face wins over its white-key underlay. Beyond the end of a
    /// black key the neighboring white key is a real playable hit region.
    pub(super) fn pitch_at(self, point: Pos2) -> Option<u8> {
        if !self.rect.contains(point) || !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        self.visible_keys()
            .find(|key| key.black && key.face.contains(point))
            .or_else(|| {
                self.visible_keys()
                    .find(|key| !key.black && key.face.contains(point))
            })
            .map(|key| key.pitch)
    }
}

pub(super) struct PianoKeyboardResponse {
    pub(super) response: Response,
    /// A primary pointer held on this keyboard, with no click/drag threshold.
    /// This is not evidence that an audio callback accepted a MIDI Note On.
    pub(super) pressed_pitch: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
struct KeyState {
    hovered: bool,
    pressed: bool,
    playing: bool,
}

fn face_color(black: bool, state: KeyState) -> Color32 {
    let rest = if black {
        Color32::from_rgb(25, 30, 37)
    } else {
        Color32::from_rgb(205, 210, 213)
    };
    if state.pressed {
        if black {
            Color32::from_rgb(177, 113, 51)
        } else {
            Color32::from_rgb(247, 181, 107)
        }
    } else if state.playing {
        if black {
            Color32::from_rgb(45, 83, 117)
        } else {
            Color32::from_rgb(138, 180, 212)
        }
    } else if state.hovered {
        theme::mix(rest, theme::BLUE, if black { 0.20 } else { 0.10 })
    } else {
        rest
    }
}

/// Draw and interact once with the whole keyboard. This avoids overlapping
/// per-key widgets stealing the black key's hit region or lighting a key on
/// hover as if it were depressed. `playing` must come from transport/timeline
/// state; this widget adds no note timers or fake musical activity.
pub(super) fn show(
    ui: &mut egui::Ui,
    geometry: PianoKeyboardGeometry,
    enabled: bool,
    playing: &[bool; MIDI_PITCHES],
    scale_root: Option<u8>,
) -> PianoKeyboardResponse {
    let response = ui.interact(
        geometry.rect,
        Id::new("piano-keyboard"),
        Sense::click_and_drag(),
    );
    let (pointer, primary_down, focused) = ui.input(|input| {
        (
            input.pointer.interact_pos(),
            input.pointer.primary_down(),
            input.focused,
        )
    });
    let enabled = enabled && ui.is_enabled() && focused && !ui.is_sizing_pass();
    let cancelled_key = Id::new("piano-keyboard-cancelled-pointer");
    let cancelled = ui.ctx().data_mut(|data| {
        let was_cancelled = data.get_temp::<bool>(cancelled_key).unwrap_or(false);
        let cancelled = was_cancelled || (!enabled && primary_down);
        if primary_down {
            data.insert_temp(cancelled_key, cancelled);
        } else {
            data.remove::<bool>(cancelled_key);
        }
        cancelled
    });
    let hit_pitch = pointer.and_then(|point| geometry.pitch_at(point));
    let pointer_pitch = (enabled && !cancelled).then_some(hit_pitch).flatten();
    let pressed_pitch =
        (enabled && !cancelled && primary_down && response.is_pointer_button_down_on())
            .then_some(pointer_pitch)
            .flatten();
    let hovered_pitch = (enabled && response.hovered())
        .then_some(pointer_pitch)
        .flatten();
    let painter = ui.painter_at(geometry.rect);
    painter.rect_filled(geometry.rect, 0.0, theme::WELL);
    let visible: Vec<_> = geometry.visible_keys().collect();
    for black in [false, true] {
        for key in visible.iter().filter(|key| key.black == black) {
            paint_key(
                &painter,
                *key,
                KeyState {
                    hovered: hovered_pitch == Some(key.pitch),
                    pressed: pressed_pitch == Some(key.pitch),
                    playing: playing[usize::from(key.pitch)],
                },
                scale_root,
            );
        }
    }
    painter.line_segment(
        [geometry.rect.right_top(), geometry.rect.right_bottom()],
        Stroke::new(1.0, theme::WELL),
    );
    PianoKeyboardResponse {
        response,
        pressed_pitch,
    }
}

fn paint_key(
    painter: &egui::Painter,
    key: PianoKeyGeometry,
    state: KeyState,
    scale_root: Option<u8>,
) {
    let height = key.pitch_row.height();
    let fill = face_color(key.black, state);
    let depressed = state.pressed || state.playing;
    let rounding = if key.black { 1.5 } else { 0.0 };
    if key.black && !depressed {
        painter.rect_filled(
            key.face.translate(Vec2::splat(1.4)),
            rounding,
            Color32::from_rgb(12, 16, 21),
        );
    }
    painter.rect_filled(key.face, rounding, fill);
    painter.rect_stroke(
        key.face,
        rounding,
        Stroke::new(
            if key.black { 0.8 } else { 0.65 },
            Color32::from_rgb(52, 61, 69),
        ),
        StrokeKind::Inside,
    );
    let top_color = if depressed {
        theme::mix(fill, Color32::BLACK, 0.42)
    } else {
        theme::mix(fill, Color32::WHITE, if key.black { 0.16 } else { 0.32 })
    };
    let bottom_color = if depressed {
        theme::mix(fill, Color32::WHITE, 0.13)
    } else {
        theme::mix(fill, Color32::BLACK, if key.black { 0.38 } else { 0.17 })
    };
    if key.face.height() >= 5.0 {
        painter.line_segment(
            [
                key.face.left_top() + Vec2::splat(1.0),
                key.face.right_top() + Vec2::new(-1.0, 1.0),
            ],
            Stroke::new(0.8, top_color),
        );
        painter.line_segment(
            [
                key.face.left_bottom() + Vec2::new(1.0, -1.0),
                key.face.right_bottom() - Vec2::splat(1.0),
            ],
            Stroke::new(0.8, bottom_color),
        );
    }
    let octave = key.pitch.is_multiple_of(12);
    let root = scale_root.is_some_and(|root| key.pitch % 12 == root % 12);
    if state.playing || root {
        let marker = Rect::from_min_max(
            Pos2::new(
                key.face.right() - 2.5,
                key.pitch_row.center().y - height * 0.25,
            ),
            Pos2::new(
                key.face.right() - 1.0,
                key.pitch_row.center().y + height * 0.25,
            ),
        );
        painter.rect_filled(
            marker,
            0.0,
            if state.playing {
                theme::BLUE
            } else {
                theme::ORANGE_DIM
            },
        );
    }
    if octave || root {
        let pitch_name = super::pitch_class_label(key.pitch);
        painter.text(
            Pos2::new(
                key.face.right() - 6.0,
                key.pitch_row.center().y + if depressed { 0.75 } else { 0.0 },
            ),
            Align2::RIGHT_CENTER,
            format!("{pitch_name}{}", i32::from(key.pitch) / 12 - 1),
            FontId::monospace((height * 0.55).clamp(7.0, 10.0)),
            if key.black {
                Color32::from_rgb(197, 208, 218)
            } else {
                Color32::from_rgb(55, 65, 74)
            },
        );
    }
}

#[cfg(test)]
#[path = "piano_keyboard_geometry_tests.rs"]
mod tests;
