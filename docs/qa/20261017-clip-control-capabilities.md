# Clip Inspector controls reflect actual rendering support

Date: 2026-10-17. Base: `aff57b2b84e3e5b7179d39269518b76397fc6c6e`.

**Current verified status:** the final integrated focused run compiled fresh and
passed all 44 tests with genuine offscreen WGPU capture, including all 12 Clip
control/cancellation lifecycle tests. See the final validation record below.
Earlier “pending” statements are retained as dated-by-stage handoff history,
not the current status. Full-suite gates were still running at this document's
final focused-test update.

## Correction and scope

The Inspector no longer offers controls that have no consumer for the selected
Clip kind. This is an honest capability correction, **not implementation of a
continuous Pattern envelope**.

| Clip kind | Gain | Fade in / Fade out | Mute |
| --- | --- | --- | --- |
| Audio | Editable | Editable; inherited Audio fade domains retained | Editable |
| Pattern | Editable note gain | Hidden; volume automation / audio-render guidance shown | Editable |
| Automation | Hidden | Hidden; source-curve guidance shown | Editable |

The Audio section remains `CLIP ENVELOPE`; Pattern uses `CLIP LEVEL`; Automation
uses `CLIP PLAYBACK`. Pattern's short hint reads “For fades, automate volume or
render to audio.” Automation's hint reads “Edit the automation curve to change
its values.” The capability correction is local to the selected Clip's
Inspector section. Subsequent validated repairs bound its continuous-pointer
history, concurrent Playlist commands, and save/cancellation lifecycle. These
changes do not rearrange other workspace controls or change the project format.

Existing serialized gain/fade fields are preserved, including ignored Pattern
fades and Automation gain/fades from older versions. Merely selecting a Clip
does not rewrite its project data, mark it dirty, or add Undo history.

## Consumer audit

- `src/timeline.rs`, `compile_pattern_clip` / `push_note_pair`: Pattern gain
  becomes `TimelineEventKind::NoteOn.gain`; no Pattern fade is compiled.
- `src/audio.rs`: the native voice uses note velocity and note gain. A plug-in
  instrument receives their product as MIDI velocity, clamped and rounded to
  the existing 1–127 note-on range. Pattern gain is therefore note strength,
  **not a post-instrument continuous gain envelope**. In particular, the
  pre-existing plug-in minimum-velocity behavior means zero gain is not a
  guaranteed plug-in mute. The UI tooltip describes the MIDI-velocity behavior;
  the supported Mute control remains available. This patch does not change it.
- `src/export.rs`, `collect_pattern_events`: ordinary native Pattern export
  consumes Clip gain in event amplitude. No Pattern fade is consumed.
- `src/timeline.rs`, Audio clip compilation, and `src/export.rs`,
  `prepare_audio_clip` / `mix_prepared_audio`: Audio gain and compiled fade
  envelopes are both consumed. The common implementation is `src/clip_fade.rs`.
- Automation compilation evaluates the source curve through its placement
  timing and mute state; it does not apply generic Clip gain or fade fields.

A real Pattern fade would need an explicitly scoped, per-placement continuous
audio envelope through overlapping voices, native and plug-in routing, playback,
offline export, and relevant tails. Multiplying note-on velocity once is not that
implementation. It is deliberately not substituted here. Existing automation
target and export support limits still apply; this change adds no new target or
selected-clip conversion support.

## Preserved red evidence

The original strict acceptance report and inputs at `/tmp/daw-strict-evidence`
are unmodified. At the base revision, the real pointer changed Pattern Fade in
from 0 to 1, but all **792,000 PCM samples were identical**. The original
`strict_creation_pattern_fade_control_must_change_rendered_pcm` test remains a
historical failing requirement for the old editable control. It was not changed
to expected-failure, ignored, deleted, or made to pass by weakening its PCM check.

Evidence identities:

- `strict_creation_acceptance_tests.rs` SHA-256:
  `5ca981fe2f6f8a8e961223e2d286f429dd1d66cecded1d656bf7f4b84fd2ddef`
