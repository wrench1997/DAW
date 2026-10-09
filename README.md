# Citrus Studio

> Status: 0.4 development snapshot. This is a usable Rust DAW foundation, not a completed FL Studio replica or a production-complete commercial DAW.

Citrus Studio is a clean-room, native music-production application written in Rust. It follows a pattern-first workflow while using its own product identity, vector iconography, layout, project format, DSP, and implementation.

Current checkout status and validation limits are recorded in `DEV_STATE.md`; development milestones and dated evidence are in `docs/DEVELOPMENT_ROADMAP.md` and `docs/WORK_LOG.md`. Historical Windows validation is not a fresh test result for this checkout. The source changes are published on the independent `ci/windows-reliability-20261009` branch. [Windows MSVC CI](https://github.com/wrench1997/DAW/actions/runs/37873807018) passed all development gates at `acdcf23`: formatting, 775 Rust tests, Clippy with denied warnings, app/helper debug builds, 14 developer-only Python harness tests, an actual plugin-free helper protocol smoke and the no-default-features check. The smoke verifies three JSON replies, invalid-command recovery, and explicit Shutdown/child cleanup. This result-summary follow-up changes documentation only. Real-device, GUI and pinned gnullvm release-package acceptance remain separate.

## Architecture

- `eframe`, `egui`, and `wgpu`: native GPU-rendered desktop UI
- `cpal`: host-aware audio input/output device profiles, deterministic stream negotiation, and callback telemetry
- `midir` 0.11/WinMM: Windows MIDI port ownership, bounded live-input handoff, and MIDI-output worker foundations
- `rtrb`: bounded lock-free SPSC command and recording queues
- Rust DSP: callback-authoritative Timeline execution, internal/audio-clip voices, a fixed-capacity 32-node/128-route Mixer DAG, typed automation rendering, graph-wide PDC, gain/mute/solo, and stereo balance pan
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
- `F5`, `F6`, `F7`, `F9`: Playlist, Channel Rack, Piano Roll, and Mixer
- `Ctrl+O`, `Ctrl+S`: open and save
- `Ctrl+Z`, `Ctrl+Y`: undo and redo
- `Ctrl+D`, `Delete`: duplicate and delete the current Playlist or Piano Roll selection
- `Ctrl+L`: apply Piano Roll Quick legato immediately
- `Alt+Q`, `Alt+S`, `Alt+U`, `Alt+F`, `Alt+L`, `Alt+A`: Piano Roll Quantize, Strum, Chop, Flam, Articulate, and Arpeggiate
- `Shift+G`, `Alt+G`: group and ungroup the selected Playlist Clips or Piano Roll notes

## 0.4 progress

### Editing and navigation

- Playlist horizontal and vertical scrolling, pointer-anchored time/track zoom, and viewport-culling across all 32 tracks and the complete configured song length
- Pattern, audio, and automation Clip modifier-click/marquee selection, Draw/Paint placement, group-aware movement and resizing, snapping with `Alt` bypass, splitting, muting, duplication, and deletion
- Playlist `S` Slip Edit moves Pattern, Audio, and Automation source content without moving Clip edges; Pattern phase reaches both realtime Timeline playback and offline export, Audio uses native asset-frame bounds, and visible Pattern ticks, waveform ranges, and automation curves follow the source offset
- Audio Clip fade handles and a two-Clip equal-power crossfade command: select exactly two same-track Audio Clips with a non-nested overlap; the command matches fade-out/fade-in to the overlap while preserving the opposite fades
- Automation data is shifted or split with its owning automation clip
- Piano Roll time/key scrolling and zoom across MIDI 0..127 and the complete active-pattern time range, with an explicit target-Channel selector, FL-style Draw/Paint/Delete/Mute/Slice/Select/Chord Stamp tools, keyboard audition, active-pattern ghost channels, marquee and modifier selection, independent time snap, note drawing, a forgiving right-edge resize grip available from every tool, duplicate, and one-step drag gestures
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

- Versioned Project v10 files, including v9 Piano Roll note-group and new Playlist Clip-group identities plus the v8 Mixer track/route and plug-in stable-ID schema; Mixer realtime slots stay independent of display order, with one instrument assignment per Channel, ten ordered effect slots per Mixer track, normalized parameters, and opaque vendor-state persistence
- Tagged plug-in-state save barriers for manual save/autosave: admitted live parameter edits drain before graph/state probes, pending graph changes must be callback-confirmed, transient control-queue pressure is retried, and unresolved state is reported before synchronized same-directory atomic project replacement; a manual save with stale state remains marked unsaved
- Full-chain load, replace, remove, enable, bypass, wet, missing/crashed state, and reported latency in the Channel/Mixer UI
- VST2 and VST3 backends on dedicated workers; a fixed 128-frame quantum adapter isolates plug-in scheduling from variable device callback sizes, and the audio callback never calls third-party plug-in code
- VST3 runs through the explicitly packaged process-isolation helper; VST2 remains inside the main process and therefore cannot survive a native access violation
- Callback-confirmed endpoint install/replace/remove, nonblocking worker retirement, bounded queues, fixed-quantum accumulation/bridge diagnostics, fail-closed event handling, and silent fallback under incomplete or invalid worker output
- Channel Generator MIDI note-on/note-off, bounded retry, all-notes-off on pause/stop/loop/mute/solo, internal-synth fallback while a generator is unavailable, and routing through its Mixer insert
- Exact-manifest `PluginParameter` delivery at Q128/block-rate for a one-slot Generator's physical slot 0 and active slots on ordinary Mixer Inserts 1..31, with shared endpoint quotas and parameter-before-Timeline-MIDI ordering at a quantum boundary
- Session- and endpoint-bound generic parameter catalog with immutable per-request snapshots, 64-item paging, bounded metadata, and explicit Create/Open Automation
- Reliable generic sliders with exact callback rejection and worker `Applied`/`Failed` receipts, retryable queue-full Drafts, per-target A/B/C coalescing to the latest value, and Project base-value commit only after the final worker `Applied`

Identified chains carry a creation-time, fixed-size slot-to-instance manifest shared by their audio and control endpoints. The callback consumes that manifest for the strict one-slot Generator/slot-0 path and for dense ordinary Mixer Insert chains; unknown, mismatched, inactive driven slots, bypassed, faulted, differently shaped, or latency-incoherent endpoints are rejected rather than guessed. Master Insert 0 and multi-slot Generator callback parameter routes remain unsupported. Catalog pages are accepted only for the exact project session, endpoint, instance, runtime slot, request, cursor, and catalog revision. An eligible slider edit follows the same exact identity route: callback rejection or worker failure is terminal, and the Project base value changes only after the worker returns `Applied`. Values currently owned by the compiled Timeline are displayed but locked. There are still no vendor editor windows, MIDI learn, gesture begin/end or grouped gesture undo, touch/latch/write automation, multi-output/sidechain bus negotiation, scanning sandbox, VST2 process isolation, or VST-aware offline rendering.

Compiled PAT and SONG notes are scheduled from authoritative Timeline frames on the audio callback and retain sample offsets while entering the fixed-quantum worker bridge. Extended Piano Roll content increases the derived Pattern period by whole bars, so notes beyond the historical sixteen-beat minimum compile in PAT mode and in a sufficiently long Playlist Pattern Clip. UI audition, the narrow hardware live-input route, and the legacy fallback command path are separate. The hardware route retains mapped sample offsets while transport is playing, but that narrow guarantee must not be generalized to every live or compatibility path.

### MIDI, media, and recording

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
- Arrangement-aware background export to stereo 24-bit PCM WAV for arranged Pattern and WAV Audio Clips, including Pattern Slip source phase, native-frame Audio source offsets, and the same plugin-free MainInput DAG fan-out/submix/tap/gain/mute/solo transfer, with buffered encoding and staged atomic replacement

Imported and recorded WAV assets now load off the UI/audio callback and enter the bounded realtime asset table. The Playlist transport starts, synchronizes, seeks, pauses, and stops clip voices; source-rate conversion, gain, fades, and the compiled Mixer-DAG controls are applied in the internal mix.

The Timeline frame clock, not a UI beat estimate, is authoritative once a revision is activated. Seek, loop range, playing state, frame, beat hint, revision, and epoch travel as one callback activation bundle with an exact receipt. The plug-in path uses a 128-frame fixed quantum: one quantum of input accumulation plus one asynchronous bridge quantum is inherent before reported plug-in latency. Each endpoint has non-borrowing System/Timeline/Live event quotas of 16/96/16 per quantum and 16/224/16 per device callback. Event overflow, stale output, endpoint identity or coherent-latency drift, and incomplete delayed-dry cases are observable and fail closed.

Current PDC covers the active MainInput DAG. It includes fixed-quantum and reported plug-in latency once, derives per-edge compensation at every join, crossfades dynamic delay-target changes, and rejects arithmetic overflow or delays outside the prepared capacity. Active sidechain buses, multi-output instruments, hardware/input latency, recording alignment, and plug-in-aware offline equivalence remain unsupported.

The arrangement exporter now mixes Pattern clips and referenced WAV Audio Clips with placement, source offset, sample-rate conversion, gain, fades, and the compiled plugin-free Mixer DAG, including chain/fan-out/diamond path summation and stable-ID display reordering. It still omits VST output/effects, automation-accurate parameter rendering, realtime nonlinear equivalence, stems, dithering choices, and plug-in delay compensation. To avoid silently producing the wrong mix, the 0.4 UI refuses offline WAV export while an enabled, non-bypassed plug-in placement or active sidechain route would be omitted.

Realtime Master Capture is connected through the File menu and diagnostics UI. It records the rendered stereo Master in real time, including whatever the live graph actually renders, using a bounded callback queue and background PCM24 writer. Install/stop are callback-confirmed; the writer synchronizes a temporary file before no-clobber publication. Gaps/overflow are surfaced as invalid-capture diagnostics. This does not implement deterministic offline plug-in bounce, automatic tails or stems. Current-session hardware validation is still pending.

Latest local hardening (not yet compiled or test-run in this environment): project-save preflight rejects non-finite persisted numbers before touching the destination; offline WAV export rejects rates outside 8000..=192000 Hz and non-finite rendered audio instead of silently changing the rate or encoding invalid samples. Master Capture collector shutdown ordering has also been corrected to drain final queued frames. See the work log for validation status.

## Commercial gaps

The following are required before Citrus Studio can be described as an FL-class commercial DAW:

- Sidechain and multi-output plug-in bus transport, automated route gain, graph-aware freeze/bounce, and richer routing ergonomics; the current MainInput DAG already supports bounded sends, submixes, display reordering, callback scheduling, and graph PDC
- Vendor editor windows, MIDI learn/controller mapping, gesture begin/end with grouped undo, touch/latch/write automation, preset management, VST2 process isolation, hardened probing, crash recovery, Master Insert and multi-slot Generator automation, and sample-exact parameter delivery
- A full sample editor with time-stretching, warping, pitch shifting, transient slicing, richer crossfade workflows, and sample-accurate realtime/offline Audio Clip conformance under rapid seeks and tempo changes
- ASIO and exclusive-mode support, vendor control-panel integration, seamless device hot-swap, hardened device-loss recovery, and sustained real-hardware validation; the current device profiles already cover selectable input/output devices plus deterministic sample-rate, channel, format, and buffer negotiation
- General track arming, multi-destination monitoring, input-latency compensation, punch/loop recording, overdub, take lanes, comping, and take management; with an exact MIDI route active, the current Record action captures only that one Generator into a fresh 16-beat Pattern placement
- Advanced Playlist and Piano Roll tools beyond the current Clip grouping/Slip core, manual scale, Chord Stamp, built-in Articulate, and Arpeggiate core: richer crossfade workflows, ripple editing, track playlists, consolidation, Audio Track linking, `.fsc` groove/slice/custom-Stamp/arpeggio templates, automatic top-down/bottom-up scale detection, Quantize/Arpeggio Note Levels, richer group management, advanced articulation banks, per-note expression, richer event lanes, and cross-pattern ghost sources
- Unified full-mix rendering with plugins/effects, automation-accurate parameters, stems, multiple bit depths, dithering, tails, PDC, and production-grade realtime export controls beyond the connected Master Capture
- Broader live MIDI: multi-input/multi-target and native-Channel routing, CC/pedal/pitch/aftertouch capture, MPE, SysEx, MIDI clock, controller learn/mapping, loop/punch/take recording, long-term input-clock drift and latency calibration, audio-clock/deadline-integrated output, and sustained physical-hardware stress
- Long-run stress, corrupt-plugin, device-change, project-migration, crash-recovery, and sample-accurate realtime/offline conformance testing
- Incremental command-based undo/dirty tracking and byte-bounded history for large projects; the current 50 ms change detector avoids per-frame clones but still serializes for detection and stores whole-project snapshots

See [docs/FL_STUDIO_PARITY.md](docs/FL_STUDIO_PARITY.md) for the detailed capability matrix.

## Realtime boundary

The output callback is designed not to wait on locks, perform filesystem I/O, call third-party plug-ins, or allocate ordinary working buffers. It mixes fixed preallocated buses, executes the fixed-capacity Mixer layout and sparse preallocated route-delay bank, accepts device callbacks up to the bounded block limit, and adapts worker exchange to stable 128-frame planar quanta through SPSC queues. Timeline execution, automation rendering, graph PDC, endpoint manifests, and retirement handoffs use fixed-capacity or preallocated callback state. Input capture writes complete interleaved frames into a bounded SPSC queue. These invariants are module-tested, but real hardware, unusual callback partitions, rapid transport changes, and a broad commercial plug-in corpus still require long-run stress testing.

## Clean-room and intellectual-property boundary

Citrus Studio does not contain Image-Line source code, artwork, icons, samples, presets, project data, branding, or copied pixel geometry. FL Studio and related names are used only to describe compatibility goals and workflow references. Citrus Studio is an independent project and is not affiliated with or endorsed by Image-Line. Comparable capability must be implemented and tested independently.

The Citrus Studio source is distributed under the [MIT License](LICENSE). Third-party components and plugin SDK bindings remain subject to their own licenses and trademarks.
