# Strict repair and workstation refresh integration — 2026-10-17

## Scope

Baseline: `aff57b2b84e3e5b7179d39269518b76397fc6c6e`. The user explicitly requested continued code repairs until acceptance succeeds and rejected the previous visual style. Development remains on `ci/windows-reliability-20261009`; no user-computer, main-branch, installer publication or upload-setting changes are part of this work.

## Implemented

- Playlist deletion now removes only automation sources orphaned by this operation, preserving valid pre-existing global automation and remaining shared/muted placements.
- Audio Clip Gain/Fades remain active. Pattern Gain remains active with a MIDI-velocity caveat for plugins; unsupported Pattern fade and Automation Gain/Fade controls are not presented as usable. Existing stored fields are preserved. **No Pattern continuous envelope DSP was added.**
- Inspector pointer-owned edits use one explicit history transaction across press/move/release, timed history observation and interruption. Discrete clip mutations cannot interleave a held/cancelled pointer transaction. Manual Save closes the transaction before capture; recovery Save waits rather than serializing an intermediate preview. Cancelled native drags are retired before mutable Inspector widgets are painted.
- Flat graphite surfaces, quieter borders, narrower Browser/Inspector, compact sections, native-accessible controls and a Windows layout menu replace the permanent workspace action strip. Important fault/dirty/offline status remains visible; extra telemetry is on hover.
- Plain Space has a frame-local owner before native widget processing, preserving global transport priority while Enter/AccessKit/pointer stay native. Text/popup/modal ownership and background focus are explicitly guarded; repeats do not cause extra transport actions.

## Current executed acceptance

At 15:09 UTC, fresh no-default compilation followed by **44 unique focused tests passed, zero failed or ignored**, with actual App input and genuine WGPU rendering enabled. This includes the unchanged original deletion-failure body, the complete built-in melody → Master automation → Manual Save → new App object Open → WAV byte/PCM chain, Inspector PCM/history/lifecycle cases, shared-source/model deletion and workstation/input/layout cases.

The corrected high-DPI capture is 2160×1360 pixels for a fixed 1080×680 logical viewport. The earlier 4320×2720 capture expanded the logical canvas and is not minimum-window DPI evidence. The first 1080 toolbar capture visibly clipped navigation despite screen-bound tests passing; toolbar sizing and actual panel-bound checks corrected it.

A new App object is not an independent OS-process restart. Native file pickers/final confirmation-option handoff, external VSTs, physical audio/MIDI devices and same-machine FL comparison are not covered by this built-in chain. Offscreen SwiftShader images are real App drawing, not native desktop screenshots or a hardware frame-rate benchmark.

## Failure history retained

- Original aff57b2 independent audit: 87 unique tests, 85 passed and two genuine failures. Both original failures were repeated; the old report remains bound to that source.
- Inspector accessibility queries initially counted both Label and TextRun. Tests now resolve semantic Label plus the exact Slider row; PCM/history assertions remain.
- WGPU timing exposed an intermediate .53 history snapshot before .57 final gain, requiring two Undos. The explicit pointer transaction fixes the production issue, with deterministic deadline-crossing tests.
- Held-pointer discrete command testing originally froze an actively sampled gain. An identical no-key control proves complete Project equality for the same frames; all non-Gain, selection, ownership/history, release and command assertions remain.
- A cancelled/disabled Slider still reported the retained native pointer position and could change the project after the saved snapshot. `stop_dragging` now runs before painting those controls; actual disk/project/dirty/history checks pass.
- Focused Play + Space, stale Settings background focus and pre/post popup ownership had real failing regressions before the corresponding fixes. A standalone fabricated repeat event was corrected to an actual down/hold/repeat/release sequence.
- A private Workspace field access caused a compile failure; a narrow read-only query fixed encapsulation. A mistakenly started older executable after that failure was immediately interrupted (exit 130), its log/captures marked INVALID and excluded from all results. Subsequent acceptance used `cargo test` so execution follows successful compilation.

## Remaining integration gates

Full no-default coverage, all-feature compilation/tests, strict Clippy, fmt, Python/package checks and exact-commit Windows quality/preview are pending at this checkpoint. Later results must be appended with their actual source attribution; historical aff57b2 Windows success does not attest these edits.

## Detail

- [Deletion ownership](20261017-automation-delete-fix.md)
- [Inspector capabilities and pointer transactions](20261017-clip-control-capabilities.md)
- [Workstation visuals and input ownership](20261017-workstation-visual-refresh.md)

The broader professional gaps from the strict audit remain: general multi-source plugin offline rendering, low-latency device qualification, advanced melody/recording workflows, sidechain/multi-output, stretch/warp and portable-media collection. This bounded repair does not claim those completed.

