# Recover moved project audio

Implemented workflow: **File → Project media / relink…**. This supplements the existing autosave/Restore/Discard workflow; it does not change the project schema or replace the existing recovery file.

## Using the workflow

1. Open the project, then choose **Project media / relink…** in the File menu. Transport pauses without resetting the playhead. Finish active recording, imports, save barriers and project/device transitions first.
2. The media inventory lists every imported/recorded asset, its persisted path, expected sample rate/channel count/frame count, and the number of Clips referencing its stable asset ID. Missing/read errors remain visible. A file that is merely accessible is not described as successfully decoded. **Refresh paths** checks availability again.
3. Choose **Locate…** for the affected asset and select its WAV. Decoding and validation run in the background with the existing WAV allocation/format limits. The selected file must match sample rate, channel count and frame count. A different storage bit depth is allowed. Mismatches explain both expected and actual values; use Import audio for intentionally different material.
4. Review the original and replacement paths. Matching timing properties cannot establish that two files contain identical audio; select the intended original explicitly. Nothing changes until **Apply relink**. Enter cannot implicitly apply a replacement.
5. Apply rechecks file length/modification time and availability in the background, then changes only the selected asset reference, refreshes its decoded samples/waveform, and preserves Clip positions, lengths, native-frame source offsets, fades, groups, routing and asset identity. All Clips referencing that asset use the reviewed file.
6. Close the dialog and **save the project** to keep the new path. Undo restores the old reference; Redo reapplies it. Both invalidate old loader generations and runtime sample registrations when media references change. Locating the same file can explicitly reload it without adding a meaningless undo edit.

The selected replacement is stored as an absolute canonical path. Save As and reopening from another working directory therefore keep the selected destination. The feature does not copy, rename, delete or overwrite either media file, and it never silently rewrites the saved project. Ordinary manual save/autosave behavior remains unchanged.

## Interrupted and failed operations

- Cancel, Close, Escape, a project/session change, or a change to the original asset identity invalidates the pending result. A stale worker cannot relink a new project, even if it reuses the same asset ID.
- One media worker is admitted at a time. Cancel does not forcibly interrupt an operating-system file read; it revokes permission to apply its result. Reopening waits for that worker to finish before starting another check, avoiding multiple large simultaneous decodes. The rest of the app remains usable after closing the dialog.
- Deletion or a length/modification-time change after review causes Apply to fail without mutating project references. There is no source-file lock or cryptographic content identity guarantee; external software should not modify the chosen file concurrently.
- A save/device/lifecycle barrier that appears during final validation defers the result back to review. Apply must be clicked again after that operation ends.
- If a file disappears later, opening the inventory and refreshing reports it again. Relink does not embed audio in the project or guarantee future file availability.

## Boundaries

This milestone repairs one explicitly selected asset at a time. Folder-wide search/remapping, automatic same-filename replacement, relative project-media packaging, content hashes, and plugin/sampler-private media dependencies remain separate work. Only the project's imported/recorded WAV asset table is managed.

## Validation and acceptance

Source regressions cover:

- Matching and mismatched timing properties, accepted bit-depth-only changes, malformed/missing/directory candidates, and preservation of source files/Clip geometry.
- Absolute-path save/reopen and missing-again inventory, changed/deleted files after review, changed asset/session/ambiguous identity rejection.
- Review before Apply, repeated requests, cancellation and Close/Open with outstanding work, disconnected worker handling, and no stale Apply.
- App integration tests cover one-step undo/redo cache identity and keyboard safety. The existing every-modal shortcut regression includes the media modal.

2026-10-09 local validation: the 15 media regressions **passed** in a supplemental harness importing the exact `project_media`, `model`, `wav`, `automation`, and `mixer_graph` sources without audio mocks. Subset Clippy with denied warnings **passed**. This is not a full application build. The first broad harness run also exposed an existing Windows-path-specific model assertion on Linux; that unrelated assertion is not changed here. The two App integration tests and full Windows application checks still require Windows CI. Linux full compilation remains blocked by the already documented ALSA native dependency; no repeat installation or workaround was attempted.

Manual Windows acceptance still required:

- Open a saved project after moving an actual WAV; relink it, audition matching positions/fades, save, restart and reopen.
- Verify Undo/Redo audibly switch references and do not leave stale waveform/PCM; verify multiple Clips sharing one asset.
- Try a mismatched WAV, Cancel during decoding, repeated Close/Open, removal after review, and a new/open/quit project transition with work pending.
- Confirm the dialog remains usable at 1080×680 and with long paths, an offline audio device and a missing-again file.
