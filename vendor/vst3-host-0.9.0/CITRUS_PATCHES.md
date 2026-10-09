# Citrus isolated native-editor extension

## Exact upstream source and license

- Package: `vst3-host 0.9.0`, MIT; original `LICENSE` retained unchanged.
- Official registry archive: <https://static.crates.io/crates/vst3-host/vst3-host-0.9.0.crate>
- SHA-256: `6ec579d54bd13b83c60c1fd8bb756cf234e36ccbfb4833ff756b417e64db7fea`.
  This was checked against the original Citrus `Cargo.lock` registry checksum before extraction.
- Upstream repository: <https://github.com/HelgeSverre/rust-vst3-host>.
- Package `.cargo_vcs_info.json` identifies commit
  `ed054908cfe057694d8cf037d0c39dfb5eb4c2ca`, package directory `vst3-host`.
- The original package sources and manifests are retained. No binary plugin or vendor SDK
  is included. The application manifest explicitly selects this directory with `[patch.crates-io]`.
  The path override is recorded in the root lockfile; upstream version stays 0.9.0.

## Bounded upstream code changes

1. `src/plugin.rs`: additive `IsolatedEditorOwner`, `IsolatedEditorCommand`, and
   `IsolatedEditorState` types; `Plugin::isolated_editor` and an unsupported default internal
   implementation; checked `try_take_parameter_edits`/`try_take_host_notifications` drains so
   transport failure cannot masquerade as no native changes; checked, non-draining
   `Plugin::native_dirty_revision`; a failed in-process stop retains the confirmed processing
   flag. No forged `WindowHandle` is needed for a helper-owned editor.
2. `src/process_isolation.rs`: additive `HostCommand::Editor` and
   `HostResponse::EditorState` wire variants, plus `NativeDirtyRevision` command/response.
   Existing variants are unchanged. Windows protocol output is retained through a private,
   non-inheritable handle; Win32 stdout and CRT descriptor 1 are redirected before plugin
   load, failing closed if isolation fails. Registry dependency manifests remain unchanged.
3. `src/internal/isolated_plugin_impl.rs`: typed forwarding and snapshot validation,
   updating legacy cached open/size state only after a valid reply; fallible edit/notification
   drains preserve transport and protocol errors. Checked drains, revision, editor lifecycle,
   restart servicing and state capture never transparently recover a dead helper: a newly
   loaded instance cannot stand in for lost native edits. Existing infallible APIs remain.
4. `src/lib.rs`: re-export the three public types.
5. `src/bin/vst3-host-helper.rs`: the preserved upstream sample helper explicitly rejects the
   new command. The production Citrus helper lives at `src/bin/vst3-host-helper.rs` in the
   repository root and implements the Windows lifecycle; building the dependency's sample
   helper is not a substitute for building or shipping the production helper. Both forward
   the checked native revision query.
6. `src/internal/com_implementations.rs`: per-instance saturating `AtomicU64` native revision
   increments independently of bounded queues for `performEdit`, `setDirty(true)`, and
   parameter-value/reload restart requests. Draining feedback never clears it. Exhaustion is
   an error rather than wraparound. Native values lost before DSP delivery latch a separate
   capture failure, including queue overflow/poison, and cannot become a clean snapshot.
7. `src/internal/plugin_impl.rs`: in-process revision forwarding; state capture rejects
   undelivered/lost native values and native revision changes during capture. Failed DSP
   delivery and a stopped legacy display drain cannot hide lost processor updates. Successful
   state restore discards stale native values, while an unreconciled loss latch stays set until
   plugin reload. Stop/deactivate return codes are checked before changing lifecycle flags,
   so a temporary zero-sample flush cannot silently succeed after failed lifecycle restoration.

Existing APIs and `CreateGui`/`CloseGui` wire values remain compatible. A new client talking
with an old helper gets a normal protocol error rather than silently assuming GUI support.
The new types reject unknown fields. Native owner identity is data only, validated in the
helper against a live HWND and PID before use. It is logical association, not unsafe pointer
transport or cross-process embedding.

The production Windows helper initializes a balanced main-thread OLE apartment before plugin
loading, failing explicitly if initialization fails. It closes its editor before SaveState/LoadState as well as
replacement/unload. SaveState flushes queued values with a genuine zero-sample process call
(one explicitly empty channel avoids upstream's channel-less full-block fallback), then
captures the component/controller state. Failed flush/capture stays an error; no phantom audio
samples are rendered. A revision is scoped to one loaded plugin instance, and is dirty-state
evidence rather than a replayable gesture log. These changes do not implement automation
recording, touch/latch modes, or reliable gesture grouping.

## Maintenance and test boundary

Do not replace this directory with a newer upstream package without porting/reviewing these
changes and rerunning Citrus helper/editor tests. Compare the directory to the exact registry
archive above; only the listed code files, this manifest, and the reviewable `CITRUS.patch` should differ.

Focused regressions cover revision persistence after drains/overflow, notification backpressure,
exhaustion/poison, unloaded-plugin checks, lossless JSON revisions above JavaScript's exact-integer
range, checked IPC failure without recovery, and helper state-detachment/zero-frame policy.

The standalone source-only fixture under `tests/fixtures/vst3-editor` has separate provenance.
It is a test input, not a runtime dependency or release payload. Protocol tests do not prove a
real editor rendered. Windows lifecycle and interactive smoke must report their own results;
Linux typechecks and unsupported-desktop outcomes do not establish Windows GUI acceptance.
