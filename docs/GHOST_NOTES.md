# Piano Roll ghost references

## Controls and scope

The always-visible Piano Roll toolbar has **GHOST NOTES** (also **Alt+V**) and a
source dropdown. The toggle defaults to on; hiding ghosts preserves the chosen
source. The source dropdown remains usable while ghosts are hidden.

- **Current Pattern** (default): notes belonging to other Channels, including
  unassigned notes when the target is an assigned Channel.
- **Ref #ID: name**: one reference Pattern, all Channels, including the target
  Channel. IDs disambiguate duplicate names. The list contains full names, wraps
  long names, scrolls through large lists, and the closed control truncates.
- A scope label inside the source menu identifies other/all Channels and read-only
  status. The tooltip explains alignment and exclusions. The toolbar keeps only
  the toggle and a fixed-width source control, so visibility changes do not move
  the grid or consume an extra row of permanent explanation.

Reference **Pattern-local beat 0 aligns with current Pattern-local beat 0**.
Notes are shown once at their stored beat and length, without looping, repetition,
Playlist clip start/offset, or arrangement alignment. This is intentionally a
composition reference, not a preview of overlapping Playlist clips.

Ghosts are cool, translucent, outlined rectangles. Editable notes retain their
solid Channel color and paint above ghosts. Ghosts register no egui interactions,
resize handles, velocity handles, or selection IDs. Clicking empty canvas beneath
one still uses the current tool on the active Pattern; it never changes the
reference. Selecting all, dragging, deleting, transformations and clipboard
commands continue to operate only on editable notes.

## State and edge cases

Source and visibility live only in PianoRollState. They do not alter the project,
audio, exported notes, project dirty flag, Undo/Redo, or persisted preferences.
The default source is restored on a new app session and when installing a New/Open
project, so reused Pattern IDs cannot carry a reference across projects.

Pattern IDs preserve the selection through rename and reorder. Removing the
chosen Pattern falls back to Current Pattern on the next render. Selecting the
currently active Pattern as reference (or navigating to the reference) shows only
its other Channels, preventing duplicate outlines over editable notes; the UI
scope label reflects that. Navigating away restores all-Channel reference scope.
An empty source displays no ghosts. Invalid/non-finite note geometry is skipped.
Only notes visible within the current viewport are painted. A visible reference
extends the horizontal visual scroll horizon far enough to reach its tail. Hiding
ghosts or deleting the source restores the active Pattern horizon and clamps the
viewport safely. Fit Notes still reveals only editable target-Channel notes; it
does not include the reference. Ghosts never change the active Pattern's length
or playback duration.

## Verification

Pure logic regressions in `src/piano_roll.rs::ghost_reference_tests` cover current
scope, hidden state, all-Channel references, exact local beats, immutable source
identity, reorder, rename, source deletion, reference=current, invalid notes and
session-only state.

Production egui input regressions in `src/app/piano_ghost_tests.rs` cover:

- Pointer-opening the source dropdown and choosing another Pattern
- Pointer toggling ghost visibility; unchanged project fingerprint and history
- Alt+V twice and Select All, excluding reference IDs
- Renaming, switching to the reference, and deleting it
- A reference at beat 257 remains scroll-reachable without changing playback,
  project fingerprint, selection, or history; hide/delete safely shrinks the view,
  and target-only Fit Notes still resolves the original editable note
- Installing another project that reuses the old Pattern ID
- Delete-tool click over a reference-only note without modifying either Pattern
- Visible, non-overlapping source and toggle controls in a 580px floating editor
  at outer screen widths 1080px and 1440px, including menu interaction

Run: `cargo test --no-default-features --bin citrus-studio ghost_`

## Deliberately not included

No Playlist-time alignment, multiple simultaneous reference Patterns, per-Channel
reference filtering, reference editing/audition, ghost velocity lane, reference
looping, import/export changes, or saved project/preferences schema changes.
