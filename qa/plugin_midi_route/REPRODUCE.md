# Publication layout and safe reproduction

Work in a disposable copy, never directly in the checked-in receipt directory.
This bundle contains scripts, attribution and logs only; it does not include
compiled binaries, plugin bundles, factory assets, or generated WAVs. Original
negative observations are retained alongside later successful bounded tests.

Use one empty working root (called VALIDATION_ROOT below):
- Copy the earlier `qa/real_vst3` reproduction scripts into that root and follow
  their `REPRODUCE.md` to obtain the verified official bundles under `plugins/`.
- Run their source-defined Stochas pattern builder to produce the owned pattern
  and actual newly-loaded blank-state export under `receipts/`.
- Copy this directory to `VALIDATION_ROOT/route-acceptance`. Its existing
  historical receipts belong to the publication; save a copy before reproduction
  writes fresh files. New results must retain their own source/binary attribution.
- Set DAW_SOURCE explicitly to a clean Git checkout of the intended DAW source.
  A plain extracted source ZIP without Git metadata is insufficient for the current
  builder's source-commit attribution. The literal fallback `<DAW_SOURCE>` is a
  publication placeholder, not a working local path.
- Set VST3_VALIDATION_ROOT explicitly to the absolute disposable working root.
  Match the compiler/dependencies and reserve the Cargo target before building.
  Never share a target with another active build or paced measurement.

The scripts intentionally construct a source copy in `route-acceptance/source`;
repeating preparation replaces that generated directory. They do not modify the
DAW_SOURCE checkout. Run `bash run_native.sh` only after preparing inputs and
building the copied executables. This launches the separately acquired real plugins.
`verify_receipts.py`, by contrast, only reads checked-in data and never runs them.

The following retained instructions describe the measured harness. Terms such as
"native" in test names mean compiled production Rust code and real plugin loads;
they do not mean a native desktop/editor GUI or physical audio-device test.

# Real VST3 production MIDI-route validation

This external acceptance harness reuses the production Timeline / DspState / PluginChain /
FixedQuantumAdapter graph. It replaces only the test fixture's mock-construction section
with genuine VST3 PluginChain loads. No helper-to-helper MIDI forwarding is used.
Production runtime source is copied without modification; added code is confined to the
snapshot's test module. Production files and binary attribution are hashed in receipts.

## Inputs

Use the independently verified official Linux bundles under ../plugins:
- Stochas 1.3.13: https://github.com/surge-synthesizer/stochas/releases/tag/v1.3.13
- Surge XT and Surge XT Effects 1.3.4:
  https://github.com/surge-synthesizer/releases-xt/releases/tag/1.3.4

Generate ../receipts/stochas-qa-pattern.state using the earlier stochas_pattern.py
source-schema builder and actual plug-in re-export. It contains our own deterministic
60/64/67/60 four-step test pattern. ../receipts/stochas-initial-state.bin is an actual
SaveState of a newly loaded blank Stochas instance. Neither binary plug-in nor factory
preset assets are included in this receipt package.

## Build and run

1. Reserve a Cargo target; don't race another checkout using the same target.
2. Set DAW_SOURCE to the intended checkout. Set CARGO_TARGET_DIR, Cargo/toolchain PATH,
   RUSTUP_HOME/CARGO_HOME and any platform linker variables normally required by the
   project. Set CARGO_PROFILE_DEV_DEBUG=0 if matching this debug receipt.
3. Run python3 build_and_copy.py. This copies source, appends the native test harness,
   builds the actual helper and test executable, and copies exact compiler-artifact paths.
   It never selects an artifact merely by modification time. Release the build target.
4. Run ./run_native.sh > receipts/native-tests-final.log 2>&1. It runs copied binaries.
   Native tests use 48 kHz stereo, 128-frame processing quantum, callbacks paced by a
   monotonic clock, and additional 300 microsecond jitter every seventh callback.
   There is no waiting inside an audio callback or its internal quantum loop.

NATIVE_ROUTE_CASE can select a substring of a matrix case name. NATIVE_SKIP_FX=1
excludes cold FX from the ordinary success matrix; the separate FX recovery test
explicitly requires the cold latency-change fence and then tests fresh-epoch recovery.

## What the assertions establish

- Real Stochas generated NoteOn/NoteOff for notes 60,64,67,60 without piano/step-note input.
- Source events have exact bridge onset and 60/120 BPM spacing (vendor rounding +/-1 sample).
- Actual ordered production MIDI-port routing drives genuine Surge audio.
- Each endpoint reports completed/submitted counts and exact plugin-output provenance;
  startup delayed dry is distinguished from nonstartup fallback, gaps and deadline misses.
- Stop/seek fences old epochs; blank-source restart and a separate held-note safety-latch
  test measure real instrument release rather than assuming every VST clears tails.
- MIDI ports Off is checked at the isolated sink track. Master still contains the host click.
  The unchanged legacy one-quantum Off path is a functional-silence/accounting baseline,
  not a performance pass: its observed deadline misses are retained and explicitly counted.
- Surge Effects lazily changes native latency 0→32. The first epoch must fence the mismatch;
  the FX test then exercises real transport stop and a fresh activation rather than ignoring it.

## Interpretation cautions

These are controlled paced/headless callback tests on cloud Linux, not a physical audio-device
or native-editor GUI test, and not Windows or Harmony Blueprint compatibility evidence.
The two endpoint MIDI bridge adds 4352 frames (90.667 ms) at 48 kHz before instrument attack;
FX adds another 2176-frame bridge plus native latency. This is not low-latency live-play proof.

The original master-only release assertion was confounded by the host's automatic metronome.
The original raw executable remains outside this source-only bundle. Its recorded
executable hash and failing log are preserved in diagnostic-attempt-4-master-confounded. The corrected
harness reads the already-rendered sink mixer track without changing the production graph.
The original one-frame reset passed this isolated test: sink release settled before 0.25 s.
A source-waveform comparison matches each residual master click to the 1100 Hz, 0.992 decay,
0.1 amplitude, tanh-limited host metronome within PCM16 quantization error. The speculative
full-quantum reset change was reverted; no stuck-note defect was demonstrated by that failure.

A separate real FX stop/reactivation test did expose an old suppression counter surviving
an epoch reset. Original negative logs are preserved. Final results must refer to the final
source manifest and rerun, not to the earlier negative or partial run.

Callback-runtime timing includes the harness output-copy closure and does not measure the
separate worker/helper's CPU time. The reported pacing-lateness metric is post-callback
schedule overrun, not physical device jitter. Exact-output, fallback, deadline and queue
counters are the relevant worker-delivery evidence. A preserved concurrent-build run lost
a source MIDI batch; its original assertion did not capture enough counters to isolate why.
The same-binary CPU-quiet comparison and separate deliberate unpaced-overload/restart case
are recorded alongside it, without erasing that failed observation.
