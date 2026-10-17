# Linux test compilation repair — 2026-10-17

Base: `314779d` (`test: accept pinned NOIRE project schema upgrade`).

## Scope

Five stale test compilation errors were repaired without changing test assertions:

- Import `OccurrenceStamp` inside genuine-review tests.
- Initialize the topology fixture's `whole_join_identities` to the same empty vector used by the ordinary fixture constructor. This fixture has no whole-song join identities to retain at construction.
- Read the conversion's sealed live binding from `job.stopped.binding`, its current production owner, retaining all existing exact-identity checks.
- Set `metadata_getter: None` in both synthetic strict-helper failures. Neither fixture supplies validated metadata-getter evidence.

No public API exposure, new ignored tests, relaxed assertions, or feature-gate changes were introduced. These mock/pure tests do not establish genuine-plugin or Windows runtime qualification.

## Baseline

The previous pristine Linux run at the same base SHA reportedly passed the all-feature binary build and failed test compilation with these five errors. Its retained QA archive is the source for that prior result; it is not represented here as a fresh run. The untouched original source remains available at `314779d`.

## Environment and commands

Cloud Linux toolchain setup:

```sh
source /workspace/scratch/c0dd1d60f798/daw-tooling-20261017/env.sh
cd /workspace/scratch/c0dd1d60f798/daw-cloud-20261017
```

The environment selects Rust 1.99.0, a local Debian development sysroot, two Cargo build jobs, and disabled dev/test debuginfo. No global toolchain installation is required.

## Initial repaired-tree results

- `git diff --check`: pass.
- `rustfmt --check --edition 2024 src/app/clip_render_genuine_review.rs src/app/clip_render_genuine_topology.rs src/app/clip_render_genuine_topology_app_tests.rs src/instrument_bounce_render_tests.rs`: pass.
- `cargo test --all-features --no-run`: pass, all five test executables built (log `all-features-test-compile.log`). Nine app test warnings exposed unused stream replay APIs/reexports; a separate vendored dependency deprecation warning also remains.

Using the resulting `citrus_studio-8de976d3546a39f5` executable with each filter and `--test-threads=1`:

| Filter | Initial result |
| --- | --- |
| `clip_render_job::genuine::review::tests` | 9 passed |
| `clip_render_job::tests::genuine_topology_app_tests` | 24 passed, 2 failed |
| `strict_failure_conversion_preserves_every_exact_category_and_metadata_refusal` | 1 passed |
| `preceding_render_fault_keeps_first_exact_strict_cleanup_error_across_later_observations` | 1 passed |

All eight `owned_topology_actual_*` tests passed. The two failures were existing neighboring tests, not established baseline failures:

- `current_source_stop_reaches_callback_while_replacement_compile_waits`
- `restoration_successor_gate_preserves_event_drain_stop_fault_clear_and_cleanup`

Each also failed in its own fresh process before reaching the Stop assertions: the harness never polled the prepared playback start created by `toggle_play`. The real GUI polls this after timeline runtime and before the clip-render job. Logs `linux-repaired-tests.log` and `linux-dispatch-isolated.log` retain the failed results; `linux-strict-failure-tests.log` retains the strict conversion results. Logs are under the tooling directory's `logs/`.

## Additional narrow harness and observer coverage repairs

The focused dispatch wait helpers now opt into an ordinary playback poll that includes the production prepared-Play poll in GUI order. The paused genuine four-operation ledger retains its original polling path. No manual playback state setters, new callback pumping permissions, relaxed deadlines, or removed assertions were added. A new regression requires an actual pending prepared Play, callback/app playback completion, and successful Stop.

Only the parent reexports of `StreamFinishPoll` and `StreamReplayEvidence` are Windows-gated, matching their actual external consumers. Linux keeps the implementation and synthetic coverage. A new joined-source-only stream regression checks writer evidence and rejects raw song export before creating a file when no clone exists. This exercises the previously unused export methods rather than hiding them with dead-code allowances or disabling Linux coverage.

