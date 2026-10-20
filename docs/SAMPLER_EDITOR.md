# Native Sampler editor

Use **+ Sampler channel** to create and select an empty, silent Sampler Channel with empty steps in every Pattern, then load a WAV. Channel creation and sample import are separate undoable actions. Alternatively, open **Sampler…** in Channel Rack for the selected Channel, or use a Channel's context menu. Load a WAV with the file picker or drop exactly one local WAV on the explicit drop rectangle. This route never creates or changes a Playlist clip.

The editor shows a compact waveform, source length, MIDI root note (60 = C4), source-frame start/end, held-note looping with independent loop boundaries, and ADSR in seconds. End boundaries are exclusive. Use Piano Roll to play a melody; Preview triggers one short owned note through the same Channel-aware audition route. Audio-offline and loading states disable Preview.

Decode and waveform extraction run on a worker. Completion checks project session, audio-load generation, stable Channel ID, and the original instrument assignment. Save, project dialogs, recording and other snapshot-owning operations defer import commits. Missing Channels, Undo/New changes, incompatible files and stopped workers cannot silently create an instrument. Import commits asset plus Channel binding as one undo step.

Slider drafts stay outside the Project until release. Release commits after the final pointer position has been applied; save/project actions explicitly seal the draft before snapshot capture; focus loss commits; Escape cancels the draft. Closing the window commits its last draft. Each completed gesture is one explicit Undo transaction. Session/generation and original-config checks prevent stale drafts from overwriting a restored Project.

VST and Sampler are exclusive. Unload a VST before loading a sample, or choose a native Channel. Remove sample clears only the WAV and leaves an empty, silent Sampler. Cancelling or failing a load also leaves it silent; Undo after loading restores that empty Sampler. Use the explicit **Use native synth** action to unload Sampler and restore the oscillator before assigning a VST. Clearing a sample and unloading Sampler are separate undoable actions, preserving the existing plugin endpoint and rollback lifecycle. Source files remain unchanged.

Sampler assets are included in timeline readiness and residency even when there are no Audio Clips. Pattern transport retains sampler asset metadata; ordinary unrelated Playlist media is omitted as before. Audition waits for both asset acknowledgment and the current compiled playback configuration, preventing an old native oscillator from sounding during sample setup.

Tests cover production egui checkbox/remove/slider input at narrow and wide sizes, exact Undo, sampler-only Pattern media dependencies, deferred and stale imports, decode failure and stale drafts. Tests require the matching native sampler core implementation.

## Offline cold-open waveform

Waveform peaks remain transient and are not saved to the project. When the Sampler editor opens a referenced source whose peaks are absent, a dedicated background worker decodes that source and returns only its small peak vector. This also works without an audio device. The editor cache is bound to project session, load generation, Channel, and exact media metadata/path; obsolete results cannot appear after New, Undo, deletion or relink. Neither Project/history nor audio registrations are changed by this visualization-only work. Missing, corrupt or property-mismatched media shows a clear waveform error with an explicit Retry waveform button, instead of retrying each frame or reporting an audible preview.
