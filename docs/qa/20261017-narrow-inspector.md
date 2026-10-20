# Inspector access at supported narrow window sizes

## Native desktop reproduction

At baseline `4f68e97`, the cloud Linux XFCE desktop was 1180 × 812. With an
isolated fresh app configuration, select the demo automation clip (`Keys volume`)
in Playlist and click the top-right Inspector icon (hover text: `Toggle Inspector`).
The icon becomes orange but no Inspector panel appears. Clicking it repeatedly
changes only the icon. This is a real native-app observation supplied by the
desktop QA run, not an offscreen screenshot inference.

The same defect is deterministic in production `App::ui`: the UI toggles
`show_inspector`, but panel rendering required both that preference and a window
width of at least 1280 logical pixels. The supported native window minimum is
1080 × 680, so essential automation and selection properties were inaccessible
through an apparently enabled control.

## Change

Inspector now follows its explicit visibility preference at every supported
window size. Both the icon and View > Inspector retain their existing behavior,
and HIDE closes the panel. Inspector visibility remains an application workspace
preference and survives restart.

The existing docked, vertically scrollable panel is retained. Sidebar resize
limits reserve a 600-pixel central editor panel, including when saved wide
sidebar sizes meet a narrower window. Browser remains independently available;
opening Inspector does not silently hide Browser. Browser retains its 168-pixel
minimum and Inspector its 220-pixel minimum. The workspace's existing 3-pixel
inset leaves 594 pixels of usable workspace bounds at the tightest combination.
No project, audio, automation, plug-in, or document-format behavior changes.

## Input-level regression

`src/app/narrow_inspector_ui_tests.rs` uses the existing display-independent
harness, actual `App::ui`, accessibility-derived widget bounds, primary-button
press/release events, and wheel events. It does not invoke toggle or editing
handlers in place of input.

Coverage:

- 1080 × 680, the observed 1180 × 812, 1279 × 812, 1280 × 812, 1440 × 900,
  and 1920 × 1080: opening, HIDE, repeated toggles, actual panel/header presence,
  on-screen controls, and a successful Shapes > Rise edit.
- View > Inspector toggling and visible/hidden persistence across a new App.
- Explicitly restored 320/340-pixel sidebar sizes followed by shrinking the
  floating workspace across the former threshold; both sidebars retain valid
  bounds and at least 600 pixels remain for the editor.
- Browser off/on while Inspector stays available; pure layout changes preserve the
  Project fingerprint.
- At minimum, observed desktop, threshold, and wide sizes: scroll to the real
  automation target and curve controls, open their on-screen menus, choose Master
  Pan and Linear, and verify the model changes.

The first regression was run against the unchanged baseline production code and
failed as intended at 1080 × 680: `enabled Inspector must actually be visible`.
Initial test-fixture compilation of the extended tests found a runtime-slot type
mismatch (`u8` vs `usize`); the fixture now converts it explicitly.

Focused command:

```sh
cargo test --offline --bin citrus-studio narrow_inspector -- --nocapture
```

Native after-fix acceptance belongs to the desktop QA run. These automated
input tests do not establish physical audio/MIDI, Windows execution, or a detached
native-window behavior claim.

## Separate floating-divider defect

Strengthening the resize precondition found an existing independent defect:
with floating editors, a Browser-divider drag from 176 toward 320 pixels snaps
back to 176 on release. The workspace cancels the active pointer whenever its
bounds change, including the sidebar drag causing that change. The red test run
finished with 21 passed and 1 failed; the exact expected-320/actual-176 assertion
and input sequence were passed to the workspace repair owner. A second run in
maximized mode allowed Browser to reach 320 but Inspector still reverted to 232,
so the workspace owner must investigate both cases. Those issues are not claimed
fixed by this Inspector patch. The narrow-layout budget test explicitly restores
wide sidebar sizes as fixture setup and validates narrowing from that state. Real
divider gesture regressions remain the workspace repair owner's responsibility.
