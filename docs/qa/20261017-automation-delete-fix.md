# Playlist deletion: preserve unrelated global automation

Date: 2026-10-17
Base: `aff57b2b84e3e5b7179d39269518b76397fc6c6e`

## Defect and contract

The existing independent real-input acceptance case,
`strict_creation_delete_pattern_must_preserve_unrelated_global_automation`,
reported that deleting an ordinary Pattern placement erased both valid,
unplaced Tempo and Master Volume lanes. Undo restored them, and Redo repeated
the data loss.

Both production deletion paths pruned **all** lanes without a remaining
placement. However, unplaced lanes are valid project-global automation in
`automation_runtime_lanes_at_beat`, the Timeline, and `Project::song_end_beat`.
Deleting an unrelated clip must preserve those lanes, including their full
points, curve, name, and target.

Merely removing lane cleanup would also be wrong: deleting the final placement
of a previously placed automation source would make that source unexpectedly
global.

## Change

`Project::delete_playlist_clips` owns deletion of an already-expanded clip-ID
selection and its dependent data:

1. Identify clips that actually exist and are removed in this operation.
2. Record automation source IDs only from those deleted **Automation** clips.
3. Remove the selected clips and their audio mixer-destination entries.
4. Remove a recorded source only when no surviving Automation placement uses
   it. A muted surviving placement still owns its source.
5. Normalize remaining clip groups.

A source that was already global is outside the deletion's ownership scope.
Unrelated placed sources and shared sources with surviving placements retain
their complete payload. Empty/stale selections are exact model no-ops. No
project-format or playback-policy change is needed.

Both production entry points now use this operation:

- Keyboard Delete after group-selection expansion, with the existing explicit
  project-history transaction.
- Playlist secondary-click and Delete/Eraser-tool requests, including mixed
  groups, with the existing discrete gesture transaction.

Secondary-click on an Automation clip continues to serve its point-editing
menu. The applicable deletion inputs for that clip are keyboard Delete,
Eraser, or deleting a mixed group through a non-Automation member.

## Regression coverage added

`src/model/playlist_deletion_tests.rs` adds 8 tests:

- Pattern deletion preserves every global/placed source and source Pattern.
- Shared automation survives removal of one placement, including a muted
  survivor, then disappears with the final placement.
- Batch deletion removes only newly unreferenced sources.
- Empty and stale IDs do not mutate the project.
- Stale `automation_id` fields on non-Automation clips do not become ownership.
- Audio deletion removes only its route, retaining media and other routes.
- Group normalization does not override the caller's selection policy.
- Real save/reopen preserves global payload and final-source removal.

`src/app/playlist_deletion_ui_tests.rs` adds 7 tests with 20 real-input scenarios.
They run the actual App UI and obtain production clip-response bounds. Key
and pointer events perform deletion; no deletion command is invoked directly.
The scenarios cover keyboard Delete, secondary-click, and D-selected Eraser;
Pattern and Audio clips; shared/final automation placements; mixed groups with
and without surviving shared placements; disabled grouping; and no selection.

Assertions compare the complete serialized Project to an independently
constructed expected result. They also check dirty/history state, exact Undo
and Redo, two consecutive deletion transactions, valid audio routes, cleared
gesture state, and runtime automation outside placements. The latter must
contain only the original global lane IDs, preventing accidental promotion of
a deleted placement's source.

## Verification state at handoff

- `rustfmt --edition 2024 --config skip_children=true` on the three new Rust
  files: passed.
- `git diff --check`: passed.
- Production entry-point/source-ownership review: completed.
- Rust compilation and execution: **not run at this handoff**, deliberately
  deferred to the shared, serialized validation run to avoid concurrent Cargo
  jobs. The added tests are not being claimed as passed.
- The original independent strict acceptance case remains unchanged and must
  be rerun against the integrated source.
- Native-device and manual desktop acceptance are not claimed.

Suggested focused gates after integration:

```sh
cargo test --all-features --bin citrus-studio playlist_delete -- --nocapture
cargo test --no-default-features --bin citrus-studio playlist_delete -- --nocapture
```

Also run the original independent strict acceptance case, the complete App
suite, required format/Clippy gates, and any integration-owned release checks.
