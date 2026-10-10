# Native control automation clips

In the Channel Rack, right-click a Volume or Pan knob and choose **Create/open automation clip**. The same menu is available on Mixer pan knobs and volume faders, including Master. The Playlist opens with the clip selected and the Select tool active.

A new clip starts at the playhead rounded down to the current snap grid. Its length is four beats, shortened at the song end. The search starts at the top visible Playlist track, continues downward, then wraps to the top; the first track without an overlapping clip is used. If no track has space, creation fails without changing the project. This compact default differs from FL Studio's selected-range/song-span default.

Both envelope endpoints start at the control's current saved value. Volume uses 0–1; pan uses -1–1. Mixer targeting uses stable identities, so display reordering does not redirect the envelope. Creating the lane and clip is one undo step, separate from a preceding knob edit. No audio routing entry is added.

Repeating the command opens the existing target's earliest placement (start time, then clip ID) and leaves its envelope, mute/enabled state, project and undo history unchanged. Multiple lanes for one target are ambiguous and are rejected. An existing lane without a Playlist placement is also rejected: older projects can play unplaced lanes globally, and placing one would change that playback. Use the existing automation workflow to review such lanes explicitly.

## Editing and playback

With the selected clip and Select tool, double-click to insert a point, drag a point to change time/value, and right-click a point to delete it. The inspector provides precise position, value and tension edits, and Linear, Tension or Hold curves. Playlist move, split and Slip retain source-relative envelope positions. Save the project to retain the lane and clip; undo/redo restores creation together.

Use SONG mode to hear Playlist automation. PAT mode intentionally excludes it. Mixer/Master volume and pan use the existing UI-rate compatibility dispatcher, not sample-accurate callback automation. Native Channel targets are sample-exact only for internal Channels without compiled Generator routes. Channel volume/pan creation is rejected for plug-in-backed Channels with guidance to automate the assigned Mixer track or a plug-in parameter instead.

Plug-in parameters already have **AUTOMATE** in the generic parameter catalog. That existing flow and its native-editor/MIDI-port guards are unchanged. Eligible plug-in automation is Q128/block-rate; native-editor last-tweaked linking and automation recording are not added here.

Offline WAV export still refuses active unsupported non-Tempo automation rather than silently omitting it. This entry-point change adds no offline rendering or bounce fidelity.

Reference: [FL Studio Automation Clips manual](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/playlist_automationclip.htm).

## Validation scope

Focused regressions cover native target/value creation, pan extremes, stable reordered Mixer identity, deterministic repeated open, alias/ambiguity handling, invalid input, placement capacity, song-end length, persistence and atomic history. Native Linux pointer/device acceptance remains a separate check; pure model tests do not certify audible or native-window behavior.

### Automated checkpoint (2026-10-10)

Rust/app source at `097ff1f` passed 1,200 all-feature app tests, 1,183 no-default/core tests, 25 helper and 13 protocol tests, both strict all-target Clippy profiles, formatting, all-bin build and helper protocol smoke. Production egui input coverage includes menu dismissal, all four control entry points, repeated open and exact Undo/Redo. The two older generic-Pan-label fixtures failed at `35620ff`; precise target-label selectors were fixed and the full suite rerun. These are headless/source checks, not native window or audible automation acceptance.

The packaging follow-up includes this guide in the preview document allowlist. Actual input/link/provenance validation covers 553 package inputs; the 239 Python cases pass with the built helper and no skips. No Windows executable or release package was produced. Standalone vendor/domain-contract suites and physical/native QA were not rerun for this workflow-only change.
