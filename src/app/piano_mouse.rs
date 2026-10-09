//! Pointer-down modifier ownership for FL-style composition. egui handles hit testing;
//! this state survives moving outside the original note, and is cleared on interruption.
use super::*;

const MAX_BEAT: f32 = 4096.0;

#[derive(Clone, Copy, Debug)]
pub(super) enum PressTarget {
    Canvas,
    Note(u64),
}

#[derive(Clone, Debug)]
enum GestureKind {
    Selection,
    Marquee {
        before: HashSet<u64>,
    },
    Move {
        anchor: u64,
        origins: Vec<PianoNote>,
        clones: Option<Vec<PianoNote>>,
    },
    Draw {
        origin: PianoNote,
        stretch: bool,
    },
}

#[derive(Clone, Debug)]
struct Gesture {
    pointer: Pos2,
    modifiers: egui::Modifiers,
    grid: Rect,
    coordinates: [f64; 4],
    session: u64,
    pattern: usize,
    channel: Option<u32>,
    tool: PianoRollTool,
    moved: bool,
    kind: GestureKind,
}

fn key() -> Id {
    Id::new("piano-mouse-gesture")
}

pub(super) fn clear(data: &mut egui::util::IdTypeMap) {
    data.remove::<Gesture>(key());
}

pub(super) fn primary_press(ctx: &egui::Context) -> Option<(Pos2, egui::Modifiers)> {
    ctx.input(|input| {
        input.events.iter().rev().find_map(|event| match event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            } if pos.x.is_finite() && pos.y.is_finite() => Some((*pos, *modifiers)),
            _ => None,
        })
    })
}

pub(super) fn owns_primary_press(response: &egui::Response) -> bool {
    response.is_pointer_button_down_on()
        || response.clicked_by(egui::PointerButton::Primary)
        || response.dragged_by(egui::PointerButton::Primary)
        || response.drag_stopped_by(egui::PointerButton::Primary)
        // Egui may not report an ongoing drag for an entire down/move/up batch.
        // The prepass resolves exactly one winner using the same painted note geometry.
        || (response.enabled()
            && response.ctx.data(|data| data.get_temp::<Id>(Id::new("piano-batched-press-owner"))) == Some(response.id)
            && primary_press(&response.ctx).is_some_and(|(pos, _)|
                response.interact_rect.contains(pos)
                    && response.ctx.layer_id_at(pos) == Some(response.layer_id)))
}

pub(super) fn note_rects(
    grid: Rect,
    viewport: crate::editor_viewport::Viewport2D,
    note: &PianoNote,
) -> (Rect, Rect, Rect, Rect) {
    let raw = Rect::from_min_size(
        Pos2::new(
            grid.left() + viewport.x.pixel_for_content(f64::from(note.start)) as f32 + 1.0,
            grid.top() + viewport.y.pixel_for_content(piano_pitch_row(note.note)) as f32 + 1.0,
        ),
        Vec2::new(
            (note.length * viewport.x.pixels_per_unit() as f32 - 2.0).max(5.0),
            viewport.y.pixels_per_unit() as f32 - 2.0,
        ),
    );
    let rect = raw.intersect(grid);
    let resize = Rect::from_min_max(
        Pos2::new(raw.right() - raw.width().min(12.0), raw.top()),
        Pos2::new(raw.right() + 3.0, raw.bottom()),
    )
    .intersect(grid);
    let body = Rect::from_min_max(
        raw.left_top(),
        Pos2::new(resize.left().max(rect.left() + 2.0), rect.bottom()),
    )
    .intersect(grid);
    (raw, rect, body, resize)
}

pub(super) fn resolve_batched_press(
    ui: &egui::Ui,
    grid: Rect,
    viewport: crate::editor_viewport::Viewport2D,
    notes: &[PianoNote],
    channel: Option<u32>,
) {
    let ctx = ui.ctx();
    let key = Id::new("piano-batched-press-owner");
    ctx.data_mut(|data| data.remove::<Id>(key));
    if !ui.is_enabled()
        || ui.is_sizing_pass()
        || !ctx.input(|i| i.pointer.primary_pressed() && i.pointer.primary_released())
    {
        return;
    }
    let Some((point, _)) = primary_press(ctx) else {
        return;
    };
    if !grid.intersect(ui.clip_rect()).contains(point)
        || ctx.layer_id_at(point) != Some(ui.layer_id())
    {
        return;
    }
    // Later notes paint/interact above earlier notes; within each note, its edge is last.
    let winner = notes
        .iter()
        .rev()
        .filter(|n| n.channel_id == channel && n.note <= 127)
        .find_map(|note| {
            let (_, _, body, resize) = note_rects(grid, viewport, note);
            let id = Id::new(("piano-note", note.id));
            if resize.intersect(ui.clip_rect()).contains(point) {
                Some(id.with("resize"))
            } else if body.intersect(ui.clip_rect()).contains(point) {
                Some(id)
            } else {
                None
            }
        })
        .unwrap_or_else(|| Id::new("piano-grid"));
    ctx.data_mut(|data| data.insert_temp(key, winner));
}

