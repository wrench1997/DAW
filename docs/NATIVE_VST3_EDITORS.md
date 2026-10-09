# Windows VST3 native editors

## Implementation stages and current verification boundary

This work adds a real helper-owned native VST3 editor lifecycle in stages. It is not a claim of
FL Studio parity, VST2 editor support, arbitrary vendor compatibility, or realtime guarantees.

1. Add a reviewed, source-vendored extension of the pinned host library and the Windows helper
   lifecycle, with a source-built instrumented editor fixture.
2. Connect Citrus worker/UI commands, authoritative status, native edit/dirty notifications,
   and save/topology/session teardown protections.

The stage-2 application controls are implemented, but native Windows acceptance remains required
before calling the feature verified. The generic parameter catalog remains a separate interface.

Actual GUI/lifecycle/interaction results must be recorded for the exact build under test.
A Linux check, JSON protocol test, fake-helper harness, or unsupported Windows desktop is not
proof that an editor rendered or was usable. This document initially records implementation
contracts; it does not record a completed Windows GUI acceptance run.

## Pinned source and maintainable extension

The local `[patch.crates-io]` override is explicit in `Cargo.toml` and `Cargo.lock`.
[Vendor provenance and exact patch scope](../vendor/vst3-host-0.9.0/CITRUS_PATCHES.md) identify
its registry archive/checksum, upstream commit, original MIT license, and the bounded modified
upstream code files. Existing upstream API and legacy `CreateGui`/`CloseGui` values are kept.
The new isolated API does not manufacture a native handle to satisfy an ignored argument.

The production helper is the repository's `src/bin/vst3-host-helper.rs`. The vendored crate's
preserved sample helper does not implement the Citrus extension and must not replace it in a
package. App and production helper are built/shipped together.

## Native ownership, lifetime, and threads

The helper creates the real native container passed to `IPlugView::attached`. The app sends only
a logical owner HWND value and PID; the helper validates them before accepting an owner. The
container is standalone and helper-owned, not embedded into Citrus and not natively owned by
its cross-process HWND. This avoids Windows automatically destroying the container before the
plugin can detach when the DAW owner disappears. Logical owner loss causes orderly close.
Repeated open focuses the existing generation rather than attaching a duplicate view. A new
owner cannot silently take over an already open generation.

The Windows helper's main thread initializes STA/OLE before any plugin is loaded, and fails
explicitly if initialization fails. OLE is released only after plugin/view teardown. That thread
owns plugin loading/unloading, controller/control calls, processing requests, and native window
operations. This is a required lifecycle condition, not broad plugin-compatibility evidence. A
separate stdin reader only parses and queues requests. Bounded alternation of commands and native
messages prevents an input/resize flood from monopolizing the event pump. Native callbacks record intent; plugin calls happen
outside those callbacks to avoid reentrant host locking. Teardown detaches the view before
native destruction, including explicit close, titlebar close, unload, replacement, owner loss,
stdin EOF, and Shutdown.

Citrus's device callback continues to exchange fixed-capacity blocks with its existing insert
worker. It does not wait for native UI, IPC, or plugin code. The helper serializes plugin work;
a slow native/plugin callback can still delay plugin processing. Existing delayed-dry fallback,
IPC timeouts, and fault isolation remain necessary. This is not a guarantee of dropout-free
operation with arbitrary plugins. VST2 remains on its existing in-process path.

## Additive editor protocol

`HostCommand::Editor` wraps `IsolatedEditorCommand`:

- `Query`: helper capability and actual window state, including user-close.
- `Open { owner }`: attach once or focus the existing native editor.
- `Focus`: focus an existing editor; fail if closed.
- `Close`: detach and destroy; repeated close is harmless.

`HostResponse::EditorState` returns supported/has-editor/open flags, accepted client dimensions,
and an attachment generation. Closed dimensions are zero. Generation is scoped to the helper
process, so callers also need instance/endpoint/session identity. Unknown options and invalid
owner identity fail explicitly. New clients connected to an old helper receive an ordinary
error, not an invented success. Errors must not fault-isolate an otherwise healthy plugin solely
because it has no editor or an action is unsupported.

## Acceptance evidence

