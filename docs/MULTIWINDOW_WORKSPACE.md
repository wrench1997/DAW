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

Text/numeric fields retain keyboard ownership. Piano now supports the bounded
Ctrl+A and Cut/Copy/Paste note workflow below. Playlist, Channel Rack and Mixer
canvas clipboard commands remain unsupported. Ordinary egui text-field
selection/cut/copy/paste remains local to that text field.

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
- text-field isolation and unchanged unsupported non-Piano canvas clipboard chords;
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

### Verified integrated source

Feature `8b9f2c7` integrates as `bffc6f48f47dfe241809a852951bb393ab3ea402` without
runtime conflicts; additive documentation histories were both retained. The entire
integrated src tree matches the independently reviewed feature. A fresh full main
rerun passed 954 app + 14 helper + 5 protocol all-feature tests, 952 no-default tests,
fmt, both strict Clippy modes, app/helper build and ordinary helper smoke. The fresh
combined UI capture rerun passed 20 actual-app tests and produced 27 genuine Vulkan
frames, converted losslessly to RGB-identical PNGs. These are integrated-source
results; Windows runtime and detached-OS-window claims are not inferred.


## Piano note clipboard

The focused, visible Piano editor supports Ctrl+A / Ctrl+C / Ctrl+X / Ctrl+V
(and the platform command modifier), plus explicit Select all notes, Copy notes,
Cut notes and Paste notes buttons. Select all selects only the current TARGET
channel, consistent with marquee selection; ghost notes are excluded. Existing
selected note groups expand when grouping is enabled. Copy/Cut with no selected
notes leaves both clipboards and the Project untouched.

Copy/Cut emits one versioned `CITRUS-NOTES/1` JSON text payload to the platform
clipboard, only when explicitly requested. It contains musical note data and an
app-instance/project-session compatibility identity, never media paths, samples,
plug-ins or arbitrary Project data. A typed copy is also held in memory. Ctrl+V
uses the incoming platform text; unrelated/malformed text is refused rather than
silently falling back to the previous local copy. The Paste notes button uses
that last local copy even if the OS clipboard subsequently changes. This is
Citrus session-local note editing, not cross-DAW MIDI or cross-project exchange.
App/project replacement invalidates the local copy and old text payloads. The
identity is a compatibility boundary, not cryptographic protection. Ordinary
text fields can still copy/paste text normally, including viewing the JSON.

Paste inserts into the active Pattern and preserves source channel assignment,
absolute pitches, relative time offsets, lengths, velocities and mute state.
Changing TARGET does not remap the copied channel. Notes retain their original
scale/quantization details: only the earliest-note anchor is snapped. In PAT mode,
that anchor is the current Pattern transport cursor snapped down to the Piano
snap grid. In SONG mode it is Pattern beat 0, since this editor has no independent
local ruler cursor and an absolute song beat would be ambiguous. The toolbar shows
the zero-based paste beat. Repeated paste at an unchanged cursor uses the same
anchor, deliberately overlapping; it neither advances the cursor nor transposes.
During playback, later paste uses the then-current PAT cursor.

Every paste allocates fresh project-wide note IDs and independent pattern-local
group IDs. Groups with fewer than two copied members become ungrouped. Cut/Paste
each commit one explicit undo transaction; an earlier pending edit stays separate.
Undo/Redo restores musical data, while selections and the clipboard remain
session UI state. Copy/select-all adds no history entry and does not dirty the
Project. Neither paste nor cut silently clamps or skips invalid notes.

The text-size cap is 16 MiB before JSON parsing; at most 65,536 notes may be copied,
and a pasted Pattern may contain at most that many notes. Nonfinite values,
out-of-range pitches/velocities, results outside the 0–4096-beat range, missing or
ambiguous channel identities and invalid/duplicate destination note identities
are rejected before mutation. Pointer gestures, interrupted drags, modal dialogs,
save/recording/device barriers and deferred whole-Project replacements block
clipboard actions. Text/numeric editors keep their own ordinary clipboard events.

The integration handles egui's semantic Copy/Cut/Paste events, which the native
winit adapter emits instead of raw C/X/V presses. An accompanying raw key in the
same input batch does not cause a second operation. Raw-key autorepeat is ignored;
separate semantic Paste events are separate intentional operations because those
events carry no repeat flag. Native OS clipboard round-trip acceptance remains a
separate check: the automated harness verifies the actual platform CopyText output
and real semantic event route without pretending it exercised an OS clipboard.


### Verified isolated clipboard slice, 2026-10-09

All locked/offline source gates passed: 966 no-default application tests;
968 application + 14 helper + 5 protocol all-feature tests; formatting;
both strict all-target Clippy configurations; app/helper build; Windows MSVC
no-default/all-target cross-check; and 167 Python tests. Fourteen added regressions
cover the clipboard data and ten actual-app input flows. The final Vulkan suite
passed all 30 UI flows and produced 30 genuine frames, including three new
clipboard captures at normal/minimum sizes, all losslessly verified after PNG
conversion. Independent source review's quadratic-selection finding was fixed with
a two-pass clipboard selection path, with a full-limit regression; channel checks
and fresh identity allocation also avoid repeated per-note scans. These results
remain separate from native OS clipboard and Windows runtime acceptance.

### Compact workspace refinement

[COMPACT_WORKSPACE.md](COMPACT_WORKSPACE.md) records the next native layout pass:
compact chrome/Rack controls, useful two-column defaults, backward-compatible
Inspector visibility, corrected reset geometry, and release-only edge alignment.
It includes continuous-pointer regressions and a same-scene baseline/refined CPU
comparison without inferring native-desktop smoothness from offscreen or footer FPS.