- `logs/defects-reproduced-isolated.log` SHA-256:
  `d32063170181a061ad49d8e4b20843aa9acc77ff3c9486941cbd21598d7d717e`
- Both original `final/pattern-fade-ignored/fade-zero.wav` and `fade-one.wav`:
  `c68af8dec443909f549a6a098a89b10f4e1d7937b5e78f1a83307bbf5fd4a6cb`

The new acceptance contract is that an unavailable fade is **not editable** and
the available controls actually affect output. It does not claim the previously
missing Pattern-fade capability now works.

## New production-input tests

`src/app/clip_control_capabilities_ui_tests.rs`, registered under the existing
`headless_ui_tests` module, initially introduced these three tests (now eight,
plus four independent lifecycle tests described below):

1. `inspector_clip_capabilities_only_present_effective_controls_on_repeated_selection`:
   real Playlist pointer selection cycles Audio → Pattern → Automation → Pattern
   → Audio. AccessKit nodes from actual `App::ui` must expose the expected
   gain/fade rows and live sliders, omit unsupported rows, and show guidance.
   Stored legacy unused values, the complete serialized Project, clean state,
   and empty Undo history are preserved throughout.
2. `inspector_audio_gain_and_each_fade_change_actual_pcm_and_undo_redo_exactly`:
   imports a one-second constant WAV through the actual Browser selection/import
   worker. Separately edits Gain, Fade in, and Fade out with real slider pointer
   events. The production renderer writes 48 kHz Float32 stereo at unity export
   gain; each edit must change more than 10,000 samples. Audio gain is checked
   against the complete scalar-multiplied reference. Both fades require the
   correct silent/full endpoints, equal-power midpoint and monotonic sample
   envelope. Each edit is one Undo transaction, and Undo/Redo must restore exact
   project JSON **and all 192,000 rendered PCM samples**.
3. `inspector_pattern_gain_changes_actual_pcm_without_rewriting_notes_and_undo_redo`:
   real Inspector Gain input changes native Pattern audio by the measured gain
   across the entire rendered buffer, changes more than 10,000 samples, and
   leaves source notes/timing and fade values intact. One Undo/Redo restores
   exact JSON and every PCM sample. The sustained fixture ends with the
   arrangement so voice-retirement threshold differences cannot mask the scalar
   check.

These are real production UI/event and offline-render tests with deterministic
fixtures. They do not establish hardware playback, third-party instrument
response, Windows native input, or actual screen visibility at every window size.

## Initial author-time handoff status (historical)

- Direct `rustfmt --edition 2024 --config skip_children=true` completed for the
  three edited Rust files.
- `git diff --check` passed.
- Cargo compilation/tests are **not yet run for this patch**, per the integration
  coordinator's request to avoid competing builds. The test expectations above
  describe authored assertions, not passed executions.

Integration command (run against the merged source, with an isolated TMPDIR):

```sh
source /workspace/scratch/c0dd1d60f798/daw-tooling-20261017/env.sh
export TMPDIR="$(mktemp -d /tmp/citrus-clip-controls-XXXXXXXX)"
cargo test --offline --locked --no-default-features --bin citrus-studio \
  clip_control_capabilities_ui_tests -- --nocapture --test-threads=1
```

The same tests should also be included in the eventual all-features gate.
Historical strict acceptance remains bound to its original source revision;
new integration results must be recorded against the actual integrated revision.

## Additional real defect found during integrated validation

The first compiled run exposed duplicate accessibility text nodes in the test
locator. The locator now requires a static `Role::Label` and separately finds
the unique enabled `Role::Slider` with the exact range and matching row. It
does not choose an arbitrary text match. Original failing logs are retained.

The next integrated run passed Audio gain/fades and the repeated-selection
capability test, but the real Pattern Gain Undo assertion failed with offscreen
capture enabled. Per-frame diagnostics established the cause: one pointer action
produced an intermediate gain of 0.53 and a release value of 0.57. The 50 ms timed
history observer captured 0.53; before Undo, `history_snapshot` still held 0.53
and the current Project held 0.57. Undo's pending-history flush created a second
entry and restored only 0.53. There was no text focus, popup, gesture blocker,
or settling plug-in topology. A no-capture pass did not establish a repair.

