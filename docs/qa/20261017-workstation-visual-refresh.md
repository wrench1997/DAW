# Compact graphite workstation refresh — 2026-10-17

## Scope

This refresh changes the real native Rust/egui paint and layout, plus the plain-Space
input ownership defect exposed by real keyboard testing. It does not replace the DAW
with a website or mockup. Original vector icons remain clean-room;
no Image-Line artwork or brand assets are bundled.

- Flat graphite surfaces replace bevels, per-button dark borders, depressed text
  offsets, and heavy Inspector cards. Warm amber is reserved for selection and
  transport; musical channel/clip colors remain project-owned.
- Smaller Browser (176-point fresh default) and Inspector (232-point fresh default),
  narrower Playlist headers and Piano keyboard, and a shorter velocity lane return
  space to the score. Panels remain resizable and restore existing saved widths.
- The top-level `WINDOWS` menu contains Arrange, Cascade, Maximize/Restore, Hide,
  and editor visibility. The permanent workspace command strip is removed. The
  `VIEW` menu retains access to the same controls. Actions still run the shared
  interaction/history boundaries and now explicitly retain modal/gesture barriers.
- Transport/navigation stay directly visible. At narrow widths navigation takes
  its own deliberate row. Open/Save move to the menu row; nonfunctional Menu/More
  buttons are removed. F5/F6/F7/F9, editing tools, snap, and shared Undo remain.
- Inspector sections become compact typographic groups separated by a single rule;
  field/mutation behavior is unchanged. Generic Inspector slider width follows its
  available panel width. Gain/Fade support and business logic are not changed.
- Footer shows concise device/engine/plug-in state. Audio offline, XRUN/queue loss,
  errors, capture/recording, and save status remain visible; detailed counters and
  frame timing move to their existing hover descriptions.
- Native egui buttons underlie icon, navigation, transport and text controls. Focus
  gets a visible amber ring, without moving text or shrinking hit rectangles.

## Evidence added

`src/app/workstation_visual_refresh_ui_tests.rs` drives the actual App UI:

1. Draw an eight-note phrase with pointer input, repeat it with keyboard commands,
   verify exact Undo/Redo, use Fit Notes, and check wide/minimum-window bounds.
2. Open the actual Windows menu for every Arrange/Cascade/Maximize/Restore action,
   repeat the flow, verify project/history stability, and check modal disabled state.
3. Drag an actual automation point and undo it, navigate to Mixer, and repeat real
   editor navigation at 1×, 1.5× and 2× scaling with minimum-size hit/bounds checks.
4. Check actual toolbar PanelState containment, disjoint hit targets, and minimum
   24-point heights at 1080, 1179, 1180, 1240 and 1440 logical points.
5. Use real Tab input to focus Play, Stop, Record, Undo, Redo, Piano, metronome,
   Add Pattern and the native Follow Pattern checkbox; Space toggles transport
   once without activating the focused control, recording, or editing the project.
6. Verify native Enter, pointer and AccessKit Click activation. A real held key
   receives an initial press, repeat, release and a new press; only the two fresh
   presses toggle transport and the focused checkbox remains unchanged.
7. Select and close a Windows menu item with Space, edit a numeric TextEdit,
   combine pointer click with Space, and open a popup on the same input frame.
   Local input cannot leak into transport when its editor/popup closes.
8. Keep stale background Play focus from activating under Settings, while real
   Tab navigation to the Settings About button still allows its native Space action.
   Pointer-close of Settings plus Space does not start transport.

Existing floating-window, Piano range and keyboard-audition tests now open the real
Windows menu before the requested action. No helper directly invokes an App action.
A native icon-button Enter test and theme text/focus contrast regression are added.
Existing exhaustive clip-header contrast checks remain in place.

Optional production-render captures:

- `workstation-refresh-authored-melody`
- `workstation-refresh-melody-minimum`
- `workstation-refresh-multiwindow`
- `workstation-refresh-automation`
- `workstation-refresh-mixer`
- `workstation-refresh-mixer-high-dpi`

## Plain-Space ownership correction

The first real-App Tab test failed at focused Play: one Space produced both the
native fake click and global transport action. egui synthesizes Space/Enter clicks
for all focused click-sense widgets without consuming the key. This was not unique
to the newly adopted native Button: earlier custom click-sense controls shared the
underlying behavior.

The existing plain-Space global-transport policy is retained. Before drawing widgets,
accepted transport Space presses are removed from egui's event list and cached for
the existing end-of-UI shortcut dispatcher. They are never replayed to egui. This
covers native buttons and checkboxes as well as custom-painted controls. Repeats
are suppressed without generating another transport action; key releases, Text,
Enter, pointer events and accessibility requests remain native.

