# Ghost notes and native sampler: implementation and acceptance plan

User requested implementation on 2026-10-18. Work starts from the verified
Library-delivered source archive of 1e6a638 (ZIP SHA-256
089f423ae662d6b9d19bb5f69d715dfd3aec1cf209908026222476e5a87f07c3).
The reset cloud workspace was restored into a new isolated source snapshot;
the older, dirty shared DAW checkout was not changed. No remote push is implied.

## Intended scope

- Ghost notes: readable, read-only, visibly distinguished from the target Channel;
  source selection for current Pattern other Channels or one reference Pattern.
  Cross-Pattern reference is Pattern-local beat-zero alignment, not automatic
  Playlist overlap/chase. Source changes do not edit the score or create Undo.
- Native sampler: one WAV asset per native Channel, root MIDI note, sample bounds,
  optional sustain loop, ADSR, actual pitched voice playback and offline render.
  Pitch changes playback speed/duration; independent time stretch is not included.
  Existing asset load/relative-media/recovery infrastructure must remain usable.
- Editor: clear Channel-specific entry, waveform and loading/error feedback,
  bounded control edits, one-gesture Undo, safe delayed import ownership.
  Existing VST assignments must not be silently destroyed or substituted.

## Required evidence (not yet results)

1. DSP fixture at root note and octave, mono/stereo and differing sample rates;
   loop and bounds never read out of range; finite output; note-off release,
   stop/seek/all-notes-off and missing-asset policy.
2. Project save/reopen retains sampler settings; legacy projects keep their
   native oscillator; invalid configuration produces a defined diagnostic.
3. Same musical input produces matching supported realtime/offline sampler PCM
   (within explicitly documented rendering differences), including channel
   gain/pan/mute/solo and routing. Export must not silently replace samples with
   oscillator notes or omit sampler-only assets.
4. App inputs: load/drop into the sampler target, not Playlist; stale import after
   New/channel deletion cannot mutate new work; editing and Undo/Redo; relevant
   Save/New/close/focus boundary; narrow editor layout.
5. Ghost toggle/reference selection/deleted reference/current Pattern switching;
   reference notes never capture note editing, selection or velocity operations.
6. Fresh build, focused tests, strict lint and applicable aggregate suites;
   actual native desktop actions where the environment permits. Hardware audio,
   external VST, and Windows are separate evidence classes, never inferred from
   headless or Linux success.

## Current status

Implementation and toolchain recovery in progress. No new feature is accepted
or native-tested by this plan. Subsequent results will identify source revisions,
failed attempts, successful reruns and remaining limitations.

## Environment evidence at 04:51 UTC

Official Rust 1.99.1 and local development sysroot were restored. Baseline
`cargo check --locked --no-default-features` passed before implementation edits.
An isolated official Debian Zenity runtime, GTK dependencies and compiled schemas
were prepared without system/service changes. The cloud desktop actually opened
its chooser and selected `root-c4.wav`; returned stdout matches the fixture path.
This closes the missing-picker dependency preparation, not yet App Save/reopen
acceptance. The fixture is a newly generated mono 48kHz C4 sine, not user music.

## Native workflow results, 05:03–05:15 UTC

On the fresh no-default `e869a37` App (immutable binary SHA-256
37bda9928530ccdaf2ede17840dd04e3e672b55310cd4d8cdaf895e469a0bee5):

- File > New; F6; + Sampler channel created a second, selected Channel.
- Load WAV opened the real Zenity picker. The 48 kHz, mono, 96,000-frame fixture
  loaded into this Channel, showing waveform, root60, native-frame bounds/ADSR.
- Loop checkbox enabled; actual fast Sustain drag changed0.8→0.25, one Ctrl+Z
  restored0.8 and one Ctrl+Y restored0.25. Scrolling reached lower controls.
- Actual Ctrl+S/native Save produced format15 native-sampler.citrus; parsed data
  retained the sample binding, loop0..96000 and Sustain0.25.
- Three mouse-authored notes (MIDI72/74/76 at local beats0.25/1.25/2.25, length1)
  were placed as one Pattern clip at beat0.25 and manually saved.
- File > Export WAV produced stereo48kHz/24-bit native-melody.wav, 180,000frames,
  3.75s, SHA-256 ae99676b775d6828fc6856d43a149bd7d0c57121742e83dde506e654469e9d95.
  Measured note frequencies523.25105/587.32948/659.25513Hz match the expected
  chromatic sample playback. This is file/PCM evidence, not speaker listening.

