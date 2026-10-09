# Local WAV sample browser

Status: implemented as a narrow Linux-first folder-listing/import slice. Native-dialog, visible GUI, physical output and audible audition acceptance are separate; no such acceptance is claimed here.

## Workflow

1. Open **Browser → SOUNDS → Open folder…** and choose a local folder. Nothing is scanned automatically on startup. Canceling the native picker leaves the current folder alone.
2. The browser shows direct subfolders and `.wav` / `.wave` file candidates, case-insensitively. Folders sort first, then names; file names are never treated as decoder validation. Double-click a folder or use **Up** to navigate. **Refresh** rereads that folder.
3. Search filters the currently listed names only. It does not recurse, search the whole disk, or find files excluded by a listing limit. A file hidden by the filter is not an import target.
4. Select a WAV candidate and click **Import to Playlist**. The existing strict decoder runs in the background. On success the app adds an Audio Asset and Audio Clip at the playhead captured when Import was clicked, on the selected clip's Playlist lane or lane 5 when none is selected. Existing lane-to-Mixer mapping is preserved. The imported duration uses the existing tempo/minimum-snap behavior.
5. A successful import is an explicit undo transaction. If an edit preview/gesture, save/recording, project transition or conflicting Project dialog starts during decoding, completion remains queued until that transaction ends; Cancel cannot restore a snapshot over a reported-success import. One Undo removes that import, leaving any preceding edit as its own history step. Redo restores the Project references; source files are never deleted or overwritten. Save the Project to retain the import.

Only one WAV import can run at a time across the File menu and Browser. The Browser disables import during recording, save/project/media operations and conflicting dialogs. The File menu retains its existing single-import workflow and shares the same decoder/preparation/history path. Import failures produce a toast plus a persistent Browser error with full details on hover. Invalid WAVs, unsupported encoding, moved/deleted files and failed reads do not create partial assets or clips.

Changing folders does not cancel an import that was explicitly started. **Cancel scan** cancels folder discovery only; it cannot undo a completed import or interrupt an operating-system call. A result from an older Project session is discarded by the existing import-generation guard.

## Supported media

The decoder supports strict RIFF/WAVE PCM 16/24/32-bit and IEEE float32, including its existing supported `WAVE_FORMAT_EXTENSIBLE` subset. MP3, FLAC, Ogg, AIFF, compressed WAV and PCM8 are not added by this feature. File bytes shown in the browser are an observation at listing time, not a claim that a file is still present or decodable. Import rereads and validates the file.

Decoded sample rate, channel count, native-frame source offsets, waveform peaks and normal Audio Clip runtime registration are preserved. There is no new project schema or audio-callback command path.

## Bounds, cancellation and privacy

- Directory enumeration runs on a control-side worker; it does not decode every candidate.
- Each listing retains at most **512 folder/WAV entries** and inspects at most **4,096 directory entries** (plus one iterator lookahead to determine that a cap was reached). Unsupported files count toward the inspection budget. A limit warning means the listing is incomplete; the retained subset depends on filesystem enumeration order, then sorts for display.
- A new navigation/refresh cancels the old request, clears its selection and rows, and coalesces repeated navigation into one latest request. There is at most one active scan doing filesystem work and one pending request, with a one-result mailbox. Both request generation and folder identity are checked before accepting results.
- Cancel, Drop, old successful results, old errors and worker disconnection are handled without joining a worker or waiting on filesystem I/O from the UI. OS calls on slow, unavailable or network-backed mounts can still take time; a replacement scan waits for the old call to return rather than spawning unbounded workers.
- Discovered symbolic links are skipped, including directory cycles, as are special files. The user-selected root may itself resolve through a link. This is browsing behavior, not a filesystem security sandbox; filesystem contents can change between inspection and import.
- Folder read errors and counts of unreadable/symbolic-link entries are displayed. Refresh or choose another folder to recover.
- WAV reads keep the existing **1 GiB source / 134,217,728 decoded interleaved samples** default ceilings. The actual source read is now bounded to the byte ceiling plus a one-byte over-limit probe, so growth after metadata inspection cannot bypass the source-read limit. Audio samples are normalized by the existing decoder.
- All work is local. Browser folder choices/search/selection are session-only and not uploaded, logged, or saved as preferences. A successfully imported source path is saved in the Project as before; this is a reference, not a copy/portable-project packaging operation. Non-UTF-8 Linux names retain exact paths while browsing even when the display needs replacement characters. The existing Project JSON path format cannot save such paths: import explicitly refuses them before changing history/assets, with a request to rename/copy the file (or non-UTF-8 parent folder) to a UTF-8 path.

## Honest UI and remaining scope

The previous hardcoded Sounds entries and synthetic preview waveform are removed. The footer now describes the real selected file and import action. It explicitly says audition is unavailable. No preview audio bus, drag-and-drop placement, sample assignment to Channels, recursive indexing, favorites, tags, waveform-preview decoding, sample editing or bundled sound content is introduced.

## Verification

The production scanner/state tests cover idle/no-auto-scan, direct-only WAV candidate listing, sort/filter behavior, output and inspection limits, stale/current navigation and refresh, cancel, Drop, missing/not-a-folder paths, symlink cycles and exact non-UTF-8 paths on Unix. Import regressions exercise the shared preparation path, source/mixer/native-frame identity, independent undo and preceding-edit history, serialization, invalid placement/ID exhaustion, invalid/deleted candidates using the real decoder, non-UTF-8 import rejection, completion barriers, and transform-cancel/gesture-commit history ordering. A reader regression verifies that a growing/infinite source is rejected after exactly the allowed bytes plus one probe.

On 2026-10-09, the separate source candidate passed all 901 Linux no-default/all-target tests, formatting, strict Clippy and the application debug build. This includes 23 new regressions: 13 scanner/state, nine import/history/barrier and one bounded-reader test. Exact commands and integration context are recorded in [WORK_LOG](WORK_LOG.md). These tests do not establish native-picker usability, GUI layout, audible playback, Linux device behavior or Windows compatibility.

Manual acceptance still required:

- Choose/Cancel a folder in the Linux native picker, browse Up/down, refresh and filter, test long/Unicode file names and a denied/unmounted directory.
- Rapidly navigate/refresh/cancel and verify no older folder or selection returns.
- Select then rename/delete/corrupt the file, import it, confirm a clear error and unchanged Project.
- Import a real supported WAV once, Undo/Redo, save/reopen and check clip position, asset reference and source file preservation.
- Use SONG playback to listen to the imported clip and compare source/channel/routing behavior. This validates arrangement playback, not a nonexistent Browser audition feature.

### Integrated verification, 2026-10-09 04:37 UTC

Reviewed feature commit `3e4335ed4a8152884640be821d603f75d12a8010` integrated cleanly as `abc9ec888a5b4c96ca229cc8544d15aabf3700ec`. The combined metering/browser source passes **901 Linux no-default/all-target tests** with zero failures/ignored on default stack, formatting, strict Clippy and application build. No executable changes were made during integration. The guide is explicitly included in the preview packaging whitelist; local packaging/helper discovery passes 56 tests and all packaged document links resolve. Windows and actual native-dialog/GUI/audio acceptance remain pending for this new slice.