Frame-start ownership prevents a closing popup/text editor from releasing the same
Space into transport. Existing memory-backed menus/combos are checked through
persistent Popup memory as well as the current painted-popup set, including the live
post-UI guard and queued-save pause exception. The existing shortcut policy, action
priority, and live save-barrier checks still decide whether the cached action runs.

Settings is an ordinary Window, so stale Background-layer keyboard focus needs an
explicit plain-Space blocker while a shortcut modal is active. This does not clear
focus, disrupt Tab traversal, disable ordinary pointer input, or swallow Space from
a focused control inside the modal's own layer. This is a bounded plain-Space fix,
not a claim that every modal/Enter interaction has been redesigned.

## Verified evidence

The valid integrated build at `22442e9` compiled successfully before the run recorded
in `validation/strict-fix-refresh/validated-build-focused.log` (under the integration
tooling directory). All **12 workstation refresh regressions** passed, together
with the native icon activation test and theme text/focus contrast test. Rustfmt
and `git diff --check` also passed for this patch series.

The broader combined run at that checkpoint was **39 passed / 5 failed**: its
remaining failures were Inspector transaction/lifecycle tests outside this
visual/input patch. After their fixes were integrated, the freshly compiled
`final-focused.log` recorded **44/44 passing**. Full-feature suite, Clippy and final
integrated acceptance remain the integration owner's responsibility.

Preserved negative evidence:

- `space-focus-before.log`: focused Play plus one Space failed before the ownership
  correction; the real Tab regression subsequently passed for all listed controls.
- `space-modal-after.log`: stale Play focus under Settings still activated transport,
  while the positive Settings About-button Space test passed. The bounded Background
  owner correction made both cases pass without removing the positive assertion.
- The first integrated screenshot clipped the navigation row at minimum width.
  Retaining 25-point PAT/SONG buttons and allocating 60/98-point toolbar height fixed
  it. The successful regression checks actual panel clipping, not viewport bounds.

Genuine SwiftShader Vulkan offscreen images in
`validation/strict-fix-refresh/captures-validated-build/` were visually inspected:

- `workstation-refresh-authored-melody.ppm` — 1440×900, pointer-authored/repeated phrase.
- `workstation-refresh-automation.ppm` — 1440×900, real point editing and compact Inspector.
- `workstation-refresh-multiwindow.ppm` — 1920×1080, Playlist, Rack, Piano and Mixer.
- `workstation-refresh-toolbar-minimum-unclipped.ppm` — 1080×680, complete transport
  and navigation within their actual panel.
- `workstation-refresh-mixer-high-dpi.ppm` — **2160×1360 pixels at 2×**, with the
  logical viewport asserted to remain **1080×680 points**. Text, controls and
  navigation remain readable and unclipped. The partial final mixer strip is the
  normal horizontally scrollable view, not missing functionality.

Transient Undo/Redo toast and hover tooltip overlays in the captures are actual App
state, not painted demonstration elements. Inspector content uses its real scroll
region; no field is squeezed into a decorative mock panel.

DPI evidence correction: the first `mixer-high-dpi` capture enlarged both logical
viewport and pixel density. It proves only a larger-canvas high-density render and
must not be used as minimum-window evidence. The corrected test fixes `screen_rect`
at 1080×680, independently asserts `Context::content_rect` and pixels-per-point,
and checks actual panel containment at 1×, 1.5× and 2×.

The `INVALID-stale-binary-*` log/capture directory was produced after a failed
integration compile and is explicitly excluded from acceptance. Only the separately
successful build and `captures-validated-build` evidence above are used here.

Offscreen rendering establishes real application paint and layout, not Windows
execution, native presentation, physical audio/MIDI, or real plug-in UI behavior.

## Full-core compatibility follow-up

The later complete frozen no-default run retained seven failures; they are not
silently replaced by the initial targeted success. Original logs remain under
`validation/strict-fix-refresh/full-core/`: batches 018/019, 026, 036/037 and 040.
Five cases were stale visual/geometry or menu-access fixtures:

- Window motion started 120 points from the edge while travel was specified in
  pixels. At the new default 1× scale it reached zero exactly, so the old assertion
  could not distinguish held motion from snapping. Its start now derives from
  actual pixels-per-point and leaves a deliberate five-point gap. Continuous held
  movement, release snapping and Alt bypass assertions remain unchanged.
