//! Persistent in-process editor windows. Musical data and runtime ownership remain in CitrusApp.
use super::workspace_geometry::{ResizeEdges, WindowGesture, snap_rect};
use super::*;
use serde::{Deserialize, Serialize};

const STORAGE_KEY: &str = "citrus-studio/workspace/v1";
const LAYOUT_VERSION: u32 = 1;
pub(super) const EDITORS: [StudioView; 4] = [
    StudioView::Playlist,
    StudioView::ChannelRack,
    StudioView::Mixer,
    StudioView::PianoRoll,
];

pub(super) fn index(view: StudioView) -> usize {
    match view {
        StudioView::Playlist => 0,
        StudioView::ChannelRack => 1,
        StudioView::PianoRoll => 2,
        StudioView::Mixer => 3,
    }
}

pub(super) fn title(view: StudioView) -> &'static str {
    match view {
        StudioView::Playlist => "Playlist · F5",
        StudioView::ChannelRack => "Channel Rack · F6",
        StudioView::PianoRoll => "Piano Roll · F7",
        StudioView::Mixer => "Mixer · F9",
    }
}

pub(super) fn window_id(view: StudioView) -> Id {
    Id::new(("citrus-editor-window-v1", index(view)))
}

fn layer(view: StudioView) -> egui::LayerId {
    egui::LayerId::new(egui::Order::Middle, window_id(view))
}

fn minimum(view: StudioView) -> Vec2 {
    match view {
        StudioView::Playlist => Vec2::new(460.0, 310.0),
        StudioView::ChannelRack => Vec2::new(400.0, 240.0),
        StudioView::PianoRoll => Vec2::new(480.0, 420.0),
        StudioView::Mixer => Vec2::new(440.0, 410.0),
    }
}

