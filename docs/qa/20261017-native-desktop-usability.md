# Native desktop usability session — 2026-10-17

## User request and scope

The user explicitly requested direct computer operation to find usability problems rather than relying on test totals or offscreen renders. The baseline is `4f68e97`. This session uses the real cloud Linux desktop, the actual built Citrus Studio executable, mouse/keyboard events and desktop screenshots. It does not operate the user's Windows PC, and does not authorize a GitHub push.

## Environment and qualification

- Cloud XFCE desktop, 1364 × 1024. Initial App client 1180 × 812; wider and narrower sizes were exercised by actual native window resizing.
- The initial native window was unpainted: Mesa repeated `Failed to attach to x11 shm`. Chromium SwiftShader cannot create this Xlib surface (`VK_KHR_xlib_surface` unsupported), though earlier offscreen tests work.
- A QA-only process-local compatibility shim makes Mesa select its existing ordinary XCB image-copy path instead of optional MIT-SHM. It changes no App UI/model code, X-server access, IPC settings, or system permissions. The actual native App then rendered and accepted input. This software presentation path is not a physical-GPU/FPS qualification.
- Cloud audio hardware is absent; audio remains explicitly Offline. No claim of audible monitoring, physical MIDI latency, or real external VST loading.
- The image lacks both the desktop FileChooser portal backend and Zenity. In this environment, rfd's portal/fallback attempts return `None`, and the App silently leaves Save unchanged. Ctrl+S and the toolbar Save were tried; no native picker appeared. This is a cloud runtime dependency limitation plus a silent failure UX boundary, not evidence that Windows Save or the project serializer is broken. No packages/services/security settings were modified to mask it.

## Actually operated baseline workflow

1. Opened File > New project, brought Piano Roll forward with F7.
2. Tried double-clicking its floating title to enlarge it: no action. Windows > Maximize editor works, but requires discovering a separate menu.
3. Drew four notes with the mouse (C4/E4/G4/C5), used Ctrl+A then Ctrl+B to produce eight notes and extend the pattern to two bars.
4. Double-clicked a note, changed its MIDI pitch via the real numeric editor, applied it, and used Ctrl+Z. The visible note and Inspector value changed/restored correctly.
5. Switched to Playlist (F5), placed the pattern, created an automation clip, and used Shapes > Sine.
6. Tried the Inspector toggle at 1180 px: its active state changes, but the sidebar never appears. Widening beyond the source's 1280 px condition reveals it.
7. Tried to edit the visible automation peak in default Draw mode: point right-click and drag do nothing. Switching to Select makes the point context menu accessible; this required discovering an undocumented mode condition.
8. In Select, two fast four-position peak drags still leave its native value at 1.000000. Right-clicking the same coordinate proves the point is hit. An equivalent fast Piano Roll drag correctly moves C4 at beat 0 to C#4 at beat 1, so the automation symptom cannot simply be dismissed as unsupported desktop input.
9. Preserved the actual test project from its recovery file before restarting or integrating changes. This is not a successful native Save picker claim.

## Findings and repair status

### P1 — Inspector toggle silently unavailable at a supported width

Confirmed by native input and source: `show_inspector && wide_layout` suppresses the panel below 1280 while the toggle remains active. Essential automation target/curve and note detail controls are therefore unavailable in the laptop-size window. A bounded responsive-panel fix and actual toggle/persistence/control tests are in integration; native fixed-binary recheck remains pending.

### P2 — Floating editor enlargement lacks a direct affordance

The title double-click does nothing, while Maximize editor is hidden in Windows. This is an interaction-cost problem, not a claim that maximization is absent. A visible title control/double-click/restore implementation is being tested while preserving the compact editor height and floating geometry.

### P1 — Visible automation points are inert in default Draw mode

The production point/tension interactions are Select-only, while the curve still paints points in Draw. The visible interface does not explain this prerequisite. Actual-input regressions reproduce the trap. Draw-mode editing and gesture ownership are being repaired without changing the meanings of other tools.

### P1 — Fast automation drags lose the edit

The native drag symptom is repeated by a coalesced-input regression. Unlike Piano's existing raw-event ownership path, the automation path only observes egui's active drag response and loses press/move/release in one frame. A separate safety regression also found that secondary-button dragging can mutate points. Repairs and native rechecks are pending.

### Related regression — Sidebar divider drag can snap back

Strengthening the actual divider-input test exposed cancellation/geometry behavior during sidebar resizing. The failing case is retained rather than replacing it with a success claim based only on seeded saved widths. It is being investigated independently of the narrow-access fix.

## Completion requirement

Rebuild the integrated App, repeat the relevant original desktop actions, retain before/after evidence, and run the affected regressions and aggregate gates. The preceding all-feature/core test results remain attributed to their older source. A green test total alone does not close the native usability findings.

## 17:16 UTC integrated native recheck (`4320da0`)

A separately built, immutable executable (SHA-256 `953f6b45fcae7ef380dcfe61d2f82834a3fefc3615a2de7b1494285b0e1ebd70`) was launched as a second process using a verified identical recovery-project copy. The original instance and its live edits were retained. The new App's actual Restore Autosave dialog restored the eight notes and automation.

