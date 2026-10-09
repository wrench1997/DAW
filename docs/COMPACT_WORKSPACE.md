# Compact native workspace

This refinement keeps the real Rust/egui editors and shared project/runtime. It follows
Image-Line's [official interface guide](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/basics_interface.htm)
for compact task surfaces and window alignment, without shipping Image-Line assets.

## Layout and controls

- Fresh/reset layouts use two coherent columns when the workspace can fit them:
  Playlist above Mixer on the left, compact Channel Rack above Piano on the right.
  At smaller sizes they retain exposed, freely movable/stackable windows; maximize,
  restore, Arrange, Cascade and the editor shortcuts remain available.
- The split respects both editors' minimum heights, including laptop-height
  workspaces. It does not create overlap merely because a Mixer reaches its minimum.
- Floating title bars carry editor context, removing the duplicated large headings.
  Frame padding, shadows, tool spacing and Playlist navigation are slimmer. All
  existing tool, group, snap, scale, assignment and plug-in controls remain reachable.
- Rack rows retain at least 24-point channel, Mute/Solo and step targets. Mute/Solo
  have explicit accessible channel labels and visible state. Beat labels use actual
  step geometry. Narrow Rack windows retain bounded horizontal and vertical scrolling;
  the last step is tested through real wheel input and a pointer click.
- The Browser's fresh default is narrower. A fresh workspace starts with Inspector
  closed; its existing toggle remains available. Inspector visibility is now saved.
  Existing v1 layout JSON without that new field retains the previous visible Inspector.
  Saved editor rectangles, visibility, stacking and focus are preserved.

## Window movement and alignment

Native egui owns held title and all eight resize gestures. There is no animation or
magnetic movement during the held gesture. On release, a window edge within eight
logical points of a workspace or overlapping visible-window edge aligns once. Alt
at release bypasses alignment. A moved window preserves its size; a resized edge
preserves its opposite edge and respects minimums. Workspace edges win exact ties.
A new pointer press cannot be intercepted by a pending alignment.

Reset rectangles are bounded before applying them. The reset frame bypasses egui's
old-size area constraint, which otherwise moves a correct new position using the
window's previous size before the new size is applied. Ordinary native constraints
resume immediately afterward. Repeated Arrange/Cascade and viewport restoration are
covered against drift and workspace overflow.

Focus switches, modal interruption, hiding/arranging/maximizing, and workspace-bound
changes clear pending alignment. A sidebar/viewport change during a held gesture
cancels it until pointer release rather than resuming in a changed coordinate system.
Existing note/clip cancellation, text ownership, project/import/native plug-in
barriers, stable editor IDs and one-dispatch shortcut handling remain intact.

## Verification and performance boundary

The actual production UI harness covers continuous title movement after native click
slop, all eight resize edges, release alignment, Alt bypass, dragging away from a snap,
peer-edge resizing, sidebar interruption, repeated arrangements, v1 migration,
workspace containment, Rack hit areas and narrow-window scrolling. Existing text,
modal, shared-history and stacking regressions also run.

`workspace_motion_measurement` is an opt-in controlled UI-CPU benchmark. Set
`CITRUS_MOTION_BENCHMARK=1` and run that one test with `--nocapture`; leave
`CITRUS_UI_CAPTURE_DIR` unset. The same standalone benchmark source was copied onto
baseline c88c7fd and the refined source with identical locked offline no-default debug
profiles (debug info/incremental disabled, default test stack). Both show four editors
at identical explicit outer rectangles in a 1920×1080 logical viewport, with Browser
and Inspector hidden in both, five channels, eight clips and twelve Piano notes.
Thirty frames warm the UI, then each drag and resize contributes 360 pointer samples.
The timed region is `UiHarness::run`: real `Context::run_ui`, accessibility collection
and the paint-shape assertion. No GPU rendering/readback or physical display is timed.

The app status-footer UI-ms/FPS display measures smoothed frame-arrival timing. It is
not evidence for this benchmark or desktop smoothness. Native egui click slop and
physical-pixel rounding still apply. Its left/top resize state may pair the current
position with the previous input frame's requested size; tests allow only that bounded
one-step pending-size offset and require stable geometry after release. These are
stored/interaction-geometry checks, not a claim that display frames were presented.

Genuine optional captures use the real egui/WGPU Vulkan path with the installed
SwiftShader CPU adapter. They establish application paint/layout in an offscreen
texture, not native desktop presentation, Windows execution, physical audio/MIDI,
plug-in editor paint acceptance, or a target frame rate.

## Controlled measurement, 2026-10-09

