# Piano note velocity and properties

This is a bounded clean-room editing slice, not full FL Studio parity. The
[official Image-Line Piano Roll manual](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/pianoroll.htm),
checked 2026-10-09, documents Alt/Opt+wheel over a note or a selected note to edit
velocity, Ctrl+Alt/Opt+wheel for finer adjustment, and double-click for properties.
It also disables timing fields when the properties dialog targets multiple notes.

## Velocity wheel

- Hover an active-Channel note and use Alt+wheel. Positive vertical wheel units
  raise velocity; Ctrl+Alt+wheel gives finer adjustment. Shift or macOS Command
  prevents this binding. Plain wheel retains navigation.
- An unselected note targets itself and its active group. Hovering a selected
  note targets the active-Channel selection and active groups. Ghost notes,
  unrelated selected Channels and off-note empty space are not edit targets.
- All targets receive one relative delta. The whole set stops when its quietest
  note reaches 0 or its loudest reaches 1, preserving dynamic differences rather
  than flattening individual values at the bounds.
- Citrus defines coarse/fine steps as 0.05/0.01 per line unit. Forty point units
  equal one line unit; a page unit is treated as one line unit. Each raw event is
  bounded to eight units in either direction. These numeric conventions are
  Citrus policies, not asserted FL Studio implementation details.
- Only finite raw Move events create edits. Start/End/Cancel and egui's synthesized
  smooth-scroll frames do not create edits. After a velocity wheel, navigation
  temporarily uses only fresh raw input until the old scroll tail drains, so
  moving into the keyboard gutter or immediately reversing a plain wheel cannot
  replay old motion. Platforms can deliver kinetic motion as ordinary raw Move
  events; egui exposes no separate momentum flag, so those events remain subject
  to the same bounded raw-event policy.
- Each effective raw event is immediately committed as one Undo step. A burst is
  deliberately multiple steps; saturation/no-op events create none. There is no
  deferred project snapshot or timer that can overwrite a later save/import.

## One properties editor

Double-click a note body or its resize grip, including a tiny zoomed-out note.
The Inspector's **Edit note properties** button opens the same editor. Draw,
Paint, Select and Stamp retain first-press ownership; Ctrl/Shift/Alt presses do
not become an unmodified properties click when modifiers are released later.
A resize must cross the click threshold before it changes a note, so touching a
short imported note does not silently lengthen it.

- One note: MIDI pitch, start and length in beats, velocity, mute and destination
  Channel. Existing scale-snap policy applies when changing its pitch/start.
- Several notes: explicitly relative semitone transpose and velocity change.
  Both use a common boundary-limited delta. Start and length are unavailable;
  mixed mute values stay mixed unless explicitly changed. Multi-note transpose
  preserves pitch intervals and does not independently re-snap each note.
- Channel choices use existing project Channels or Unassigned. Reassignment
  normalizes affected groups. Stable note IDs are never reallocated.
- Fields are a private draft. Reset restores the opening values; Cancel/Escape
  discards them. Apply validates current target identities/state and commits at
  most one Undo step. Enter/Escape inside a numeric field belongs to that field
  first. Invalid input remains open with an error, ready to fix.
- Merely opening the editor preserves imported sub-minimum lengths and legacy
  out-of-horizon timing. Velocity-only edits do not rewrite those timing fields.
  New timing changes use the same 1/64-beat minimum as Draw and resize, and must
  fit the interactive 0–4096-beat horizon. Start-only edits preserve valid short
  lengths, including 1/24-beat notes; the final legal minimum-length note starts
  at 4095.984375 beats. End validation checks the exact stored start and length
  without allowing f32 addition to conceal an overshoot.
- Properties never replace the Project with an opening snapshot. Unrelated newer
  project data survives; changed target notes or stale project/pattern/Channel
  identity fail closed. The opening target set remains frozen while the dialog
  is visible, even if a programmatic selection changes.
- Clipboard/global editor shortcuts, background gestures, queued audio import
  completion and native-editor opening honor the modal/snapshot boundaries.
  Saves serialize committed notes, never the visible draft.

