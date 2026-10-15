# Drag the playhead; let the arrangement set its end

**Locally verified, 2026-10-15; source `885306f`.** Scoped input, persistence,
Timeline/WAV, audio-split, strict Clippy and actual WGPU frame checks passed.
Desktop **Citrus Studio 新版测试02** starts the separate copy at
`D:\DAW-Preview\Citrus-Studio-20261015-02\Start-Citrus.cmd`;01 and user works are
unchanged. Cloud checks for this candidate and native02/NOIRE acceptance remain.

## Position and view are separate from music

- Press or hold-drag the Playlist ruler or its orange playhead to seek. Moving
  that held gesture into a lane continues seeking and never draws a clip.
- A fresh press in a lane still draws the currently selected Pattern. Draw/Paint,
  selection and clip editing keep their existing tool semantics.
- `1 BAR`, `4 BARS`, `8 BARS` and `FIT` change the view, not the song duration.
  The horizontal view slider uses the same editing horizon as the canvas.
- Moving or resizing clips can cross the previous song end. The existing finite
  coordinate limit of 4096 beats remains an editing quota, not a four/eight-bar
  song limit. Existing bounded renderer admission still applies.

## AUTO follows content; MANUAL preserves an explicit endpoint

New songs created in the UI and the startup demo enable **AUTO**. The shared
`Project::song_end_beat()` rounds the final unmuted placement up to a complete
four-beat bar, with four beats for an empty song. Pattern, Audio and Automation
placements contribute their own edges; inherited exact Audio split geometry is
used. Enabled legacy automation without any placement contributes its last
point. A muted placement prevents that lane becoming globally active again.

Deleting or moving the final placement changes the derived endpoint. Growing
notes does not resize an already placed or trimmed Clip. The prior Piano period
rules, including the default Timeline minimum, remain unchanged.

Older files and existing `Project::blank()` / `Project::default()` callers retain
their explicit `song_length_beats` with `auto_song_length = false`. The UI labels
this **MANUAL** and offers AUTO as an opt-in. Turning AUTO off starts the manual
range at the current derived endpoint. This policy change has one undo step;
view changes and PAT/SONG selection do not enter music history.

MANUAL is not a length lock: existing Draw/import/MIDI/audio-record creation
commands retain their prior endpoint-extension behavior. Group movement/resize
does not rewrite a manual endpoint. Loading a valid short manual range no longer
silently extends it to sixteen beats or to every Clip edge.

All three automation-creation entries (Playlist button, native-control context
menu and plug-in parameter) use the same placement bounds: AUTO can create at a
late playhead even when the old manual cache is only 64 beats; MANUAL trims to
its actual endpoint safely when that range is shorter than the current snap.
Creating an Automation Clip does not create an Audio Clip mixer destination.

The effective endpoint drives SONG looping, TempoMap/Timeline compilation,
built-in arrangement WAV and the supported experimental whole-song export path.
Whole-song plug-in/FX/PDC/fixed-tail restrictions stay in force. This does not
make realtime recording, selected-clip conversion and whole-song offline export
interchangeable.

Audio splits may edit a Clip tail outside a MANUAL playback range. Only that
non-realtime source-geometry query can extend the retained tempo source,
including later tempo automation; ordinary playback/export queries remain
strictly bounded. It reuses the 262,144-checkpoint admission limit rather than
extrapolating a constant boundary tempo. Splitting and saving unplayed tails must
preserve the manual endpoint and the audible PCM/WAV prefix.

## PAT and SONG show what will actually play

The selected mode has a high-contrast face: PAT is green, SONG is orange. The
Pattern chooser, playback-scope caption, Playlist title and ruler range accent
share the actual mode/endpoint. PAT plays one projection of the selected Pattern;
SONG plays its arrangement. Choosing another Pattern while in PAT changes that
projection; in SONG it chooses the next Pattern to draw.

Left-click or plain `L` changes mode without switching the focused editor.
Right-click PAT opens the Rack; right-click SONG opens the Playlist. Recording
still blocks mode changes. Mode preference remains independent of the Project
and music history.

## Open and edit the Pattern used by a Playlist clip

**New linking candidate: implemented and formatted; input/render gates pending.**
Double-click a Pattern clip with Piano notes to open its Piano roll. A step-only
Pattern opens the Rack; hold Alt to open the other editor. Opening locates the
clicked source bar, including source offset, without starting playback, seeking
the song or changing PAT/SONG. Audio and Automation keep their own editing paths.

Changing the main Pattern selects its first Channel with Piano notes. Choosing
a Channel manually in the Rack remains your choice on later frames or F7. The
Rack Channel context menu also opens that Channel's Piano roll.

Piano edits change the shared Pattern, so every Playlist reference updates its
note preview and compiled playback; Undo/Redo uses the existing shared history.
Placements retain their individual start, offset and trimmed length. There is
currently no per-placement follow-length policy: adding later source notes does
not automatically extend every existing placement.

In SONG, the Piano cursor follows the active placement's source-local position,
not the absolute song beat. An explicitly opened active occurrence takes
precedence; otherwise the latest-starting active occurrence wins, with Clip ID
as a tie-break. Muted placements, gaps and the song end do not show a playing
cursor. PAT uses its actual local loop. Rack steps follow the real transport
clock and their own step period, including Swing sampled at the pair start.

The interaction reference is Image-Line's [Playlist manual](https://www.image-line.com/fl-studio-learning-content/fl-studio-online-manual/html/playlist.htm)
(Automatic/Alt editor entry and click-position navigation) and [Piano roll manual](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/pianoroll.htm)
(Pattern auto-locate and occurrence context).

## Format and verification

Project **v13** persists the range policy. Files through v12 load with MANUAL;
new saves require a v13-capable build so older software cannot ignore AUTO and
silently truncate. Raw pre-v13 objects opting into AUTO must normalize before
saving; existing manual callers retain their entry-point contract. Original
works are never rewritten merely by loading them.

New regressions cover held ruler ownership, playing seek, actual PAT/SONG and
Pattern chooser inputs plus compiled note differences, recording/keyboard/
right-click behavior, zoom isolation, range undo, movement/resize past the old
end, cache invalidation, wide/narrow layout, range persistence and real built-in
WAV duration. A separate ignored test reads an explicitly supplied existing
nonempty plug-in-state project, saves only private copies, and checks the source
bytes remain unchanged.

For genuine app rendering, set `CITRUS_UI_CAPTURE_DIR` to an absolute private
directory and run `app::headless_ui_tests::playlist_transport_tests` serially.
The existing WGPU offscreen renderer reads production `App::ui` paint output;
it creates no native window or audio device. GPU availability must be coordinated.
The renderer writes genuine PPM readbacks; Pillow can convert those pixels
losslessly to PNG for inspection without changing the renderer.
Without that environment variable the same tests exercise actual egui input on
CPU. The24 new nonignored regressions, explicit Saved13 read-only/private-copy
round-trip,12 actual offscreen inputs and final mode/layout frame refinements
passed. Root inspected the four final frames. The copied preview's no-plugin
helper smoke passed; its main GUI/native desktop/soundcard/NOIRE remain untested
by the agent. Current sources preserve original failure evidence in the workflow record.