// Existing v1 layouts opened with an Inspector; retain that presentation when the
// new optional field is absent. Fresh layouts leave more room for the editors.
fn legacy_inspector_visible() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct EditorWindow {
    pub visible: bool,
    /// Workspace-relative logical points, independent of project/media paths.
    pub rect: Option<Rect>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ChromeGesture {
    view: StudioView,
    kind: WindowGesture,
}

// egui 0.35 exposes the active widget ID, but no public Window gesture API.
// Keep the native IDs isolated here and guard them with real pointer regressions.
fn chrome_gesture(ctx: &egui::Context) -> Option<ChromeGesture> {
    let dragged = ctx.dragged_id()?;
    for view in EDITORS {
        if dragged == window_id(view).with("__title_click") {
            return Some(ChromeGesture {
                view,
                kind: WindowGesture::Move,
            });
        }
        let edge_id = Id::new(layer(view)).with("edge_drag");
        for (name, left, right, top, bottom) in [
            ("left", true, false, false, false),
            ("right", false, true, false, false),
            ("top", false, false, true, false),
            ("bottom", false, false, false, true),
            ("left_top", true, false, true, false),
            ("left_bottom", true, false, false, true),
            ("right_top", false, true, true, false),
            ("right_bottom", false, true, false, true),
        ] {
            if dragged == edge_id.with(name) {
                return Some(ChromeGesture {
                    view,
                    kind: WindowGesture::Resize(ResizeEdges {
                        left,
                        right,
                        top,
                        bottom,
                    }),
                });
            }
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Workspace {
    version: u32,
    pub focused: StudioView,
    pub maximized: bool,
    #[serde(default = "legacy_inspector_visible")]
    pub inspector_visible: bool,
    pub windows: [EditorWindow; 4],
    pub order: [StudioView; 4],
    #[serde(skip)]
    pub dirty: bool,
    #[serde(skip)]
    pub(super) bounds: Option<Rect>,
    #[serde(skip)]
    reset_geometry: bool,
    #[serde(skip)]
    raise_focused: u8,
    #[serde(skip)]
    restore_order_cursor: usize,
    #[serde(skip)]
    cancel_pointer_gesture: bool,
    #[serde(skip)]
    pointer_dragging: bool,
    #[serde(skip)]
    chrome_gesture: Option<ChromeGesture>,
    #[serde(skip)]
    pending_rects: [Option<Rect>; 4],
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: LAYOUT_VERSION,
            focused: StudioView::PianoRoll,
            maximized: false,
            inspector_visible: false,
            windows: [EditorWindow {
                visible: true,
                rect: None,
            }; 4],
            order: EDITORS,
            dirty: false,
            bounds: None,
            reset_geometry: true,
            raise_focused: 2,
            restore_order_cursor: 0,
            cancel_pointer_gesture: false,
            pointer_dragging: false,
            chrome_gesture: None,
            pending_rects: [None; 4],
        }
    }
}

impl Workspace {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|storage| storage.get_string(STORAGE_KEY))
            .as_deref()
            .and_then(Self::decode)
            .unwrap_or_default()
    }

    fn decode(text: &str) -> Option<Self> {
        let mut state: Self = serde_json::from_str(text).ok()?;
        if state.version != LAYOUT_VERSION
            || EDITORS
                .iter()
                .any(|view| state.order.iter().filter(|other| *other == view).count() != 1)
            || state.windows.iter().any(|window| {
                window.rect.is_some_and(|rect| {
                    !rect.is_finite() || rect.width() <= 0.0 || rect.height() <= 0.0
                })
            })
        {
            return None;
        }
        state.reset_geometry = true;
        state.raise_focused = 2;
        state.restore_order_cursor = 0;
        if !state.windows[index(state.focused)].visible {
            state.maximized = false;
            if let Some(view) = state
                .order
                .iter()
                .rev()
                .find(|view| state.windows[index(**view)].visible)
            {
                state.focused = *view;
            }
        }
        Some(state)
    }

    #[cfg(test)]
    pub(super) fn reset_for_test(&mut self) {
        self.reset_geometry = true;
        self.chrome_gesture = None;
        self.pending_rects = [None; 4];
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(text) = serde_json::to_string(self) {
            storage.set_string(STORAGE_KEY, text);
        }
    }

    fn focus(&mut self, view: StudioView) {
        self.focused = view;
        self.windows[index(view)].visible = true;
        // Preserve all other relative stacking order.
        let position = self
            .order
            .iter()
            .position(|item| *item == view)
            .unwrap_or(0);
        self.order[position..].rotate_left(1);
        self.restore_order_cursor = EDITORS.len();
        self.raise_focused = 2;
        self.dirty = true;
    }

    fn hide(&mut self, view: StudioView) {
        self.windows[index(view)].visible = false;
        if self.focused == view {
            self.maximized = false;
            if let Some(next) = self
                .order
                .iter()
                .rev()
                .find(|other| self.windows[index(**other)].visible)
            {
                self.focused = *next;
                self.raise_focused = 2;
            }
        }
        self.dirty = true;
    }

    fn arrange(&mut self, cascade: bool) {
        self.maximized = false;
        for view in EDITORS {
            self.windows[index(view)] = EditorWindow {
                visible: true,
                rect: None,
            };
        }
        self.order = EDITORS;
        self.focused = StudioView::PianoRoll;
        if cascade && let Some(bounds) = self.bounds {
            for (i, view) in EDITORS.into_iter().enumerate() {
                self.windows[index(view)].rect = Some(Rect::from_min_size(
                    Pos2::new(i as f32 * 32.0, i as f32 * 40.0),
                    Vec2::new(bounds.width() * 0.78, bounds.height() * 0.76),
                ));
            }
        }
        self.reset_geometry = true;
        self.restore_order_cursor = 0;
        self.raise_focused = 2;
        self.dirty = true;
    }
}

/// Fully contain window chrome after viewport/side-panel/DPI changes and corrupt-but-finite positions.
fn bounded_rect(rect: Rect, bounds: Rect, view: StudioView) -> Rect {
    let size = rect
        .size()
        .max(minimum(view))
        .min(bounds.size().max(Vec2::splat(1.0)));
    let min = Pos2::new(
        rect.min
            .x
            .clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left())),
        rect.min
            .y
            .clamp(bounds.top(), (bounds.bottom() - size.y).max(bounds.top())),
    );
    Rect::from_min_size(min, size)
}

