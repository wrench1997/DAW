# Compact editor controls — 2026-10-17

Base: `12a9b375ddc11bc645d180b6878ee135d9b69302`. This follow-up changes presentation and reuses existing editor commands and shape transactions; no shape API, audio routing or project format change.

## Behavior

- Piano uses the same NOTE EDIT, SCALE / CHORD and VIEW menus at every width. The ordinary wide editor has two control rows. Tools, local time snap, active Channel, ghost visibility and Fit Notes stay directly accessible.
- NOTE EDIT retains clipboard, repeat-range and selected-note Channel assignment. TOOLS retains transforms. Scale lock and the selected Stamp chord appear in the closed setup label. Narrow editors keep the active repeat range visible; the paste anchor remains available in NOTE EDIT.
- The Automation selection shows target, source-curve value and SHAPES immediately below its identity, before metadata. Values use the selected placement's source offset and half-open timeline interval, the lane's interpolation/loop mapping and native scale (BPM for tempo). Disabled/muted/outside states are explicit. The tooltip distinguishes source-curve values from final audible output.
- Shape source bounds, shared-placement warning, outside points, interpolation, no-op history behavior and one-step Undo/Redo retain the original implementation. Readouts do not dispatch values or edit the project.

## Validation

- `cargo test --offline --bin citrus-studio piano_ -- --test-threads=3`: **224 passed**. Includes mouse/range/keyboard/expression, clipboard, focus/modal/held-pointer and history guards.
- Final `piano_snap_integration_tests`: **4 passed**, with genuine offscreen captures of wide, 650px, 630px, populated 480×420, NOTE EDIT, active lock/Stamp and longest lock/chord labels at 480×420. The longest label and controls remain inside the window; grid retains at least 144 logical pixels of height.
- Final `automation_shapes_ui_tests`: **7 passed**, covering source-region shape application, one-step Undo/Redo, no-op reapplication, top-of-inspector visibility, native negative pan, tempo BPM, empty/disabled/outside values, loop mapping, and separate placements of a shared lane with different source offsets. Display and menu dismissal preserve fingerprint/dirty/history. Fixtures explicitly establish a saved baseline for the periodic dirty checker.
- `cargo clippy --offline --all-targets --all-features -- -D warnings`: passed. Existing vendored `fetch_update` deprecation warning remains; no new lint failures.
- `git diff --check`: passed.

Captures use the production egui UI and WGPU readback with Chromium SwiftShader Vulkan; PNGs are lossless conversions of PPM frames. Visually inspected: `piano-compact-wide`, `piano-range-floating-480x420`, `piano-range-floating-edit-menu`, `piano-compact-scale-lock-stamp`, `piano-compact-longest-lock-stamp-480`, `automation-top-summary`, and automation shape menu/source-region frames. They are offscreen Linux UI evidence, not a native Windows or real-device audio run.

## Integrated local validation

Integrated with the narrow Windows late-getter fixture repair on base `12a9b375ddc11bc645d180b6878ee135d9b69302`. The combined candidate passed formatting, strict all-feature/all-target Clippy, all-feature binary build and test compilation. The integrated exact-name union passed **273/273 tests, zero failures or ignores**: **224 piano + 7 automation UI + 42 strict backend**. All were scheduled once across 39 fresh-process batches (maximum 20 tests each, one test thread, unique absolute TMPDIR). This is targeted integration coverage, not a new full-suite run.

The tested source manifest SHA256 is `69f5613ff0132a263e001bb67da62c8e8bfb0c56a36dc4630d603949ce4cfa75`; mutable QA reports are excluded. Initial integrated formatting drift and the corrected passing gate logs are both retained. Captures above were reviewed before integration and were not recaptured for this gate.

Status: **integrated and locally verified; exact-candidate Windows CI pending**. Base quality success and the historical preview fixture failure are separate evidence. No commit or push was performed by the validation worker.