- At the same native 1180 × 812 size, clicking Toggle Inspector now visibly opens the panel. This original defect is fixed in actual desktop operation.
- Double-clicking the floating Piano title now enlarges it; Restore in its existing header returns the original floating geometry and the Piano regains the foreground after layout settles. The new square title control is visible.
- **A stronger native divider drag still failed:** dragging from the Inspector boundary into the floating Piano created an unintended D#5 note at beat 7.5, extending the Pattern from two to three bars. Ctrl+Z restored the prior music. This is a coalesced original-press ownership defect, not an acceptable sidebar-resize result. The fixed test-only divider cases therefore do not close native acceptance. A regression and a further ownership repair are in progress.
- Native original-press coordinates and the stable screen state were retained during diagnosis; the equivalent Piano note drag itself was separately verified to work correctly.

The original project recovery copy was byte-verified before testing transitions: SHA-256 `1760453c4ff2a45bd19747bf80fac527d79e3531ed01830fe0d0cda3cc0cb998`, eight notes, two placements, one automation lane. The first note is C#4 at beat 1. These data were authored during this test session, not imported user work.

## 17:41 UTC original native failures passed on integrated `0f2d928`

A third, independently launched native App used the immutable executable with SHA-256 `54b91f161fd090b3f72ac1a9059b0e7cbae12557faf21a1e75f458944ea81786`. The prior live instances and edits remain preserved. The actual recovery dialog restored the same eight-note project.

- Inspector opens at 1180×812. A fast divider drag from 1038→1000 widens it; 1001→1040 shrinks it and the released width persists. A further 1041→990 drag crossing a 43 px gap into the floating Piano widens the sidebar without writing a note or changing the Pattern's two bars/Song's three bars. This repeats the original failing native interaction, not just a headless approximation.
- The square title control enlarges Playlist directly. Piano title double-click and Restore had also passed the preceding native integration check; their source is unchanged by the final pointer fix.
- In ordinary Draw mode, selecting the Sine clip now exposes actual node handles. At 1 BAR zoom, a four-position native drag of the peak from (687,556) to (687,574) changes its value from 1.0 to0.1. The actual right-click menu, still in Draw, displays `Native value: 0.100000`.
- After closing that menu, a single Ctrl+Z restores the original curve; Ctrl+Y restores the notch. No clip movement/resizing or unrelated music edits were observed.
- `native-automation-draw-after.jpg` is an actual 1180×812 native window capture, not an offscreen WGPU render. The earlier missing-Inspector desktop image is retained separately.

The originally reported native interaction defects are now repaired and rechecked. Final complete all-feature/internal-engine suites are still being completed. Physical audio, native file chooser, external VST and Windows runtime/CI remain outside this successful mouse/keyboard result. The cloud file-chooser dependency limitation is documented in BUILD_AND_RELEASE rather than being mislabeled as a successful Save test.

### Native recovery-data cross-check at 17:44 UTC

The actual final recovery file was parsed and compared with the verified input
recovery file after the desktop actions and Undo/Redo. The eight-note Pattern
payload and both placements are exactly unchanged. The only changed top-level
field is automation_lanes, and the only changed point is index 16 at beat 2.0:
value 1.0 → 0.10000002384185791, with position and tension unchanged. This agrees
with the native menu's six-decimal 0.100000 display. Recovery-file SHA-256 changed
from 1760453c4ff2a45bd19747bf80fac527d79e3531ed01830fe0d0cda3cc0cb998 to
b7add8b80c14b63ff9878083f6235bab4c2616c724e714ab3a6e3835abe19214.

This independently checks that the successful sidebar/automation input replay
left no hidden extra notes or moved clips. It remains recovery-file evidence,
not a claim that the unavailable native Save chooser worked. All three native
instances were minimized with their live states retained rather than discarded.

## 17:51 UTC full-feature gate completed

The fresh final-source all-feature/all-target run completed all 2,779 unique
scheduled tests: 2,770 passed, zero failed, nine existing optional tests ignored.
The App accounted for 2,707 scheduled cases (2,698 passed/nine ignored); helper
and protocol targets contributed 72 passing cases. Strict all-feature/all-target
Clippy and formatting passed. Python ran 368 cases: 359 passed/nine existing
platform/optional skips. The final no-default run is separately in progress;
its overlapping cases must not be added to the all-feature unique total.

### File-picker dependency follow-up

A read-only check of official Debian trixie package metadata indicates an
isolated Zenity setup would require approximately 12 missing packages, including
GTK4 and libadwaita, about 9.4 MB downloaded and 42 MB unpacked. This is dependency
feasibility only; no new package was downloaded or executed, no service or system
configuration was changed, and native Save has not been retested successfully.
The missing chooser remains an open cloud-runtime acceptance issue.

## 18:04 UTC final independent no-default gate completed

The fresh no-default App completed all 2,099 unique scheduled cases: 2,098
passed, zero failed, one existing optional ignore. Three protocol targets are
feature-disabled and list zero tests, with successful terminal runner output;
they contribute no test count. Strict no-default/all-target Clippy and final
formatting passed. The two configurations overlap and are not summed.

For the packaged data comparison, native-before-recovery.citrus is the verified
1760453c… copy of fixed-run-data/Autosave.citrus. The older intermediate backup
before-native-fixes.citrus is a different snapshot and is not the final input
evidence. The final before/after project payload differs only at the documented
automation value; the packaging validator independently checks this.

All claimed fixes are bounded to these tested interactions. Native chooser,
physical audio, commercial VST and new Windows CI remain open acceptance items.