On a separate cold-start all-feature `751dd2b` App (binary SHA-256
3446f66957eaa6d57c6f2a2bcd5e45bfbb83e320fb28bb21f5b723c04f0fb15a):

- File > Open restored all three notes, placement, sample binding, loop and ADSR.
- **New failure retained:** with no audio engine, restored waveform remained
  blank because the ordinary asset reload path depends on engine readiness.
  An independent, session-safe background waveform preview is being added.
- Created empty Pattern2, selected reference Pattern1 in the actual dropdown:
  its three read-only outline notes appeared at unchanged local beats.
- Right-clicking a ghost, Ctrl+A in the empty target, and Alt+V hide/show did not
  edit either Pattern. Manual Save before/after has identical SHA-256
  b02ae93df0efac7f469ac4680393748696ac8eeaf264a1d023ac4183887a71bd.
- **Layout polish retained:** short/long scope text changed toolbar wrapping
  during Alt+V, nudging grid geometry. A fixed scope-label footprint is in review.

Physical audio remains Offline. Both instances and their test states are retained.
Final full-suite evidence must be tied to the source after these follow-up fixes.

## 05:40 UTC final native replay passed on frozen source0d4d6f8

The third native process used the all-feature executable with SHA-256
386f10adc5640d5adc7594ad7559f06e0d9810b356ef07fc74b22cc373a72245.

- Cold File > Open restored the sampler, notes and settings. The independent
  background preview visibly rebuilt the waveform while Audio remained Offline.
  Opening/scanning/scrolling and a manual Save left the original Project bytes
  unchanged (b02ae93…); waveform presentation did not dirty the score.
- The compact source selector displayed Pattern1 as Pattern2's read-only
  reference. Native Alt+V hide/show no longer shifted the toolbar/grid. Right
  click and Ctrl+A still left the score unchanged, confirmed by exact saved hash.
- + Sampler channel now created a persisted empty sampler (asset_id:null), with
  explicit silence/load instruction and disabled Preview. Cancelling the real
  Load WAV picker kept this state. The empty Channel was manually saved.
- With the empty Channel retained, File > Export WAV still produced the exact
  earlier 180,000-frame stereo24-bit waveform, SHA-256
  ae99676b775d6828fc6856d43a149bd7d0c57121742e83dde506e654469e9d95.
  Thus the empty instrument neither blocked export nor altered the loaded
  sampler's melody. Its actual runtime silence has separate callback tests.
- Final test Project was saved with Pattern1 selected so the authored notes are
  easy to find. All three process states were minimized and retained.

`native-final-acceptance.json` and the native files/screenshots record exact
source, binary, project and export attribution. The exported audio came from the
actual File-menu workflow, not an independently substituted synthesizer.

Qualifications: there is no audio device for speaker/live-device listening;
physical native MIDI input is not implemented in this change; OS-native file
**dragging** was not separately exercised (dropped-file events are covered by
App tests). Windows and commercial VST remain unverified. The two final frozen
complete test configurations are still running; their counts will be appended.

## 05:45 UTC final aggregate gates completed

Both configurations freshly compiled frozen source0d4d6f8 and scheduled every
discovered case exactly once in bounded serial batches. They ran with distinct
temporary roots; their overlapping cases are not summed.

- All features/all targets: 2,845 unique cases, 2,836 passed, zero failed, nine
  existing optional ignores. App:2,764 passed/nine ignored; helper/protocols:72
  passed. Every target and every group completed successfully.
- No-default: App2,165 unique cases, 2,164 passed, zero failed, one existing ignore.
  Three feature-disabled protocol targets each list zero tests, with terminal
  successful summaries and no invented coverage.
- Strict Clippy under both configurations/all targets, formatting and App/helper
  builds passed. The existing vendor deprecation warning was not suppressed.
- Python368 cases:359 passed/nine existing skips.

Initial all-feature751dd2 failures are retained as historical evidence: one
compact-toolbar geometry regression and two assertions hard-coded to old target
format14. The former was fixed without relaxing geometry; the latter now assert
CURRENT_PROJECT_FORMAT_VERSION while preserving policy/media/state invariants.
The old control red test also proves that mere display used to clamp valid long
ADSR or canonicalize Some(fullend); final controls preserve exact saved values.

This concludes the bounded cloud implementation and verification. No GitHub push
was retried, no Windows installer is claimed, and source/receipt/portable test
Project artifacts are delivered separately from device or commercial acceptance.