pub(super) fn active(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<Gesture>(key()).is_some())
}

pub(super) fn unmodified_properties_press(ctx: &egui::Context) -> bool {
    let modifiers = primary_press(ctx)
        .map(|(_, modifiers)| modifiers)
        .or_else(|| {
            ctx.data(|data| data.get_temp::<Gesture>(key()))
                .map(|g| g.modifiers)
        })
        .or_else(|| {
            ctx.data(|data| {
                data.get_temp::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"))
            })
            .map(|g| g.modifiers)
        })
        .unwrap_or_else(|| ctx.input(|input| input.modifiers));
    modifiers == egui::Modifiers::NONE
}

pub(super) fn selecting(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<Gesture>(key()))
        .is_some_and(|g| matches!(g.kind, GestureKind::Selection | GestureKind::Marquee { .. }))
}

pub(super) fn insertion_start(raw: f64, snap: PianoSnap, bypass: bool) -> f32 {
    (if bypass {
        raw
    } else {
        crate::piano_snap::quantize_floor(raw, snap)
    })
    .clamp(
        0.0,
        f64::from(MAX_BEAT - crate::piano_roll::MIN_NOTE_LENGTH_BEATS),
    ) as f32
}

fn clone_notes(project: &Project, origins: &[PianoNote]) -> Option<Vec<PianoNote>> {
    if project
        .active_pattern()
        .notes
        .len()
        .checked_add(origins.len())?
        > crate::piano_roll::MAX_TRANSFORM_NOTES
    {
        return None;
    }
    let mut ids = piano_note_ids(project);
    let mut groups: HashSet<_> = project
        .patterns
        .iter()
        .flat_map(|p| &p.notes)
        .filter_map(|n| n.group_id)
        .collect();
    let mut remapped = HashMap::new();
    let mut counts = HashMap::new();
    for note in origins {
        if let Some(group) = note.group_id {
            *counts.entry((note.channel_id, group)).or_insert(0) += 1;
        }
    }
    origins
        .iter()
        .map(|origin| {
            let mut note = origin.clone();
            note.id = allocate_piano_note_id(&mut ids)?;
            note.selected = false;
            note.group_id = match note.group_id {
                Some(group) if counts.get(&(note.channel_id, group)).copied().unwrap_or(0) > 1 => {
                    let group_key = (note.channel_id, group);
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        remapped.entry(group_key)
                    {
                        entry.insert(allocate_piano_note_id(&mut groups)?);
                    }
                    Some(remapped[&group_key])
                }
                _ => None,
            };
            Some(note)
        })
        .collect()
}

impl CitrusApp {
    fn piano_mouse_coordinates(&self) -> [f64; 4] {
        [
            self.piano_viewport.x.origin(),
            self.piano_viewport.y.origin(),
            self.piano_viewport.x.pixels_per_unit(),
            self.piano_viewport.y.pixels_per_unit(),
        ]
    }

    fn begin_piano_mouse_history(&mut self) {
        if self.piano_roll_gesture_before.is_none() {
            self.flush_pending_editor_history();
            self.piano_roll_gesture_before = Some(self.project.clone());
        }
    }

