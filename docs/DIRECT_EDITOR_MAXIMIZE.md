# Direct editor maximize and restore

Floating editors have a square maximize button beside the native close button.
The control has an editor-specific accessibility label (for example, `Maximize
Piano Roll`) and a tooltip. Double-clicking the native title bar also maximizes
that editor. Both operate on the existing saved workspace maximize state.

The maximized editor has a visible `Restore` button in its existing header;
double-clicking its editor title also restores. No extra workspace row is added. The Windows menu still provides
`Maximize editor` and `Restore windows`, and F5/F6/F7/F9 can switch the maximized
editor as before.

Floating rectangles and the other editors' relative stacking order are preserved.
The clicked editor becomes active. A title press remains a native move gesture
when dragged; pressing the maximize button then dragging off it cancels the click
without moving the window or painting notes. Dialog and focus barriers continue to
apply. These controls do not modify the musical project or create Undo entries.

The real App input regressions in `workspace_maximize_ui_tests.rs` cover:

- all four editors, repeated maximize/restore and actual stacking at 1180×812;
- native title double-click, single click and held second press;
- pointer-down on maximize followed by a held drag into the Piano Roll;
- continuous native title movement after an initial click;
- modal blocking and switching the editor while maximized;
- first-press activation of an unfocused editor's maximize button.

These tests drive the production `eframe::App::ui` using real egui input and
accessibility bounds. They do not establish native desktop, Windows, device/audio,
or plug-in acceptance. Native observation and aggregate validation are recorded
separately by the integrating task.
