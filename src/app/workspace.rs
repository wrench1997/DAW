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

fn editor_name(view: StudioView) -> &'static str {
    match view {
        StudioView::Playlist => "Playlist",
        StudioView::ChannelRack => "Channel Rack",
        StudioView::PianoRoll => "Piano Roll",
        StudioView::Mixer => "Mixer",
    }
}

fn title_response(ctx: &egui::Context, view: StudioView) -> Option<Response> {
    ctx.read_response(window_id(view).with("__title_click"))
}

/// Add a standard maximize affordance on the native title bar, next to its close
/// button. Use the real title response instead of assuming font/DPI coordinates.
fn maximize_button(ui: &mut egui::Ui, view: StudioView) -> bool {
    let Some(title) = title_response(ui.ctx(), view) else {
        return false;
    };
    let rect = Rect::from_min_max(
        Pos2::new(title.rect.right() - 24.0, title.rect.top() + 2.0),
        Pos2::new(title.rect.right() - 2.0, title.rect.bottom() - 2.0),
    );
    let mut chrome = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("editor-maximize", index(view)))
            .max_rect(rect),
    );
    chrome.set_clip_rect(title.rect);
    // Capture drags too: pressing this button and moving must not start a native
    // title move or draw notes after the pointer enters the canvas.
    let response = chrome.interact(
        rect,
        window_id(view).with("maximize"),
        Sense::click_and_drag(),
    );
    let label = format!("Maximize {}", editor_name(view));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, chrome.is_enabled(), &label)
    });
    let visuals = chrome.style().interact(&response);
    chrome.painter().rect_filled(rect, 2, theme::PANEL);
    chrome.painter().rect(
        rect,
        2,
        visuals.weak_bg_fill,
        visuals.bg_stroke,
        StrokeKind::Inside,
    );
    chrome.painter().rect_stroke(
        Rect::from_center_size(rect.center(), Vec2::new(10.0, 9.0)),
        0,
        visuals.fg_stroke,
        StrokeKind::Inside,
    );
    response
        .on_hover_text(format!("{label} (or double-click the title)"))
        .clicked()
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
    #[serde(skip)]
    restore_requested: bool,
    #[serde(skip)]
    sidebar_drag: Option<Id>,
    #[serde(skip)]
    sidebar_owner: Option<Id>,
    #[serde(skip)]
    sidebar_input_frame: Option<u64>,
    #[serde(skip)]
    sidebar_pointer_blocked: bool,
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
            restore_requested: false,
            sidebar_drag: None,
            sidebar_owner: None,
            sidebar_input_frame: None,
            sidebar_pointer_blocked: false,
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
    pub(super) fn cancel_editor_pointer_until_release(&mut self) {
        self.workspace.cancel_pointer_gesture = true;
    }

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

    pub(super) fn maximized_editor_title(
        &mut self,
        ui: &mut egui::Ui,
        view: StudioView,
        title: &str,
        subtitle: &str,
    ) {
        let heading = ui.label(RichText::new(title).strong().size(11.0).color(theme::TEXT));
        let title_response = ui.interact(
            heading.rect,
            window_id(view).with("maximized-title"),
            Sense::click(),
        );
        let restore = ui
            .small_button("Restore")
            .on_hover_text("Restore floating windows (or double-click the editor title)")
            .clicked();
        self.workspace.restore_requested |= title_response.double_clicked() || restore;
        ui.label(RichText::new(subtitle).size(9.0).color(theme::MUTED));
    }

    fn toggle_editor_maximized(&mut self, view: StudioView) {
        self.finish_editor_interaction();
        if self.workspace.focused != view {
            self.workspace.focus(view);
        }
        self.workspace.maximized = !self.workspace.maximized;
        self.workspace.dirty = true;
        self.workspace.restore_order_cursor = 0;
        self.workspace.raise_focused = 2;
    }

    fn hide_editor(&mut self, view: StudioView) {
        // Every focus transition shares the same edit/history boundary, including fallback
        // focus after closing a Rack/Mixer while their timed observer has pending changes.
        self.finish_editor_interaction();
        self.workspace.hide(view);
    }

    pub(super) fn workspace_view_menu(&mut self, ui: &mut egui::Ui) {
        // Moving these controls out of the canvas must retain its modal/gesture barriers.
        if !ui.input(|input| input.focused)
            || self.shortcut_blocking_layer_active()
            || self.top_shortcut_modal().is_some()
            || self.workspace.cancel_pointer_gesture
        {
            ui.disable();
        }
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
        if ui.button("Arrange windows").clicked() {
            self.arrange_editors(false);
            ui.close();
        }
        if ui.button("Cascade windows").clicked() {
            self.arrange_editors(true);
            ui.close();
        }
        let any_visible = self.workspace.windows.iter().any(|window| window.visible);
        if ui
            .add_enabled(
                any_visible,
                egui::Button::new(if self.workspace.maximized {
                    "Restore windows"
                } else {
                    "Maximize editor"
                }),
            )
            .clicked()
        {
            self.toggle_editor_maximized(self.workspace.focused);
            ui.close();
        }
        if ui
            .add_enabled(any_visible, egui::Button::new("Hide editor"))
            .clicked()
        {
            self.hide_editor(self.workspace.focused);
            ui.close();
        }
    }

    pub(super) fn cancel_piano_mouse_pointer(&mut self, ctx: &egui::Context) {
        self.workspace.cancel_pointer_gesture |= ctx.input(|input| input.pointer.primary_down());
        ctx.stop_dragging();
        self.finish_piano_roll_gesture();
    }

    /// Reserve the divider from the raw press position before egui renders any
    /// panels or floating editors. Native hit testing otherwise uses the final
    /// position of a coalesced press/move and may award the gesture to a note or
    /// an overlapping floating resize edge instead of the sidebar.
    pub(super) fn prepare_sidebar_pointer(&mut self, ui: &egui::Ui) {
        let ctx = ui.ctx();
        let frame = ctx.cumulative_frame_nr();
        let first_pass = self.workspace.sidebar_input_frame != Some(frame);
        let (down, released, pressed, focused) = ctx.input(|input| {
            (
                input.pointer.primary_down(),
                input.pointer.primary_released(),
                input.pointer.primary_pressed(),
                input.focused,
            )
        });
        if first_pass {
            self.workspace.sidebar_input_frame = Some(frame);
            self.workspace.sidebar_pointer_blocked = false;
            if pressed || (!down && !released) {
                self.workspace.sidebar_drag = None;
                self.workspace.sidebar_owner = None;
            }
        }
        let blocked = !ui.is_enabled()
            || !focused
            || self.shortcut_blocking_layer_active()
            || self.top_shortcut_modal().is_some()
            || ctx.any_popup_open()
            || self.workspace.cancel_pointer_gesture;
        // A modal/popup owns Escape even while an older sidebar pointer is held.
        let escape = !blocked
            && self.workspace.sidebar_owner.is_some()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if blocked || escape {
            if self.workspace.sidebar_owner.is_some() {
                // Retain a cancelled owner through every release-frame pass.
                self.workspace.sidebar_owner = Some(Id::new("cancelled-sidebar-pointer"));
                self.workspace.sidebar_drag = None;
                ctx.stop_dragging();
                self.workspace.cancel_pointer_gesture |= down;
                self.workspace.sidebar_pointer_blocked = true;
            }
            return;
        }
        if first_pass && pressed {
            let buttons = ctx.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            ..
                        } => Some((*pos, *pressed)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            });
            // Do not resize from an endpoint belonging to a later gesture in
            // the same frame. Ambiguous batches still cannot leak into editors.
            let ordered = matches!(buttons.as_slice(), [(_, true)] | [(_, true), (_, false)]);
            self.workspace.sidebar_owner = None;
            self.workspace.sidebar_drag = None;
            for (origin, pressed) in buttons {
                if !pressed {
                    continue;
                }
                for (panel, visible) in [
                    ("browser", self.show_browser),
                    ("inspector", self.show_inspector),
                ] {
                    if !visible {
                        continue;
                    }
                    let panel_id = Id::new(panel);
                    let divider = panel_id.with("__resize");
                    let on_divider = ctx.read_response(divider).is_some_and(|response| {
                        response.enabled() && response.rect.contains(origin)
                    });
                    let in_panel = egui::containers::panel::PanelState::load(ctx, panel_id)
                        .is_some_and(|state| state.outer_rect.contains(origin));
                    if on_divider || in_panel {
                        self.workspace.sidebar_owner = Some(if ordered {
                            panel_id
                        } else {
                            Id::new("ambiguous-sidebar-pointer")
                        });
                        self.workspace.sidebar_drag = (ordered && on_divider).then_some(divider);
                        break;
                    }
                }
                if self.workspace.sidebar_owner.is_some() {
                    break;
                }
            }
        }
        self.workspace.sidebar_pointer_blocked |= self.workspace.sidebar_owner.is_some();
        if let Some(id) = self.workspace.sidebar_drag {
            self.workspace.sidebar_pointer_blocked = true;
            // The native Panel owns width clamping and persistence. A complete
            // batch needs drag_stopped, never a phantom held drag after release.
            // Reapply each layout pass, retaining suppression for the whole frame.
            ctx.set_dragged_id(id);
            if !down {
                ctx.stop_dragging();
            }
        }
    }

    pub(super) fn sidebar_pointer_active(&self) -> bool {
        self.workspace.sidebar_pointer_blocked
    }

    pub(super) fn sidebar_content_blocked(&self, panel: &str) -> bool {
        self.sidebar_pointer_active()
            && (self.workspace.sidebar_drag.is_some()
                || self.workspace.sidebar_owner != Some(Id::new(panel)))
    }

    pub(super) fn piano_pointer_blocked(&self) -> bool {
        self.workspace.cancel_pointer_gesture
            || self.workspace.chrome_gesture.is_some()
            || self.sidebar_pointer_active()
    }

    pub(super) fn editor_pointer_gesture_active(&self) -> bool {
        self.workspace.pointer_dragging || self.workspace.cancel_pointer_gesture
    }

    pub(super) fn editor_pointer_cancellation_active(&self) -> bool {
        self.workspace.cancel_pointer_gesture
    }

    fn editor_drag_in_progress(&self, ctx: &egui::Context) -> bool {
        if self.sidebar_pointer_active() {
            return false;
        }
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
        let mut enabled = !interrupted && !blocked && !self.sidebar_pointer_active();
        if !enabled {
            ui.disable();
        }
        let bounds = ui.available_rect_before_wrap().shrink(3.0);

        // Suppression of editor input is not an interruption of the sidebar's
        // own Gain/Fade transaction. Only real cancellation/modal barriers finish
        // musical gestures here; Inspector sliders retain one edit until release.
        if interrupted || blocked {
            self.cancel_piano_range_gesture(ui.ctx());
            ui.ctx().data_mut(|data| {
                data.remove::<PianoNoteResizeGesture>(Id::new("piano-note-resize-gesture"));
                piano_mouse::clear(data);
            });
            self.finish_playlist_gesture();
            self.finish_piano_roll_gesture();
        }
        if self.workspace.maximized {
            let view = self.workspace.focused;
            // Sidebar resize handles extend into the center panel. Keep the
            // maximized canvas out of that small strip; otherwise its later
            // click-and-drag widget steals the Inspector divider's press.
            let inset = ui.style().interaction.resize_grab_radius_side + 1.0;
            let mut content = ui.available_rect_before_wrap();
            if self.show_browser {
                content.min.x += inset;
            }
            if self.show_inspector {
                content.max.x -= inset;
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                ui.shrink_clip_rect(content);
                ui.add_enabled_ui(enabled, |ui| self.render_editor(ui, view));
            });
            if std::mem::take(&mut self.workspace.restore_requested) && enabled {
                self.toggle_editor_maximized(view);
                ui.ctx().stop_dragging();
                ui.ctx().request_repaint();
            }
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
            && (self.editor_drag_in_progress(&ctx)
                || self.playlist_gesture_before.is_some()
                || self.piano_roll_gesture_before.is_some()
                || piano_mouse::active(&ctx)
                || piano_range::active(&ctx))
        {
            // Cancel only editor-owned gestures invalidated by the new coordinate
            // system. A sidebar's own resize changes these bounds on every held
            // frame and must survive through release so egui saves its new width.
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
        let mut maximize = None;
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
                StudioView::Playlist => self.playlist_scope_caption(),
                StudioView::ChannelRack => format!(
                    "{} · {} steps",
                    self.project.active_pattern().name,
                    self.project.active_pattern().length_steps
                ),
                StudioView::PianoRoll => format!(
                    "{} · {} · {} bars",
                    self.project
                        .channels
                        .get(self.selected_channel)
                        .map(|channel| channel.name.as_str())
                        .unwrap_or("Unassigned"),
                    self.project.active_pattern().name,
                    (pattern_playback_length_beats(&self.project) / 4.0).ceil() as usize
                ),
                StudioView::Mixer => "Insert routing & effects".to_owned(),
            };
            let mut heading = egui::Atoms::new(
                RichText::new(format!("{}   ·   {context}", title(view)))
                    .size(11.5)
                    .color(if self.workspace.focused == view {
                        theme::TEXT
                    } else {
                        theme::MUTED
                    }),
            );
            // Reserve text width for the title-bar action even with long context names.
            heading.push_right(egui::Atom::custom(
                window_id(view).with("maximize-space"),
                Vec2::new(26.0, 0.0),
            ));
            let mut window = egui::Window::new(heading)
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
                        .corner_radius(3)
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 2],
                            blur: 8,
                            spread: 0,
                            color: Color32::from_black_alpha(80),
                        })
                        .fill(theme::PANEL)
                        .stroke(Stroke::new(
                            1.0,
                            if self.workspace.focused == view {
                                theme::HIGHLIGHT
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
                let maximize_clicked = maximize_button(ui, view);
                if enabled
                    && (maximize_clicked
                        || title_response(&ctx, view).is_some_and(|title| title.double_clicked()))
                {
                    maximize = Some(view);
                }
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
        if let Some(view) = maximize {
            self.toggle_editor_maximized(view);
            ctx.stop_dragging();
            ctx.request_repaint();
            return;
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
                        ui.set_min_width(768.0);
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
