# Direct automation editing in Draw — 2026-10-17

## Observed defects

A fresh project starts with Playlist Draw. `+ AUTOMATION` creates a selected
Master-volume clip, and `SHAPES → Sine` paints 33 nodes, but the previous canvas
registered point/tension input only in Select. The visible points therefore did
nothing in the ordinary creation workflow.

Enabling Draw alone exposed two additional actual-input failures: an entire
press/move/release batch, or a final motion combined with release, lost its edit;
a secondary-button point drag changed the curve. Installed egui's drag responses
do not report a whole same-frame press/release as a drag and omit final released
motion. The existing Piano editor separately handles coalesced pointer input.

## Implemented contract

- Selected automation points and eligible tension handles work in Draw and Select.
  Other tools retain their normal clip behavior. Unselected clips select first.
- Point hover exposes a grab cursor and editing/modifier/context-menu help. Tension
  exposes vertical-drag feedback. The clip header moves the clip; its header-edge
  corner resizes it. A curve endpoint's own hit target continues to edit that point.
- A primary press resolves one visible handle before native clip input. It respects
  UI enablement, canvas clipping, window layers, clip paint order and node priority.
  A held handle owns its gesture even when the pointer crosses another clip body,
  resize grip or Audio fade control. Right-click remains the point menu/tension reset.
- Pointer origin and the ordered release endpoint are retained independently of
  egui's drag threshold. A release followed by another press or multiple primary
  pairs in one frame cannot borrow another gesture's endpoint. Ambiguous new
  captures are rejected instead of manufacturing a curve edit.
- Retained state is bound to one handle owner, preventing a hidden, retired point
  or tension gesture from reviving after editor changes. Escape restores the
  original project and redo; Save and New interruptions settle once and retire the
  old pointer. Final history remains one explicit transaction per changed gesture.
- No persisted format, interpolation, automation DSP, routing, audio rendering,
  Inspector layout, or other tool policy is changed.

## Regression history

All tests drive production `CitrusApp::ui` through real egui pointer/key events;
model fixtures and project JSON are used only to establish baselines and inspect
exact results. These are display-independent input tests, not native desktop or
audio-device acceptance.

1. Original source: 1/9 passed, 8 failed because Draw exposed no interactive nodes.
2. Draw gate only: 6/9 passed. Coalesced input and secondary-drag tests reproduced
   production defects. The initial resize test targeted an endpoint's node hit
   region; it was corrected to the reserved header corner without weakening the
   clip geometry/history assertions.
3. Initial input fix: 9/9 new tests and 138 related automation tests passed.
4. Expanded ownership tests: 16/16 passed. A first dense-Sine fixture incorrectly
   assumed point 9 was the peak; it now finds the generated full-depth peak and
   requires exactly that point to change, with every neighbor unchanged.
5. Independent static review added cross-clip reservation, inactive-owner checks,
   ordered multi-event rejection and displaced native stop/click suppression.

6. The first prepass candidate passed all 12 cross-clip scenarios but failed
   three existing coalesced Undo checks because it synthesized a native drag
   after the same-frame release. Reservation now assigns native drag state only
   while the primary button is held. All completion paths additionally require
   no retained native drag before testing Undo/Redo.

## Final focused validation

- 18/18 new production-App tests passed with no default features on Linux.
- Strict no-default/all-target Clippy passed with `-D warnings`.
- `cargo fmt --all -- --check` and `git diff --check` passed.
- Both independent read-only reviews closed their specific ownership/lifecycle
  findings after checking the final implementation.

The 18 cases cover the exact generated Sine peak at 1 BAR, point and tension
mutation, normal and coalesced events, final-release motion, cross-clip body/resize/
Audio-fade endpoints in both paint orders, one-step exact Undo/Redo, context cancel,
blank-click preservation, insertion, secondary-drag preservation, multiple-click
batches, Escape/redo, Save-to-disk and New interruption, hidden retired owners,
offscreen hit rejection, multipass replay, other tools, and header/edge geometry.
The Audio-fade endpoint case uses clip metadata to test pointer ownership; it is
not an audio-media loading or DSP acceptance test.

The final-source related automation suite passed **147/147**, with zero failed or
ignored, from an immutable copy of the freshly compiled executable. It includes
these 18 cases plus existing point/tension modifiers, insertion, context history,
shape/source boundaries, deletion ownership, model evaluation and timeline/DSP
checks. Counts overlap; do not add 18 and 147 as unique coverage.

Full repository/all-feature and native desktop gates belong to integrated
acceptance; this note does not reattribute earlier full-suite results to this source.

## Reproduction and evidence

From the isolated tooling environment:

- `cargo test --offline --no-default-features --bin citrus-studio automation_draw_ui_tests -- --nocapture --test-threads=1`
- `cargo test --offline --no-default-features --bin citrus-studio automation_ -- --test-threads=1`
- `cargo clippy --offline --no-default-features --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

Native follow-up should repeat fresh Draw → + AUTOMATION → SHAPES → Sine,
1 BAR zoom, then drag the exact peak, inspect its right-click value, Undo and Redo.
Also verify the header/edge move/resize distinction and a fast drag crossing
another clip. Headless checks do not establish rendered desktop/device behavior.
