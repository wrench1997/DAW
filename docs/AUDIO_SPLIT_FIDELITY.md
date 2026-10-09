# Audio split fidelity (project format v11)

## Scope and behavior

Playlist Audio Clip splitting now calls `audio_clip::split_audio_clip`. This is the
production edit used by the UI, not a parallel implementation in tests.

- A split preserves each fade's original reference domain. A partial ramp does
  not restart or stretch to fit either child. Repeated splits retain that domain.
- Moving or duplicating a slice carries its envelope. Resizing an inherited slice
  trims/reveals the original envelope; it does not stretch that inherited ramp.
  Legacy unsplit clips retain their normalized fade behavior.
- Editing/resetting one fade replaces only that side. Creating a crossfade keeps
  the untouched outer sides. Handles are clamped to the visible part of a slice,
  with inherited-domain explanations in their tooltips and the inspector.
- Reference origins before beat zero are represented by signed frame anchors.
  Compilation extends the boundary tempo to those origins rather than muting a
  valid moved slice. A song boundary that had already cropped the original clip
  is retained as part of its fade provenance.

## Source sampling and exact boundaries

The native source offset remains the original integer asset-frame root. Saved
clock intervals preserve source progress as differences of rounded output-grid
endpoints:

`elapsed = sum(round(end_seconds * rate) - round(start_seconds * rate))`

Realtime sampling uses `root_native + (elapsed + local_output_frame) *
native_rate / output_rate`. Export adds the same elapsed output frames to its
original once-resampled asset offset. This preserves each renderer's own sample
phase, including 44.1 kHz/48 kHz conversion, nonzero roots and repeated cuts.
Adjacent intervals coalesce; a new cut after a move or tempo change records
another interval. No interval traversal, allocation or tempo conversion runs in
the audio callback.

Existing source in-points stay sample-locked after a move or later tempo edit.
A new cut uses the current tempo map. Therefore changing tempo after splitting
need not match a hypothetical clip that had never been split. A real Slip edit
explicitly chooses a new native-frame in-point; zero Slip movement leaves the
inherited phase untouched.

The Playlist still displays f32 beat lengths. Optional exact-length references
keep child joins and the parent's final end exact despite f32 subtraction and
addition. Separate legacy realtime/export extents preserve the previous
renderers' different beat arithmetic. An explicit resize uses ordinary new
Playlist geometry. Fresh fade edits and subsequent splits also retain these
precise extents.

A child beyond exhausted media is silent. Export checks exhaustion on its
resampled asset grid so an endpoint-preserving downsampler's final frame is not
lost. Invalid original native offsets still fail validation.

## Compatibility and validation

- Existing v10 projects with absent reference fields load with legacy fade/export
  behavior. Saving writes v11; older builds reject this future version before
  normalization. The save status explicitly says an updated build is required.
- `serde_json/float_roundtrip` is enabled so f64 clock/domain values survive
  serialization without a one-ULP parser shift.
- Fade/length domains are finite, positive and bounded to one million beats;
  signed fade offsets and optional crop limits are bounded too.
- Source provenance is limited to 4,096 intervals. Endpoints are finite seconds
  in [0, 1 billion], and every cumulative signed duration remains within
  +/-1 billion seconds. Invalid metadata is rejected before load normalization
  or save filesystem changes. Split failure leaves the original untouched.
- The new realtime root-clock calculation intentionally removes cumulative f64
  addition drift. It preserves musical source timing and makes split/chase/
  callback partition results deterministic. It is not a claim of bit-identical
  resampled realtime PCM against the old cumulative-rounding implementation.
  Offline v10 compatibility is separately checked against the old formulas.
  A standalone 60-second calculation measured the old incremental error as
  0.0000362694 native frames at 44.1→48 kHz and 0.0001073941 at 48→44.1 kHz.

## Verification

The supplemental Linux harness imports the actual production model, automation,
mixer graph, tempo map, Audio Clip helpers, fades, Playlist, export, timeline,
executor and WAV modules. At this milestone it passes 200 tests, with one
pre-existing Windows-path fixture excluded on Linux. Its Clippy checks pass with
warnings denied. This narrower harness does not replace the complete app build.

New coverage includes:

- 2,016 bitwise offline PCM comparisons for DC plus stereo ramps/impulses,
  same-rate and 44.1/48 kHz conversion, native source offsets, eight fade pairs,
  seven cut locations, cuts in/out/on ramps, nested cuts, constant/hold/ramped tempo
- Exact WAV comparisons after moving/retempoing a slice, v11 save/load including
  multiple source intervals, and v10 loading against independent legacy formulas
- Nonbinary beat joins/final ends, source exhaustion, the downsampled endpoint
  frame, signed/cropped fade anchors, one/two-frame endpoints and zero-frame children
- Per-side edits/resets, edit-then-resplit, outer-crossfade preservation, waveform
  preview/fallback alignment, fingerprints, and gesture history restoration

Independent review also exercised 198,000 nonbinary seam/end cases and 268,200
legacy fade-anchor combinations, including song crops. These review experiments
are supplemental, not replacements for checked-in regressions.

Additional real-application validation on 2026-10-09:

- `cargo check --offline --locked --no-default-features --all-targets` passed,
  including the actual app and test modules, with existing platform dead-code
  warnings only.
- The real application test binary linked and all three new callback tests
  passed: split/nested PCM, seek/partition equality and a 24-hour root-clock
  test. The measured combined run took about 3.73 seconds and peaked at 74 MiB.
- Four real app-level tests passed for preview/fallback, reference fingerprints,
  per-side edit/crossfade/resize Undo/Redo and complete production split history.

This local validation used the already-installed ALSA 1.2.14 runtime (version
queried from the library itself), truthful task-local pkg-config/linker metadata
and alsa-sys's bundled bindings. No package was installed, no system file was
changed and no headers or API were fabricated. This is a real Linux
no-default-feature check/test result, not a Windows or VST integration claim.

Full Windows all-feature app tests, GUI interaction, physical-device/audio-driver
behavior and real VST hosting remain separate validation requirements. No package
version or remote publication is part of this isolated implementation milestone.