Three sequential, interleaved baseline/refined pairs were run with no concurrent
compilation or Vulkan capture. Each row summarizes 360 held-pointer samples. Values
are milliseconds of unoptimized UI CPU work, not rendered or displayed FPS.

| Build | Pair | Gesture | Median ms | p95 ms | Max ms | Max geometry step, pt | Unchanged samples |
|---|---:|---|---:|---:|---:|---:|---:|
| baseline | 1 | drag | 18.335 | 21.374 | 30.223 | 2.166 | 0 |
| baseline | 1 | resize | 20.255 | 23.403 | 26.777 | 1.803 | 1 |
| baseline | 2 | drag | 18.585 | 21.211 | 29.447 | 2.166 | 0 |
| baseline | 2 | resize | 20.596 | 25.248 | 46.175 | 1.803 | 1 |
| baseline | 3 | drag | 17.968 | 21.177 | 29.311 | 2.166 | 0 |
| baseline | 3 | resize | 20.615 | 23.287 | 25.937 | 1.803 | 1 |
| refined | 1 | drag | 18.014 | 19.454 | 21.523 | 2.166 | 0 |
| refined | 1 | resize | 21.004 | 26.520 | 50.603 | 1.803 | 1 |
| refined | 2 | drag | 19.409 | 23.192 | 25.765 | 2.166 | 0 |
| refined | 2 | resize | 20.274 | 24.395 | 30.838 | 1.803 | 1 |
| refined | 3 | drag | 17.842 | 19.963 | 24.638 | 2.166 | 0 |
| refined | 3 | resize | 20.535 | 24.583 | 31.389 | 1.803 | 1 |

The median of the three run medians is 18.335 → 18.014 ms for movement and
20.596 → 20.535 ms for resize. Median run-p95 is 21.211 → 19.963 ms for movement
and 23.403 → 24.583 ms for resize. Results are mixed and noisy; no general CPU
speedup or frame-rate target is claimed. The value of this change is denser layout,
correct reset geometry and bounded release alignment.

Both builds produce exactly the same measured continuity: maximum successive stored
geometry steps 2.166 points for movement and 1.803 for resize, zero unchanged movement
samples, one unchanged resize sample per 360-sample run, and no >8-point jumps. The
resize sample is the native pending-size boundary, not evidence of a dropped display
frame. All four initial outer rectangles match between builds.

## Verified isolated source slice

- Locked/offline Linux no-default/all-target: **975 passed**.
- Locked/offline Linux all-feature/all-target: **977 application + 14 helper + 5
  protocol tests passed**. Default test stack; zero failures/ignored tests.
- Formatting, both strict all-target Clippy configurations, all-feature app/helper
  build, Windows MSVC no-default/all-target source cross-check, and all **167 Python
  regressions** passed. The existing vendor dependency deprecation is unchanged.
- Final capture run: **26 harness entries passed**, comprising 25 existing/new input
  and flow checks plus the opt-in benchmark entry (timing was executed separately in
  the six controlled runs above). It produced **30 genuine Vulkan PPM checkpoints**;
  every PNG conversion was verified RGB-identical.
- Wide compact arrangement, fresh minimum-size arrangement, restored saved layouts,
  narrow Rack after real scrolling, and the existing inspector/modal/import flows
  were visually checked. Independent read-only review found no remaining blocker.

These counts describe this isolated compact-workspace slice based on c88c7fd. They
must not be added arithmetically to a separately developed feature's test counts;
combined source, including newer Piano clipboard controls, needs fresh integration
checks and captures. No Windows runtime or native desktop acceptance is inferred.


### Combined compact workspace and Piano clipboard checkpoint

At `8d6b54c11fe07386b105de8d41e209bb0f45af5e`, both reviewed features are integrated.
Fresh full Linux locked/offline tests pass **991 application + 14 helper + 5 protocol**
with all features and **989 application** without default features, zero failures or
ignored tests on the default stack. Formatting, both strict Clippy configurations,
app/helper builds and actual ordinary helper smoke pass. Windows MSVC no-default/
all-target cross-check and all **167 Python tests** pass.

The combined genuine Vulkan run passes **36 harness entries**: 35 actual input/flow
checks and the opt-in benchmark entry, whose timing mode was not enabled here. It
produces **33 actual app renders**, every PNG RGB-identical to PPM readback. Both
clipboard semantic/text/history barriers and compact title/all-edge/Alt/snap/modal/
migration regressions execute together. Wide/minimum layouts and clipboard controls
were visually inspected. Independent source integration review found no blocker.

The six controlled baseline/refined timing runs above describe the isolated compact
slice and do not establish a general speedup for this merged candidate. This source
checkpoint still requires its own Windows execution; native paint, physical devices,
OS clipboard round-trip and detached OS editor windows are not inferred as passing.
