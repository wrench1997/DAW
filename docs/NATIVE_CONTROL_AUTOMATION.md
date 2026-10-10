# Native control automation clips

In the Channel Rack, right-click a Volume or Pan knob and choose **Create/open automation clip**. The same menu is available on Mixer pan knobs and volume faders, including Master. The Playlist opens with the clip selected and the Select tool active.

A new clip starts at the playhead rounded down to the current snap grid. Its length is four beats, shortened at the song end. The search starts at the top visible Playlist track, continues downward, then wraps to the top; the first track without an overlapping clip is used. If no track has space, creation fails without changing the project. This compact default differs from FL Studio's selected-range/song-span default.

Both envelope endpoints start at the control's current saved value. Volume uses 0–1; pan uses -1–1. Mixer targeting uses stable identities, so display reordering does not redirect the envelope. Creating the lane and clip is one undo step, separate from a preceding knob edit. No audio routing entry is added.

Repeating the command opens the existing target's earliest placement (start time, then clip ID) and leaves its envelope, mute/enabled state, project and undo history unchanged. Multiple lanes for one target are ambiguous and are rejected. An existing lane without a Playlist placement is also rejected: older projects can play unplaced lanes globally, and placing one would change that playback. Use the existing automation workflow to review such lanes explicitly.

## Editing and playback

With the selected clip and Select tool, right-click a blank part of the curve (or double-click it) to insert a point. Right-click a point for Copy value, Paste value, precise normalized value entry, or Delete point. Cancel/Escape dismisses the menu without edits. The inspector provides precise position, value and tension edits, and Linear, Tension or Hold curves. Playlist move, split and Slip retain source-relative envelope positions. Save the project to retain the lane and clip; undo/redo restores creation together.

Use SONG mode to hear Playlist automation. PAT mode intentionally excludes it. Mixer/Master volume and pan use the existing UI-rate compatibility dispatcher, not sample-accurate callback automation. Native Channel targets are sample-exact only for internal Channels without compiled Generator routes. Channel volume/pan creation is rejected for plug-in-backed Channels with guidance to automate the assigned Mixer track or a plug-in parameter instead.

Plug-in parameters already have **AUTOMATE** in the generic parameter catalog. That existing flow and its native-editor/MIDI-port guards are unchanged. Eligible plug-in automation is Q128/block-rate; native-editor last-tweaked linking and automation recording are not added here.

Offline WAV export still refuses active unsupported non-Tempo automation rather than silently omitting it. This entry-point change adds no offline rendering or bounce fidelity.

Reference: [FL Studio Automation Clips manual](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/playlist_automationclip.htm).

## Validation scope

Focused regressions cover native target/value creation, pan extremes, stable reordered Mixer identity, deterministic repeated open, alias/ambiguity handling, invalid input, placement capacity, song-end length, persistence and atomic history. Native Linux pointer/device acceptance remains a separate check; pure model tests do not certify audible or native-window behavior.

### Automated checkpoint (2026-10-10)

Rust/app source at `097ff1f` passed 1,200 all-feature app tests, 1,183 no-default/core tests, 25 helper and 13 protocol tests, both strict all-target Clippy profiles, formatting, all-bin build and helper protocol smoke. Production egui input coverage includes menu dismissal, all four control entry points, repeated open and exact Undo/Redo. The two older generic-Pan-label fixtures failed at `35620ff`; precise target-label selectors were fixed and the full suite rerun. These are headless/source checks, not native window or audible automation acceptance.

The packaging follow-up includes this guide in the preview document allowlist. Actual input/link/provenance validation covers 553 package inputs; the 239 Python cases pass with the built helper and no skips. No Windows executable or release package was produced. Standalone vendor/domain-contract suites and physical/native QA were not rerun for this workflow-only change.

### FL-style point editing contract

Clipboard and typed values use a normalized 0–1 range. For pan, 0 is fully left (-1 native), 0.5 is center (0 native), and 1 is fully right (+1 native). The menu also shows the captured native value. Typed NaN, infinity and out-of-range values cannot be applied. The clipboard is internal to this application and is not the system clipboard.

Drag a point with Shift to lock value, Ctrl to lock time, or Alt to bypass snap. Times snap to the Playlist grid and remain in the visible clip's source span, including split/slipped clips. Points cannot cross or overwrite their neighbours: a colliding horizontal move retains the last valid time while vertical editing remains possible. Inserting at an occupied time does nothing. A menu action resolves its captured point, target and range again before changing anything; stale edits are discarded.

Each point action and completed drag is one undo transaction, separate from an immediately preceding project edit. Escape during a point drag restores its pre-drag project state; release outside the editor still finishes the transaction. Automation clips do not invoke the Playlist's generic right-click clip deletion; use the ordinary Delete command to remove a clip. Clipboard copy and canceled/no-op actions do not create history entries.

This bounded UI pass does not add Step/Slide drawing, segment-specific curve types, song-range creation, unique copies, multi-target binding, output-range scaling or LFOs. Source checkpoint tests are included; native-window/audio acceptance and actual execution results are reported separately.


### On-curve tension handles

Set the lane to **Tension** in the inspector, then select its clip with the Select tool. A diamond on each non-flat, sufficiently wide segment adjusts the left point's outgoing tension. Linear and Hold modes have no tension handles; this gesture never changes the lane's curve mode.

Drag a diamond vertically to bend the displayed curve in that direction. Horizontal movement is ignored. Hold Ctrl for ten-times finer adjustment; pressing or releasing Ctrl while holding still does not jump the curve. Tension stays within -1 to +1 and responds immediately when dragging back from a limit. Right-click a diamond to reset its outgoing tension to zero. The tooltip and active-drag readout show tension, the sampled native value and its native range (for example, pan -1 to +1).

For split or slipped clips, the diamond is at the midpoint of the segment's intersection with the clip's source span. Its value is sampled from the original segment, including when both original endpoints are outside that span. Scrolling hides an offscreen diamond rather than moving it to a new source time. Flat, disabled, invalid or narrower-than-24-pixel segments have no handles; handles do not overlap editable point hit areas. Curve drawing, point nodes and handles use the same Playlist-time X mapping.

A completed drag or reset is one Undo step, separate from a preceding edit. Escape restores the pre-drag project; releasing outside the Playlist still completes the drag. No-op gestures and reset-at-zero leave history, redo and saved-state status unchanged. This is a UI-only use of the existing outgoing tension data and curve evaluation. No project-format, engine or segment-specific interpolation changes are included.

Source tests cover mapping, source clipping, ignored modes, stale captures, Ctrl transitions, limit reversal, persistence and production egui pointer/history flows. Execution and native-window/audio acceptance must be reported separately for the exact checkpoint.