## 15:25 UTC full-suite feedback and repair

The first complete no-default run exercised 2,047 unique tests: 2,039 passed, seven failed and one existing opt-in test was ignored. The failures were not suppressed. The Inspector genuinely exceeded its requested 240-point width because slider rows used an outer-container budget; production rows now reserve their actual inner label/readout width, long property values wrap and routing choices truncate. Existing movement, piano anchor, flat-button feedback and menu-navigation fixtures were corrected to measure the actual refreshed geometry. The minimum floating Piano test now observes the real velocity lane and requires its full containment and a usable height instead of adding the obsolete 75-point constant.

A fresh compilation then ran all seven originally failing cases: seven passed, zero ignored. Python discovery ran 368 cases: 359 passed and nine existing platform/optional cases were skipped. Complete no-default repetition and all-feature gates remain running. A further fine-decimal/Shift readout-width edge is being qualified before final acceptance.

## 15:34 UTC complete internal-engine result

The frozen internal-engine executable at the `7fb749e` source content completed all 186 batches: **2,046 passed, zero failed, one existing opt-in ignored**, covering 2,047 unique cases. Strict all-feature/all-target and no-default/all-target Clippy passed at that source, as did all-feature application/helper compilation. The dependency's existing `fetch_update` deprecation warning remains recorded separately from root Clippy failures.

A subsequent fine-decimal Inspector hardening preserves adaptive precision while reserving six decimal places. Its new Shift-drag test initially included offscreen Mixer Pan controls by horizontal position; it now identifies actual Inspector accessibility descendants, retaining all descendant numeric bounds (including any overflowing control), panel containment and exact imported-value checks. Fresh rendered qualification and the final all-feature complete suite remain pending; the earlier full pass is not silently reattributed to later source.

## 15:38 UTC final fine-adjustment qualification

The observed AccessKit header is a `Role::Label` whose `value` is `INSPECTOR`, not a `label` property. The test now follows its actual ancestry and checks every numeric descendant; it never filters away an overflowing Inspector control by coordinates. After this locator correction, a fresh all-feature build passed the unique precision case (all four Channel/Mixer × 240/340 scenarios, exact idle values and real Shift drag/hold/release). Four fresh WGPU native-generator/effect Inspector images at 240/340 were inspected and show the action rows contained and wrapping. These are mock-native-plugin UI state fixtures, not third-party plugin loading acceptance.

The final all-feature/all-target suite and strict Clippy are running from integrated source `93e85ad`; Windows exact-commit checks are next. Publication to the existing development branch is a testable checkpoint, not an acceptance declaration.

## 15:52 UTC final all-feature suite passed; remote verification blocked

Integrated application/test source `93e85ad` plus documentation/package metadata checkpoint `a710c4d` passed the complete Linux all-feature/all-target test run across five executables: **2,719 passed, zero failed, nine existing opt-in ignored (2,728 unique cases)**. The App accounts for 2,647 pass/nine ignored; helper 59 pass; protocol executables 5+6+2 pass. Every discovered test was scheduled exactly once, in isolated bounded batches. Final-source strict Clippy passed both feature configurations, formatting passed and final all-feature App/helper binaries built successfully. Final-source no-default focused repetition passed 45/45; a complete final-source internal-engine repetition is still running.

GitHub push was rejected twice by the write-authorization gate, including one retry after retrieving the prior explicit branch authorization. No alternate write route was attempted. Remote remains last verified `aff57b2`; the user was asked for fresh explicit approval of this round's source/docs/tests push to the same development branch. **New Windows CI has not started**, and old Windows results are not evidence for this revision. Local commits/source backups are retained. Main-branch merging and installer publication remain outside this checkpoint.

## 16:02 UTC final internal-engine repetition passed

The complete final-source no-default App suite finished all 186 batches: **2,047 passed, zero failed, one existing opt-in ignored**, covering 2,048 unique tests. This includes the new fine-precision/Shift regression. Together with the final all-feature/all-target suite (2,719 passed, zero failed, nine opt-in ignored), both feature configurations have complete discovery-based coverage on the final application/test source. Counts overlap between configurations and must not be summed into a unique-test claim. Formatting, strict Clippy for both configurations, all-feature binaries, and Python discovery remain successful as recorded above. Only documentation changed after those source builds.

Cloud-local repair gates are complete. Remote push/Windows exact-commit validation remains blocked pending the requested authorization; no new Windows or real-device/plugin success is claimed. The source/evidence ZIP contains the committed source, rendered screenshots and complete final test logs, with the original audit failures retained in this history.
