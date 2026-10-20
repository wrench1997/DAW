# Sidebar divider ownership

A real App-input regression exposed two independent sidebar resize failures:

- In the floating workspace, any held pointer plus changed workspace bounds
  stopped egui's active drag. A sidebar resize changes those bounds itself, so
  the divider lost ownership before release and egui retained its old width.
- In the maximized Playlist, the canvas covered the Inspector divider's inner
  edge and took ownership of the press instead of resizing the panel.

Bounds changes now cancel only invalidated editor-owned gestures. Sidebar resize
ownership survives to release, when egui persists the final width. Maximized
editors reserve the native side-resize grab width plus one point beside visible
sidebars, preventing the canvas from claiming those presses. This adds no new
vertical workspace chrome.

`workspace_sidebar_ui_tests.rs` drives the real App UI for floating and maximized
Browser and Inspector dividers. Each check asserts native drag ownership during
all held frames, saved widths after growing and shrinking, stable layout mode,
and unchanged musical project/history/dirty state. A separate held-note test
confirms that a true coordinate-system change still ends the edit, suppresses
further held movement, and leaves one undoable transaction. Existing title and
all-eight-edge resize interruption tests remain applicable.

These are production-input checks, not native Windows or audio/device acceptance.
Native desktop observations and aggregate validation are recorded separately by
the integrating task.

## Native laptop follow-up: original press ownership

At 1180 × 812, the native recheck found two additional failures: the Inspector
resize strip overlapped a floating Piano edge, and a press inside Inspector that
arrived with movement into Piano in the same frame could create a note. The latter
also occurred with a 28-point gap between Piano and Inspector. Complete
press/move/release batches instead lost the divider's released width.

`prepare_sidebar_pointer` now reserves visible sidebars from the raw primary press
before any panel or editor is rendered. The native divider remains responsible
for width limits and persistence. Editor input stays suppressed through the
release frame and all its layout passes. Ordinary sidebar controls remain usable;
this suppression does not end their Gain/Fade history transactions. Escape,
focus loss and modal interruption cancel ownership through release. Ambiguous
multiple-primary-gesture batches cannot resize from a later gesture's endpoint.

`workspace_sidebar_press_ui_tests.rs` covers both sidebars, floating-edge overlap,
flush/gapped Piano geometry, held/complete/partial batches, forced multiple passes,
sidebar blank/control origins, interruptions, phantom-drag cleanup and later valid
Piano input. Before repair, seven of the initial eight cases failed, including
three actual note mutations; the separate ordinary Inspector blank partial-batch
case also reproduced a mutation. The ordinary control-origin case was already
safe and is retained as a positive comparison. Existing Inspector Gain/Fade,
transaction, Undo/Redo and lifecycle tests remain part of regression validation.
