# Citrus Studio

> Status: 0.5.0-alpha.1 development prerelease. This is a usable Rust DAW foundation, not a completed FL Studio replica or a production-complete commercial DAW.

Project compatibility: this prerelease loads existing v10/v11 projects and saves project format v12. Older builds cannot reopen those new saves. Keep a backup of existing projects before evaluating the prerelease.

Citrus Studio is a clean-room, native music-production application written in Rust. It follows a pattern-first workflow while using its own product identity, vector iconography, layout, project format, DSP, and implementation.

Development is continuing toward a complete song workflow: create and edit an arrangement, save it, recover missing media, export the supported mix, and reopen it without losing work. Passing CI is a development baseline, not completion of that workflow or commercial release acceptance. Current implementation status is in [DEV_STATE.md](DEV_STATE.md); prioritized acceptance and dated evidence are in the [development roadmap](docs/DEVELOPMENT_ROADMAP.md) and [work log](docs/WORK_LOG.md).

The development branch is `ci/windows-reliability-20261009`; Linux remains primary. The [compact workspace](docs/COMPACT_WORKSPACE.md), [Piano melody shortcuts](docs/PIANO_KEYBOARD_EDITING.md), [mouse composition](docs/PIANO_MOUSE_WORKFLOW.md), typed clipboard and [note expression](docs/PIANO_NOTE_EXPRESSION.md) now include [independent edit/repeat ranges, precise local snap and compact minimum-window controls](docs/PIANO_RANGES_AND_SNAP.md). Range metadata is session UI state, not an audio playback loop; paste uses the containing left-visible four-beat bar. Citrus boundary/precision policies and remaining FL/OS/device gaps are explicit. At **`9782e62`**, [Windows preview](https://github.com/wrench1997/DAW/actions/runs/37914387094) fully passed 177 Python tests, 1,102 app +13 helper +5 protocol Rust tests, optimized static-CRT/provenance/PE/ZIP/hash checks and extracted-helper smoke; upload was disabled. [Quality](https://github.com/wrench1997/DAW/actions/runs/37914386973) passed source and trusted native state/interaction/lifecycle checks, while before/after paint failed and repaint comparison was skipped. New routing source needs its own Windows run; exact local results are in DEV_STATE/WORK_LOG.

[VST3 scanning](docs/VST3_SCANNING.md) now reads real default-class metadata and
MIDI capabilities through the isolated production helper rather than guessing from
filenames. Legacy caches request a rescan and failures stay explicitly unverified.
[Real free-plugin validation](docs/REAL_VST3_VALIDATION.md) records official Surge
instrument/FX offline audio and configured Stochas event generation at their
original historical sources. New downstream routing evidence is recorded separately.

[Plugin MIDI ports](docs/PLUGIN_MIDI_ROUTING.md) connect one channel MIDI producer to one or more instruments during playback. The first slice has exclusive sink inputs, constant tempo, conservative high-latency buffering and explicit safety limits. [Five real production-graph tests](docs/PLUGIN_MIDI_ROUTE_VALIDATION.md) passed with Stochas → Surge and genuine FX recovery. The 90.667 ms minimum two-endpoint bridge delay, observed debug callback over-budget, old Off-mode misses and unresolved historical concurrent-load loss remain explicit. Harmony Blueprint itself and platform-specific plugin UI/device acceptance remain unverified.

## Architecture

- `eframe`, `egui`, and `wgpu`: native GPU-rendered desktop UI
- `cpal`: host-aware audio input/output device profiles, deterministic stream negotiation, and callback telemetry
- `midir` 0.11/WinMM: Windows MIDI port ownership, bounded live-input handoff, and MIDI-output worker foundations
- `rtrb`: bounded lock-free SPSC command and recording queues
- Rust DSP: callback-authoritative Timeline execution, internal/audio-clip voices, a fixed-capacity 32-node/128-route Mixer DAG, typed automation rendering, graph-wide PDC, gain/mute/solo, and stereo balance pan
- [Measured mixer meters](docs/MIXER_METERING.md): real post-fader stereo sample peaks, pre-protection Master level, dBFS, peak hold and CLIP/fault status with stable-track identity and bounded callback handoff
- `vst` and `vst3-host`: worker-backed VST2/VST3 instruments and ordered Mixer effect chains, with a bundled VST3 isolation helper
- `serde`: versioned, human-readable `.citrus` projects

Application code, UI, project model, MIDI/WAV parsing, recording, automation, and the internal audio engine are implemented in Rust. Filesystem work, project serialization, media decoding, recording collection, and WAV encoding stay outside the realtime callback.

## Run

Install stable Rust and either Visual Studio C++ Build Tools or a compatible LLVM-MinGW toolchain, then run:

```powershell
cargo build --release --all-features --bins
cargo run --release
```

Building all bins first places the required VST3 helper beside the development executable. The packaged development build targets Windows 10/11 x64. A distributable VST3-enabled build must keep `vst3-host-helper.exe` and the toolchain runtime `libunwind.dll` beside `citrus-studio.exe`.

For isolated QA, `CITRUS_STUDIO_DATA_DIR` may point to an absolute directory; autosave, plug-in cache, and recordings then use that directory without touching the normal user-data location. Relative or empty overrides are ignored.

Useful shortcuts:

- `Space`: play or pause
- `L`: switch PAT (current Pattern loop) and SONG (Playlist arrangement) playback
- `Escape`: stop
- Playlist `P`, `B`, `D`, `T`, `C`, `S`, `E`: Draw, Paint, Delete, Mute, Slice, Slip Edit, and Select; `1` to `4` remain aliases for Select, Draw, Slice, and Mute
- `F5`, `F6`, `F7`, `F9`: show and focus Playlist, Channel Rack, Piano Roll, and Mixer
- `Ctrl+O`, `Ctrl+S`: open and save
- `Ctrl+Z`, `Ctrl+Y`: undo and redo
- Playlist `Ctrl+D`: duplicate the selection; `Delete`: delete selected Playlist Clips or Piano notes
- Piano `Alt+wheel`: relative note/selection velocity; `Ctrl+Alt+wheel`: finer control. Double-click a note for transactional properties; see [velocity and note properties](docs/PIANO_NOTE_EXPRESSION.md) for bounds, Undo and supported fields.
- Piano `Ctrl/Cmd+D`: deselect; `Ctrl/Cmd+B`: repeat the selected phrase to its right (all active-Channel notes when none selected)
- Piano `Shift+Left/Right`: move one Piano snap step; `Shift+Up/Down`: transpose one semitone; `Ctrl/Cmd+Up/Down`: transpose one octave
- Piano `Shift+D`: discard lengths to Piano snap; `Ctrl+Q` (macOS `Opt+Cmd+Q`): quantize starts and durations; `Shift+Q`: quantize starts only; `Alt+V`: toggle ghost notes
- These Piano melody operations use the selected notes/groups in the active Channel, or all notes in that Channel when none are selected. See [Piano keyboard editing](docs/PIANO_KEYBOARD_EDITING.md) for bounds and parity details.
- `Ctrl+L`: apply Piano Roll Quick legato immediately
- `Alt+Q`, `Alt+S`, `Alt+U`, `Alt+F`, `Alt+L`, `Alt+A`: Piano Roll Quantize, Strum, Chop, Flam, Articulate, and Arpeggiate
- `Shift+G`, `Alt+G`: group and ungroup the selected Playlist Clips or Piano Roll notes

## Development progress

### Editing and navigation

- [Simultaneous editor windows](docs/MULTIWINDOW_WORKSPACE.md): move/resize/hide Playlist, Channel Rack, Piano Roll and Mixer independently inside the native app, with saved layout, focus-aware editing, shared project/Undo and maximize/restore; detached OS editor windows remain unimplemented
- Playlist horizontal and vertical scrolling, pointer-anchored time/track zoom, and viewport-culling across all 32 tracks and the complete configured song length
- Pattern, audio, and automation Clip modifier-click/marquee selection, Draw/Paint placement, group-aware movement and resizing, snapping with `Alt` bypass, splitting, muting, duplication, and deletion
- Playlist `S` Slip Edit moves Pattern, Audio, and Automation source content without moving Clip edges; Pattern phase reaches both realtime Timeline playback and offline export, Audio uses native asset-frame bounds, and visible Pattern ticks, waveform ranges, and automation curves follow the source offset
- Audio Clip fade handles and a two-Clip equal-power crossfade command: select exactly two same-track Audio Clips with a non-nested overlap; the command matches fade-out/fade-in to the overlap while preserving the opposite fades
- Automation data is shifted or split with its owning automation clip
- [Piano edit ranges and snap](docs/PIANO_RANGES_AND_SNAP.md): independent ruler selection/repeat width, range shortcuts, visible-bar paste, Off/fine/triplet rational grids and compact NOTE EDIT / SCALE menus at narrow widths; no playback-loop or global Playlist-snap change
- Piano Roll time/key scrolling and zoom across MIDI 0..127 and the complete active-pattern time range, with an explicit target-Channel selector, FL-style Draw/Paint/Delete/Mute/Slice/Select/Chord Stamp tools, keyboard audition, active-pattern ghost channels, marquee and modifier selection, independent time snap, note drawing, a forgiving right-edge resize grip available from every tool, duplicate, and one-step drag gestures
- Piano-only select-all and session-local Cut/Copy/Paste, with original channel/group/note values, a visible viewport-bar paste anchor in PAT and SONG and one-step undo; [clipboard semantics and limits](docs/MULTIWINDOW_WORKSPACE.md#piano-note-clipboard)
- Persistent Project v10 Playlist Clip groups and v9 Piano note groups with `Shift+G`/`Alt+G`, a nondestructive grouping-behavior switch, linked visual markers, and group-aware click/marquee selection, movement, resizing, mute, delete, duplicate, transforms/Inspector edits where applicable, and history
- Manual scale helpers provide C..B roots, 13 built-in scale/mode choices, root-aware key/grid shading, and optional pitch constraint for newly drawn notes, canvas moves, and Inspector pitch/time moves. Existing out-of-scale notes remain unchanged until one of those edits; Alt temporarily bypasses both canvas time and scale snap, while incoming MIDI is not rewritten
- Chord Stamp provides 17 interval presets, snapped hover preview, target-Channel placement, stable note IDs, same-step duplicate suppression, scale-aware pitch constraint, optional `ONLY ONE` return to Draw, chord audition, and one undo transaction per successful click. The scale and Stamp choices are versioned editor preferences rather than Project music data
- Realtime-preview Quantize, Strum, Chop, Flam, Articulate, and Arpeggiate dialogs operate on the selected notes in the target Channel or fall back to all notes in that Channel; Reset/Cancel restore the exact pre-dialog state and Accept commits at most one undo transaction. Articulate includes Legato/Portato/Staccato/Small gap/Chop chords presets, deterministic length variation, boundary scope, and immediate `Ctrl+L` Quick legato. Arpeggiate includes Up/Down/Up-down/Down-up directions, 1–4 octave range, Time/Block/Chord synchronization, gate, stable ID reuse, and optional generated-note grouping
- Named patterns with independent step and note data
- FL-style PAT/SONG playback selection: PAT compiles only the selected Pattern into an isolated callback Timeline, excludes Playlist audio and automation, follows the current Pattern when the selector changes, and grows its loop to the next complete four-beat bar when Piano Roll notes extend the score; SONG compiles the full Playlist arrangement. The mode is an application preference, `L` toggles it, and right-clicking PAT/SONG opens the Channel Rack/Playlist respectively
- Channel-to-mixer routing controls and realtime internal-instrument routing
- Contextual Browser, Inspector, tool state, mixer controls, autosave/recovery, and bounded snapshot undo/redo for core edits

### Typed automation

- Serializable targets for master, mixer, channel, tempo, swing, and plugin parameters
- Sorted and deduplicated points with safe insertion, movement, update, and deletion
- Linear, hold, and bounded monotonic tension curves
- Optional beat-loop evaluation
- Stable runtime frames with last-lane-wins target merging
- Sparse epsilon-filtered runtime deltas with explicit cleared targets
- A fixed-capacity typed Timeline executor emits distinct chase values, in-block transitions, and exclusive block endpoints
- A transactional audio-callback kernel renders linear and hold values per sample and commits value/shape state only after the whole block succeeds

The callback compiler and kernel cover every compiled target, but rendering a value is not the same as applying it to DSP. **Working narrow scope:** `ChannelVolume`, `ChannelPan`, and `ChannelMute` are applied sample-exactly to an internal/native Channel only when that Channel has no compiled Generator route; the values affect raw source samples before source PDC. Exact-manifest plug-in endpoints can consume `PluginParameter` automation at 128-frame quantum boundaries for a one-slot Generator's physical slot 0 and for active physical slots on ordinary Mixer Inserts 1..31. These routes are Q128/block-rate rather than sample-exact, are invariant to supported device-callback splitting, and order each parameter update before Timeline MIDI at the same quantum boundary. Mixer Insert values are read from transactional control history using the active graph-PDC source timing plus the physical-slot latency prefix, and every submitted quantum is attested against the coherent latency revision. Endpoint identity, manifest, active-slot, PDC-plan, or latency-revision drift fails closed and requests Timeline resynchronization. Generator-backed application of the native Channel targets, multi-slot Generator parameters, Master Insert 0 parameters, and the Master, Mixer-control, `Tempo`, and `Swing` target families remain explicitly pending/unsupported by the callback. The older UI-rate automation dispatcher still updates some of those controls and plug-in parameters, but that compatibility path is not sample-accurate. The exact paged generic catalog supports explicit automation creation and reliable live slider edits for eligible parameters. Timeline-owned targets remain read-only while their compiled automation owns the value. MIDI learn/controller mapping, gesture begin/end with grouped undo, automation recording, and touch/latch/write modes are not implemented.

### VST instruments and effects

- Versioned Project v12 files with persisted plugin MIDI ports (legacy projects default Off), including inherited Audio Clip fade/source/exact-length references plus v10 Playlist Clip-group and v9 Piano Roll note-group identities plus the v8 Mixer track/route and plug-in stable-ID schema; Mixer realtime slots stay independent of display order, with one instrument assignment per Channel, ten ordered effect slots per Mixer track, normalized parameters, and opaque vendor-state persistence
- Tagged plug-in-state save barriers for manual save/autosave: admitted live parameter edits drain before graph/state probes, pending graph changes must be callback-confirmed, transient control-queue pressure is retried, and unresolved state is reported before synchronized same-directory atomic project replacement; a manual save with stale state remains marked unsaved
- Full-chain load, replace, remove, enable, bypass, wet, missing/crashed state, and reported latency in the Channel/Mixer UI
- VST2 and VST3 backends on dedicated workers; a fixed 128-frame quantum adapter isolates plug-in scheduling from variable device callback sizes, and the audio callback never calls third-party plug-in code
- VST3 runs through the explicitly packaged process-isolation helper; VST2 remains inside the main process and therefore cannot survive a native access violation
- Callback-confirmed endpoint install/replace/remove, nonblocking worker retirement, bounded queues, fixed-quantum accumulation/bridge diagnostics, fail-closed event handling, and silent fallback under incomplete or invalid worker output
- Channel Generator MIDI note-on/note-off, bounded retry, all-notes-off on pause/stop/loop/mute/solo, internal-synth fallback while a generator is unavailable, and routing through its Mixer insert
- Exact-manifest `PluginParameter` delivery at Q128/block-rate for a one-slot Generator's physical slot 0 and active slots on ordinary Mixer Inserts 1..31, with shared endpoint quotas and parameter-before-Timeline-MIDI ordering at a quantum boundary
- Session- and endpoint-bound generic parameter catalog with immutable per-request snapshots, 64-item paging, bounded metadata, and explicit Create/Open Automation
- [Windows VST3 native editors](docs/NATIVE_VST3_EDITORS.md): helper-owned windows, bounded lifecycle/dirty-state reporting, exact save capture and base-value preservation; Linux/VST2 and automated instances remain unsupported
- Reliable generic sliders with exact callback rejection and worker `Applied`/`Failed` receipts, retryable queue-full Drafts, per-target A/B/C coalescing to the latest value, and Project base-value commit only after the final worker `Applied`

Identified chains carry a creation-time, fixed-size slot-to-instance manifest shared by their audio and control endpoints. The callback consumes that manifest for the strict one-slot Generator/slot-0 path and for dense ordinary Mixer Insert chains; unknown, mismatched, inactive driven slots, bypassed, faulted, differently shaped, or latency-incoherent endpoints are rejected rather than guessed. Master Insert 0 and multi-slot Generator callback parameter routes remain unsupported. Catalog pages are accepted only for the exact project session, endpoint, instance, runtime slot, request, cursor, and catalog revision. An eligible slider edit follows the same exact identity route: callback rejection or worker failure is terminal, and the Project base value changes only after the worker returns `Applied`. Values currently owned by the compiled Timeline are displayed but locked. Windows VST3 native Open/Close controls and exact state capture are now integrated; native Linux/VST2 editors remain unsupported, automated instances are explicitly excluded, and Windows fixture/real-vendor execution is tracked separately. There are still no MIDI learn, gesture begin/end or grouped gesture undo, touch/latch/write automation, multi-output/sidechain bus negotiation, scanning sandbox, VST2 process isolation, or VST-aware offline rendering.

Compiled PAT and SONG notes are scheduled from authoritative Timeline frames on the audio callback and retain sample offsets while entering the fixed-quantum worker bridge. Extended Piano Roll content increases the derived Pattern period by whole bars, so notes beyond the historical sixteen-beat minimum compile in PAT mode and in a sufficiently long Playlist Pattern Clip. UI audition, the narrow hardware live-input route, and the legacy fallback command path are separate. The hardware route retains mapped sample offsets while transport is playing, but that narrow guarantee must not be generalized to every live or compatibility path.

### MIDI, media, and recording

- [Local WAV browser](docs/LOCAL_SAMPLE_BROWSER.md): explicitly chosen folders, bounded one-level listing, current-folder search, Up/Refresh/Cancel and validated Import to Playlist with one-step undo; hardcoded Sounds items and the synthetic preview are removed. Audible Browser audition remains unimplemented.
- Standard MIDI File format 0 and format 1 import/export with PPQ timing
- Backend-independent realtime MIDI 1.0 core for bounded channel-message validation, integer timestamp mapping, overload coalescing, panic signaling, and fixed-capacity note pairing
- Windows `midir` 0.11/WinMM port enumeration and hot-plug refresh, with one selected input connection routed through its dedicated bounded SPSC to one callback-confirmed one-slot Generator. The route is stamped with exact project, endpoint, instance, and slot identity; install/remove receipts are exact, receivers retire off the callback, and disconnect, replacement, transport-epoch, or overload cleanup drives CC123/all-notes-off safety
- The first event anchors the connection timestamp to the current audio-device frame; later timestamps use integer delta mapping, late events clamp to the current callback, and future events remain queued. Playing delivery preserves mapped sample offsets subject to the Generator's bounded Live Q quota. While paused, only the target Generator and its enabled downstream Mixer-DAG closure advance for audible monitoring; Timeline, native/audio voices, unrelated Generators, and non-downstream plug-in paths stay frozen
- A dedicated bounded MIDI-output worker with an emergency lane exists, but it still sends promptly rather than from audio-clock deadlines and is not integrated with the audio render path
- Import merges file tracks into the active Piano Roll pattern and reads the initial tempo
- Export writes the active pattern. Realtime Note On/Off accepted by the exact live Generator route can also be captured while the Timeline is playing: callback device/timeline frames are retained, the frozen TempoMap is used at finalization, and a valid take is committed all-or-none as one new Pattern plus one Playlist clip and one undo step
- The recording slice is deliberately limited to one callback-confirmed input/Generator route and one non-looping 16-beat Pattern period. Paused audition is never captured; a route, project, Timeline revision/epoch, loop, timestamp, capacity, or source-context change invalidates the take without mutating the Project
- Strict RIFF/WAVE import for PCM 16/24/32-bit and IEEE float32, including supported `WAVE_FORMAT_EXTENSIBLE` files
- Explicit file/allocation limits, malformed/truncated-file rejection, normalized interleaved samples, and deterministic linear resampling
- Lock-free input callback handoff and fixed-block background PCM24 streaming into a synchronized, atomically published WAV
- Completed recordings are registered as project assets and automatically placed on the Playlist at the recorded beat, using the selected clip's lane or the default recording lane

Recording memory is bounded: the callback ring is capped at 32 MiB and the writer streams in 64 KiB blocks. A take stops at the same 134,217,728-interleaved-sample safety limit used by WAV import (about 23.3 minutes for 48 kHz stereo), reports omitted samples, and never publishes a partial file as complete.

The audio-device milestone defines serializable, host-aware CPAL profiles for both output and input. A profile identifies either the host default or a stable `(host_id, device_id)` pair. Sample rate and channel count use explicit `Default`, `Exact`, or `Nearest` policies; sample format uses default, exact, or deterministic automatic selection; and buffer size uses backend-default, fixed, or nearest selection. Catalog ordering and negotiation are deterministic, and Settings can select both the output device and the input used for recording. Device preferences are stored through `eframe` storage independently of Project data.

Settings uses a compact category rail modeled on a studio application's System Settings flow. Its Audio page keeps device, sample rate, format, channel count, buffer, status, and Apply/Refresh controls on the primary surface, while raw callback/PDC/Timeline diagnostics live on the Debug page. It keeps three layers distinct: the requested profile, the effective profile resolved for the running stream, and actual callback observations such as callback frame count, timing, and stream faults. Output changes use a controlled restart path that prepares and warms a candidate while retaining the old engine, then attempts the requested profile, the last-known-good profile, and the system default in that order. This is not seamless hot-swap: integrated restart/rollback failure paths are still being validated, and real devices need sustained unplug/replug, callback-partition, and long-run testing. On Windows the current default CPAL backend is WASAPI; ASIO is not enabled.

### Playback, mixing, and export

- Lock-free output command queue and preallocated internal/audio-clip voice state
- Audio-callback Timeline execution for compiled Pattern notes, Audio Clips, automation packets, and exact frame/epoch cursor state
- Atomic seek/loop/play activation: candidate Timeline, fixed Mixer layout, sparse route-delay bank, exact endpoint/latency identities, chase, automation state, voices, PDC reset, and transport fields are preflighted together; a rejected activation leaves the currently audible revision and delay histories running
- Internal synth playback through mixer level, mute, solo, routing, track pan, and master pan
- Preallocated 32-track stereo block buses execute a validated acyclic MainInput graph with stable track/route IDs, up to 128 active route slots, fan-out sends, serial submixes, PreEffects/PostEffects/PostFader taps, route gain, ordered VST chains, and a Master-only final sink. Mixer display order is independent of callback routing identity
- Graph-wide PDC aligns raw native/audio sources, worker-backed Generators, Insert stages, and every active MainInput edge at each DAG join. Sparse preallocated route delays, dynamic coherent-latency retiming, exact revision attestation, and fail-closed resynchronization keep callback mutation bounded
- Stereo audio-clip voice DSP with source-rate conversion, interpolation, gain, routing, pan, mute/solo, and drift-aware sync primitives
- Seek-aware realtime Audio Clip registration, play/sync/stop lifecycle with source offsets, normalized fades, Mixer routing, and generated waveform previews
- Arrangement-aware background export to stereo PCM16, PCM24 or float32 WAV (PCM24 remains the legacy default) for arranged Pattern and WAV Audio Clips, including Pattern Slip source phase, native-frame Audio source offsets, and the same plugin-free MainInput DAG fan-out/submix/tap/gain/mute/solo transfer, with buffered encoding and staged atomic replacement

Imported and recorded WAV assets now load off the UI/audio callback and enter the bounded realtime asset table. The Playlist transport starts, synchronizes, seeks, pauses, and stops clip voices; source-rate conversion, gain, fades, and the compiled Mixer-DAG controls are applied in the internal mix.

The Timeline frame clock, not a UI beat estimate, is authoritative once a revision is activated. Seek, loop range, playing state, frame, beat hint, revision, and epoch travel as one callback activation bundle with an exact receipt. The plug-in path uses a 128-frame fixed quantum: one quantum of input accumulation plus one asynchronous bridge quantum is inherent before reported plug-in latency. Each endpoint has non-borrowing System/Timeline/Live event quotas of 16/96/16 per quantum and 16/224/16 per device callback. Event overflow, stale output, endpoint identity or coherent-latency drift, and incomplete delayed-dry cases are observable and fail closed.

Current PDC covers the active MainInput DAG. It includes fixed-quantum and reported plug-in latency once, derives per-edge compensation at every join, crossfades dynamic delay-target changes, and rejects arithmetic overflow or delays outside the prepared capacity. Active sidechain buses, multi-output instruments, hardware/input latency, recording alignment, and plug-in-aware offline equivalence remain unsupported.

The arrangement exporter now exposes [format/rate/level review](docs/WAV_EXPORT_OPTIONS.md) and mixes Pattern clips and referenced WAV Audio Clips with placement, source offset, sample-rate conversion, gain, fades, and the compiled plugin-free Mixer DAG, including chain/fan-out/diamond path summation and stable-ID display reordering. It still omits VST output/effects, automation-accurate parameter rendering, realtime nonlinear equivalence, stems, dithering choices, and plug-in delay compensation. To avoid silently producing the wrong mix, the UI and rendering preflight refuse offline WAV export while an enabled, non-bypassed plug-in placement, active sidechain route or active unsupported non-Tempo automation would be omitted.

Realtime Master Capture is connected through the File menu and diagnostics UI. It records the rendered stereo Master in real time, including whatever the live graph actually renders, using a bounded callback queue and background PCM24 writer. Install/stop are callback-confirmed; the writer synchronizes a temporary file before no-clobber publication. Gaps/overflow are surfaced as invalid-capture diagnostics. This does not implement deterministic offline plug-in bounce, automatic tails or stems. Current-session hardware validation is still pending.

Validated baseline hardening: project-save preflight rejects non-finite persisted numbers before touching the destination; offline WAV export rejects rates outside 8000..=192000 Hz and non-finite rendered audio instead of silently changing the rate or encoding invalid samples. Master Capture collector shutdown ordering drains final queued frames. Their regressions passed in the Windows suite above; this does not establish GUI or real-device acceptance.

## Next product milestones

The next acceptance target is one dependable song workflow, with a reproducible project and evidence for every step. These are prioritized milestones, not completed features or delivery promises.

1. **Edit without changing the sound unintentionally.** Existing Slip, equal-power fades/Crossfade and grouped undo are connected. The newly integrated [Audio split-fidelity implementation](docs/AUDIO_SPLIT_FIDELITY.md) preserves the original per-side fade domains, source phase and exact clip ends across repeated cuts, movement, save/reopen, callback playback and offline rendering. Focused numerical/persistence/real-callback tests passed before integration; full candidate Windows validation passed; manual GUI/audio validation remains pending.
2. **Save and recover a usable project.** Atomic saves, plug-in-state barriers, autosave and restore already exist. [Project media / relink](docs/PROJECT_MEDIA.md) is now integrated with a persistent inventory, visible media errors and validated, undoable relinking that preserves asset identity and native-frame offsets. The integrated media checkpoint passed all Windows gates with 792 Rust tests, including 17 new media/App regressions; manual GUI/audio validation remains pending. A restored project must be explicitly savable and reopen with the same arrangement and audio references.
3. **Deliver the mix honestly.** Plugin-free stereo PCM24 arrangement export and realtime Master Capture already exist. The newly integrated [export workflow](docs/OFFLINE_EXPORT_WORKFLOW.md) adds progress, cancellation with destination preservation, single-job/session guards and explicit refusal of active unsupported non-Tempo automation. The progress/cancellation core passed the 805-test Windows checkpoint. New encoding/rate/level settings and v11 split rendering passed combined Windows validation; native-dialog/physical output acceptance remains separate. Acceptance must also verify duration, source offsets/fades, final frames, failed writes and playable output. Offline plug-in rendering, automation equivalence, tails, stems and freeze/bounce remain separate missing capabilities; live capture does not complete them.
4. **Install and finish a session on real Windows hardware.** A fresh pinned-toolchain Release package must launch on a clean system, find its helper/runtime, and pass the create/edit/save/recover/export/reopen scenario plus device and plug-in interruption checks. An opt-in [MSVC preview-packaging lane](docs/WINDOWS_PREVIEW.md) is now integrated for Windows validation; no verified Windows package is claimed yet, uploads default off, and it does not replace the pinned gnullvm release contract. No release candidate or commercial-maturity claim is established yet.

The [roadmap](docs/DEVELOPMENT_ROADMAP.md) defines the exact acceptance checks and evidence levels. Broader recording, plug-in, MIDI, automation, editing, performance and accessibility limitations remain in the [capability matrix](docs/FL_STUDIO_PARITY.md); a successful song-flow milestone does not imply FL-class parity.

## Realtime boundary

The output callback is designed not to wait on locks, perform filesystem I/O, call third-party plug-ins, or allocate ordinary working buffers. It mixes fixed preallocated buses, executes the fixed-capacity Mixer layout and sparse preallocated route-delay bank, accepts device callbacks up to the bounded block limit, and adapts worker exchange to stable 128-frame planar quanta through SPSC queues. Timeline execution, automation rendering, graph PDC, endpoint manifests, and retirement handoffs use fixed-capacity or preallocated callback state. Input capture writes complete interleaved frames into a bounded SPSC queue. These invariants are module-tested, but real hardware, unusual callback partitions, rapid transport changes, and a broad commercial plug-in corpus still require long-run stress testing.

## Clean-room and intellectual-property boundary

Citrus Studio does not contain Image-Line source code, artwork, icons, samples, presets, project data, branding, or copied pixel geometry. FL Studio and related names are used only to describe compatibility goals and workflow references. Citrus Studio is an independent project and is not affiliated with or endorsed by Image-Line. Comparable capability must be implemented and tested independently.

The Citrus Studio source is distributed under the [MIT License](LICENSE). Third-party components and plugin SDK bindings remain subject to their own licenses and trademarks.