The source-only [fixture](https://github.com/wrench1997/DAW/blob/37ae191416ce64ee19dd3fba7ca84d97ca744da5/tests/fixtures/vst3-editor/README.md) has pinned MIT provenance.
It is built locally from Rust, never installed from an arbitrary third-party binary. It stays
outside the application dependency graph and release/package whitelist.

Keep these evidence layers separate:

1. Protocol and pure lifecycle unit tests: serialization, repeated commands, invalid states,
   bounded queues and wire-state validation; no visual claim.
2. Real source-built `IPlugView` fixture through the production isolated helper: attachment,
   content scale, requested/accepted resize, repeated focus/open/close, titlebar close,
   unload/reload, helper/owner loss, and cleanup.
3. Repository-authored native test control: visible paint and actual button interaction which
   emits parameter begin/change/end plus dirty notification. This is interactive evidence for
   the fixture only.
4. Commercial acceptance still needs authorized real-plugin/device tests, per-monitor DPI,
   long sessions, audio load, and clean-machine packaging.

Native detach/destruction failure injection is still required coverage and has not been run.
The current fixture and pure tests do not inject `removed()` or `DestroyWindow` failures.
A failed native container destruction terminates the isolated helper with a diagnostic instead
of continuing with a false closed state; verifying that terminal path requires an injectable
Windows test seam.

A runner without a usable interactive desktop must report **NOT VERIFIED / UNSUPPORTED**, with
its exact reason. It must not convert absent GUI coverage into a pass. No global hooks, arbitrary
plugin downloads, commercial SDK agreements, or user computer access are required by this test.

## Stage 2: application controls and state safety

The channel device inspector and mixer insert rows expose **EDITOR** (open/focus) and a close
control. Capability comes from the actual worker/helper snapshot. On Linux the button is disabled,
`open` and `supported` remain false, and `has_editor` still reflects plugin metadata. Generic
parameters remain available. Native VST2 editors are not implemented.

Editor commands carry exact running endpoint/instance/slot identity and use the existing bounded
worker admin queue. Per-slot control-only snapshots retain pending/completed commands, actual
window state, errors, a monotonic native dirty revision, and the latest captured opaque state.
They do not depend on the lossy diagnostic/event ring, and the audio callback never locks or
reads this publication. Each worker polls native feedback at most once per 100 ms, draining
`TakeParameterEdits`, host notifications and accumulated restart flags. The helper's separate
atomic dirty revision survives bounded feedback queue overflow. Transport failure is an error;
editor queries, feedback, restart servicing and state capture must not silently respawn a fresh
plugin and substitute its clean/default state.

Opening an editor conservatively marks the project unsaved, covering an edit followed immediately
by closing the DAW before its next feedback poll. Explicit native close, detected titlebar close,
and project state requests detach before capturing. The final native parameter queue is flushed
with a zero-sample process call, so a stopped transport cannot leave a just-edited controller value
absent from component state. The snapshot is retained even if a plugin omits dirty callbacks.
Feedback loss, capture failure or required component reload remains a visible unsaved/faulted state;
there is no automatic default-state recovery.

A native preset can change unreported parameters. Its captured opaque state therefore supersedes
stale generic base overrides. Existing automation/base keys are retained while capture is pending,
then refreshed atomically from actual plugin values for the same generation and native revision
as the opaque snapshot. Saved base IDs are first checked against one current metadata snapshot
(with the generic catalog's 4,096-item validation limit), so an unknown VST3 ID returning zero
cannot masquerade as a valid base. Empty base sets need no generic metadata. Failed/removed
parameter reads or unsupported metadata fail capture visibly; missing values never become zero
bases. A later generic edit survives subsequent saves until another
native revision or editor generation requires refreshed bases. An open generic catalog is refreshed
after capture. Native gestures are not currently written into Citrus automation or
individual undo steps. This preview refuses native Open for any instance referenced by a plugin-
parameter automation lane, even if stopped or the lane is unplaced. Open also waits for the exact
callback timeline/fingerprint to be synchronized and for any previous resident/legacy automation
ownership to be released after a lane is removed or retargeted. Existing generic editing and
automation remain available. Assigning/retargeting automation to an instance with a native editor
pending/open or an unconsumed capture is blocked. This prevents transient automated controller
values from silently replacing release bases until controller/base ownership is separated.
Generic live edits are disabled while native editing or capture is pending.
Topology changes, Undo/Redo and whole-project transform previews are blocked until the editor is
closed and the app has consumed its exact retained capture. Native Open is refused while transform
previews, editing gestures, deferred MIDI/project candidates or project lifecycle transitions hold
an older project snapshot; native Close remains available outside state-save transactions.
Save/restore and unload all detach first; normal project save barriers still require their exact
tagged state receipts. Unsupported editor actions do not fault an
otherwise healthy plugin. Best-effort close remains available for a faulted instance.

## Feature-branch verification record (2026-10-09, before integration)

- Linux `cargo test --offline --locked --all-features --all-targets`: 883 tests passed
  (864 application, 14 helper, 5 editor protocol), using the default thread stack.
- Linux `cargo test --offline --locked --no-default-features --all-targets`: 862 tests
  passed on the default thread stack; strict no-default Clippy also passed.
- Linux strict all-feature/all-target Clippy and Windows MSVC all-feature/all-target typecheck
  and strict Clippy: passed. One unchanged upstream dependency deprecation warning remains
  in `internal/data_exchange.rs`.
- Windows source fixture all-feature/all-target strict Clippy: passed, including all three
  deliberate stdout-pollution paths. This is cross-target compilation, not Windows execution.
- Python protocol/build-receipt/package regressions: 89 tests passed on Linux.
- Real Linux helper no-plugin smoke: three valid replies, invalid-command recovery, explicit
  shutdown/exit zero and child reaping passed.
- Focused exact-source vendored-library audit: 13 targeted tests passed (native/revision,
  closed-state validation and failed-stop transitions) using a temporary production-dependency
  manifest. Upstream's unused example dev dependencies were not fetched; this is not a claim
  of passing the entire upstream suite or its absent external plugin fixtures.
- The original registry archive checksum and exact patch reconstruction passed. Applying
  `CITRUS.patch` reproduces every upstream file in the vendored directory, with only the seven
  documented code files modified and the upstream license/manifests unchanged.
- Linux native GUI gate: exit 77, **UNSUPPORTED / NOT VERIFIED**.
- Windows GUI rendering, keyboard/mouse interaction, DPI, audio load and real-plugin compatibility:
  **NOT VERIFIED** in this Linux environment. Windows MSVC cross-target checks verify types only.
- Independent source review covered parameter-base preservation, missing IDs, native/automation
  exclusion, callback timeline handoff and stale project-snapshot barriers. It did not execute GUI.

The production helper privately owns a duplicate protocol output handle before loading any plugin.
Windows public Win32 stdout and CRT descriptor 1 are redirected to stderr (or NUL if no diagnostic
handle exists), with initialization failure terminating the helper. The trusted fixture deliberately
writes valid fake protocol responses through Rust stdout, direct Win32 output and CRT stdio. Its
real Windows smoke requires all three to appear on stderr while protocol replies remain correct.
Pure Python routing tests do not establish that the Windows handle routing executed.

## Integrated source checkpoint (2026-10-09 05:25 UTC)

At `8c09f457bc994271d9d8bff824b38960bb240131`, the complete combined Linux tree passed
936 application + 14 helper + 5 protocol tests with all features, and 934 application tests
without default features, all on the default test stack. Formatting, both strict Clippy
configurations, app/helper builds, real no-plugin helper smoke and 153 Python regressions passed.
The numbers above in the feature-branch record describe an earlier isolated tree, not this suite.

The packaged preview now admits only the exact reviewed `vst3-host` 0.9.0 path override,
with its original MIT license and pinned four-file provenance bundle. The original crate archive
checksum is explicitly distinguished from modified source identity. Vendor source has explicit
LF checkout attributes so Windows checkout cannot silently change the reviewed provenance bytes.
The published fixture README link is pinned to the integrated source commit rather than an
unpublished feature-branch hash.

The trusted Windows fixture harness additionally verifies stopped native edits captured through
SaveState, native revision, restored project context and exact component/controller bytes in a
fresh instance. Linux returns 77, UNSUPPORTED / NOT VERIFIED. Windows execution remains pending;
neither the proposed CI step nor its passing Python orchestration tests establishes a GUI pass.

An independent integration review found a separate pre-existing Browser race: queued generator
replacement could overwrite completed WAV import. The shared project snapshot-transition predicate
now protects both import completion and native Open. The actual-app regression covers successful
and failed replacement plus independent import Undo; MIDI teardown is injected at its completion
seam. This finding was not covered by the earlier feature-branch review.