    pub(super) fn piano_mouse_gesture(
        &mut self,
        ui: &egui::Ui,
        grid: Rect,
        press: Option<PressTarget>,
    ) {
        let ctx = ui.ctx();
        let (down, released, pointer, modifiers) = ctx.input(|i| {
            (
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
                i.modifiers,
            )
        });
        let pointer = pointer.filter(|p| p.x.is_finite() && p.y.is_finite());
        let channel = self
            .project
            .channels
            .get(self.selected_channel)
            .map(|c| c.id);
        let mut gesture = ctx.data_mut(|data| {
            let gesture = data.get_temp::<Gesture>(key());
            clear(data);
            gesture
        });
        let enabled = ui.is_enabled()
            && ctx.input(|input| input.focused)
            && !ui.is_sizing_pass()
            && self.workspace.focused == StudioView::PianoRoll
            && !self.piano_pointer_blocked()
            && !ctx.any_popup_open()
            && !self.project_snapshot_transition_pending()
            && !self.project_lifecycle_barriers_active();
        if !enabled
            || gesture.as_ref().is_some_and(|g| {
                g.grid != grid
                    || g.coordinates != self.piano_mouse_coordinates()
                    || g.session != self.project_session
                    || g.pattern != self.project.active_pattern
                    || g.channel != channel
                    || g.tool != self.piano_roll_state.tool
                    || (!matches!(g.kind, GestureKind::Selection | GestureKind::Marquee { .. })
                        && self.piano_roll_gesture_before.is_none())
            })
        {
            if gesture.is_some() {
                self.cancel_piano_mouse_pointer(ctx);
            }
            return;
        }
        if let (Some(target), Some((pointer, modifiers))) = (press, primary_press(ctx)) {
            let selection = modifiers.ctrl || modifiers.command;
            let notes = &self.project.active_pattern().notes;
            let kind = match target {
                PressTarget::Note(id) => {
                    if selection
                        || (self.piano_roll_state.tool == PianoRollTool::Select && modifiers.shift)
                    {
                        if modifiers.shift {
                            toggle_note_group_selection(
                                notes,
                                &mut self.piano_roll_state.selection_ids,
                                id,
                                self.piano_roll_state.grouping_enabled,
                            );
                        } else {
                            select_note_group_members(
                                notes,
                                &mut self.piano_roll_state.selection_ids,
                                id,
                                self.piano_roll_state.grouping_enabled,
                                false,
                            );
                        }
                        GestureKind::Selection
                    } else if matches!(
                        self.piano_roll_state.tool,
                        PianoRollTool::Draw
                            | PianoRollTool::Paint
                            | PianoRollTool::Select
                            | PianoRollTool::Stamp
                    ) {
                        ensure_note_group_selected(
                            notes,
                            &mut self.piano_roll_state.selection_ids,
                            id,
                            self.piano_roll_state.grouping_enabled,
                        );
                        // A ghost channel is context only. Hidden selections never mutate behind it.
                        self.piano_roll_state.selection_ids.retain(|id| {
                            notes.iter().any(|n| n.id == *id && n.channel_id == channel)
                        });
                        let origins: Vec<_> = notes
                            .iter()
                            .filter(|n| self.piano_roll_state.selection_ids.contains(&n.id))
                            .cloned()
                            .collect();
                        if self.piano_roll_state.tool == PianoRollTool::Stamp {
                            GestureKind::Selection
                        } else {
                            self.begin_piano_mouse_history();
                            GestureKind::Move {
                                anchor: id,
                                origins,
                                clones: None,
                            }
                        }
                    } else {
                        return;
                    }
                }
                PressTarget::Canvas
                    if selection || self.piano_roll_state.tool == PianoRollTool::Select =>
                {
                    let before = self.piano_roll_state.selection_ids.clone();
                    if !modifiers.shift {
                        self.piano_roll_state.clear_selection();
                    }
                    GestureKind::Marquee { before }
                }
                PressTarget::Canvas if self.piano_roll_state.tool == PianoRollTool::Draw => {
                    let Some(mut pitch) = piano_pitch_at_pixel(
                        self.piano_viewport.y,
                        f64::from(pointer.y - grid.top()),
                    ) else {
                        return;
                    };
                    let start = insertion_start(
                        self.piano_viewport
                            .x
                            .content_at_pixel(f64::from(pointer.x - grid.left())),
                        self.piano_roll_state.local_snap,
                        modifiers.alt,
                    );
                    if self.piano_roll_state.snap_to_scale && !modifiers.alt {
                        pitch = snap_pitch_to_scale(
                            pitch,
                            self.piano_roll_state.scale_root,
                            self.piano_roll_state.scale,
                        );
                    }
                    if notes.len() >= crate::piano_roll::MAX_TRANSFORM_NOTES
                        || notes.iter().any(|n| {
                            n.channel_id == channel
                                && n.note == pitch
                                && (n.start - start).abs() < 0.0001
                        })
                    {
                        return;
                    }
                    let Some(id) = allocate_piano_note_id(&mut piano_note_ids(&self.project))
                    else {
                        return;
                    };
                    let origin = PianoNote {
                        id,
                        channel_id: channel,
                        group_id: None,
                        note: pitch,
                        start,
                        length: self
                            .piano_roll_state
                            .last_note_length
                            .max(crate::piano_roll::MIN_NOTE_LENGTH_BEATS)
                            .min(MAX_BEAT - start),
                        velocity: 0.75,
                        selected: false,
                        muted: false,
                    };
                    self.begin_piano_mouse_history();
                    self.project.active_pattern_mut().notes.push(origin.clone());
                    self.piano_roll_state.select_only(id);
                    if let Some((channel_id, mixer_track)) = self
                        .project
                        .channels
                        .get(self.selected_channel)
                        .and_then(|c| {
                            self.project
                                .mixer_runtime_slot(c.mixer_track)
                                .map(|slot| (c.id, slot))
                        })
                    {
                        self.trigger_channel_note(
                            channel_id,
                            pitch,
                            0.75,
                            self.beat_position,
                            0.25,
                            mixer_track,
                        );
                    }
                    GestureKind::Draw {
                        origin,
                        stretch: modifiers.shift,
                    }
                }
                _ => return,
            };
            gesture = Some(Gesture {
                pointer,
                modifiers,
                grid,
                coordinates: self.piano_mouse_coordinates(),
                session: self.project_session,
                pattern: self.project.active_pattern,
                channel,
                tool: self.piano_roll_state.tool,
                moved: false,
                kind,
            });
        }
        let Some(mut gesture) = gesture else {
            return;
        };
        let Some(pointer) = pointer else {
            self.cancel_piano_mouse_pointer(ctx);
            return;
        };
        // A release can be the first frame beyond the click threshold. Retain the decision.
        let movement_threshold = ctx.options(|options| options.input_options.max_click_dist);
        gesture.moved |= pointer.distance(gesture.pointer) > movement_threshold;
        if down || released {
            match &mut gesture.kind {
                GestureKind::Selection => {}
                GestureKind::Marquee { before } => {
                    let marquee = Rect::from_two_pos(gesture.pointer, pointer).intersect(grid);
                    if gesture.moved {
                        let painter = ui.painter().with_clip_rect(grid.intersect(ui.clip_rect()));
                        painter.rect_filled(
                            marquee,
                            0.0,
                            theme::SETTINGS_ACTIVE.gamma_multiply(0.12),
                        );
                        painter.rect_stroke(
                            marquee,
                            0.0,
                            Stroke::new(1.0, theme::SETTINGS_ACTIVE),
                            StrokeKind::Inside,
                        );
                        if released {
                            let notes = &self.project.active_pattern().notes;
                            let hits: HashSet<_> = notes
                                .iter()
                                .filter(|n| n.channel_id == channel)
                                .filter(|n| {
                                    let left = grid.left()
                                        + self
                                            .piano_viewport
                                            .x
                                            .pixel_for_content(f64::from(n.start))
                                            as f32;
                                    let top = grid.top()
                                        + self
                                            .piano_viewport
                                            .y
                                            .pixel_for_content(piano_pitch_row(n.note))
                                            as f32;
                                    let rect = Rect::from_min_size(
                                        Pos2::new(left, top),
                                        Vec2::new(
                                            n.length
                                                * self.piano_viewport.x.pixels_per_unit() as f32,
                                            self.piano_viewport.y.pixels_per_unit() as f32,
                                        ),
                                    );
                                    rect.intersects(grid) && rect.intersects(marquee)
                                })
                                .map(|n| n.id)
                                .collect();
                            let hits = expand_note_group_selection(
                                notes,
                                &hits,
                                self.piano_roll_state.grouping_enabled,
                            );
                            self.piano_roll_state.selection_ids = if gesture.modifiers.shift {
                                before.union(&hits).copied().collect()
                            } else {
                                hits
                            };
                        }
                    }
                }
                GestureKind::Move {
                    anchor,
                    origins,
                    clones,
                } if gesture.moved => {
                    if let Some(anchor_note) = origins.iter().find(|n| n.id == *anchor) {
                        let delta = pointer - gesture.pointer;
                        let lock_time = modifiers.ctrl || modifiers.command;
                        let lock_pitch = modifiers.shift && !gesture.modifiers.shift;
                        let raw_start = if lock_time {
                            anchor_note.start
                        } else {
                            moved_note_start(
                                anchor_note.start,
                                delta.x / gesture.coordinates[2] as f32,
                                self.piano_roll_state.local_snap.keyboard_step(),
                                modifiers.alt || self.piano_roll_state.local_snap == PianoSnap::Off,
                            )
                            .unwrap_or(anchor_note.start)
                        };
                        let pitch_delta = if lock_pitch {
                            0
                        } else {
                            -(delta.y / gesture.coordinates[3] as f32)
                                .round()
                                .clamp(-127.0, 127.0) as i16
                        };
                        if let Ok((mut time, pitch)) = clamp_group_move_delta(
                            origins,
                            raw_start - anchor_note.start,
                            pitch_delta,
                        ) {
                            let end = origins
                                .iter()
                                .map(|n| n.start + n.length)
                                .fold(0.0_f32, f32::max);
                            time = time.min((MAX_BEAT - end).max(0.0));
                            if gesture.modifiers.shift
                                && clones.is_none()
                                && (time != 0.0 || pitch != 0)
                            {
                                if let Some(created) = clone_notes(&self.project, origins) {
                                    self.piano_roll_state.selection_ids =
                                        created.iter().map(|n| n.id).collect();
                                    self.project
                                        .active_pattern_mut()
                                        .notes
                                        .extend(created.clone());
                                    *clones = Some(created);
                                } else {
                                    self.notify("No room to clone these notes".into());
                                    self.cancel_piano_mouse_pointer(ctx);
                                    return;
                                }
                            }
                            if !gesture.modifiers.shift || clones.is_some() {
                                let targets = clones.as_ref().unwrap_or(origins);
                                let scale_snap = self.piano_roll_state.snap_to_scale
                                    && !modifiers.alt
                                    && !lock_pitch;
                                let scale_root = self.piano_roll_state.scale_root;
                                let scale = self.piano_roll_state.scale;
                                for origin in targets {
                                    if let Some(note) = self
                                        .project
                                        .active_pattern_mut()
                                        .notes
                                        .iter_mut()
                                        .find(|n| n.id == origin.id)
                                    {
                                        note.start = origin.start + time;
                                        let pitch = (i16::from(origin.note) + pitch) as u8;
                                        note.note = if scale_snap {
                                            snap_pitch_to_scale(pitch, scale_root, scale)
                                        } else {
                                            pitch
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
                GestureKind::Draw { origin, stretch } if gesture.moved => {
                    let delta = pointer - gesture.pointer;
                    let mut next = origin.clone();
                    if *stretch {
                        let raw_end = self
                            .piano_viewport
                            .x
                            .content_at_pixel(f64::from(pointer.x - grid.left()));
                        let end = if modifiers.alt {
                            raw_end
                        } else {
                            crate::piano_snap::quantize_nearest(
                                raw_end,
                                self.piano_roll_state.local_snap,
                            )
                        } as f32;
                        next.length = (end - origin.start)
                            .max(crate::piano_roll::MIN_NOTE_LENGTH_BEATS)
                            .min(MAX_BEAT - origin.start);
                        self.piano_roll_state.last_note_length = next.length;
                    } else {
                        if !modifiers.ctrl && !modifiers.command {
                            next.start = moved_note_start(
                                origin.start,
                                delta.x / gesture.coordinates[2] as f32,
                                self.piano_roll_state.local_snap.keyboard_step(),
                                modifiers.alt || self.piano_roll_state.local_snap == PianoSnap::Off,
                            )
                            .unwrap_or(origin.start)
                            .min(MAX_BEAT - origin.length);
                        }
                        if !modifiers.shift {
                            next.note = (i16::from(origin.note)
                                - (delta.y / gesture.coordinates[3] as f32)
                                    .round()
                                    .clamp(-127.0, 127.0) as i16)
                                .clamp(0, 127) as u8;
                            if self.piano_roll_state.snap_to_scale && !modifiers.alt {
                                next.note = snap_pitch_to_scale(
                                    next.note,
                                    self.piano_roll_state.scale_root,
                                    self.piano_roll_state.scale,
                                );
                            }
                        }
                    }
                    if let Some(note) = self
                        .project
                        .active_pattern_mut()
                        .notes
                        .iter_mut()
                        .find(|n| n.id == origin.id)
                    {
                        *note = next;
                    }
                }
                _ => {}
            }
        }
        if released || !down {
            self.finish_piano_roll_gesture();
        } else {
            ctx.data_mut(|data| data.insert_temp(key(), gesture));
        }
    }
}
