# Citrus native-editor and transport extensions

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
   Existing variant names remain compatible; additive process fields are detailed below.
   Windows protocol output is retained through a private,
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

## Authoritative MIDI-routing transport extension

The same seven reviewed source files also carry the additive per-block transport and output
loss contract used by Citrus MIDI routing:

- `ProcessTransport` exposes independent project sample/quarter-note positions, current tempo,
  playing state, and time signature. `Plugin::set_process_transport` validates it and the direct
  implementation applies it to the next process context. Sample/PPQ positions freeze while
  stopped; playing PPQ advances by each block's duration at its tempo, never by multiplying an
  absolute sample position by the latest tempo. Continuous processing time remains independent.
- The isolated implementation caches context without sending an additional IPC command.
  `Process` and `ProcessBuses` carry optional, serde-defaulted `transport` in the same audio
  request; both helpers apply it immediately before processing. Each audio response explicitly
  acknowledges applied transport with a serde-defaulted `transport_applied` flag; an explicit
  context request fails closed if a mismatched/older helper omits or rejects that acknowledgement.
  Legacy tempo/time-signature/
  playing setters remain supported, and requests without context retain their legacy setup.
  Existing callers should set context for every quantum, especially after seeks or loop wraps.
- `Plugin::take_output_events_with_loss` returns events plus a loss latch. SDK event capture
  rejection, bounded event-list/output-queue overflow, and isolated backlog loss are observable.
  Legacy event drains remain available but do not clear the latch. Each audio response includes
  a serde-defaulted `output_events_lost` boolean so a routing host can panic the destination
  rather than silently lose note-offs. There should be one draining consumer per plugin.
- The event wire codec rejects more than 4096 events or more than 8 MiB of aggregate event
  payload, and isolated accumulated output uses the same limits. Helpers reject oversized
  flat process frame counts before allocating output buffers. Unsupported MIDI event/bus
  policy remains a responsibility of the application, which must treat dropped note data as
  a destination-safety concern.

Headless regressions exercise transport validation, independent PPQ/seek origins, tempo
changes, stopped and zero-sample contexts, single-request context forwarding, output-loss
propagation/clearing, bounded overflow, and backward-compatible JSON defaults. These tests
make no native GUI or real-plugin interoperability claim.

### Regenerating the reviewable patch

Verify the original registry archive against the SHA-256 above and extract a fresh copy.
For each changed upstream source file, produce a unified diff with paths `a/<relative path>`
and `b/<relative path>` against that pristine copy. Concatenate those diffs in sorted path
order into `CITRUS.patch`; exclude `CITRUS.patch` and this manifest themselves. Apply it with
`patch -p1` to another pristine extraction, then byte-compare every upstream file with this
vendor directory. Only this manifest and the patch are additional provenance files. Keep the
original LICENSE, `.cargo_vcs_info.json`, package version, and registry checksum unchanged.
Finally review the changes and update the explicitly pinned manifest/patch SHA-256 values in
`scripts/package_windows_preview.py` and its packaging regression test. Those pins deliberately
require a fresh source review when this extension changes.

## Linux standalone native-editor extension

The production helper now also implements standalone X11 plugin windows usable through the
session's system XWayland server. Linux uses `Open { owner: None }`; Windows HWND/PID validation
is unchanged. No compositor is spawned and no Wayland parent handle is reinterpreted as X11.
See `docs/LINUX_VST3_EDITORS.md` for the window, focus, DPI and validation boundaries.

- `com_implementations.rs` exposes Linux `IRunLoop` from both the factory host context and
  each attachment's plug frame. Bounded registries use unique registration tokens to reject
  stale readiness/timer snapshots after callback reentry. COM references are released outside
  registry locks. Closing a registry permanently rejects reentrant registration; reopening an
  editor creates a fresh frame registry. Factory callbacks continue while its editor is closed.
- `plugin_impl.rs` retains the factory through plugin teardown, while keeping the host context
  alive through module unload. Failure guards close registrations before unloading a partially
  loaded module. Normal teardown closes factory registrations before module unload; retired frame
  callbacks cannot run after detach. Factory and frame callbacks are serviced on the helper main
  thread, including idle periods with no stdin traffic.
- `process_isolation.rs` now fails closed on Unix protocol descriptor isolation errors. A private
  close-on-exec descriptor is required, stdout redirection is checked, and absent or protocol-
  aliasing stderr is replaced with a valid `/dev/null` diagnostic sink before plugin loading.

Focused Linux regressions exercise callback reentry, registration identity, closed-registry
resurrection, descriptor/timer bounds, factory/frame separation and failed protocol claims.
They do not establish mixed Wayland/XWayland runtime acceptance or sanitizer coverage.
