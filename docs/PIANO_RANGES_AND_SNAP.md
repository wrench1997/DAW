# Piano edit ranges and local snap

This is a bounded clean-room melody-editing slice. The ruler's orange highlight is
an **edit/repeat range**, not a playback loop. Transport, audio loops and the
independent Playlist snap are unchanged.

## Range commands

- Ctrl/Cmd-drag, or unmodified double-click-and-drag, on the Piano ruler creates
  a normalized range using the local snap.
  Alt bypasses time snapping; Ctrl/Cmd+Shift-drag adds to the existing selection.
  The modifiers on pointer-down own the whole gesture. A click without a drag,
  including Ctrl/Cmd-click or a stationary double click, leaves the previous range
  alone. Double-drag uses egui's click time/distance thresholds and only a prior
  unmodified click on this same ruler, owner and viewport can arm it.
- Creating a range selects active-TARGET Channel notes whose **start** lies in
  `[start, end)`, and expands their groups when grouping is enabled. Notes merely
  overlapping its left edge are excluded; a note starting at its right edge is
  excluded. The comparisons use the note model's f32-representable endpoints.
  This boundary rule is an explicit Citrus policy, not documented FL behavior.
- Ctrl/Cmd+Enter or **Range from selection** uses the exact selected active-Channel
  note/group extent. Empty selection is a no-op. Invalid or out-of-horizon notes
  cannot create an unsafe range.
- Ctrl/Cmd+Left/Right or **Range left/right** moves just the range by its width,
  clamped as a whole at 0 and 4096 beats. Notes stay where they are. Range movement
  does not select a new set of notes. This independence and boundary clamping are
  explicit Citrus choices.
- **Clear range** removes the interval and preserves note selection. Ctrl/Cmd+D
  retains its existing note-deselect behavior and preserves the range.

Range and note selection are independent after creation. Ctrl/Cmd+B copies the
selected active-Channel notes/groups by the range's exact width, including notes
outside or spanning the interval. Copies may overlap existing notes; no notes are
trimmed, deleted, or replaced. With no active-Channel selection, all notes in that
Channel repeat. This fallback also applies after drawing an empty range. The
ruler tooltip explains it. Without a range, the previous selected-extent spacing
remains, with at least one keyboard step and no invented FL bar/gap rounding.

A repeat selects the new copies and leaves the range fixed. Note IDs are fresh
across the Project; copied groups get fresh identities in the target Pattern.
Pitch, Channel, relative start/length, velocity and mute are preserved. Bounds,
identity or allocation failures reject the complete edit. Each effective repeat
is one Undo; range changes themselves never dirty the Project or add history.
The note model stores f32 starts: repeated fractional-range copies can accumulate
bounded f32 rounding error, especially near beat 4096. Copies deliberately retain
arbitrary off-grid phrase phase rather than being snapped onto a grid. Canonical
keyboard nudges use indexed targets and therefore do not share that accumulation;
this slice adds no persistent high-precision phase ledger or note-schema change.

Range metadata belongs to the current Project session, Pattern index and stable
Pattern ID plus the TARGET Channel ID. New/Open/replacement or changing Pattern
or TARGET Channel clears it. It is not saved
inside a Project, preferences, or Undo. A held ruler gesture cancels and restores its
opening range/selection on focus, modal, hidden editor, Channel, owner, viewport
or geometry interruption. It cannot resume until the pointer is released.

## Narrow Piano windows

Below 640 logical pixels of editor width, **NOTE EDIT** contains the clipboard,
range and selected-note Channel commands; **SCALE / CHORD** contains those setup
controls. All actions retain their keyboard shortcuts. TARGET, local snap and a
short range/paste summary remain visible. This preserves the 480×420 minimum and
the user's zoom while keeping the note canvas and velocity lane usable. Opening
menus retains the same focus, snapshot and held-pointer barriers; explicit menu
commands use their own allowed popup path.

## Paste anchor

Paste starts at the four-beat bar containing the left edge of the Piano viewport,
shown as **Paste beat …**. It is identical in Pattern and Song modes and ignores
the playhead and repeat range. Repeated paste uses that same visible anchor.
The Project model currently has no variable time-signature field; the Piano
ruler and this anchor use four-beat bars. Floor-to-containing-bar is Citrus's
explicit interpretation of the manual's “leftmost visible bar,” whose partial-bar
rounding is unspecified. Existing clipboard identity, focus and Undo guards remain.

## Local time snap

