# FL-inspired native UI refactor — 2026-10-22

Base: main 1201b71ec508f9f52d76eb6b68732bf6c575e787. User approved the concept on October 22. This is a visual/interaction-layer change, not a replacement audio engine or project format.

## Implemented scope

- Graphite surface hierarchy, restrained rounded control edges, coherent floating frame/chrome and amber activity/focus.
- Piano tools remain native controls. Narrow target/status spacing reduces secondary clutter without removing edit commands. All existing shortcut labels, including Mixer F9, remain authoritative.
- Editable notes use teal, selected notes have amber outlines; muted blue-gray reference notes remain read-only and expose no interactive handles. Musical content colors do not recolor the chrome.
- Sampler retains waveform and WAV operations, now includes an actual ADSR visualization and four rotary controls with exact numeric entry and accessibility. Existing release-committed undo transactions remain authoritative.
- Saved user layouts are retained; explicit Arrange applies the new composition. The 144-point minimum pitch-grid test and adaptive 48–64-point velocity lane are retained.

## Final review checkpoint (09:07 UTC)

- Native build, `cargo fmt --all --check`, and strict Clippy for default/no-default/all-features: passed. Default/all-features emit only the existing vendored vst3-host deprecation warning; the root crate passes `-D warnings`.
- Full all-target regression on one unchanged source snapshot: default 2318 passed / 1 ignored; no-default 2183 passed / 1 ignored; all-features 2855 passed / 9 ignored. Each configuration has zero failed, unresolved, or restarted tests. Counts overlap across configurations and must not be added as unique coverage.
- Compact/wide production UI interactions passed in those complete suites. Quarter-beat range gesture and pointer-jitter checks cover 1×/1.5×/2× UI scales; Sampler lifecycle tests include 1×/1.5×. This is headless production UI coverage, not physical high-DPI display acceptance.
- Python main scripts: 372 run, 364 passed / 8 existing skipped. Separate fixture build script: 11/11 passed.
- Native 1364 × 1024 and 1180 × 812 screenshots and pointer/keyboard smoke: passed with the compact overlap limitation below.
- Native Save As/Open and restart recovery: passed. Offline export: non-silent 48 kHz / 24-bit stereo output, with the existing automation restrictions verified below.
- Wide pixels were independently reviewed as a genuine concept-directed implementation, not a 1:1 replica. Final user visual acceptance remains pending.

The complete exact-name manifests, outcomes, source/binary hashes, and timestamps are in `evidence/final-regression-summary.json` in the review package. All discovered tests ran once per configuration using non-overlapping bounded exact-name batches, with single-threaded UI execution; this avoids accumulating GUI contexts without dropping assertions. The package includes the runners and terminal status logs. Ignored cases are pre-existing explicit saved-plugin-state or genuine-plugin/manual qualification fixtures; they are not counted as passed. See the separate final configuration reports.

No push, merge, release or installer publication is included. Hardware audio, physical MIDI and commercial VST acceptance are not established by cloud tests. The approved concept contains inaccurate sample labels; implementation retains real data-derived octave labels and existing shortcuts.

## Concept correction, 2026-10-22

The first native capture was rejected as a palette-only approximation. It is retained as historical evidence, not acceptance. The subsequent implementation changes actual composition and control geometry: wide lower-right piano roll (12-point semitones, 24 points per beat and a 32-beat visual floor), a grouped one/two-row score toolbar, a two-column waveform/Root/Loop Sampler with ADSR curve and four transactional knobs, an eight-strip compact Mixer with FX/routing details in a popup, a compact 16-step Channel Rack, and grouped project-browser content with collapsed help.

Playback length, note data, file format, audio callback and shortcut mappings are unchanged. F9 remains Mixer. The generated concept's duplicate octave labels and incorrect shortcut text are not requirements.

Native review uses the saved Night Orchard fixture with an actual imported decaying C4 WAV. Audio hardware is unavailable in the cloud; device-output audition is not claimed. All screenshots for acceptance must come from the actual app, not the concept generator. Final visual QA and full regression are still pending at this checkpoint.

### Native iteration 5 (08:14 UTC, still in progress)