Final rebuilt-tree results are pending the integration validation run. The manually ignored genuine selected-clip qualification remains ignored; it requires exact run binding, once-only authorization, and an owned-group watchdog. No native-plugin qualification is claimed.

## Integration follow-up and retained failures

The first rebuilt full-suite pass still exposed three focused dispatch failures (including the new regression). A diagnostics-only reproduction established: a prepared start existed, `issued_request=None`, prepared token `None`, no queued callback work, matching callback/mailbox request 1, and no toast, while workers were Ready. The focused harness stopped invoking callbacks after asynchronous worker settlement, but prepared-token publication occurs at callback completion. The real device continues invoking callbacks.

The two focused service helpers now also service an actual pending prepared Play. The genuine paused four-operation ledger is unchanged. The original current-source Stop test additionally waits for both callback playback and the App's consumption of that exact acknowledgment, retaining its existing `app.playing` assertion. Initial diagnostic and intermediate failure logs are preserved as `dispatch-diagnostic-only.log` and `dispatch-complete-repair.log`.

Two other existing audio tests failed once during broad serial batches: the core loop-restart missing-identity test expected immediate prepared-request acceptance; the all-feature capture revocation test expected an immediate combined Ready snapshot. Neither failure reproduced in five original frozen-binary isolated runs (`original-safety-isolates.log`). They are not claimed as proven baseline failures or deterministic product bugs.

Reviewed test-only robustness changes account for explicitly permitted asynchronous worker behavior:

- Loop restart: retry the same production request admission using the existing two-second mock-worker wait, retaining no-allocation/no-drop checks on each attempt. No DSP rendering, reset, token fabrication, or musical-time advancement is introduced. Every downstream missing-identity/stopped assertion remains.
- Capture revocation: after callback readiness is genuinely republished, observe combined worker/callback readiness using the existing bounded wait. No callback, DSP, or readiness republication occurs during this wait. Immediate negative checks after callback entry/pending commands and after the admission fault remain unchanged.

These changes tolerate legal transient worker/admin state; the initial failures and clean original isolates are both retained as evidence.

A subsequent topology rerun hit `AlreadyExists` in fixture directory creation because a reused process ID collided with an intentionally retained directory from a failed run. That is recorded separately from assertion failures. The original directory was not deleted or reused. Subsequent runs set `TMPDIR=$(mktemp -d "$DAW_TOOLING/repair-tests-XXXXXXXX")` so each run owns a fresh directory while preserving failed evidence.

## Corrected affected-suite results

Fresh unique temporary directory; serial execution, no competing test runner:

```sh
cargo test --all-features --bin citrus-studio --no-run
# Execute target/debug/deps/citrus_studio-8de976d3546a39f5 separately for each filter:
# genuine_topology_app_tests
# audio::tests::capture_settlement_tests::
# audio::tests::stopped_epoch_safety_tests::loop_restart_tests::
# joined_source_only_stream_keeps_writer_evidence_but_refuses_complete_song_replay
# Each invocation uses --test-threads=1 --nocapture.
```

All-feature results: **27/27 topology**, **7/7 capture settlement**, **19/19 loop restart**, **1/1 joined-source export refusal**. No failures or ignored tests in these groups. Log: `repaired-affected-all-feature-unique-tmp.log`; compile log: `repaired-final-all-feature-compile.log`.

```sh
cargo test --no-default-features --bin citrus-studio --no-run
# Execute target/debug/deps/citrus_studio-5110e11b0710c272 separately for:
# audio::tests::capture_settlement_tests::
# audio::tests::stopped_epoch_safety_tests::loop_restart_tests::
# Each invocation uses --test-threads=1.
```

No-default results: **7/7 capture settlement**, **19/19 loop restart**. Log: `repaired-affected-core-tests.log`; compile log: `repaired-final-core-compile.log`.

The whole-workspace final format/lint/build gates and aggregate test accounting are owned by the integration report. Earlier strict all-feature Clippy passed after the observer coverage repair; the vendored upstream `fetch_update` deprecation warning is retained, not suppressed or changed here.
