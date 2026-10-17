# Windows strict-topology terminal reporting wait

## Historical failure remains unresolved

- Published candidate: `6f4046425ee09d6cd7983db159315f11a061af53`.
- Windows quality run: `39028484318`; 2,551 passed, 1 failed, 21 ignored.
- Failing test: `strict_clone_dry_topology_preserves_descriptors_and_sends_exact_positive_zero_inputs`, one of 378 sequential topology cases.
- The stack identifies `Job::finish` calling the generic 15-second `wait_until`, awaiting the actual receipt termination acknowledgment. It does not identify a protocol stage, child exit state, or the topology case.
- The run retained `strict-bounce-mock-2528-46` in runner temporary storage, but uploaded no artifact containing its audit or ledger. Historical fixture contents are unavailable. No fresh run can reconstruct them.
- The failed test unwound and the suite finished. Its guard drains admission after cancellation/release, so this attempt did not remain permanently blocked after that cleanup. This does not prove the original delay was benign scheduling, nor exclude a pre-cancellation production stall.
- The earlier late-getter Python-finalizer fixture failure is a separate incident; no evidence connects it to this topology failure.

## Proven test-contract defect and narrow correction

`normal_job` allows an original 20-second render deadline, while the old reporter failed after only 15 seconds. The renderer subsequently permits a five-second cleanup reporting grace. Thus the observer could fail while authorized work was still within its original budget. This mismatch is proven from source; it is not a diagnosis of the unavailable historical child state.

Only test code changes. `Job::finish` computes one immutable observation deadline at entry: the later of entry time and the original render deadline, plus a documented mirror of the renderer's five-second cleanup reporting grace and one explicit second of observer allowance. Checked additions fail explicitly on overflow. Polls cannot renew the window. This changes neither the original render deadline nor any production protocol, cleanup, cancellation, kill or reap policy.

The finite reporting window is **not an OS termination guarantee**. Production retains the same child and admission after cleanup grace expires until concrete completion. The existing failure guard likewise keeps draining ownership rather than abandoning a live child; overall test runtime is therefore not claimed to be bounded by the reporter.

On observation expiry, the fixture writes and prints the actual receipt status, acknowledgment, admission state, topology, last command, bounded ledger/audit tails, and whether the original deadline elapsed. Outcome consumption stays behind the receipt's acknowledgment gate. File observations are explicitly best effort: unreadable or partially written JSON lines can be omitted. Original outcome, destination, acknowledgment, admission, topology, positive-zero PCM, cooperative shutdown and independent PID-exit assertions remain unchanged.

## Concrete ownership evidence

The fresh-helper owner observes the actual owned `Child::try_wait` status and requires pipe cleanup before returning `CleanupState::Reaped`. Only that result allows `StrictBackend` to release its client/registration. The renderer then destroys the backend, sends the outcome, marks termination, releases admission, and finally sets the receipt acknowledgment. Kill requests and Python exit-intent markers cannot substitute for concrete reap.

## Validation

- Deterministic reporting-window test: passed, including original-deadline coverage, exact expiry boundary, late entry, and non-renewal under later observations.
- Complete 378-case dry-topology test: passed on Linux, 16.12 seconds total. This is a per-job observer; the matrix's aggregate duration is not its timeout.
- Independent read-only review: passed; no production edits or weakened terminal/reap checks.
- Full strict-backend group: 43 passed, 0 failed (71.75 seconds), covering all shared `Job::finish` callers.
- `cargo clippy --locked --all-features --all-targets --jobs 1 -- -D warnings`: exit 0; the pre-existing vendored `fetch_update` deprecation warning is unchanged.
- `cargo fmt --all -- --check` and `git diff --check`: passed after broader validation.
- No Windows rerun or Windows qualification is claimed for this correction. The historical quality failure remains a failed result.