- Captured an unmodified real native 1364×1024 workspace in `qa-results/native-concept-v5-wide.jpg`.
- Confirmed on native pixels: all 16 Rack steps and all 8 compact Mixer strips are visible. The previous Rack failure was a real floating-container `set_min_width(768)` constraint, removed in favor of vertical-only scrolling. Isolated panel tests alone had not caught it; a 400px full floating-window regression now clicks step 16 directly.
- Created 9 real Sub Orchard harmony notes through the native piano UI, kept 12 Neon Keys notes, saved `concept-review.citrus` through the native Save As dialog, verified the serialized 12+9 notes, and restored the project in a restarted native application. Teal editable notes and blue-gray read-only ghosts are now both visible. These are actual project notes, not painted screenshot overlays.
- Sampler was manually dragged into the left-middle position in this progress image. Explicit Arrange is being corrected to include its position and to respect the workspace/Mixer boundary; this image is not evidence that automatic positioning is finished.
- Still pending: final toolbar/grid polish, default/Arrange Sampler placement, compact native verification, independent visual acceptance, final full-source regression, and review package. No push, merge or release.
- Found and fixed a genuine overview-zoom regression: an armed quarter-beat ruler selection moves only 6 logical pixels at 24px/beat and had been below the old global drag threshold. A local 2px threshold applies only to explicitly armed range gestures; ordinary clicks/note gestures are unchanged. The complete 24-test range module passes, including 1×/1.5×/2× quarter-beat and 1px jitter checks.
- Diagnostic runs are not final acceptance: 1877 non-headless core tests passed, one existing external saved-plugin-state fixture remains ignored. Final source-hash-consistent all-target runs will follow visual freeze.

### Native iteration 6 (2026-10-22, 08:20 UTC)

The 1364 × 1024 real application capture is `qa-results/native-concept-v6-wide.jpg`.
Arrange was invoked before first opening Sampler; no manual window positioning was used.
Sampler's right edge is 616 and Piano's left edge is 624; its bottom edge is 791 and
Mixer's top edge is 799. Rack exposes all 16 steps, Mixer exposes all eight strips,
and Piano's grouped toolbar occupies one row with FIT NOTES in Tools. The grid
no longer has sub-pixel subdivision moiré. The project contains 12 real Neon Keys
notes and nine real Sub Orchard harmony notes, rendered as editable teal and
read-only blue-gray ghosts. A separate pixel review approved this as a genuine
progress image, not final user acceptance or a pixel-identical concept replica.

Native Save As created `concept-review.citrus`; subsequent process restarts restored
the WAV asset and both note layers from that project. Native precision input changed
Attack from 0.005 to 0.250 s and updated the dial and envelope. This exposed a
multi-character undo transaction bug, and File-menu inspection exposed Sampler's
incorrect foreground layer. Both are being fixed with dedicated regression tests;
iteration 6 is not the final regression build.

### Native offline export (08:30 UTC)

The native File → Export WAV flow correctly rejected the demo's enabled Mixer 6
volume automation, which is outside the existing offline renderer's supported
automation set. Temporarily disabling that lane through the real Inspector allowed
the native export dialog to complete. `native-export.wav` is stereo 48,000 Hz,
24-bit PCM, 360,000 frames (7.5 seconds), peak 0.9499998 and RMS 0.1106716.
Metrics are in `qa-results/native-export-metrics.json`. This verifies non-silent
local offline rendering, not hardware audio playback. The saved review project
retains its original automation; this temporary export state is not the deliverable.

### Final native interaction pass (iteration 7, 08:46 UTC)

The final native binary SHA-256 is `9bea996f0314800be96c437246e77859a8a99ccf7323d62f68e0be24de906e43`.
The production/test source hash used by the bounded runner is
`39a133d8d6495f959969363536efc8d865a9e01e6f4b093a6a070c7522e5cb96`.

Verified in the actual application, not only an isolated widget test:
- Multi-frame keyboard entry 0.005 → 0.250 s followed by one Ctrl+Z restores 0.005 s.
- Typing 0.750 s then Escape cancels the whole uncommitted edit and restores the dial/envelope.
- Pointer dragging the Attack dial changes its value and envelope; one Ctrl+Z restores it.
- File's complete menu remains above Sampler.
- At 1180 × 812, F6 brings Rack forward and clicking the sixteenth step works without horizontal scrolling; Ctrl+Z restores that step. F7 exposes the two-row Piano toolbar, and F9 brings all eight Mixer strips forward.
- Save As produced `review-v7.citrus`, then native Open reopened it with SAVED status. Its serialized project data is exactly equal to `concept-review.citrus`: all temporary test changes were undone. It contains 21 notes, the C4 sample and the original enabled automation. Reopening Sampler restores its real waveform and original ADSR values.

Evidence: `native-v7-wide.jpg`, `native-v7-compact-piano.jpg`,
`native-v7-compact-sampler.jpg`, `native-v7-fixed-undo-menu.jpg` in `qa-results/`.
The wide capture is the final build after actual save/reopen.

Compact limitation: minimum usable editor sizes cannot all fit simultaneously in
1180 × 812. Floating windows overlap, including approximately 25 pixels of Mixer
title behind Sampler. Use F6/F7/F9, the remaining title buttons or close Sampler
to focus the editor being used. No claim is made that the compact view is an
overlap-free or pixel-identical rendition of the concept. The 1364 × 1024 layout
is non-overlapping at the Sampler/Piano/Mixer boundaries.

A separate read-only diff review found no must-fix regression in controls,
ghost read-only rendering, Sampler draft/snapshot boundaries or window layering.
It did not independently perform native execution. User visual acceptance of the
revised concept implementation remains pending; this is a review delivery, not a
release approval.
