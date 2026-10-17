# Windows late-getter fixture exit evidence

## Original failure retained

- Published source: `12a9b375ddc11bc645d180b6878ee135d9b69302`.
- Windows preview run: `39024203064`.
- Root tests: 2,542 passed, 1 failed, 21 ignored.
- Failure: `strict_selected_bundle_actual_held_getter_completes_after_cancel_or_deadline_without_publish`, original `src/instrument_bounce_strict_backend_tests.rs:3285`, missing `exit` ledger row.
- The failure happened after assertions established real terminal EOF, held getter completion, tail metadata, a complete snapshot, and exactly one genuine late-write success/failure outcome.
- `Job::finish` waits for termination acknowledgment; the backend acknowledges child completion only after concrete OS reap. Reading the ledger before child termination is therefore not the supported explanation.
- The run retained a temporary fixture path but uploaded no artifacts. The historical audit, ledger and late-write outcome are unavailable. Forced termination before Python's `atexit` handler and handler failure cannot be distinguished conclusively. No claim is made that the historical root cause has been proved or that a product cleanup defect was observed.
- The nearby `EINVAL` traceback in fixture `1026` is unrelated to failing fixture `1051`.

## Narrow repair

The synthetic late-getter child owns no plug-in. After the actual late-write outcome, its normal path now records protocol completion and an explicitly labeled exit-intent marker, flushes its ledger, and calls `os._exit(0)`. This removes unrelated Python interpreter finalization from the production terminal cleanup's unchanged 100 ms natural-exit window. A marker is never treated as OS-exit proof.

The existing positive test retains getter/snapshot/write ordering, original cancellation/deadline and typed failure, no later DSP/MIDI requests, unchanged project/capture/destination, and no temporary WAV. It now additionally requires independent concrete reap with no cleanup error or diagnostic, rejecting abnormal exit and forced termination.

A separate negative control completes the same late snapshot/write path, enters an observed 30-second `atexit` blocker, and requires actual forced termination and concrete reap. It must retain the cancellation and cleanup failure, have no clean-reap evidence or exit marker, and publish nothing. The production 100 ms cutoff is not lengthened or retried.

Both tests print the completed attempt's outcome, ledger and disposal audit before assertions so future failures preserve attributable evidence in CI output. No production code, transport budget, real VST, user device, workflow retry or skip changed.

## Validation

- Unmodified isolated Linux equivalent: 1 test passed, both cancellation and deadline paths (2.07 s).
- Repaired selected-bundle group: 4 passed, 0 failed (2.43 s), including the forced-finalizer control.
- Independent read-only review: no blockers; exit intent and actual reap remain distinct.
- Full strict-backend group: 42 passed, 0 failed (71.55 s).
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo clippy --locked --all-features --all-targets -- -D warnings`: exit 0 (32.53 s); the pre-existing vendored `fetch_update` deprecation warning remains unchanged.
- Windows validation of the repair: not yet run. Linux results do not replace the failed Windows preview or qualify a Windows package.

## Combined integration gate

After integration with compact editor controls, the strict backend filter passed **42/42** within the combined **273/273** exact-name bounded test run. Formatting, strict all-feature/all-target Clippy and all-feature binaries passed on the integrated source. This remains Linux source/fixture evidence; the next exact-candidate Windows run is pending.


## Subsequent Windows failure and failed-stream repair

The first repair did not qualify Windows. On source `36f7d0ba343a0c0ae04ff6413ccea1ca614b3a53`, quality run `39029992024` failed the same held-getter test at line 3390: **2,552 passed, 1 failed, 21 ignored**. Unlike the original failure, this run retained a complete diagnostic JSON record in its failed-job log.

The failing attempt was cancellation. Its ledger proves actual host EOF, held-getter completion, remaining metadata and full snapshot assembly, followed by `late_snapshot_write_failed` with `OSError`, errno 22. There was no protocol-complete or exit-intent marker. The independent audit proves actual reap after forced termination, exit code 1, with the cancellation preserved as primary and cleanup failure separately reported. This is a valid fail-closed product terminal outcome, but it does not satisfy this fixture's intended positive clean-exit scenario.

The next source operation after logging the failed write was `sys.stdout = open(os.devnull, 'w')`. That unnecessary open/reassignment was removed. The natural-exit path needs only ledger writes before `os._exit(0)`; the deliberate blocked-finalizer control likewise does not need stdout recovery. The captured interval does not prove which operation blocked: `sys.__stdout__` can retain the original stream, and process scheduling remains possible. No historical destructor root cause is claimed.

A new regression uses a sole-owner synthetic stdout only after actual host EOF and snapshot assembly. Its write deterministically raises `OSError(22)`; subsequent flush or destruction logs a trap and blocks. Both cancellation and deadline paths must preserve snapshot/write/exit-branch ordering and independently prove clean OS reap. The original real-pipe scenario remains separate. New write-attempt and exit-branch markers make any later failure more attributable. No production deadline, cleanup contract or acceptance assertion was relaxed.

Validation of this second repair:

- Selected-bundle group: **5 passed, 0 failed** (4.52 s), including both new poisoned-stream paths, both existing real-pipe paths and forced-finalizer negative control.
- Mutation probe extracted the actual Python branch: repaired code exited 0 without a trap; restoring stdout reassignment synchronously entered the observed destructor trap, did not reach protocol completion, and required termination. This proves the synthetic regression catches the removed hazard; it does not reproduce the Windows root cause.
- Full strict-backend group: **44 passed, 0 failed** (74.63 s).
- `cargo clippy --locked --all-features --all-targets -- -D warnings`: exit 0; pre-existing vendored `fetch_update` deprecation warning unchanged.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- New exact-source Windows validation: pending. Linux evidence does not supersede either failed Windows run.

Local evidence is retained under tooling `logs/late-stdout-repair-selected.log`, `validation/late-stdout-repair/mutation-result.json`, and the accompanying `mutation_check.py`. Original Windows failed log and extracted JSON remain preserved as `windows-ci/39029992024-failed.log` and `windows-ci/39029992024-late-bundle.json`.
