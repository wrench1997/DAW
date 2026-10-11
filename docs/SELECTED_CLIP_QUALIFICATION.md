# Selected-instrument clip conversion: Linux qualification

One complete headless real-App run passed at source commit
`a97007998901f1d65266113cd418130a5863be42`, using the official Linux
Surge XT 1.3.4 instrument and the opt-in `experimental-fresh-helper` feature.
The run exercised the actual review, shared acceptance method, conversion,
playback, Undo, Redo and cleanup. This Linux qualification is bound to
`a9700799`/`build031`; Windows CI is separately required for the exact accepted
integration revision.

This is a narrow experimental qualification. It does not establish native-window
pointer interaction, Windows plug-in runtime, physical-device or speaker output,
real-time performance, other instruments, or a commercial-release acceptance.

## Using the experimental feature

On a Linux build with `experimental-fresh-helper`, choose
**Experimental dry-bounce source (new VST3 load only)** in the instrument picker
before loading a new instrument. Existing sources are not silently reconfigured.
In SONG mode, select one eligible unmuted Pattern Clip and open **Render selected
clip to audio** from the Inspector or Edit menu. Review the interval and gain,
choose an existing permanent output directory, acknowledge the fixed tail, then
choose **Render and convert**. The app explains unsupported configurations.

Rendering uses a fresh dry snapshot; live voice/LFO history and random phase are
not reproduced. Channel gain transfers to the Audio Clip without normalization.
The original MIDI clip is muted and its retained generator disabled. Undo restores
the captured source; the created WAV remains on disk after Undo or cancelled
insertion. See [implementation and limits](SELECTED_CLIP_APP.md).

These user controls are separate from the ignored qualification test's explicit
run-binding opt-in described below.

## What passed

All ten fixed stages completed: preflight, source load, initial audibility/stop/base
value, conversion, raw-media audit, converted playback/stop, Undo, Redo,
Redo playback/stop and cleanup.

- The genuine source was loaded through the App. Its real review was rendered
  headlessly, then the same `begin_clip_render_stop` acceptance method used by
  the button was invoked with the reviewed choices.
- The selected interval began at frame 30,000 and covered 24,000 musical frames
  plus a fixed 12,000-frame tail. The resulting WAV contains 36,000 stereo frames
  at 48 kHz, IEEE float32. Independent reconstruction from retained strict-render
  PCM matched every WAV sample bit. The renderer's 96 block-padding frames were
  checked separately and excluded from the file.
- Converted and Redo playback each passed the fixed per-sample f32 output oracle,
  including gain and master processing. Initial source playback passed the fixed
  audibility checks. Three fixed stop windows observed quiescence.
- Undo restored the authoritative before-Project image on a fresh endpoint,
  supplied the captured 51,929-byte state once, and read back the authoritative
  base value of 0.75. Opaque state saved back after restoration was not claimed
  byte-identical.
- Redo restored the committed after-Project image and reused the identical
  retained PCM `Arc`, without another render, capture or source construction.
- Three helpers were used: initial source, strict clone and Undo source. Each
  source worker joined and its original helper process was absent. The clone
  separately recorded checked reap, disposal and backend destruction before
  publication. Cleanup completed without quarantine.

Independent PCM/state/lifecycle reviews passed. Callback-output equality and
Project/history equality were executed in-memory assertions; their full buffers
and Project snapshots were not separately dumped for post-run reconstruction.
Stop-window observations do not establish a general silence or timing guarantee.

## Evidence attribution

The successful run is `genuine007`; its terminal receipt SHA-256 is
`f19470c6bde796a5ac07e18ffa8e6dd750725ee92527cd99887e1cbc928eb50f`.
Six earlier genuine attempts remain recorded failures.

The supporting mock history contains 876 unique passing names and 2,159 passing
executions across revisions, with 13 failed executions retained. These are not a
single full-suite pass at the final source. The final source's `build031` compiled
the harness and ordinary helper and passed formatting. It did not rerun mocks or
Clippy: 188 passing mocks and three Clippy profiles remain attributed to
`300b7d41916335e400ee67a6afc3553161c8dcb3` (`build030`), whose formatting failure
remains recorded; the final code delta was verified as whitespace-only.

## Reproducing the qualification

The checked-in fixture is a gated reference, not a portable one-command test.
Its source and asset location pins, external launcher/watchdog and private run
binding belong to the recorded run. A new machine or source revision requires a
new reviewed binding and separately recorded result; this document does not
supply the original private execution bundle.

1. Start from an identified clean source revision, locked dependencies and a
   recorded Linux toolchain. Build the App test harness with
   `experimental-fresh-helper` and a separate non-test ordinary helper. Record
   exact compiler inputs, feature sets and executable hashes.
2. Obtain Surge XT 1.3.4 from the [official release](https://github.com/surge-synthesizer/releases-xt/releases/tag/1.3.4).
   Verify the [published archive/module hashes and license provenance](../qa/real_vst3/receipts/README.md#surge-xt--surge-xt-effects-134).
   Use the initialized instrument fixture, not a substitute preset or plug-in.
3. Prepare a fresh create-new output directory and a reviewed source/asset/tool/
   environment binding with an owned-process watchdog and phase-gated helper
   launcher. Preserve the original numerical criteria, callback windows, pump
   ceilings and 120-second outer deadline. Any portability adaptation needs its
   own source/build review and must not weaken those checks.
4. Invoke only
   `app::clip_render_job::genuine::genuine_selected_clip_app_conversion_undo_redo`
   with the exact ignored-test arguments and binding-hash opt-in required by
   the fixture's admission code in `src/app/clip_render_genuine_support.rs`.
   Ordinary test runs leave it ignored. Retain failures rather than retrying
   within the same run or changing criteria after observing output.
5. Verify all ten stages, before/after input hashes, raw PCM-to-WAV equality,
   capture/restoration evidence, playback assertions and exact cleanup ownership.
   Publish a bounded summary with its own source and result identity.

Raw audio, captured state bytes, event journals, plug-in binaries, credentials
and machine-private paths are excluded from this public summary. Historical
asset links identify the tested release; their availability was not rechecked
while preparing this documentation. Windows packaging and CI require their own
exact-revision results; all-feature source coverage alone is not Windows
experimental-runtime qualification.
