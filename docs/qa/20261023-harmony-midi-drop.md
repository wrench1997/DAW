# Harmony Blueprint MIDI drop — Windows local work

## Environment and preservation

- Computer: `WIN-MF931KNK000`.
- Original repository: `D:\workspaces\DAW`, remote `https://github.com/wrench1997/DAW.git`.
- Original branch: `ci/windows-reliability-20261009`, clean at `314779d87d629f2c8e31c361afd12af30cc5c8a2`.
- Original HEAD is the ancestor of `1201b71ec508f9f52d76eb6b68732bf6c575e787` (0 local-only / 8 baseline-only commits).
- Original checkout was not edited. No push, merge, publication, installation, activation or system security change.
- Independent clone: `C:\Users\Administrator\Documents\Codex\2026-10-23\task\DAW-harmony`.
- Branch: `feat/harmony-midi-drop-20261023`, based on the specified `1201b71`.
- Delivered UI patch is preserved separately in commit `956843d`.
- No AGENTS.md or .agents guidance was found in the original repository, delivery source, or the checked workspace parents.
- Available: Git, Cargo/Rust, Python, Node, PowerShell. `rg` was not available.

## Delivered source verified locally

Library item `libfile_904bab67d5f4819184b54a6ea5f03897` was prepared and downloaded to the local task directory as `daw-fl-review-20261022.zip` (8,642,577 bytes).

Archive SHA256: `924fafdf61c6000273e312b5878438fed14b6d42a5ebca8c08bac0c0645dfbdd`.
All 1,401 entries in its SHA256SUMS matched after extraction. The patch passed `git apply --check` against the specified baseline before application.

## Installed plugin and external evidence

Found `C:\Program Files\Common Files\VST3\Harmony Blueprint.vst3`.
Its `Contents\x86_64-win\Harmony Blueprint.vst3` reports file/product version **1.2.2**. An ARM64 binary is also present. Installation alone does not establish successful loading or activation.

The [official page](https://www.dreimusic.com/harmony-blueprint) confirms Record Melody drag-out, chord-map guide drag-out, and separate MIDI-output routing to another instrument. The supplied YouTube video was not watched.

## Implemented behavior

- Drop a `.mid`/`.midi` file onto the intended Channel Rack row, the selected instrument's Piano Roll note grid, or the Sampler's explicit **Drop MIDI notes here (append at beat 1)** region.
- All MIDI voices/tracks go to that explicit stable channel ID. Notes keep beat positions, lengths, pitches and velocities, with new unique note IDs.
- Append at beat 1 is the fixed, visible policy: existing target and other-channel notes, BPM, sample configuration and arrangement remain intact. Existing notes may overlap the newly appended notes; Undo removes the whole drop.
- Path payloads are read immediately, before another frame/dialog, and byte payloads are supported. No deferred reread of a plugin temporary file.
- Input is bounded to 16 MiB, 65,536 notes per Pattern and 4,096 beats; malformed, empty, missing-target or unsupported timing input fails without changing the project. Multiple-file drops are rejected.
- The candidate project is committed via the existing explicit history transaction and timeline invalidation path; one Undo/Redo covers the whole drop.
- The waveform region still loads WAV audio. MIDI adds notes rather than replacing the WAV. This does not implement plugin real-time MIDI output routing to Sampler.

## Verification

- `cargo check --locked --offline`: passed.
- `cargo test --locked --offline --bin citrus-studio midi_drop -- --test-threads=1`: **4 passed** in the authorized local execution context.
- Tests cover production App UI drop handling for Rack paths, Piano bytes and Sampler bytes; preservation of existing notes/channels/BPM; target pitch, duration and velocity; one-step Undo/Redo; disk save/load; invalid data and missing targets.
- The export test imports MIDI, saves/reopens the project, renders and decodes a real WAV using the native Sampler, and checks amplitude and gate duration. A 0.25-beat note at the preserved 120 BPM occupies exactly 1,000 frames at 8 kHz; amplitude matches velocity 64/127.
- Initial sandbox run failed Windows atomic file replacement with OS error 5; local rerun passed. An initial render-only fixture had three mixer tracks and was corrected to the required 32-track save schema.
- Full default binary unit regression: **2,213 passed, 0 failed, 7 ignored**, 392.95 seconds. Command: `cargo test --locked --offline --bin citrus-studio -- --test-threads=2`. Ignored tests require pre-existing private Kontakt/NOIRE test assets and explicit opt-in; this is not commercial-plugin acceptance.
- All three existing integration targets passed: reset-origin protocol **6**, VST3 editor protocol **5**, VST3 process/transport protocol **2**; raw log `regression-integration.log`. Combined unit/integration result: **2,226 passed, 0 failed, 7 ignored**.
- `cargo check --locked --offline --no-default-features`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo build --locked --offline --bins`: passed. `target\debug\citrus-studio.exe` is 68,613,120 bytes and adjacent `vst3-host-helper.exe` is 5,053,952 bytes. Debug build; GUI launch was not verified.
- Raw logs are preserved in the task directory: `check.log`, `check-no-default.log`, `midi-tests-local.log`, `regression-default.log`, `build-local.log`, `fmt-final.log`. The initial sandbox/fixture failures remain in `midi-tests.log` for traceability.

## Remaining acceptance blocker

The required Windows Computer Use entry point `node_repl` is absent from the current available tools. Its skill explicitly requires that entry point for Windows app automation. The alternative official CUA entry point was also checked: `cua.getState()` returned `apps: []` and browser surfaces only; `cua.listWindows()` returned `cua.listWindows is not a function`. Its tool declaration explicitly disables native computer APIs. This is a session capability blocker, not an activation failure or an approval denial. Therefore no real plugin GUI was opened or dragged, activation status is unknown, and whether Harmony Blueprint's native OS payload reaches egui remains **unverified**. No OS bridge is claimed or simulated as compatibility evidence. No hardware playback was observed or heard.

Next native acceptance: open the built DAW, load Harmony Blueprint, assign a WAV to a separate Sampler channel, record a melody, drag the recorded melody and chord-map guide to the three supported targets, verify audible playback and editable notes, then Undo/Redo, arrange, save/reopen and export. If its OS drop does not reach egui, inspect the actual payload before implementing a native bridge. Do not request license keys or passwords.

## Short manual acceptance path

1. Launch `C:\Users\Administrator\Documents\Codex\2026-10-23\task\DAW-harmony\target\debug\citrus-studio.exe`. The built `vst3-host-helper.exe` is alongside it. Use a new test project.
2. Load Harmony Blueprint from the installed VST3 bundle. Create a separate target instrument channel and use its Sampler **Load WAV** control to assign a sound (the delivery package includes `demo\key.wav`).
3. Use Harmony Blueprint **Record Melody** and drag the recorded MIDI onto the target channel row. Open that channel's Piano Roll and check pitch, start, length and velocity. Existing notes should remain, and project BPM should not change.
4. Repeat on the Piano Roll grid or the Sampler's **Drop MIDI notes here (append at beat 1)** region. WAV waveform drop is a separate audio-loading region. Dragging the chord map imports its reference notes as playable notes on the target instrument.
5. Undo once, then Redo; edit a note, arrange the Pattern, save a new `.citrus`, close/reopen it, and export WAV. Verify output on the user's actual audio device. Any activation prompt must be handled by the user without sharing credentials.
