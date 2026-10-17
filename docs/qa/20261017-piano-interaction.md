# Piano-roll note interaction QA — 2026-10-17

Baseline: `314779d`.

## Defects found in baseline

- At the default 60 px/beat, a 0.125-beat note is painted 5.5 px wide. Its resize target covered the entire visible note body, making a body drag resize instead of move.
- The end handle ignored secondary-button deletion.
- Delete, Mute and Slice on the end handle could start a resize instead of applying the selected tool.
- Delete dragging only worked when the grid owned the initial press; starting on a note did not use the same erase gesture.

## Changes

The body retains 65% of short painted notes; the resize grip uses at most 35% or 12 px and keeps its existing 3 px extension beyond the end. There is no positive-area overlap between these targets. Egui registration and the batched-input ownership prepass use identical geometry.

Both body and end handle support secondary-button delete. End-handle primary input respects Delete/Mute/Slice; resize remains available with Draw/Paint/Select/Stamp. Delete uses the existing focus/session/viewport-safe gesture lifecycle and one history transaction, whether pressed on a note or blank grid. Ordered pointer events cover both multi-frame and single-frame batched input. Repeated stationary events do not erase another overlapping note. Both erase and batched press resolution reject notes culled by the renderer, so the minimum-width hitbox cannot make invisible offscreen notes interactive.

## Regression coverage

Actual `UiHarness` egui pointer events, including down/move/up in one frame:

- Short (0.125-beat) and ordinary (0.75-beat) notes: independent body movement and end resizing, unchanged other dimension, no duplicate creation, one-step Undo.
- Secondary delete from both body and handle at both lengths.
- Delete/Mute/Slice handle actions with Undo.
- Short-note body drag interrupted by window focus loss; no resumed mutation.
- Erase drag beginning on a short note or blank grid, multi-frame and batched; one-step Undo.
- Minimum zoom (1 px/beat): a hidden short note at beat 126 cannot steal Draw or be deleted at the left edge of a viewport beginning at beat 128. Multi-frame and batched paths are covered.
- Existing overlap, modifier, grouped-move, modal/geometry/focus and Undo/Redo tests remain included in the `piano_mouse` filter.

Validation: all 29 tests in the final `piano_mouse` suite passed (no-default-features, Linux cloud, 81.08 s). This run includes the visibility correction and all added regressions. Log: `daw-tooling-20261017/logs/piano-mouse-final.log` in the adjacent restored toolchain workspace. The initial 28-test suite and the targeted minimum-zoom test also passed independently. `git diff --check` is clean. No Windows desktop, audio-driver, or third-party plugin validation is claimed.