The correction gives **Clip Inspector Gain/Fade pointer gestures** the existing
Playlist gesture transaction owner. On the first drag-start response it retains
the pre-edit Clip in a Project snapshot. The timed observer is suspended by that
owner; existing pointer-release and editor-interruption paths commit the final
state as one explicit transaction. Only gesture start clones a Project. Other
Inspector fields and text-entry semantics are unchanged by this bounded repair.

The original PCM/Undo tests again use the ordinary `UiHarness::key` call. The
temporary diagnostic helper is removed; its failed and passing logs remain
evidence. A fourth test,
`inspector_envelope_pointer_gestures_ignore_observer_deadlines_and_remain_separate`,
crosses both observer deadlines deterministically between real pointer frames
for Pattern Gain, Audio Gain, Fade in, and Fade out. It requires no history entry
during a drag, exactly one on release, distinct entries for two consecutive
gestures, and exact full-Project Undo/Redo. Only observer clocks are adjusted;
musical edits and history commands remain real UI events. No sleep, extra focus
click, or manual history flush is used to hide the failure.

This additional patch is formatted and source-reviewed; its execution is pending
the coordinator's next exact-revision compiled run. Do not claim it passed based
on the earlier no-capture success.

### Pointer-owner lifecycle boundaries

Further review covers the new shared transaction owner rather than treating one
happy-path Undo pass as sufficient:

- Playlist Delete, Duplicate, Group, Ungroup, and crossfade entry points refuse
  a concurrent pointer-owned transaction (including the cancel-until-release
  period). A fresh command after release works normally. This prevents a
  separate edit from replacing history before the old pointer snapshot commits.
- Manual save first seals an active Playlist gesture, then suppresses its old
  pointer until release. The Clip Inspector respects that suppression and lost
  window focus even when the save completes before the next UI frame paints a
  save blocker. `mark_saved` therefore cannot absorb an intermediate snapshot.
- Background recovery is deferred while the pointer owns its snapshot, before
  updating the recovery deadline. The common save entry also refuses an
  intermediate autosave. Release allows the still-due recovery to proceed.
- Additional real-input tests preserve a pending Clip-name edit as a separate
  transaction, cover lost focus / Settings / editor-switch interruptions, and
  reject held-pointer Delete / Duplicate / Group / Ungroup while verifying each
  command works after release. Recovery is checked on disk, with full Project
  Undo/Redo preservation. The lifecycle reviewer supplies separate manual-save,
  New Cancel/Discard, and discarded-multipass tests.

These bounded entry guards and the new tests are authored and formatted here;
the coordinator owns their exact integrated-revision execution. Earlier failure
logs are not overwritten and no existing PCM or Undo assertions are relaxed.

The first integrated discrete-command boundary run retained both Clips and all
non-Gain fields, but failed its frozen-Gain assertion (0.62 to 0.75 while the
pointer was still down). That failure remains in `validated-build-focused.log`.
The revised test pairs each key frame with an identically arranged, still-held
no-key harness. It requires exact whole-Project equality, including Gain, on
every corresponding frame and after release; separately, every field other
than the owned Gain must remain identical to the held baseline. Selection,
active gesture ownership, zero intermediate history, exact Undo/Redo, and a
working fresh command after release are still required. This is an explicit
control for ongoing pointer sampling, not permission for a key-induced change.
Execution of this stronger controlled comparison is pending integration.

Manual-save lifecycle validation then found a real first-cancelled-frame write:
the disk held Gain 0.48, while the still-held Inspector advanced the current
Project to 0.51. `ui.disable()` alone does not retire egui's existing
`potential_drag_id`; disabled Slider responses can still carry
`interact_pointer_pos`. Inspector paints before the Workspace cancellation pass,
so it sampled the old press once after saving. Its cancellation/focus guard now
calls `ctx.stop_dragging()` before constructing disabled widgets, clearing the
pointer ownership before any mutable slider can run. The cancellation flag still
lasts until release. The reviewer's unchanged manual-save test remains the
acceptance gate: on-disk/current equality, clean state, continued held movement
and release without rewrites, then exact one-step Undo/Redo.


