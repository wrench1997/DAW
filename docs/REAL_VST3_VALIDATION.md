# Genuine VST3 validation: source-only evidence

The repository includes [reviewable scripts and receipts](../qa/real_vst3/REPRODUCE.md)
from real third-party Linux x86-64 VST3 checks on 9 October 2026. These are scoped,
source-pinned results, not a blanket compatibility claim for the application.
No plugin binaries, factory assets, toolchain, helper/probe executable or audio
renders are distributed in this bundle. The included Stochas state is an owned
deterministic test pattern and its actual plugin re-export.

## Established results

1. **Surge XT / Effects 1.3.4, production runtime at b57076a.** The exact production
   scanner/PluginChain source passed bit-exact empty-chain dry audio, a real MIDI
   note to instrument PCM, known audio to changed effect PCM, ordered instrument
   to FX audio processing and clean shutdown. Each worker block was awaited:
   this is controlled offline DSP acceptance, not a real-time-device benchmark.
   [Runtime receipt](../qa/real_vst3/receipts/production-runtime.log).
2. **Direct production-helper lifecycle/DSP.** 2,855 instrument parameters,
   parameter set/readback, 51,929-byte state save/restore, silence before note-on,
   finite held-note audio, near-silent release, dry/bypass matching input and wet
   delay/tail checks passed. Program-selection errors are preserved as a limit.
   [Measurement JSON](../qa/real_vst3/receipts/validation.json).
3. **Stochas 1.3.13 MIDI output.** The blank default produced no events. A separate,
   source-schema-defined owned C4/E4/G4/C4 pattern produced actual helper output:
   35 events in 400 playing blocks at 120 BPM, then a final NoteOff at offset zero
   of the first stopped block. Separate 60/120 BPM tests measured steady sixteenth
   spacing of 12,000±1 / 6,000±1 samples at 48 kHz, correct pitch order and balanced
   release. The first onset gap after stopped pre-roll is excluded from steady
   spacing. [Findings](../qa/real_vst3/receipts/stochas-summary.md),
   [full events](../qa/real_vst3/receipts/stochas-pattern-output.json),
   [tempo/stop data](../qa/real_vst3/receipts/stochas-tempo-check.json).
4. **Corrected scanner at 1c673eb, then integrated e6bd216.** Genuine Surge instrument/effect default-class
   metadata, event capabilities and cache round-trip passed. Surge XT Effects
   reports application category Effect and factory category Fx. The tested
   scanner/runtime/helper/vendor/Cargo/build source is unchanged in integration
   commit `e6bd216863f6180c2745de05c6701c6300d41cf2`. On 9 October 2026, the shipped
   portable scanner probe was also compiled and actually rerun against that exact
   integrated source and a freshly built helper with the same historical hash.
   The same classifications, MIDI capabilities, cache round-trip and clean helper
   lifecycle passed. Both scanner checks only loaded metadata and shut down;
   neither reran processing.
   [Original scan](../qa/real_vst3/scanner/actual-scan.log),
   [integrated-source rerun](../qa/real_vst3/scanner-integrated/actual-scan.log),
   [exact rerun build provenance](../qa/real_vst3/scanner-integrated/scanner-reproduction-build.json).

The old b57076a runtime log intentionally retains its incorrect filename-derived
Effects-as-Instrument label. It is not rewritten to look like the later scanner
run. See [VST3 scanning](VST3_SCANNING.md) for implementation behavior.

## Not established by this historical bundle

Later [production MIDI-route validation](PLUGIN_MIDI_ROUTE_VALIDATION.md) separately
records exact e54a6e4 Stochas → Surge → FX evidence, including its limitations.
The older files here retain their original source identity and measurements.

- Downstream DAW MIDI-generator-to-instrument routing at these historical sources, including Harmony Blueprint
- Native plugin editor rendering/interaction
- Physical MIDI/audio hardware, speaker output or real-time-device deadlines
- Windows runtime/plugin compatibility or real multi-class binary selection
- Surge program/factory-preset selection; the attempted program call was rejected

## Audit and reproduction

- [Official release provenance and plugin hashes](../qa/real_vst3/receipts/README.md)
- [Original hashes, path normalization and adapted-source boundaries](../qa/real_vst3/PROVENANCE.md)
- [Strict full file inventory](../qa/real_vst3/CONTENTS-SHA256.txt)
- [Portable reproduction prerequisites and commands](../qa/real_vst3/REPRODUCE.md)

From the repository root, run `python3 qa/real_vst3/verify_receipts.py` for read-only
hash, JSON, Python-syntax, state/XML and measurement consistency checks. It never
executes a plugin and is not a replacement for rerunning the actual checks.
Reproduce in a disposable copy; new runs must not overwrite historical evidence.
The portable scanner and build script are explicitly identified adaptations
relative to the historical scanner receipt. Their exact shipped bytes were used
for the separately identified integrated-source metadata rerun.
