# Piano Roll mouse composition

This bounded slice implements the everyday note-entry gestures described by the
[Image-Line Piano Roll manual](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/pianoroll.htm),
checked 2026-10-09. It does not claim full FL Studio parity.

## Composition gestures

- Draw: press empty grid space to create one note; drag to reposition it before
  release. Paint remains the separate multi-note drag tool.
- Ctrl-click selects a note/group. Ctrl-drag on empty grid selects a rectangle
  without changing the current tool. Ctrl-click empty space clears selection.
- Ctrl+Shift-click toggles a note/group; Ctrl+Shift-marquee adds to selection.
  Select-tool Shift-click toggles and Shift-marquee adds.
- Hold Shift before dragging a note body to clone the selected phrase. A normal
  click or sub-threshold pointer jitter does not clone.
- Press a note first, then hold Shift to lock pitch; press first, then hold Ctrl
  to lock time. The modifier order is captured on pointer-down.
- Shift-drag empty space with Draw to set one note's length.
- Touching a note sets the next Draw/Paint/Stamp length. Group resizing remembers
  the touched anchor's resulting length, rather than the last group member.
- Alt bypasses time snapping from the initial Draw position, during movement and
  resizing, and for Stamp placement and its preview.

## Deliberate Citrus semantics and limits

The input layer exposes generic Shift, so both Shift keys use the behavior above.
FL's distinct right-Shift stretch/compress behavior is not implemented. Applying
Alt to Stamp time is a consistent Citrus policy, not a separately verified FL
Stamp command. Existing Paint overlap behavior is preserved; this is not a claim
of exact FL monophonic Paint semantics (the manual's Paint descriptions differ).

A marquee intersects visible note rectangles and then expands their active group;
it selects only the current target channel. A move/clone similarly excludes ghost
channel notes even if a previous clipboard operation selected them. Newly cloned
notes preserve channel, velocity, mute, length and within-group offsets, with
project-wide fresh note IDs and separate group IDs. A singleton cloned group is
ungrouped. Existing scale snapping remains per note, which can change intervals.

Draw-length gestures keep their starting pitch/time and clamp backward drags to
the minimum length. Moves retain the grab offset and clamp the entire phrase
against beat zero, beat 4096 and MIDI pitches 0–127. Shift-cloning requires a real
move beyond the input click threshold and a nonzero effective displacement.
No double-click bypasses the chosen tool to create an unexpected note.

Each new Draw/move/clone gesture owns a pre-press project snapshot and commits one
Undo step on release. Selection alone does not dirty the project. Focus, modal,
window geometry, hidden editors, changed target/pattern and blocked snapshot
transitions terminate the gesture; already previewed edits retain the existing
workspace's one-step commit-on-interruption behavior and cannot resume while the
interrupted pointer remains held. Undo removes that step. Save/import/session
barriers prevent starting note changes, including clicks that do not become drags.

## Verification boundary

`src/app/piano_mouse_tests.rs` drives the real production app through pointer and
modifier sequences, rather than a second input model. Optional capture uses the
existing display-independent Vulkan renderer. These tests and images do not
establish native Windows desktop focus, physical mouse/keyboard drivers, OS MIDI,
audio device output or third-party plug-in editor acceptance.

Undo and Redo wait until an active project gesture, or its interrupted held
pointer, has released. They neither pop older history nor implicitly complete a
new pointer edit. Ordinary Undo/Redo menu clicks remain available after release.
Host-window focus-loss events also end read-only marquee gestures so releasing
Ctrl while away cannot turn an old selection press into Paint on return.
Legacy notes extending beyond the interactive 4096-beat horizon are preserved
when a common group resize cannot obey that horizon; a limit must never turn
another group member's positive length negative.

Input batching is tested explicitly: modifier ownership comes from the actual
primary-down event, and complete press/move/release batches recover the press
point only within the enabled, frontmost widget's clipped bounds. Note bodies
and edge grips take priority over the grid. Paint includes the initial press
cell even when the next pointer move arrives in that same frame.

## Isolated source validation, 2026-10-09

- Linux no-default-features/all-targets: 1,011 application tests passed.
- Linux all-features/all-targets: 1,013 application, 14 helper and 5 protocol tests
  passed; zero failed or ignored in either profile.
- Formatting, both strict all-target Clippy profiles and all-feature/all-bin build
  passed. The inherited vendored host warning remains dependency-only.
- All 22 new production-pointer flows also passed serially with explicit Vulkan
  capture. `piano-mouse-cloned-phrase` is 1997 × 1123 pixels;
  `piano-mouse-minimum-drawn-length` is 1123 × 707 pixels from a 1080 × 680
  logical viewport. RGB-identical PNG conversion and pixel inspection confirm the
  actual selected notes, length preview result, velocity lane and controls remain
  visible in simultaneous-window and minimum-size layouts.

The capture adapter was the installed SwiftShader CPU Vulkan implementation,
without a display surface or native window. Fresh combined-tree and Windows
checks remain the integration stage; none of these isolated Linux results claim
Windows/native-editor or physical audio/MIDI acceptance.
