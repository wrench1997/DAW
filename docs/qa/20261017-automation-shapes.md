# Automation source shapes — 2026-10-17

## Implemented scope

The selected Automation clip's Inspector now offers a **SHAPES** menu: Rise,
Fall, Eased rise, Steps, Sine, and Triangle. Depth controls the fraction of the
parameter's range around its midpoint. Cycles (1–16) and phase apply to repeating
Steps/Sine/Triangle. Rise/Fall/Eased rise ignore cycles and phase.

The operation uses the clip's source offset through source offset + length,
not its Playlist placement. This is a shared-source edit: every placement using
the lane observes the changes. The menu and tooltip say so. Points outside the
inclusive source region remain byte-for-byte unchanged, although adjoining
segments necessarily interpolate to the new boundary values. Target, native
value range, enabled flag, loop and lane-wide interpolation are retained.

Shapes are sampled control points, not new DSP or a filter implementation.
Generated outgoing tensions are zero. Existing Hold interpolation is retained
and visibly explained in the menu; smooth presets then play as small steps.
Steps has eight ascending plateaus per cycle and uses very short ramps at jumps
under Linear/Tension rather than changing the entire shared lane to Hold.

The demo's previously misleading “Filter sweep” clip is now “Keys volume”,
matching its lane name. It targets mixer 6, where the actual Neon Keys channel
(channel 5 and the demo piano notes) is routed. The lane now uses Tension so its
existing tension values have an effect. No filter target, new DSP, external VST,
or native-plugin verification is claimed.

## Edit and safety contract

`AutomationLane::apply_shape` returns `Result<bool, AutomationError>`; `true`
means a real change. The UI applies to a candidate project and uses the existing
explicit project history transaction, yielding one Undo entry independent of the
normal timed history coalescer. Reapplying an identical preset creates no history.

- At most 513 newly generated points per operation; existing outside points remain.
- Native ranges (including BPM and bipolar pan) are respected through normalized
  internal shape values; UI values are still in native units.
- Nonfinite options and invalid source ranges fail without mutation.
- Cycles and depth are clamped; finite phase wraps into one cycle.
- Epsilon collisions, including collisions with outside boundary-neighbor points,
  fail atomically rather than silently deleting those outside points.
- Very small or unrepresentable spans fail with an actionable toast.
- Beat/Value/Curve labels and tooltips replace unexplained B/V/T abbreviations;
  point controls wrap inside narrow inspectors.

## Validation

Model tests: 6/6 passed in the no-default-features test binary on Linux.
They cover all six shapes across volume/pan/tempo ranges, finite extreme native
ranges, determinism/no-op application, point cap, interpolation/config preservation,
outside-point retention, phase/depth behavior, step plateaus, evaluated Hold-ramp
interiors (33 sampled points rather than a flat two-point Hold), nonfinite input,
small/huge-position epsilon failures, and atomic rejection.

Production egui tests in `automation_shapes_ui_tests.rs` exercise actual menu
clicks for Sine and Rise, slipped source-region bounds, outside-point retention,
single Undo/Redo round-trip, repeat-preset no-op history, and demo routing/name.
Execution: 3/3 passed with no default features on Linux. The menu application test
also passed with genuine WGPU readback enabled (1/1).

No physical display, listening test, device, Windows user profile, or native
plugin acceptance is implied by these model/headless UI checks.

## Actual render inspection

The existing offscreen harness rendered the production app via Vulkan SwiftShader
(Subzero CPU adapter), with no window/display/surface. The Sine application test
produced 1498 × 936 PNGs converted losslessly from WGPU PPM readback:

- `daw-tooling-20261017/captures/automation/automation-shapes-source-region.png`
- `daw-tooling-20261017/captures/automation/automation-shapes-menu.png`

Inspected the open-menu image: all six labels, Depth/Cycles/Phase controls and
shared-source notice are readable without popup overflow. The applied sine is
visible in the selected Keys volume clip. The inspector is scrollable; control
point rows are below the initial fold at this size. This is actual app UI, not a
design mockup. Adapter metadata is alongside the captures in `adapter.txt`.

Reproduction: source the tooling env, set
`VK_ICD_FILENAMES=/usr/lib/chromium/vk_swiftshader_icd.json` and absolute
`CITRUS_UI_CAPTURE_DIR`, then run the no-default-features test binary with
`automation_shapes_menu_applies_slipped_source_and_one_undo_redo --nocapture --test-threads=1`.