Piano-local snap now includes Off; 1/24, 1/16, 1/12, 1/8, 1/6, 1/4, 1/3 and 1/2
beat; one beat; two beats; and a four-beat bar. Triplet choices are labeled.
These are fixed beat grids; Main/Line/Cell and zoom-dependent editing are not
implemented. Scale snapping remains independent; turning time snap Off does not
turn off pitch-scale snapping. Alt retains its existing bypass behavior.

- Draw/Paint/Stamp placement floors to the preceding grid point. Movement,
  resize, ruler endpoints and quick quantize round to the nearest point; halfway
  ties round away from zero. These arithmetic rules are Citrus policy. Canonical
  on-grid keyboard nudges and Chop/Arpeggiate use indexed rational positions,
  avoiding accumulated triplet drift. Off-grid keyboard phrases retain their
  current f32 phase and common relative offset rather than being quantized.
- Off uses raw pointer timing. Note lengths still have a positive 1/64-beat
  interactive minimum. Keyboard Shift+Left/Right and Shift+D use an explicit
  1/64-beat fallback. Quick quantize with Off leaves notes unchanged and displays
  an explanation; the full Quantize dialog uses an explicit positive 1/64-beat
  initial grid that can be changed. No action divides by an Off/zero step.
- Quick quantize remains Piano-local. The Image-Line manual describes global
  snap for its quick commands; Citrus does not claim parity for that scope.
- Grid rendering uses rational f64 lattice positions, only the visible range,
  a four-pixel minimum spacing and a hard 2,048-line cap. At low zoom, it skips
  integer multiples of the chosen grid without changing actual snapping.
  Triplets never display quarter-beat subdivisions as fake triplet targets.
  Off/coarse modes retain dimmer beat reference lines and bar lines.
- The enum setting persists in Piano preferences, independently of Project and
  Playlist snap. Existing version-1 preferences without this field default to
  the previous quarter-beat snap. Unknown/invalid settings follow the existing
  safe reset-and-notify path; Project schema remains unchanged.

## Sources and verification boundary

Image-Line's official [Piano Roll](https://www.image-line.com/fl-studio-learning-content/fl-studio-online-manual/html/pianoroll.htm)
manual documents Ctrl+drag ruler selection and range-width Ctrl+B, including
selected notes outside the range and the all-notes fallback. Its
[Piano Roll menu](https://www.image-line.com/fl-studio-learning-content/fl-studio-online-manual/html/pianoroll_menu.htm)
documents range shortcuts and leftmost-visible-bar paste. Checked 2026-10-09.
Full FL time-selection playback behavior and undocumented boundary/rounding
behavior are outside this slice. Clear range
is an explicit Citrus control; the Piano manual's general deselect wording does
not establish a Piano ruler-clear gesture.

The production egui tests drive actual ruler, note, menu and keyboard input with
existing modal/history/clipboard/window ownership. Pure tests cover precision,
settings migration, view bounds and grid caps. Optional captures use the genuine
application egui/WGPU Vulkan readback path. These do not establish native Windows
execution, OS focus/input, physical MIDI/audio or plug-in acceptance.

## Isolated-source validation, 2026-10-09

- Final Linux no-default/all-target gates: **1,092 application tests passed**.
- Final all-feature/all-target gates: **1,094 application +14 helper +5 protocol
  tests passed**, with zero failed or ignored tests in either configuration.
- Formatting, both strict all-target Clippy modes, all-feature app/helper build,
  real ordinary helper smoke and Windows MSVC no-default/all-target source
  cross-check pass. The unchanged vendored dependency warning remains isolated.
- All **167 Python tests** and the actual **29-file** packaged document/provenance
  relative-link closure pass, including the expression and range guides.
- The complete production-app capture suite passes **105 entries**: 104 input/flow
  checks and the included opt-in benchmark entry, with timing mode disabled. It
  yields **44 genuine Vulkan frames**, all PNGs RGB-identical to their readbacks.
  The post-lint rebuilt core test binary is byte-identical to the captured binary.
- Populated wide/triplet, maximized-small and actual floating **480×420** layouts
  were inspected. The floating minimum preserves ordinary 24-pixel pitch zoom,
  about 7.5 pitch rows, two active melody notes and their distinct velocity handles.
  Its NOTE EDIT popup remains readable and the actual Clear/Select/Range/Copy/Paste,
  Undo and Scale-menu flows pass. Independent read-only source/pixel review found
  no remaining blocker after the precision, lifecycle and compact-layout fixes.

This is Linux source, production-input and display-independent rendering evidence.
It is not Windows execution, a native desktop or physical input/audio/MIDI test,
a performance benchmark, or proof of native third-party plug-in acceptance.