fn initial_rect(view: StudioView, bounds: Rect) -> Rect {
    // A coherent two-column desktop rather than four oversized cascading canvases.
    // Small workspaces retain exposed title bars and the existing maximize workflow.
    let gap = 4.0;
    let left = (bounds.width() * 0.52).round();
    let rack_height = 258.0_f32.min(bounds.height() * 0.42);
    let playlist_height = (bounds.height() * 0.48)
        .min(
            (bounds.height() - minimum(StudioView::Mixer).y)
                .max(minimum(StudioView::Playlist).y + gap),
        )
        .round();
    let (offset, size) = if bounds.width() >= 1080.0 && bounds.height() >= 724.0 {
        match view {
            StudioView::Playlist => (Vec2::ZERO, Vec2::new(left - gap, playlist_height - gap)),
            StudioView::Mixer => (
                Vec2::new(0.0, playlist_height),
                Vec2::new(left - gap, bounds.height() - playlist_height),
            ),
            StudioView::ChannelRack => (
                Vec2::new(left, 0.0),
                Vec2::new(bounds.width() - left, rack_height - gap),
            ),
            StudioView::PianoRoll => (
                Vec2::new(left, rack_height),
                Vec2::new(bounds.width() - left, bounds.height() - rack_height),
            ),
        }
    } else {
        match view {
            StudioView::Playlist => (
                Vec2::ZERO,
                Vec2::new(bounds.width() * 0.78, bounds.height() * 0.56),
            ),
            StudioView::ChannelRack => (
                Vec2::new(bounds.width() * 0.35, 28.0),
                Vec2::new(bounds.width() * 0.65, rack_height),
            ),
            StudioView::Mixer => (
                Vec2::new(0.0, bounds.height() * 0.42),
                Vec2::new(bounds.width() * 0.60, bounds.height() * 0.58),
            ),
            StudioView::PianoRoll => (
                Vec2::new(bounds.width() * 0.30, bounds.height() * 0.32),
                Vec2::new(bounds.width() * 0.70, bounds.height() * 0.68),
            ),
        }
    };
    bounded_rect(Rect::from_min_size(bounds.min + offset, size), bounds, view)
}

impl CitrusApp {
    pub(super) fn focus_editor(&mut self, view: StudioView) {
        if self.workspace.focused != view {
            self.finish_editor_interaction();
        }
        self.workspace.focus(view);
    }

    fn finish_editor_interaction(&mut self) {
        self.workspace.chrome_gesture = None;
        self.workspace.pending_rects = [None; 4];
        self.workspace.cancel_pointer_gesture |= self.workspace.pointer_dragging
            || self.playlist_gesture_before.is_some()
            || self.piano_roll_gesture_before.is_some();
        self.finish_playlist_gesture();
        self.finish_piano_roll_gesture();
        self.flush_pending_editor_history();
        self.last_history_capture = Instant::now() - Duration::from_millis(500);
    }

    fn arrange_editors(&mut self, cascade: bool) {
        self.finish_editor_interaction();
        self.workspace.arrange(cascade);
    }

    pub(super) fn commit_editor_project(&mut self, candidate: Project) {
        if commit_explicit_project_history_transaction(
            &mut self.project,
            &mut self.history_snapshot,
            &mut self.history_fingerprint,
            &mut self.undo_stack,
            &mut self.redo_stack,
            &mut self.dirty,
            candidate,
        ) {
            let now = Instant::now();
            self.last_history_check = now;
            self.last_history_capture = now;
            self.automation_evaluator.reset();
            self.refresh_timeline_fingerprint(now, true);
        }
    }

    pub(super) fn flush_pending_editor_history(&mut self) {
        if self.piano_roll_transform.is_some()
            || self.playlist_gesture_before.is_some()
            || self.piano_roll_gesture_before.is_some()
        {
            return;
        }
        let fingerprint = project_fingerprint(&self.project);
        if fingerprint != self.history_fingerprint {
            let before = std::mem::replace(&mut self.history_snapshot, self.project.clone());
            push_bounded_project_history(&mut self.undo_stack, before);
            self.history_fingerprint = fingerprint;
            self.redo_stack.clear();
            self.dirty = true;
            self.last_history_check = Instant::now();
            self.last_history_capture = Instant::now();
        }
    }

    fn hide_editor(&mut self, view: StudioView) {
        // Every focus transition shares the same edit/history boundary, including fallback
        // focus after closing a Rack/Mixer while their timed observer has pending changes.
        self.finish_editor_interaction();
        self.workspace.hide(view);
    }