## Final integrated focused validation

The coordinator compiled the integrated candidate fresh before running the tests.
The production-source baseline was
`e26a81b2673529cfd5c806082930646eeb20a660`, including the final pre-paint drag
retirement correction. The coordinator also restored the original strict
Pattern-deletion test body in `src/app/creation_chain_tests.rs` and clarified its
export-boundary comment; that compiled file's SHA-256 was
`6792e6cfc74b5a8bfb310c3cab43334af7ea966c6f71dd5e0a3b760170bcc687`.
This identifies the tested working tree without presenting its uncommitted
strict test restoration as part of the baseline commit.

`daw-tooling-20261017/validation/strict-fix-refresh/final-focused.log` records:

- Fresh Cargo test-profile compilation completed in 11.24 seconds.
- **44 passed, 0 failed, 0 ignored**; 2,003 filtered out; 71.04 seconds.
- Genuine SwiftShader Vulkan offscreen WGPU execution, not a static mock image
  or a native display/audio-device run.
- All eight capability/PCM/Undo/pointer-boundary tests passed. This includes
  Audio Gain and both actual fades, native Pattern Gain without note rewriting,
  unsupported-control absence, clock-crossing gestures, pending-name history,
  focus/modal/editor interruption, discrete-command isolation, and deferred
  on-disk recovery.
- All four independently authored lifecycle tests passed: held manual save via
  shortcut and common entry, New Cancel, New Discard, and forced egui multipass.
  Manual save requires the saved file and current Project to match while the
  original pointer remains held, rejects continued old-pointer movement/release,
  and verifies clean state plus exact one-step Undo/Redo.
- The full controlled discrete-key comparison passed: each key frame's entire
  Project, including Gain, equals its no-key peer. The earlier frozen held-Gain
  assertion was therefore a fixture mistake, not evidence that Delete changed
  Gain. Every non-owned field, selection, and history invariant remains checked.
- This focused batch also passed the original strict deletion test, the separate
  creation/PCM chain, shortcut ownership, and visual checks. Those other repairs
  are documented by their owners, not added to this feature's capability claim.

Final log SHA-256:
`5c972ce7782cf55ff945b7415a4bd6be15834d90e71a2d40a70723d3f77aa3fa`.

### Failure and correction trail retained

- Original unsupported Pattern Fade: the source, failed log, and identical PCM
  files under `/tmp/daw-strict-evidence` were rehashed after final validation;
  all four identities still match the earlier evidence record.
- `pattern-gain-diagnostic-capture.log` retains the real timed-history split;
  its no-capture counterpart was never used as proof of repair.
- Initial and independent lifecycle locator failures came from matching both
  static Label and TextRun nodes. The corrected helpers require `Role::Label`
  and independently identify the unique enabled, range-matched Slider.
- `validated-build-focused.log` retains the 39-pass/5-fail run, including the
  frozen-Gain fixture failure and four lifecycle locator failures. SHA-256:
  `596a2c91f9a0c87eb0773c758a6ea5aa21d63306f38b5a03efd8b20cea54b042`.
- `discrete-control-comparison.log` records the subsequent passing exact no-key
  control experiment before the final 44-test run.
- `lifecycle-focused.log` retains the real saved-0.48/current-0.51 cancellation
  defect. SHA-256:
  `3c70b11c2c8b1545581724035d9f8010d9de5d446aa02df234d697624c4f17a5`.
- The private-field compilation error was fixed through a narrow read-only
  cancellation query. The mistakenly launched old-binary run was stopped and
  marked `INVALID-stale-binary-after-compile-failure.log`; it is not validation
  of the corrected candidate.

No PCM, disk-state, or Undo assertion was removed to obtain the passing result.
Continuous Pattern fades remain unsupported and hidden with guidance; plug-in
MIDI-velocity semantics and all previously stated scope limits still apply.
The full 2,047-test no-default-features run had begun at this point; its result
and subsequent feature gates belong to the coordinator's final gate record and
are not claimed here before completion. This final update changes documentation
only; the verified Rust source is frozen.
