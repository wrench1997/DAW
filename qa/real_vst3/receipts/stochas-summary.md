# Stochas 1.3.13 candidate check

Official download: https://stochas.org/download/
Official release: https://github.com/surge-synthesizer/stochas/releases/tag/v1.3.13
Archive: https://github.com/surge-synthesizer/stochas/releases/download/v1.3.13/stochas-1.3.13.360d5ca.linux-x86_64.tgz
SHA-256: c552d9d63c7e09e5d781d1d5c71b7fe389f2940c8a18b888ef22beebe6e4807c
MD5: 0103355fb28f8266b1a0a2c71493b489, matches official artifact_md5sum.txt.
Plugin binary SHA-256: 5f7f44ddffcd7c4cbc0a68ce3b9eee71232a23024d7daec7cf792b2b45492c5c
Source license: GPL v3, https://github.com/surge-synthesizer/stochas/blob/v1.3.13/COPYING

Genuine Linux VST3 loaded with the production helper. Reported version 1.3.13, vendor Surge Synth Team, category Instrument|Effect, MIDI input and output both true, zero audio input buses and one stereo output bus. Metadata, parameter and processing queries succeeded. Default pattern emitted zero events during 400 blocks of transport playback, and incoming note-on/off emitted no output events.

This negative result agrees with the tagged official source: SequenceData.cpp initializes MIDI passthrough to NONE and SequenceLayer::clear creates blank patterns; SequenceData.h initializes each cell probability to -1 (disabled); PluginProcessor.cpp has only a dummy program and empty setCurrentProgram. No configured factory pattern was found in the release archive or source tree. No opaque state was fabricated and no GUI was used. A supported configured pattern is needed before claiming positive sequencer MIDI-out validation.

Sources:
- https://github.com/surge-synthesizer/stochas/blob/v1.3.13/src/SequenceData.cpp
- https://github.com/surge-synthesizer/stochas/blob/v1.3.13/src/SequenceData.h
- https://github.com/surge-synthesizer/stochas/blob/v1.3.13/src/PluginProcessor.cpp

## Follow-on positive generator test

After examining the tagged official source, an owned deterministic four-step C4/E4/G4/C4 test pattern was authored through the exact XML schema in src/Persist.cpp, src/SequenceData.h and src/Constants.h. `stochas_pattern.py` starts from a genuine exported state, validates the host envelope and JUCE XML header, preserves the unchanged private trailer, writes explicit known note cells, then requires the actual plugin's load→SaveState re-export to match those cells. It does not modify the plugin binary or substitute a fixture.

The original blank-default result remains valid. With this explicitly authored test state, genuine Stochas output is positive:

- 120 BPM, 400 blocks: 35 NoteOn/NoteOff events during playback; stopping mid-note produced the final NoteOff on the next block at offset 0.
- The four-step generated pitches are exactly 60, 64, 67, 60, repeated. No MIDI notes are injected into Stochas to produce them.
- Separate 60/120 BPM tests measured steady 16th-note spacing of 12,000±1 / 6,000±1 samples at 48 kHz. The first onset gap is excluded from the steady-period comparison because transport starts after a stopped pre-roll.
- Stopped pre-roll produced no events. No NoteOn followed stop; per-pitch on/off balances ended at zero.
- Stochas itself produced only silent PCM, consistent with a MIDI generator. These tests prove its real event output through the production helper, not the DAW's as-yet-unverified downstream MIDI routing.

Owned pattern state: stochas-qa-pattern.state; human-readable actual re-export: stochas-qa-pattern.xml.
Event receipts: stochas-pattern-output.json and stochas-tempo-check.json.
Builders/tests: stochas_pattern.py and stochas_tempo.py.

Additional source references:
- https://github.com/surge-synthesizer/stochas/blob/v1.3.13/src/Persist.cpp
- https://github.com/surge-synthesizer/stochas/blob/v1.3.13/src/Constants.h
- https://github.com/juce-framework/JUCE/blob/4f43011b96eb0636104cb3e433894cda98243626/modules/juce_audio_processors/processors/juce_AudioProcessor.cpp
- https://github.com/juce-framework/JUCE/blob/4f43011b96eb0636104cb3e433894cda98243626/modules/juce_audio_plugin_client/juce_audio_plugin_client_VST3.cpp
