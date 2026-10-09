# Multiwindow editor workspace

Citrus Studio now opens Playlist, Channel Rack, Piano Roll and Mixer together as
real, movable and resizable egui editor windows inside the native app. These are
production editing surfaces sharing one Project, transport, audio engine, plug-in
runtime and undo history. They are not separate OS windows or a web mockup.

## Working with windows

- F5 / F6 / F7 / F9 and the existing toolbar show, activate and raise Playlist,
  Channel Rack, Piano Roll and Mixer respectively.
- Drag a title bar to move an editor. Drag an edge/corner to resize it. Clicking an
  exposed editor or one of its controls activates it without clearing unrelated
  clip, note, channel or mixer selections.
- The title-bar close button and `Hide editor` hide only that editor. Use its
  shortcut, toolbar button or View-menu checkbox to bring it back.
- `Maximize editor` fills the workspace with the active editor. Navigation while
  maximized switches that editor; `Restore windows` restores the floating layout.
- `Arrange windows` restores the four-window layout; `Cascade windows` provides a
  stacked alternative. Browser and Inspector remain available beside the workspace.
- Each editor has its own toolbar context. Playlist snap/tools cannot accidentally
  become Piano snap/tools just because Piano owns keyboard focus.
- The Channel Rack has bounded horizontal/vertical scrolling for small windows;
  Piano controls wrap, and Mixer strips/effect slots retain their own scrolling.

## Persistence and editing safety

A versioned application preference stores only window visibility, workspace-relative
position/size, stacking order, active editor and maximized state. No project/media
paths or musical data are added to layout preferences. Missing, unsupported or
invalid preferences restore usable defaults. Geometry is bounded to the available
workspace after viewport or side-panel changes. Layout changes don't dirty the
musical project or add undo entries.

There is one shortcut dispatch after all windows render. Supported editor-specific
commands such as Delete, Ctrl+D, tool selection and grouping target only the active,
visible editor. Undo/Redo operate on the shared project. Switching, hiding or
arranging editors commits the preceding edit boundary so quick cross-editor edits
remain separate. Piano Delete/Duplicate are explicit undo transactions.

Text/numeric fields retain keyboard ownership. Canvas Ctrl+A and Cut/Copy/Paste are
still unimplemented, as before this feature; this change does not claim editor
clipboard support. Ordinary egui text-field selection/cut/copy/paste remains local
to that text field.

Blocking dialogs and save/project/device transitions disable all editor windows.
Interrupted clip/note/control drags are canceled until pointer release rather than
continuing in a newly focused editor. Dialogs and plug-in windows retain their own
pointer interactions. Existing native plug-in editor, state-save, import and
project-transition barriers remain in effect. Musical editor viewport positions are
preserved across egui's temporary window-sizing passes.

## Verification

The production display-independent harness operates real widget bounds using
pointer and key events. Added regressions cover:

- all four simultaneous editor windows and stable IDs;
- actual title-bar dragging, edge resizing, close/reopen, maximize/restore;
- first-press resizing and pan-knob dragging in an unfocused Mixer;
- saved geometry/focus/stacking round-trip, default migration and offscreen recovery;
- actual z-order after maximize → editor switch → restore and after Arrange;
- correctly routed Delete/Ctrl+D, shared Undo, independent Rack/Mixer edit boundaries;
- real Rack step edits and simultaneous Piano target-channel presentation;
- text-field isolation and unchanged unsupported canvas clipboard chords;
- Playlist/Piano body and note-edge drags interrupted by editor switches or dialogs;
- blocked background clicks leaving Settings above the workspace, while Settings
  itself remains draggable;
- hiding all editors without allowing hidden selections to receive editing shortcuts.

Optional capture mode renders the real egui paint data through display-independent
Vulkan/WGPU. Four new checkpoints show the default multiwindow workspace, moved and
resized windows, a genuine shared pattern edit, and the minimum app viewport. See
[HEADLESS_UI_QA.md](HEADLESS_UI_QA.md) for the reproducible harness and capture commands.

These checks do not establish detached native OS-window behavior, native desktop
window-manager integration, physical audio/MIDI hardware, or plug-in editor visual
acceptance. Those boundaries are unchanged.

### Verified isolated candidate, 2026-10-09

- Linux locked/offline all-feature/all-target: **954 app + 14 helper + 5 protocol tests passed**.
- Linux locked/offline no-default/all-target: **952 tests passed**.
- Formatting, strict all-target Clippy in both feature configurations, and all-feature
  app/helper builds passed. The existing vendor dependency deprecation warning is
  unchanged.
- Windows MSVC no-default/all-target cross-check passed; this is type-checking,
  not Windows execution or a Windows binary delivery.
- The final Vulkan capture run passed **20 real app UI flows**, including eight
  floating-workspace flows, and produced **27 genuine offscreen checkpoints**.
  The four new multiwindow images include 1997×1123 normal-size and 1123×707
  minimum-size output. PNG conversion was verified RGB-identical to PPM readback.
- Independent source review checked interaction ownership and the native safety
  boundaries. Its discovered drag/modal/stacking issues were fixed and covered by
  the final input regressions.
