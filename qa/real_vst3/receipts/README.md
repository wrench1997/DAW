# Genuine third-party VST3 receipts: 9 October 2026

These are source-pinned measurements: historical offline DSP/MIDI and scanner
results, plus a separately recorded integrated-source metadata-only rerun.
The package preserves measurements, event JSON, owned state/XML and logs, plus
portable reproduction source. No plugin binary, factory asset, executable,
toolchain, compiled probe or generated audio is included.

## What was measured

- **Production runtime, b57076a:** exact production scanner/PluginChain source;
  bit-exact empty-chain dry audio; MIDI note to Surge instrument stereo PCM;
  known audio to changed Surge Effects PCM; ordered instrument-to-FX audio chain;
  all tested blocks completed and clean joined shutdown. This bounded-wait,
  controlled offline test does not establish real-time device performance.
- **Direct production helper:** 2,855 instrument parameters, set/readback,
  51,929-byte state save/restore, silent pre-note, finite nonzero held-note PCM,
  near-silent release, effect dry/bypass matching input to float precision and
  100% wet delay with changed audio and a tail.
- **Stochas 1.3.13:** the blank default emits no output events. A source-defined,
  owned C4/E4/G4/C4 pattern emits genuine NoteOn/NoteOff through the helper,
  with 60/120 BPM and stop checks. [Detailed result](stochas-summary.md).
- **Corrected scanner, 1c673eb:** [separate metadata-only evidence](../scanner/actual-scan.log)
  reports Instrument/Instrument|Synth and Effect/Fx correctly, with verified
  identities, event capabilities and cache round-trip. No processing rerun is
  claimed by this later scan.
- **Integrated scanner rerun, e6bd216, 9 October 2026:** the shipped portable
  scanner/build sources ran successfully against the exact integration commit.
  [Actual metadata output](../scanner-integrated/actual-scan.log) and
  [build/source/compiler/dependency hashes](../scanner-integrated/scanner-reproduction-build.json)
  are separate from the historical scan. The fresh helper's SHA-256 equals the
  historical helper hash. No DSP processing was requested in this rerun.

## Official plugin provenance

### Surge XT / Surge XT Effects 1.3.4

- [Official downloads](https://surge-synthesizer.github.io/downloads/)
- [Official release](https://github.com/surge-synthesizer/releases-xt/releases/tag/1.3.4)
- [Linux plugin-only archive](https://github.com/surge-synthesizer/releases-xt/releases/download/1.3.4/surge-xt-linux-1.3.4-pluginsonly.tar.gz)
- [Upstream GPL v3 license](https://github.com/surge-synthesizer/surge/blob/main/LICENSE)
- Archive SHA-256: `dd431b75f5fa197c4bffa6ca27ca46970f0a94c834119bb1db7decdeec4c28db`
- Archive MD5: `0180f06ec7a8445b1c749471e29c702b`, matching the retained
  [vendor checksum list](vendor-md5sum.txt)
- Instrument binary SHA-256: `b8584398f314819241ca7287117d5559ed838d37ef08faab9847f27ba2993cec`
- Effects binary SHA-256: `9b2da5f1b4a81a02ad55c88eba55257eb4a7f1747a1fd0866b8eaf5b7c8d0f45`

### Stochas 1.3.13

- [Official download](https://stochas.org/download/)
- [Official release](https://github.com/surge-synthesizer/stochas/releases/tag/v1.3.13)
- [Linux archive](https://github.com/surge-synthesizer/stochas/releases/download/v1.3.13/stochas-1.3.13.360d5ca.linux-x86_64.tgz)
- [Upstream GPL v3 license](https://github.com/surge-synthesizer/stochas/blob/v1.3.13/COPYING)
- Archive SHA-256: `c552d9d63c7e09e5d781d1d5c71b7fe389f2940c8a18b888ef22beebe6e4807c`
- Archive MD5: `0103355fb28f8266b1a0a2c71493b489`, matching the retained
  [vendor checksum list](stochas-vendor-md5sum.txt)
- Binary SHA-256: `5f7f44ddffcd7c4cbc0a68ce3b9eee71232a23024d7daec7cf792b2b45492c5c`

These download links and hashes record the tested releases; availability has not
been rechecked by the source-only receipt packaging step.

## Host attribution and preserved evidence

The measured production helper was copied from a Linux build of
`9a4d37b0f30f7aff568aa4b66f7b7e2227cfd91e` with piano-only work in progress.
The original build owner confirmed the helper/vendor/Cargo/build inputs were
unchanged, and independent source diff verified the relevant equivalence to
`b57076ad990869f4a421cc816d67409b5c69b694`.
Its SHA-256 was `ca91f242e409ab632427d5191c33ecae94bbd5172a8e7f1a98928d60f4f365fd`.
The same executable was used for the corrected scanner check. Runtime source was
captured with `git show` at b57076a, imported unmodified and linked with production
host rlibs. No mock factory or substitute backend was used.

- [validation.json](validation.json) and [summary](validation-summary.log): helper
  lifecycle, state, parameters and DSP statistics
- [instrument metadata](instrument-probe.json) and [effect metadata](effects-probe.json):
  genuine identity, parameters, bus and unit/program queries
- [runtime log](production-runtime.log) and [stderr](production-runtime.stderr.log):
  original production-chain results with path-only normalization
- [historical compile-command template](runtime-build-command.json): normalized
  command arguments, not a self-contained or newly executed build
- [Stochas blank-default metadata](stochas-probe.json), [owned-pattern events](stochas-pattern-output.json),
  [tempo events](stochas-tempo-check.json), [owned state](stochas-qa-pattern.state)
  and [actual XML re-export](stochas-qa-pattern.xml)
- [Provenance and normalization](../PROVENANCE.md), [full file hashes](../CONTENTS-SHA256.txt)
  and [reproduction instructions](../REPRODUCE.md)

The historical test generated WAVs, but this source-only bundle omits them.
`render_probe.py` generates a 3.109333-second initialized C4 sample;
`validate.py` generates 1.504-second dry stereo tones (220/330 Hz) and wet delay.
Those are reproduction outputs, not missing files required to inspect the
published measurements. No factory preset assets were used.

## Limits and observed failures

- The b57076a runtime log retains the old filename-based scanner's incorrect
  Effects-as-Instrument label. The later corrected metadata scan is separate.
- SelectProgram(unit 0, program 0) returned `unknown unit 0 or no program-change parameter`
  for both Surge bundles. Program/factory-preset selection is not validated.
- The positive instrument-to-FX test is an audio chain. It does not show generated
  MIDI being routed from Stochas to a downstream DAW instrument.
- No Harmony Blueprint, native editor, physical device, speaker output, Windows
  runtime, real multi-class binary or real-time-device compatibility is claimed.
