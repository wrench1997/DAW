# Checked event admission: bounded correctness result

All four default2048 delivery cases and the fresh-state immediate-note test passed. Both test invocations exited0. This is a new unoptimized correctness regression for the changed helper, not a new performance qualification.

Frozen source BASE: `bf573a45826466db0952eb1f4bd598937c68d1c5`, plus reviewed Rust diff `482031cc4050ca3747db2b276a5b2957b0a55d32f98a07c684eb408683584e23`. Base alone is NOT the tested source. Full copied-source hashes and the independently verified41-file review manifest bind this run; all124 hashes exactly match final commit7d8a41693be5edf9857a044e9b6e75478b422059 (receipts/final-source-equivalence.json).

- Test SHA256: `eea1adf80330fe8380abeb146024a1197ca6831122a76f2f704480108637f667`
- Helper SHA256: `59b6bcbdb7a90b08fe5c8ebb2da2ba3ffe368c1089d70a6c7d4e7ab28b90d086`
- Unchanged preallocated harness SHA256: `c38f30c4b50fe7eb1c5d69acec6c59dd941ba394797b21e3960acb6b5d37b072`

| Case | Delivery | PDC frames | Sink onset frame | Source On/Off | Core interval overruns |
|---|---|---:|---:|---|---:|
| b2048_routedfalse_changingfalse | PASS | 4896 | 10989 | 0/0 | 0 |
| b2048_routedfalse_changingtrue | PASS | 4896 | 10947 | 0/0 | 12 |
| b2048_routedtrue_changingfalse | PASS | 7328 | 7371 | 8/8 | 0 |
| b2048_routedtrue_changingtrue | PASS | 7328 | 7375 | 8/8 | 14 |

Every case observes genuine FX latency0→32, a visible safe-stop fault, actual stopped Retry with a newer timing revision and fresh epoch, then explicit playback. Original finite-PCM/onset, event count/pitch/spacing, exact latency/PDC/epoch and worker/adapter delivery assertions remain. Each endpoint processes375 quanta, delivering357 exact outputs plus18 startup quanta; no new worker deadline/loss/fault is permitted by the passing assertions. Capture overflow flags are false and frame/event overflow counters0. Nominal fixed callbacks end with an896-frame remainder; changing callbacks use2048/31/1024/127, ending at48000frames.

Fresh Surge: state is supplied at construction before the first Process; one immediate NoteOn[144,60,100], no settling note. First nonzero frame25, peak0.21446438133716583; controller0.8691863417625427 before and0.8691863417625427 after; component volume−6.27905654907227dB retained. Input-state hash and complete observed fields are in SUMMARY.json. No preset/state payload is distributed.

Changing ordinary/routed debug cases retain12/14 raw-core interval overruns. Passing buffered delivery therefore does not prove realtime wall-deadline compliance. The old optimized199-file appendix remains historical4fdfbc2/244f622 evidence and is not relabeled for this changed helper. No native GUI, physical audio device, speaker output or GUI/DSP-stall resolution claim is made. Timing quantiles and isolated native CPU service durations were not collected.

See INVENTORY.json for original/raw and published hashes. Only task-local path prefixes were normalized; original values and outcomes remain unchanged. The private reverse map remains outside this archive. No plugin binary, test executable, factory asset, state/preset, WAV payload or full repository source is included.
