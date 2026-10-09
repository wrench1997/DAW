# Parameter storage: bounded correctness result

All four default 2048 delivery cases and the fresh-state immediate-note test passed. Both test invocations exited 0. This is a new unoptimized correctness regression for the changed helper, not a new performance qualification.

Source: `fb7b91a82d31226485a22e9e5e0b73d1f9a1fe3f` (integrates bf573a4). All 124 production-file hashes match git objects.

- Test SHA256: `d09d2d72e4a5c76adad988c475247911713075e5a1d96570e1b396df96836362`
- Helper SHA256: `a29e4942b5fe027e9891a28e5d7f7611d2c5b2f12ef5f283599d15c4278aee64`
- Unchanged preallocated harness SHA256: `c38f30c4b50fe7eb1c5d69acec6c59dd941ba394797b21e3960acb6b5d37b072`

| Case | Delivery | PDC frames | Sink onset frame | Source On/Off | Core interval overruns |
|---|---|---:|---:|---|---:|
| b2048_routedfalse_changingfalse | PASS | 4896 | 10969 | 0/0 | 0 |
| b2048_routedfalse_changingtrue | PASS | 4896 | 10947 | 0/0 | 13 |
| b2048_routedtrue_changingfalse | PASS | 7328 | 7408 | 8/8 | 0 |
| b2048_routedtrue_changingtrue | PASS | 7328 | 7412 | 8/8 | 14 |

Every case observes genuine FX latency 0→32, a visible safe-stop fault, actual stopped Retry with a newer timing revision and fresh epoch, then explicit playback. Original finite-PCM/onset, event count/pitch/spacing, exact latency/PDC/epoch and worker/adapter delivery assertions remain. Each endpoint processes 375 quanta, delivering 357 exact outputs plus 18 startup quanta; no new worker deadline/loss/fault is permitted by the passing assertions. Capture overflow flags are false and frame/event overflow counters0. Nominal fixed callbacks end with an 896-frame remainder; changing callbacks use 2048/31/1024/127, ending at 48000 frames.

Fresh Surge: state is supplied at construction before the first Process; one immediate NoteOn[144,60,100], no settling note. First nonzero frame24, peak0.22329643368721008; controller0.8691863417625427 before and0.8691863417625427 after; component volume −6.27905654907227 dB retained. Input-state hash and complete observed fields are in SUMMARY.json. No preset/state payload is distributed.

Raw-core interval overrun counts for this run are reported in the table and SUMMARY.json. Passing buffered delivery therefore does not prove realtime wall-deadline compliance. The old optimized 199-file appendix remains historical 4fdfbc2/244f622 evidence and is not relabeled for this changed helper. No native GUI, physical audio device, speaker output or GUI/DSP-stall resolution claim is made. Timing quantiles and isolated native CPU service durations were not collected.

See INVENTORY.json for original/raw and published hashes. Only task-local path prefixes were normalized; original values and outcomes remain unchanged. The private reverse map remains outside this archive. No plugin binary, test executable, factory asset, state/preset, WAV payload or full repository source is included.