Only fields backed by the existing note model are exposed. There are no invented
pan, pressure, release, probability, timbre, MPE or per-note audio controls. This
slice does not add new synthesis behavior or change the project format.

## Verification

The short-timing properties correction has source regressions for real numeric
Start-only edits, typed 1/64- and 1/24-beat lengths, the final legal start, grouped
Undo/Redo and invalid bounds. This source checkpoint is **unvalidated** until its
own test results are recorded; the historical results below do not validate it.

`src/app/piano_expression_tests.rs` operates the production app through actual
egui pointer, key, text and wheel input. Pure candidate regressions cover invalid
numbers, duplicate global identities, stale sessions and the full 65,536-note
selection. Optional images use the existing real egui/WGPU Vulkan offscreen path.

The checks cover relative dynamics and bounds, per-event and single-Apply Undo,
small resize-only notes, Draw/Paint/Stamp repetition, press-time modifiers,
numeric focus, modal/save/import interruption, frozen scope, unrelated edits,
save/reopen, minimum layout, and wheel-tail reversal. These are Linux app/input
regressions and offscreen renders, not native desktop, Windows execution, physical
input/audio/MIDI or vendor plug-in acceptance.

## Isolated-source validation, 2026-10-09

- Linux no-default-features/all-targets: **1,044 application tests passed**.
- Linux all-features/all-targets: **1,046 application + 14 helper + 5 protocol
  tests passed**, with no failures or ignored tests.
- Formatting, both strict all-target Clippy profiles, all-feature/all-bin build,
  and the real helper protocol smoke pass. The inherited vendored host's
  deprecation warning remains dependency-only.
- Windows MSVC no-default-features/all-target source cross-check passes. This is
  not Windows execution, a Windows package, or native device verification.
- All **18 expression checks** (15 production-input flows and 3 candidate
  regressions) also pass in explicit capture mode. The copied core test binary
  was byte-verified before capture.
- Three genuine SwiftShader CPU Vulkan renders were produced without a display
  surface: `piano-properties-single-draft` and `piano-properties-group-relative`
  at 1997 × 1123 pixels, plus `piano-properties-minimum` at 1123 × 707 pixels.
  RGB-identical PNG conversion and pixel inspection confirm legible fields,
  explicit relative/mixed state, visible Apply/Cancel/Reset, and no clipped dialog
  at the 1080 × 680 logical minimum.

The complete core/all-feature suites include the preceding keyboard and mouse
flows; these are aggregate executed results, not summed isolated counts. The
new branch has not established fresh Windows runtime or native plug-in paint
acceptance. Existing platform/hardware limits remain unchanged.


## Verified integrated source checkpoint

Reviewed `a26a7d7e798ed0aa3752e2c56eaae5abfd78ad5e` integrates as
`9a4d37b0f30f7aff568aa4b66f7b7e2227cfd91e`. The entire runtime source, Cargo inputs
and vendored library match the reviewed feature; only additive documentation
reconciliation was needed. Fresh complete Linux gates pass **1,046 application
+14 helper +5 protocol all-feature tests** and **1,044 no-default application tests**,
with zero failed/ignored on default stack. Formatting, both strict Clippy profiles,
all-bin build, actual ordinary helper smoke, Windows source cross-check and all
**167 Python tests** pass.

The fresh complete UI run passes **79 entries**: 78 production-input flows plus the
opt-in benchmark entry (timing disabled). The three pure expression candidate
regressions are included in the full Rust gates, not misreported as UI flows.
It produces **40 genuine Vulkan frames**; every PNG is RGB-identical. All three
single/group/minimum properties captures were inspected, including readable fields
and Apply/Cancel/Reset. This combines preceding keyboard/mouse/clipboard/workspace
flows with the 15 new expression input flows. Native desktop, OS input/clipboard,
hardware and real-vendor acceptance remain separate; current Windows execution
must be established for the published checkpoint.
