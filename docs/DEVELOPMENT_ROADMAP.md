# Development roadmap

Updated: 2026-10-09 UTC. This roadmap tracks acceptance, not promised delivery dates. Current changes are published on an independent validation branch. Current handoff: source fixes and independent static review complete; 15 new test attributes added but none executed. Local tooling is unavailable; The first Windows GitHub Actions run found formatting differences, which are fixed pending rerun.

## P0 — Establish a trustworthy baseline

- Restore an approved Rust toolchain; record rustc/cargo host, versions, dependency availability and exact command output.
- Run locked baseline tests, formatting and Clippy; preserve failures rather than treating absent tooling as a code failure or a pass.
- Acceptance: exact tested revision/working tree, command, exit status and per-target test summary are in WORK_LOG; Windows-only gates remain separate from host checks.
- Current blocker: local Cargo/Rust are unavailable. Windows GitHub Actions is running on the independent branch; its first run failed at formatting before Rust tests. Exact formatting fixes are ready for rerun.

## P0 — Protect project saves

- Audit finite-number validation before serialization and atomic replacement; ensure malformed in-memory data cannot replace a recoverable project with unreadable JSON.
- Add regression coverage for NaN/infinity in representative nested fields, preservation of an existing target on error, and a successful valid round-trip.
- Acceptance: focused storage tests plus complete locked tests pass; failure preserves old bytes; normal v10 projects still load and save. Existing migration semantics remain intact.
- Status: finite-number preflight and cleanup implemented in `src/model.rs`; four regression tests added (including 60 corruption combinations). Tests blocked before execution because Cargo is absent.

## P1 — Make Playlist editing trustworthy

- Audit grouped Slip bounds, native-frame arithmetic, overlap geometry and crossfade transactionality.
- Keep the existing two-Clip equal-power crossfade; fix demonstrated gaps rather than rebuilding an already connected feature.
- Acceptance: regression tests for valid overlap, non-overlap, nested/cross-track/non-Audio rejection, invalid geometry and no partial mutation; UI undo/redo and mouse tests with real media at more than one tempo.
- Status: arithmetic/bounds hardening and atomic Playlist gesture snapshots (clips, automation lanes, audio routes) implemented with added regression tests; runtime validation pending.
- Known unresolved gap: splitting inside a fade duplicates normalized fades onto both halves; exact envelope preservation needs origin/extent metadata and coordinated model/migration/render/UI changes.

## P1 — Validate Realtime Master Capture end to end

- Verify callback-confirmed install/stop, PCM24 duration and channel layout, no-clobber publication, overflow/gap invalidation, file-size cap and shutdown finalization.
- Acceptance: focused tests plus hardware capture with a legally available instrument/effect; verify WAV and diagnostics against the audible Master and retain evidence.
- Status: stop-before-drain ordering fix and deterministic final-frame regression implemented; three additional offline-export validation tests added. None executed yet.
- Boundary: this captures live output. Deterministic offline plug-in bounce, automatic tail handling, stems and offline PDC equivalence are separate milestones.

## P2 — Release candidate gates

- Run fmt, all-feature/all-target Clippy, locked full tests and all-bin release build on the intended Windows toolchain.
- Perform helper protocol smoke, clean Windows launch, actual Audio Slip/fade/Crossfade mouse tests, group-resize and transport regressions, project save/reopen/recovery checks.
- Acceptance: exact candidate hash and logs, all required runtime files, documentation/package link checks and explicit unresolved issues; no commercial-ready claim based on unit tests alone.

## Later commercial programs

- Unified plugin-aware offline rendering: same graph semantics, automation timing, tails/PDC, stems and freeze/bounce.
- Recording workflow: track arming/monitoring, calibrated latency, punch/loop/takes/comping; robust device-loss recovery.
- Plugin safety/productization: VST2 isolation, vendor editors, presets, compatibility corpus and scanner recovery.
- Advanced editing: ripple/track playlists/consolidation, stretch/warp/pitch, richer Piano Roll/controller workflows.
- Large projects: bounded parsing/history, incremental dirty tracking, media relinking; stress/accessibility/localization/signing/update delivery.

## Documentation contract

Update DEV_STATE when the current state changes. Append dated WORK_LOG entries for implementation changes and every material validation outcome. Keep README and the parity matrix aligned with actual connected code. Record tests as **passed**, **failed**, **blocked before execution**, or **not run**; do not convert test attributes, historical claims or source inspection into executed-test evidence. A later entry supersedes an earlier status without rewriting the historical log.
