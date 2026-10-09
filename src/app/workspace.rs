//! Persistent in-process editor windows. Musical data and runtime ownership remain in CitrusApp.
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
        StudioView::ChannelRack => Vec2::new(400.0, 270.0),
        StudioView::PianoRoll => Vec2::new(480.0, 420.0),
        StudioView::Mixer => Vec2::new(440.0, 410.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct EditorWindow {
    pub visible: bool,
    /// Workspace-relative logical points, independent of project/media paths.
    pub rect: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Workspace {
    version: u32,
    pub focused: StudioView,
    pub maximized: bool,
    pub windows: [EditorWindow; 4],
    pub order: [StudioView; 4],
    #[serde(skip)]
    pub dirty: bool,
    #[serde(skip)]
    bounds: Option<Rect>,
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
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: LAYOUT_VERSION,
            focused: StudioView::PianoRoll,
            maximized: false,
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
    let (x, y, w, h) = match view {
        StudioView::Playlist => (0.0, 0.0, 0.65, 0.54),
        StudioView::ChannelRack => (0.49, 0.0, 0.51, 0.44),
        StudioView::Mixer => (0.0, 0.52, 0.54, 0.48),
        StudioView::PianoRoll => (0.43, 0.40, 0.57, 0.60),
    };
    bounded_rect(
        Rect::from_min_size(
            bounds.min + bounds.size() * Vec2::new(x, y),
            bounds.size() * Vec2::new(w, h),
        ),
        bounds,
        view,
    )
}

impl CitrusApp {
    pub(super) fn focus_editor(&mut self, view: StudioView) {
        if self.workspace.focused != view {
            self.finish_editor_interaction();
        }
        self.workspace.focus(view);
    }

    fn finish_editor_interaction(&mut self) {
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
        let blocked = !ui.is_enabled()
            || self.shortcut_blocking_layer_active()
            || self.top_shortcut_modal().is_some();
        if blocked
            && (self.editor_drag_in_progress(ui.ctx())
                || self.playlist_gesture_before.is_some()
                || self.piano_roll_gesture_before.is_some())
        {
            self.workspace.cancel_pointer_gesture = true;
        }
        let interrupted = self.workspace.cancel_pointer_gesture;
        if interrupted {
            ui.ctx().stop_dragging();
            ui.ctx().data_mut(|data| {
                data.remove::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"));
            });
            if !ui.input(|input| input.pointer.primary_down()) {
                self.workspace.cancel_pointer_gesture = false;
            }
        }
        self.workspace.pointer_dragging = self.editor_drag_in_progress(ui.ctx());
        let enabled = !interrupted
            && ui.is_enabled()
            && !self.shortcut_blocking_layer_active()
            && self.top_shortcut_modal().is_none();
        if !enabled {
            ui.disable();
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("WORKSPACE").size(9.0).color(theme::MUTED));
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
            ui.ctx().data_mut(|data| {
                data.remove::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"));
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
            ctx.data_mut(|data| {
                data.remove::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"));
            });
        }
        let bounds_changed = self.workspace.bounds != Some(bounds);
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
        let order = self.workspace.order;
        for view in order {
            if !self.workspace.windows[index(view)].visible {
                continue;
            }
            let stored = self.workspace.windows[index(view)].rect;
            let rect = stored
                .map(|rect| rect.translate(bounds.min.to_vec2()))
                .map(|rect| bounded_rect(rect, bounds, view))
                .unwrap_or_else(|| initial_rect(view, bounds));
            let mut open = true;
            let mut window = egui::Window::new(RichText::new(title(view)).size(12.0))
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
                        .inner_margin(egui::Margin::symmetric(4, 2))
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
            if reset {
                window = window.fixed_rect(rect);
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
                        ui.set_min_width(920.0);
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