    pub(super) fn workspace_view_menu(&mut self, ui: &mut egui::Ui) {
        for view in EDITORS {
            let mut visible = self.workspace.windows[index(view)].visible;
            if ui.checkbox(&mut visible, title(view)).changed() {
                if visible {
                    self.focus_editor(view);
                } else {
                    self.hide_editor(view);
                }
            }
        }
        ui.separator();
        if ui.button("Arrange editor windows").clicked() {
            self.arrange_editors(false);
            ui.close();
        }
        if ui.button("Cascade editor windows").clicked() {
            self.arrange_editors(true);
            ui.close();
        }
    }

    pub(super) fn cancel_piano_mouse_pointer(&mut self, ctx: &egui::Context) {
        self.workspace.cancel_pointer_gesture |= ctx.input(|input| input.pointer.primary_down());
        ctx.stop_dragging();
        self.finish_piano_roll_gesture();
    }

    pub(super) fn piano_pointer_blocked(&self) -> bool {
        self.workspace.cancel_pointer_gesture || self.workspace.chrome_gesture.is_some()
    }

    pub(super) fn editor_pointer_gesture_active(&self) -> bool {
        self.workspace.pointer_dragging || self.workspace.cancel_pointer_gesture
    }

    fn editor_drag_in_progress(&self, ctx: &egui::Context) -> bool {
        ctx.dragged_id()
            .and_then(|id| ctx.read_response(id))
            .is_some_and(|response| {
                if self.workspace.maximized {
                    response.layer_id.order == egui::Order::Background
                } else {
                    EDITORS
                        .into_iter()
                        .any(|view| response.layer_id == layer(view))
                }
            })
    }

