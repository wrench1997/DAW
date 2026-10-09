# FL-inspired native workstation refresh

This is an original Citrus Studio visual design implemented in the existing Rust/egui
application. It is not a web mockup or an embedded browser. The design reference is
Image-Line's [official interface guide](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/basics_interface.htm).
No Image-Line logo, screenshot, icon, or other asset is shipped in the application.
Citrus branding and the existing vector icon set are retained.

## Design changes

- Cool blue-gray surface layers distinguish menu, controls, content and recessed fields.
- Compact horizontal workspace navigation replaces the large tiles. Narrow windows use
  a second transport row while keeping every workspace and plug-in action reachable.
- The Browser has quieter tabs, a recessed selection/import footer, and a narrower
  default width. Folder and import behavior is unchanged; empty states remain honest.
- Playlist lanes use restrained track-color headers, a useful wider initial time view,
  colored clip title strips, and less intrusive grid lines and borders.
- Pattern previews use stored note and step data. Step repetition follows the stored
  step length; Piano notes follow the Song compiler's sixteen-beat minimum and whole-bar
  extension. Each uses its own source offset/period. Empty and missing patterns get
  explicit states. Stored step positions are shown on the editor grid.
- Audio waveforms continue to use decoded asset peaks. Missing peaks show an unavailable
  state instead of generated decorative waveform bars. The decorative transport wave
  was also removed.
- White clip titles retain at least 4.5:1 contrast through bounded header-only darkening,
  including arbitrary bright custom colors; persisted clip colors are unchanged.
- Mixer strips have readable nameplates, colored selection cues, recessed faders,
  hardware-like pan knobs, and consistent Mute/Solo targets. All ten effect slots and
  routing details remain available. Horizontal scrolling is visibly discoverable.
- Fader paint, pointer inverse mapping, ticks, and thumb bounds share one padded range.
  Zero gain reads as negative infinity and unity as 0 dB. Actual meter acquisition,
  semantics, routing and audio dispatch are unchanged.

## Verification

The production app is exercised by the existing display-independent UI harness.
New coverage includes:

- Real pointer clicks on the painted fader thumb at zero, midpoint and unity at two
  heights, with no unintended gain change; drags reach both endpoints.
- Real pointer pan dragging, verifying independent gain state.
- Actual asynchronous decoding/import of a clearly named, deterministic four-second
  PCM WAV fixture, including derived waveform peaks and unchanged source bytes.
- Pattern paint regressions for empty/missing data, muted notes, independent step and
  Piano cycles, and source-offset ranges.
- A sampled RGB palette sweep for white clip-title contrast.

The capture suite produces nineteen genuine egui/WGPU offscreen app checkpoints,
including Playlist, Mixer and decoded-waveform views at 1440×900 and 1080×680 logical
sizes. At the retained 1.04 zoom these yield 1498×936 and 1123×707 pixel images. The
lossless PPM-to-PNG conversion is checked for equal RGB pixels. See
[HEADLESS_UI_QA.md](HEADLESS_UI_QA.md) for reproduction commands and boundaries.

These checks do not establish native desktop window presentation, physical audio/MIDI
hardware, native file dialogs, plug-in editor presentation, or full accessibility
conformance. Custom Mixer gain/pan controls still lack keyboard value adjustment and
track-specific accessibility labels. Existing accessibility-addressed navigation and
pointer workflows are retained and tested; no broader accessibility claim is made.