- The pressed-button fixture required a one-pixel text translation removed by the
  flat design. It now checks fixed text/hit bounds, the actual HIGHLIGHT-to-WELL
  painted fill, no activation during hold, and exactly one activation on release.
- Piano zoom computed its anchor using the old 72-point keyboard width. It now reads
  the production `piano-grid` before and after zoom; the 0.01-beat anchor tolerance
  remains unchanged. Production wheel math already used the actual grid.
- Two Piano mouse/occlusion tests still clicked the removed permanent Cascade
  control. Both now open the actual WINDOWS menu through the shared UI helper;
  their covered-region, note-paint and history assertions are retained.

The sixth case was a **real Inspector production overflow**, not a stale fixture.
An outer-panel slider estimate omitted section and numeric readout widths, feeding
back into panel expansion. A requested 240-point panel grew to 271.9 points and past
the 1440-point viewport. Property, WET and Route gain tracks now budget from their
actual row after labels/margins, including numeric and trailing-label widths.
Long property values wrap and routing combos fit their remaining width. Controls,
native response semantics, validation and value precision are unchanged. No panel
clipping or relaxed containment assertion masks overflow.

The seventh case retained the old 75-point velocity-lane assumption after the real
lane changed to 64. The production lane now exposes its actual hover response rect;
the test requires the entire lane inside the real 480×420 floating window, at least
48 points of editable height, and both note handles inside the lane and window.
The six-row grid minimum and all real menu/control operations remain required.

After fresh compilation, `seven-regressions.log` records **7/7 passing**, including
Channel and Mixer Inspector panels exactly 240/340 points wide and wholly inside
the viewport. The broader final frozen suite is tracked separately by integration;
this seven-case result is not a substitute for that full run.

A subsequent readout review identified a related precision edge: native adaptive
formatting can use five decimals at wide sizes and six during Shift fine dragging.
The shared property slider's paint-width budget now reserves six decimals without
changing `max_decimals` or rounding stored values. A new actual-App regression uses
0.123456 and -0.123456 in Channel/Mixer at 240/340 points, verifies twelve idle frames
leave values and width unchanged, then performs a real Shift numeric drag and checks
panel/control containment throughout hold, release and settling. This final
precision regression awaits the integration run; no result is inferred from the
previous seven-case binary.

The first fresh all-feature precision run (`precision-final.log`) passed the native
Inspector action layout gate but failed the new precision test's node selection.
Its x-only filter also selected a horizontally offscreen Mixer Pan knob at
`[[2021,239]-[2053,271]]`. The fixture now retains actual AccessKit node IDs and
walks the Inspector title's parent/child tree to its numeric-containing content
container. It checks every numeric descendant, regardless of bounds, and rejects
falling back to the whole-App root. The real readout drag uses that same subtree.
Both left and right containment, fixed panel width, idle precision, drag ownership
and actual value-change assertions remain required; this correction awaits a
fresh run rather than treating the initial failed fixture as passing evidence.


## Final precision and layout verification

The actual tree diagnostic (`precision-tree-diagnostic.log`) established that the
visible title is a `Role::Label` with `value="INSPECTOR"`, rather than a button-style
`label` property. Its title row has five descendants and no numeric controls; its
next content ancestor has 69 descendants, including the four volume/pan rails and
readouts, with no toolbar nodes. The fixture now matches that observed semantic
node. `precision-subtree-final.log` preserves the preceding title-lookup failure.

After this correction, a fresh all-feature build completed in 20.69 seconds and the
single precise-value regression **passed** in 3.08 seconds, recorded in
`precision-descendants-fixed.log`. All four Channel/Mixer × 240/340 combinations
retained the strict numeric bounds and real Shift-drag checks. The source was this
visual worktree; the integrated all-feature full suite remains a separate gate.

The integrated frozen no-default suite also completed all 186 groups and all 2047
scheduled tests: **2046 passed, 0 failed, 1 ignored**, as recorded in
`full-core-final/summary.json`. That binary predates the final six-decimal budget
and newly added precision regression, so those changes rely on the separate fresh
precision result above rather than the frozen suite.

All four fresh production captures in `captures-inspector-final/` were visually
inspected: `native-generator-inspector-240/340.ppm` and
`native-effect-inspector-240/340.ppm`. The real plug-in action rows wrap inside their
panels, WET tracks/readouts remain contained, and routing and status text remain
readable. These renders accompany the passing native-Inspector action test in
`precision-subtree-final.log`; that log's separately failed title-lookup fixture is
not counted as a passing run.
