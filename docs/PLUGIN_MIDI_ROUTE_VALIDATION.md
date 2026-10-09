# Real production MIDI-route validation

The [source-only harness and receipts](../qa/plugin_midi_route/REPRODUCE.md) test
real Stochas 1.3.13 → Surge XT 1.3.4 through the actual Timeline, DSP graph,
PluginChain and fixed-quantum adapter. They do not manually forward helper output
to another helper. The independently built snapshot preserves all 117 production
source files from `e54a6e49fe02b78ff299ce56dddd781925f7fe3c`; only external test code
is appended. Integrated runtime `5e8ff0f622077e6a3817dce122f03ec6e33c1ab1`
is byte-equivalent for those files. [Source binding](../qa/plugin_midi_route/receipts/final-source-binding.json).

## Results and limitations together

- All five compiled real-plugin acceptance tests passed. The paced callback matrix
  covers 128/256/512/2048 frames at 120 BPM and 512 frames at 60 BPM. Each routed
  endpoint reports 750 submitted/completed quanta, 734 exact plugin outputs and
  16 startup-delay quanta, with zero nonstartup fallback or worker deadline misses.
  Generated C4/E4/G4/C4 order, sample offsets and tempo spacing are asserted.
- Genuine Surge Effects' initial 0→32-sample latency change is fenced. Stopping and
  activating a fresh epoch rebuilds coherent PDC and restores the real audio chain.
  The earlier failed suppression-reset observation is retained with the fix/rerun.
- Single held note and same-pitch retrigger/chord safety cleanup settle to silence
  without explicit test note-offs and do not resurrect on the new epoch. A deliberate
  unpaced overload publishes a fault, fails closed and recovers on paced restart.
- A prior concurrent-build run lost a source MIDI batch. Its precise cause remains
  unresolved because the original failure lacked the necessary endpoint counters.
  The same binary passed a CPU-quiet comparison; that does not erase the failure.
- The unchanged ports-Off one-quantum path has 15/10 observed endpoint deadline
  misses. Its sink-silence/accounting result is not realtime qualification.
- A debug 128-frame callback reached 3.116 ms, exceeding its 2.667 ms period. Timing
  includes the harness output-copy closure; it is not physical device jitter or
  an optimized production benchmark. Passing worker delivery is a separate fact.

At 48 kHz the two endpoint bridges add **4,352 frames / 90.667 ms**, before
instrument attack or downstream FX. Another worker-backed FX stage adds its bridge
and plugin latency. These results do not establish low-latency live play, physical
audio-device deadlines, native plugin editor UI, Windows or Harmony Blueprint
compatibility. See [implemented routing scope](PLUGIN_MIDI_ROUTING.md).

## Evidence and reproduction

- [Final summary](../qa/plugin_midi_route/receipts/summary.json)
- [Full final test log](../qa/plugin_midi_route/receipts/native-tests-final.log)
- [Real callback/event matrix](../qa/plugin_midi_route/receipts/native-production-graph.json)
- [FX recovery](../qa/plugin_midi_route/receipts/native-fx-reactivation.json)
- [Deliberate overload/restart](../qa/plugin_midi_route/receipts/native-overload-recovery.json)
- [Concurrent-load failure](../qa/plugin_midi_route/receipts/concurrent-load-outcome.json)
- [Original import and publication identity](../qa/plugin_midi_route/PUBLICATION.json)
- [Strict current file inventory](../qa/plugin_midi_route/CONTENTS-SHA256.txt)

Run `python3 qa/plugin_midi_route/verify_receipts.py` for plugin-free integrity and
positive/negative result consistency checks. Optional `--repo` plus `--source-ref`
checks every recorded production source byte against an explicit Git revision.
The current inventory covers all shipped files; `INVENTORY.json` preserves the
original normalized worker archive's provenance, including raw hashes. Only the
reproduction guide was clarified for this publication. Raw path-reversal maps and
original executables remain outside the bundle. No vendor binaries, factory assets,
compiled probe or generated WAV are redistributed.

The old Master-only release assertion was confounded by the host metronome. The
corrected test observes the already-rendered instrument track; its prior failure
and separate waveform explanation remain preserved. The metronome toggle is a
separate feature, not silently included in this routing checkpoint. Historical
[helper-only and scanner evidence](REAL_VST3_VALIDATION.md) retains its original
scope rather than being relabeled as production routing acceptance.
