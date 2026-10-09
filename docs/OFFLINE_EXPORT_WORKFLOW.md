# Offline WAV export workflow contract

This change improves the existing built-in renderer's safety and controls. It does not add VST offline bounce, sidechain rendering, stems, tail detection, or complete live/offline parity. The existing whole-song peak gain reduction above 0.95 remains unchanged.

## Fidelity preflight

Both the File menu and the rendering entry point reject enabled, nonempty, active non-Tempo automation. The error identifies the lane name, stable ID, and target, and remains visible until dismissed. Realtime Master Capture is offered as the existing route for recording the live result.

Activity follows the existing automation placement convention:

- A legacy lane without any corresponding Automation Clip is global, including endpoint holding before/after its points.
- A lane with placements is active only in its unmuted half-open clip windows that overlap the exported song.
- Disabled/empty lanes and placements that are entirely outside the song do not block export. Unrelated clips cannot disable a global lane.

Tempo automation remains supported by TempoMap. Active plug-ins and unsupported sidechains continue to fail explicitly.

## Background control and destination safety

The app captures a project snapshot and owns one background export at a time. Progress is an atomic, monotonic snapshot of completed preparation/render/mix/check/encode work, rather than an elapsed-time estimate. A single-slot result channel carries the terminal outcome. Repeated clicks and a Cancel request cannot start a second renderer while the old worker is still active.

Cancellation is checked between preparations and pattern cycles, every 4096 rendering/mixing/checking/encoding frames, between final output operations, and immediately before publication. Individual media reads/resampling, allocation, tempo-map construction, sorting, and OS flush/sync calls are not interruptible; the UI remains responsive and shows the pending cancellation until the worker reaches a checkpoint.

The output is staged beside the destination. Accepted cancellation closes and removes that staging file, leaving an existing destination unchanged. An atomic state transition arbitrates cancellation against the final rename: if Cancel wins, publication is impossible; once commit wins, Cancel is disabled and the worker completes or reports a commit failure. Progress does not show 100% until publication succeeds. There is no success notification for a cancelled export.

Installing another project requests cancellation and suppresses any old-session completion/error. The old slot is retained until its worker retires. The new project cannot accumulate an additional renderer during that interval. Closing the app requests cancellation; an OS/process termination cannot guarantee staging cleanup.

## Regression coverage

Production-source tests cover all non-Tempo target variants, placement/legacy gating, early and ten in-pipeline cancellation checkpoints, destination preservation and staging cleanup, the final cancellation/commit boundary, monotonic progress, repeated starts, stale project-session results, worker disconnects, normal export output, and a headless egui pass through progress/cancelling/error surfaces.

The focused source-only harness can compile exporter, job state, and egui surfaces without an audio device. It is not a complete application build or a Windows GUI/audio-device acceptance test. The normal Windows all-feature test/Clippy/build gates still apply to the integrated commit.
