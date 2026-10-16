# Offline WAV export options

FILE → Export WAV opens settings, then a review. “Choose destination and export”
opens the native Save dialog; its existing overwrite confirmation is unchanged.
Cancelling the Save dialog returns to review. Back keeps the current draft; Close
or Escape discards it. Starting an export remembers the options for this app run
only. No options or destination are added to a project file or saved preferences.
New/Open invalidates any pending review, and a still-running/cancelling export
retains its single worker slot until it exits.

## Encoding and sample rate

- Stereo, full arrangement; PCM16, PCM24, or IEEE float32 WAV.
- Rates offered: 44.1, 48, 88.2, 96, 176.4, and 192 kHz, plus the current valid
  device rate or a rate already selected this session. The shared renderer accepts
  8,000 through 192,000 Hz. Invalid device rates default to 48 kHz.
- First-use defaults: device rate (48 kHz when offline), PCM24, legacy peak
  attenuation. “Reset to legacy / device-rate defaults” restores those choices.
  Choosing a file rate does not change the audio device or the project.
- Existing source-rate conversion uses deterministic linear resampling. This is
  not a claim of mastering-grade conversion.
- Integer PCM is rounded, without dither. PCM24 retains the previous quantizer.
- Float uses IEEE format tag 3, a WAVEFORMATEX header, and a fact chunk with the
  frame count. PCM uses format tag 1. No codecs or binary dependencies are added.
- Standard RIFF size limits apply. No RF64, MP3, compressed formats, or stems.

## Output level is a deliberate choice

**Attenuate peaks above 0.95 (legacy default):** scan the rendered stereo sample
peak and apply `0.95 / peak` only when it exceeds 0.95 (about −0.45 dBFS). Otherwise
use unity gain. This never boosts a quiet mix. It is a single gain reduction,
not loudness normalization, compression, a true-peak limiter, or compliance with a
loudness standard. Completion reports the actual reduction, if any.

**Preserve level:** apply unity export gain. PCM export rejects any sample outside
±1.0 rather than clipping or changing gain. The error offers lowering the mix,
choosing float32, or explicitly choosing peak attenuation. The old destination is
preserved. Float32 writes finite samples outside ±1.0 unchanged.

**Float reimport caveat:** the existing Citrus WAV reader clamps decoded float
samples to ±1.0. Exported float headroom is preserved in the file, but reimporting
an over-unity float file into Citrus is not lossless. This patch deliberately does
not alter that compatibility-sensitive reader behavior. A separate follow-up is
needed before promising lossless over-unity import/export workflows.

All formats reject non-finite samples or gain before publication. Rendering,
encoding and cancellation use the same ExportControl, buffered staged-file writer
and atomic commit boundary. An accepted cancellation cannot replace the target.
Cancel is disabled once final publication begins; failure still reports an error.

## Offline processing limits

The ordinary built-in/WAV renderer supports Tempo and Master Volume/Pan/Mute
automation. Active VST instruments/effects, sidechains and other active automation
are rejected before choosing a destination and again in the renderer.

With `experimental-fresh-helper`, eligible Linux/Windows projects can instead
enter experimental Whole export: exactly one active VST3 Generator at the current
sample rate, no other music/audio source, optional fully wet VST3 effects in one
static ordered PostFader chain to Master, and sample-exact Master Volume/Pan/Mute
automation. Captured source state/bases and strict clones guard every stage;
cumulative native latency is cropped once. General graph PDC remains unsupported.

The current source adds source-Generator physical-slot-0 `PluginParameter`
automation, guarded by stored/captured finite normalized bases, actual writable
and automatable catalog metadata, clone identity and restored-value checks.
Parameters use raw Q128 block-start values before MIDI/DSP, not sample-accurate
changes within a block. The fixed tail holds the last values actually sent in a
musical block and sends no new tail parameter values. Recompilation, all 64
directed cases (19 new), the genuine Q128 full-PCM SDK run and retained SDK cases
passed locally. Initial test-backend compile and cursor-oracle failures were
repaired in tests, not production behavior; details are in the
[offline export workflow](OFFLINE_EXPORT_WORKFLOW.md). At `4cffe`, Root strict
Clippy/no-default/fmt, inspected two-size Review WGPU and both cloud workflows
(including Q128 SDK/real PrintWindow and preview packaging) passed. A separate added saved-lane SDK case now passed locally: the lane is
authored before ordinary Manual Save, the disk/fresh-App payload and actual
base/opaque restoration are exact, and genuine Whole matches all 96,768 stereo
frames. The earlier after-Open lane case remains separate and passed again.
The new test uses the legal 0.0625-beat placement; its initial compile/type and
disk-normalization failures received test-only fixes, with full payload checks
retained. All four shared-flow SDK cases and this revision's strict Clippy/
no-default/fmt passed. Saved-case code `12ae7f3` also passed both cloud workflows,
including its genuine full-PCM SDK case; unchanged production/WGPU evidence is reused.

Branches, sends, sidechains, off-path Inserts (even disabled/bypassed), active
partial-wet Inserts, additional sources, Effect/other-instance parameter
automation, native Channel/non-Master Mixer controls, Tempo and Swing remain
refused, not rendered with missing sources, effects or controls. The verified
serial-FX/Master baseline and current Generator-automation validation status are
separate; see [offline export workflow](OFFLINE_EXPORT_WORKFLOW.md).

The VST review requires acceptance of the chosen fixed tail window. Unknown or
longer plugin tails may be truncated; export does not detect acoustic silence.
See [Windows melody workflow](WINDOWS_MELODY_WORKFLOW.md) for the capture, identity
and actual NOIRE acceptance boundaries. Realtime Master Capture records the live
Master output; selected MIDI-to-audio conversion is a separate operation. Neither
substitutes for whole-song offline export.

## Regression scope and fixture

Focused tests cover exact headers (including format tag, byte rate, block align,
fact count, RIFF/data sizes), sample-rate/frame counts, production-reader
round-trips for all formats, raw float headroom, finite input, level policies,
original-file preservation, stage cleanup, cancellation through finalization,
and actual egui pointer/keyboard settings/review/back/close interactions.

`tests/fixtures/export-legacy-pcm24.wav` is a 524-byte golden result generated by
unmodified renderer commit `bcf0c7d6ae700dae3dc21de992d06d3ceabc3531`. The regression
builds the same five-sample PCM16 source and short project, including a 1.5 clip
gain requiring legacy attenuation, and compares both compatible and options APIs
byte-for-byte. SHA-256:
`660796307fb724e75beb94036301e2126032cfe92ef83d346437b96a3872206b`.

Headless egui regression tests are not a full app GUI/native Save-dialog pass.
Windows full-app/all-feature CI and manual native dialog checks remain distinct
acceptance gates.
