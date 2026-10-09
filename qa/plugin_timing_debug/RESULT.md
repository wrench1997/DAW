# Callback timing: retained debug/source-linked QA receipts

**This is unoptimized correctness and scheduling-stress evidence, not an optimized latency benchmark or a complete qualification pass.** The test and production helper compiler profiles use `opt_level=0`, debug assertions and overflow checks enabled, with debug information disabled. Every retained run's process exited **101 (failed)**. Passing matrix cells do not erase failing tests, setup assertions, or earlier source defects. Separate optimized qualification is pending and is not included here.

Measured production source: `f68b9a0210e95f1d26ec3c67ca0e389bcdfa798b` (run004–006). Earlier run001–003 use `f164e91fb989f0475d4d6038fbe7fd8008b1cd49` and are distinct historical attempts. Initial preflight material is also retained separately.

## Exact final debug matrix

run005 is the coordinated quiet condition: **13/16 PASS**. run006 uses the exact same test/helper binaries with four numerical CPU-load threads for 1.1 seconds during each one-second measured segment: **10/16 PASS**. Every FAIL in these two matrices is classified `DeadlineMiss`.

| Budget B | Path | Callback partitions | run005 quiet | run006 CPU4 |
|---:|---|---|---|---|
| 128 | Ordinary timeline | Fixed | FAIL | FAIL |
| 128 | Ordinary timeline | Changing | PASS | FAIL |
| 128 | Stochas routed | Fixed | FAIL | FAIL |
| 128 | Stochas routed | Changing | PASS | FAIL |
| 256 | Ordinary timeline | Fixed | FAIL | PASS |
| 256 | Ordinary timeline | Changing | PASS | PASS |
| 256 | Stochas routed | Fixed | PASS | PASS |
| 256 | Stochas routed | Changing | PASS | FAIL |
| 512 | Ordinary timeline | Fixed | PASS | PASS |
| 512 | Ordinary timeline | Changing | PASS | PASS |
| 512 | Stochas routed | Fixed | PASS | FAIL |
| 512 | Stochas routed | Changing | PASS | PASS |
| 2048 | Ordinary timeline | Fixed | PASS | PASS |
| 2048 | Ordinary timeline | Changing | PASS | PASS |
| 2048 | Stochas routed | Fixed | PASS | PASS |
| 2048 | Stochas routed | Changing | PASS | PASS |

Fixed partitions use B; changing partitions cycle B,31,B/2,127. Sampling is 48 kHz, processing quantum 128, guard 2, with lookahead K = 3/4/6/18 and one-worker bridge L = 512/640/896/2432 samples for B = 128/256/512/2048. Each measured cell covers 48,000 frames / 375 quanta. Accepted cells require 375 completed blocks per worker, 375−K exact plugin output quanta and K startup/delayed-dry quanta per adapter, with no new deadline, overflow, bridge-gap, or drift errors.

All four B2048 cells passed in both conditions after the actual Surge Effects native latency 0→32 fence, stopped Retry, newer prepared timing revision, and explicit restart. This is a real replanned pass, not a bypassed cold fault. Reported PDC is 3L+32 for the routed path and 2L+32 for ordinary timeline notes; B2048 therefore records 7328 / 4896 samples. Exact note events, onset positions, sink peaks, before/after counters, identities, epochs and fault objects remain in each full JSON receipt.

## Other checks and the negative result

- run005 rejects B+1 raw callbacks **before any new submission** for all four budgets. B128/256/512 report `CallbackBudgetExceeded`; B2048 reports `UnsupportedCallback` for 2049 frames. All Retry checks remain stopped. Explicit restart passes at B256/512/2048; B128 fails with `DeadlineMiss` at endpoint 12002, epoch 6, sequence 1. This admission test as a whole remains failed.
- Fresh restored Surge controller/component/first-note proof passed. In run005, controller before/after is 0.8691863417625427; component volume before/after is −6.27905654907227 dB; one NoteOn [144,60,100] produces finite first-note audio (1024 frames, peak 0.21964623034000397, first nonzero sample 16). Input-state SHA256 and complete worker telemetry are retained. This is a fresh backend instance test, not GUI or hardware qualification.
- The four-second blank-source cleanup is **not a full pass**. run004 has zero source events and its natural tail is below 1e−6 during the 1.75–2.00 s quarter (peak 3.8933475821067987e−7), before a later `DeadlineMiss` at sequence 950 (approximately 2.53 s). This is limited evidence of tail decay. Last-second zeros after a fault cannot prove successful cleanup. run005 faults earlier at sequence 237 and records `cleanup_passed=false`; its subsequent recovery also faults at endpoint 11001, epoch 8, sequence 41. Both failures remain intact.
- run001's 0/16 matrix is an incomplete external-wrapper setup failure. run002 corrects CPAL control order and records 10/16 passes, three real paced deadlines, and three B2048 cold stalls. run003 reproduces the f164 source defect in all four B2048 cases: native FX latency becomes 32, timeline identity clears, processing stays suspended/Priming without a classified plugin fault, and submissions stay zero. It is trace-only, not quiet acceptance.
- run004 records 9/16 passes and 7 failures. Four B2048 cold assertions incorrectly expected worker latency to include lookahead before any submitted block; the production source correctly classified `LatencyDrift`. Its cold-assertion diagnosis and all failed receipts are retained. The correction leading to run005 changes the harness cold expectation only; lower-profile deadline failures remain real failures.

## Interpretation and scope

Callback runtime includes the source-linked harness/control wrapper and its master-output copy closure. The harness also adds sink capture, event inspection, allocations, pacing/sleep scheduling, and a 300 μs perturbation every seventh callback. Worker/helper DSP time is not separately instrumented. Callback-start lateness measures this host's pacing schedule. Neither metric is isolated native DSP execution time or physical-device latency.

This headless test exercises production Timeline/DspState/PluginChain/FixedQuantumAdapter paths with official genuine Linux Stochas and Surge modules. It does not exercise a physical CPAL/audio backend, qualify hard real-time scheduling, establish native-editor realtime safety, or imply Windows/Harmony Blueprint compatibility. No latency-tier recommendation follows from this debug matrix alone.

## Audit and reproduction

See `REPRODUCE.md`, `NORMALIZATION.md`, `INVENTORY.json`, `PROVENANCE_CHECKS.json`, and the untouched-measurement receipt copies. `ORIGINAL_README.md` and `receipts/status.json` are historical preparation records, not the final outcome. All recorded original SHA256 values retain their historical meaning even when a published text file has path-only normalization. `PUBLICATION_SHA256SUMS` verifies publication bytes. No plugin binaries, test/helper executables, serialized state, factory assets, or WAV payloads are distributed.