    pub(super) fn editor_workspace(&mut self, ui: &mut egui::Ui) {
        if self.workspace.inspector_visible != self.show_inspector {
            self.workspace.inspector_visible = self.show_inspector;
            self.workspace.dirty = true;
        }
        let blocked = !ui.is_enabled()
            || !ui.input(|input| input.focused)
            || self.shortcut_blocking_layer_active()
            || self.top_shortcut_modal().is_some();
        if blocked
            && (self.editor_drag_in_progress(ui.ctx())
                || self.playlist_gesture_before.is_some()
                || self.piano_roll_gesture_before.is_some()
                || piano_mouse::active(ui.ctx())
                || piano_range::active(ui.ctx()))
        {
            self.workspace.cancel_pointer_gesture = true;
        }
        if blocked || self.workspace.cancel_pointer_gesture {
            self.workspace.chrome_gesture = None;
            self.workspace.pending_rects = [None; 4];
        }
        let interrupted = self.workspace.cancel_pointer_gesture;
        if interrupted {
            self.cancel_piano_range_gesture(ui.ctx());
            ui.ctx().stop_dragging();
            ui.ctx().data_mut(|data| {
                data.remove::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"));
                piano_mouse::clear(data);
            });
            if !ui.input(|input| input.pointer.primary_down()) {
                self.workspace.cancel_pointer_gesture = false;
            }
        }
        self.workspace.pointer_dragging = self.editor_drag_in_progress(ui.ctx());
        let mut enabled = !interrupted && !blocked;
        if !enabled {
            ui.disable();
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("WORKSPACE").size(9.0).color(theme::MUTED))
                .on_hover_text("Drag title bars or resize window edges. Release within 8 points of a workspace or window edge to align. Hold Alt to release without snapping.");
            if ui.small_button("Arrange windows").clicked() {
                self.arrange_editors(false);
            }
            if ui.small_button("Cascade windows").clicked() {
                self.arrange_editors(true);
            }
            let any_visible = self.workspace.windows.iter().any(|window| window.visible);
            if ui
                .add_enabled(
                    any_visible,
                    egui::Button::new(if self.workspace.maximized {
                        "Restore windows"
                    } else {
                        "Maximize editor"
                    })
                    .small(),
                )
                .clicked()
            {
                self.finish_editor_interaction();
                self.workspace.maximized = !self.workspace.maximized;
                self.workspace.dirty = true;
                self.workspace.restore_order_cursor = 0;
                self.workspace.raise_focused = 2;
            }
            if ui
                .add_enabled(any_visible, egui::Button::new("Hide editor").small())
                .clicked()
            {
                self.hide_editor(self.workspace.focused);
            }
        });
        let bounds = ui.available_rect_before_wrap().shrink(3.0);

        if !enabled {
            self.cancel_piano_range_gesture(ui.ctx());
            ui.ctx().data_mut(|data| {
                data.remove::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"));
                piano_mouse::clear(data);
            });
            self.finish_playlist_gesture();
            self.finish_piano_roll_gesture();
        }
        if self.workspace.maximized {
            ui.add_enabled_ui(enabled, |ui| self.render_editor(ui, self.workspace.focused));
            return;
        }
        ui.painter().text(
            bounds.center(),
            Align2::CENTER_CENTER,
            "F5 Playlist   ·   F6 Channel Rack   ·   F7 Piano Roll   ·   F9 Mixer",
            FontId::proportional(11.0),
            theme::MUTED.gamma_multiply(0.55),
        );
        let ctx = ui.ctx().clone();
        if !self.workspace.windows[index(StudioView::PianoRoll)].visible {
            self.cancel_piano_range_gesture(&ctx);
            ctx.data_mut(|data| {
                data.remove::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"));
                piano_mouse::clear(data);
            });
        }
        let bounds_changed = self.workspace.bounds != Some(bounds);
        if bounds_changed
            && self.workspace.bounds.is_some()
            && ctx.input(|input| input.pointer.primary_down())
        {
            // Resizing the desktop or toggling a side panel must not resume a stale
            // window/note drag against a different coordinate system.
            self.finish_editor_interaction();
            self.workspace.cancel_pointer_gesture = true;
            ctx.stop_dragging();
            enabled = false;
            ui.disable();
        }
        self.workspace.bounds = Some(bounds);
        let reset = self.workspace.reset_geometry || bounds_changed;
        self.workspace.reset_geometry = false;
        // Hit-test the previous frame's frontmost layer before rendering any editor. This
        // selects the keyboard target without mutating unrelated note/clip/channel selection.
        if enabled
            && let Some(pointer) = ctx.input(|input| {
                input
                    .pointer
                    .any_pressed()
                    .then(|| input.pointer.interact_pos())
                    .flatten()
            })
            && let Some(hit) = ctx.layer_id_at(pointer)
            && let Some(view) = EDITORS.into_iter().find(|view| layer(*view) == hit)
        {
            // A fresh primary press may already be egui's new fader/edge drag. It belongs
            // to the editor being activated, rather than an interrupted previous drag.
            let new_press = ctx.input(|input| input.pointer.primary_pressed());
            let dragging = self.workspace.pointer_dragging;
            if new_press {
                self.workspace.pointer_dragging = false;
            }
            self.focus_editor(view);
            if new_press {
                self.workspace.pointer_dragging = dragging;
            }
        }
        if self.piano_roll_gesture_before.is_none() {
            ctx.data_mut(|data| {
                data.remove::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"));
            });
        }
        if enabled && ctx.input(|input| input.pointer.primary_down()) {
            if let Some(gesture) = chrome_gesture(&ctx) {
                self.workspace.chrome_gesture = Some(gesture);
            }
            // Never steal a fresh press for a deferred geometry correction.
            self.workspace.pending_rects = [None; 4];
        }
        let order = self.workspace.order;
        for view in order {
            if !self.workspace.windows[index(view)].visible {
                continue;
            }
            let pending = self.workspace.pending_rects[index(view)].take();
            let stored = pending.or(self.workspace.windows[index(view)].rect);
            let rect = stored
                .map(|rect| rect.translate(bounds.min.to_vec2()))
                .map(|rect| bounded_rect(rect, bounds, view))
                .unwrap_or_else(|| initial_rect(view, bounds));
            let mut open = true;
            let context = match view {
                StudioView::Playlist => format!(
                    "{} bars · {}",
                    (self.project.song_length_beats / 4.0).ceil() as usize,
                    self.project.active_pattern().name
                ),
                StudioView::ChannelRack => format!(
                    "{} · {} steps",
                    self.project.active_pattern().name,
                    self.project.active_pattern().length_steps
                ),
                StudioView::PianoRoll => format!(
                    "{} · {}",
                    self.project
                        .channels
                        .get(self.selected_channel)
                        .map(|channel| channel.name.as_str())
                        .unwrap_or("Unassigned"),
                    self.project.active_pattern().name
                ),
                StudioView::Mixer => "Insert routing & effects".to_owned(),
            };
            let mut window = egui::Window::new(
                RichText::new(format!("{}   ·   {context}", title(view))).size(11.0),
            )
            .id(window_id(view))
            .open(&mut open)
            .enabled(enabled && self.top_shortcut_modal().is_none())
            .interactable(enabled && self.top_shortcut_modal().is_none())
            .collapsible(false)
            .drag_area(egui::WindowDrag::TitleBar)
            .fade_in(false)
            .fade_out(false)
            .default_rect(rect)
            .min_size(minimum(view).min(bounds.size()))
            .max_size(bounds.size())
            .constrain_to(bounds)
            .frame(
                egui::Frame::window(ui.style())
                    .inner_margin(egui::Margin::symmetric(2, 1))
                    .corner_radius(2)
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 2],
                        blur: 4,
                        spread: 0,
                        color: Color32::from_black_alpha(80),
                    })
                    .fill(theme::PANEL)
                    .stroke(Stroke::new(
                        1.0,
                        if self.workspace.focused == view {
                            theme::ORANGE.gamma_multiply(0.75)
                        } else {
                            theme::GRID
                        },
                    )),
            );
            if reset || pending.is_some() {
                // The target is already bounded. Native Area constraint uses its previous
                // size before Resize applies the new one; on reset that can shift an
                // otherwise correct new position using stale geometry.
                window = window.fixed_rect(rect).constrain(false);
            }
            let shown = window.show(&ctx, |ui| {
                // Each editor is rendered exactly once with its own stable ID namespace.
                ui.push_id(("editor-content", index(view)), |ui| {
                    self.render_editor(ui, view)
                });
            });
            if let Some(shown) = shown {
                let relative = shown.response.rect.translate(-bounds.min.to_vec2());
                if self.workspace.windows[index(view)].rect != Some(relative) {
                    self.workspace.windows[index(view)].rect = Some(relative);
                    self.workspace.dirty = true;
                }
            }
            if !open {
                self.hide_editor(view);
            }
        }
        if enabled
            && ctx.input(|input| input.pointer.primary_released())
            && let Some(gesture) = self.workspace.chrome_gesture.take()
            && let Some(relative) = self.workspace.windows[index(gesture.view)].rect
        {
            let actual = relative.translate(bounds.min.to_vec2());
            let others: Vec<_> = EDITORS
                .into_iter()
                .filter(|view| {
                    *view != gesture.view && self.workspace.windows[index(*view)].visible
                })
                .filter_map(|view| self.workspace.windows[index(view)].rect)
                .map(|rect| rect.translate(bounds.min.to_vec2()))
                .collect();
            let corrected = if ctx.input(|input| input.modifiers.alt) {
                bounded_rect(actual, bounds, gesture.view)
            } else {
                snap_rect(actual, bounds, minimum(gesture.view), &others, gesture.kind)
            };
            if (corrected.min - actual.min).length() > 0.5
                || (corrected.max - actual.max).length() > 0.5
            {
                self.workspace.pending_rects[index(gesture.view)] =
                    Some(corrected.translate(-bounds.min.to_vec2()));
                ctx.request_repaint();
            }
        }
        if enabled {
            if self.workspace.restore_order_cursor < EDITORS.len() {
                // egui batches all move_to_top calls in one pass as an unordered set.
                // Reapply one layer per immediate repaint so persisted stacking is exact,
                // including when all windows reappear after a maximized editor.
                let view = self.workspace.order[self.workspace.restore_order_cursor];
                if self.workspace.windows[index(view)].visible {
                    ctx.move_to_top(layer(view));
                }
                self.workspace.restore_order_cursor += 1;
                ctx.request_repaint();
            } else if self.workspace.raise_focused > 0 {
                if self.workspace.windows[index(self.workspace.focused)].visible {
                    ctx.move_to_top(layer(self.workspace.focused));
                }
                self.workspace.raise_focused -= 1;
                ctx.request_repaint();
            }
        }
    }

    fn render_editor(&mut self, ui: &mut egui::Ui, view: StudioView) {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 2.0);
        ui.spacing_mut().button_padding = Vec2::new(5.0, 2.0);
        ui.spacing_mut().interact_size.y = 24.0;
        let sizing = ui.is_sizing_pass();
        let playlist_viewport = self.playlist_viewport;
        let piano_viewport = self.piano_viewport;
        match view {
            StudioView::Playlist => self.playlist(ui),
            StudioView::PianoRoll => self.piano_roll(ui),
            StudioView::Mixer => self.mixer(ui),
            StudioView::ChannelRack => {
                egui::ScrollArea::both()
                    .id_salt("channel-rack-scroll")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.set_min_width(720.0);
                        self.channel_rack(ui);
                    });
            }
        }
        if sizing {
            // egui probes unconstrained content while sizing a new/resized Window. Those
            // temporary extents must never reset a musical editor's pan/zoom position.
            self.playlist_viewport = playlist_viewport;
            self.piano_viewport = piano_viewport;
        }
        self.workspace.pointer_dragging = self.editor_drag_in_progress(ui.ctx());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_version_defaults_and_round_trip() {
        let mut state = Workspace::default();
        state.windows[0].rect = Some(Rect::from_min_size(
            Pos2::new(22.0, 33.0),
            Vec2::new(600.0, 350.0),
        ));
        state.hide(StudioView::ChannelRack);
        state.focus(StudioView::Mixer);
        state.maximized = true;
        let json = serde_json::to_string(&state).unwrap();
        let restored = Workspace::decode(&json).unwrap();
        assert_eq!(restored.windows, state.windows);
        assert_eq!(restored.focused, StudioView::Mixer);
        assert_eq!(restored.order, state.order);
        assert!(restored.maximized);
        assert!(Workspace::decode(&json.replace("\"version\":1", "\"version\":999")).is_none());
        assert!(Workspace::decode("{}").is_none());
        assert!(Workspace::decode("invalid").is_none());
        assert_eq!(
            Workspace::load(None)
                .windows
                .iter()
                .filter(|window| window.visible)
                .count(),
            4
        );
    }

    #[test]
    fn legacy_v1_layout_retains_geometry_and_inspector_visibility() {
        let mut state = Workspace::default();
        state.windows[0].rect = Some(Rect::from_min_size(
            Pos2::new(17.0, 28.0),
            Vec2::new(640.0, 380.0),
        ));
        let mut legacy = serde_json::to_value(&state).unwrap();
        legacy.as_object_mut().unwrap().remove("inspector_visible");
        let loaded = Workspace::decode(&legacy.to_string()).unwrap();
        assert_eq!(loaded.windows, state.windows);
        assert!(loaded.inspector_visible);
        assert!(!Workspace::default().inspector_visible);
        for visible in [true, false] {
            state.inspector_visible = visible;
            assert_eq!(
                Workspace::decode(&serde_json::to_string(&state).unwrap())
                    .unwrap()
                    .inspector_visible,
                visible
            );
        }
    }

    #[test]
    fn tiled_defaults_respect_editor_minima_at_laptop_boundary_heights() {
        for height in [724.0, 740.0, 768.0, 900.0, 1080.0] {
            let bounds = Rect::from_min_size(Pos2::new(200.0, 130.0), Vec2::new(1200.0, height));
            for (i, view) in EDITORS.into_iter().enumerate() {
                let rect = initial_rect(view, bounds);
                assert!(bounds.contains_rect(rect));
                for other in EDITORS.into_iter().skip(i + 1) {
                    assert!(
                        !rect.intersect(initial_rect(other, bounds)).is_positive(),
                        "overlap at {height}: {view:?} / {other:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn focus_preserves_other_windows_and_geometry() {
        let mut state = Workspace::default();
        let initial = state.windows;
        state.focus(StudioView::ChannelRack);
        assert_eq!(state.windows, initial);
        assert_eq!(state.order.last(), Some(&StudioView::ChannelRack));
        state.hide(StudioView::ChannelRack);
        assert_ne!(state.focused, StudioView::ChannelRack);
        state.focus(StudioView::ChannelRack);
        assert!(state.windows[index(StudioView::ChannelRack)].visible);
    }

    #[test]
    fn window_bounds_recover_offscreen_and_tiny_viewports() {
        for size in [Vec2::new(940.0, 680.0), Vec2::new(320.0, 200.0)] {
            let bounds = Rect::from_min_size(Pos2::new(180.0, 115.0), size);
            for view in EDITORS {
                for rect in [
                    Rect::from_min_size(Pos2::new(-4000.0, 9000.0), Vec2::new(9000.0, 1.0)),
                    initial_rect(view, bounds),
                ] {
                    let bounded = bounded_rect(rect, bounds, view);
                    assert!(bounds.contains_rect(bounded));
                    assert!(bounded.size().x >= minimum(view).x.min(bounds.width()));
                    assert!(bounded.size().y >= minimum(view).y.min(bounds.height()));
                }
            }
        }
    }
}
