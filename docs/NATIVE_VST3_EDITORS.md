# Native VST3 editors

The later [ownership preparation](PLUGIN_PROCESSOR_DOMAINS.md) keeps execution
single-threaded. [Reused Surge restoration](PLUGIN_STATE_RESTORE_LIMITS.md) has
confirmed pre-existing first-note and controller-consistency failures; fresh-state
success does not establish unrestricted native save/restore acceptance.

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
control. Capability comes from the actual worker/helper snapshot. Linux uses standalone X11 containers
(including system XWayland) with `Open { owner: None }`; see [Linux lifecycle and acceptance](LINUX_VST3_EDITORS.md).
Display/open failure remains explicit and generic parameters remain available. Native VST2
editors are not implemented.

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

## First Windows runtime evidence (2026-10-09)

[Quality run 37888700443](https://github.com/wrench1997/DAW/actions/runs/37888700443) at
`b76ba3228053c5c5af63e5621df999553543a69d` passed 933 application + 13 helper + 5 protocol
Rust tests, 14 helper and 47 editor Python tests, strict source gates, fixture Clippy/build/receipts
and the ordinary helper smoke. Actual native execution passed the strict visible/input-desktop
prerequisites, Rust/Win32/CRT stdout isolation, helper-owned attach, 560×400 resize and content-scale
request. It then **failed** at the native button's `PrintWindow(..., PW_CLIENTONLY)` capture.
This is exit 1, not an unsupported-desktop exit 77. The later button event, native state round-trip
and remaining lifecycle cases were not reached; no full native GUI or state acceptance is claimed.
A diagnostic rerun retains the failing paint assertion rather than substituting synthetic pixels or
turning an API error into a pass. Real vendor, physical input/audio, and DPI-transition testing remain open.

## Diagnostic and aggregate acceptance follow-up

At `8961529`, [quality 37889957193](https://github.com/wrench1997/DAW/actions/runs/37889957193)
again passed source gates and partial attach/resize/stdout isolation. Diagnostic capture found
both the live verified helper button and a newly created same-process standard Button reject
PrintWindow flags 1 and 0, with advisory last-error 0. This does not establish a helper-only bug,
a rendering fix, or successful native paint. The [preview run](https://github.com/wrench1997/DAW/actions/runs/37889957208)
passed all package/source/ZIP/helper gates, including reviewed vendor provenance, with upload off.

The next harness records explicit PASS/FAIL/SKIP stages. Paint failure still fails overall
acceptance; an unavailable baseline/edited bitmap also makes repaint comparison SKIP. It can
continue independent native control/state/lifecycle checks only after freshly validating helper
PID, HWND classes/ancestry/control ID and exact dimensions immediately before native events.
Non-paint identity/protocol failures stop dependent session operations; separate fresh-helper
Shutdown/EOF/crash checks remain bounded. These new results require a Windows run and are not
inferred from Python orchestration tests. No desktop/driver/security settings are changed.

## Exact trusted-fixture feedback correction

At `2e49883`, [quality 37891374461](https://github.com/wrench1997/DAW/actions/runs/37891374461)
additionally passed native button interaction with ordered gesture/dirty feedback,
repeat focus/owner rejection and all three fresh-helper Shutdown/EOF/crash cleanup
cases. Paint remained failed. SaveState returned and detached, but the next assertion
expected one legacy feedback record and received two identical Cutoff=0.25 records;
restore and the remaining same-session lifecycle were therefore not run.

Exact reviewed source explains that pair: the zero-sample native flush stashes one
controller performEdit, the trusted fixture echoes the applied value through DSP
outputParameterChanges, and the host drains DSP output before the GUI stash. The
corrected harness requires exactly these two integer-typed records, with no sorting,
deduplication or arbitrary duplicate tolerance. It rejects wrong counts/IDs, divergent
values and malformed types. The fixture's one performEdit plus one setDirty(true)
must advance its native revision exactly twice; ordered begin/value/end, stable
capture revision and exact restored component/controller bytes remain mandatory.
This is the pinned fixture/host contract, not a universal VST3 notification rule.
Paint is still independently failed, and the corrected state check needs actual
Windows execution before state restore can be called verified.


## Verified exact state restore and lifecycle at c88c7fd

At `c88c7fd2fc368ab1e358f1726b64cb72a10fcfda`, [quality 37893177378](https://github.com/wrench1997/DAW/actions/runs/37893177378)
passed all source gates (951 application + 13 helper + 5 protocol Rust tests),
14 ordinary helper and 61 native harness Python tests, and fixture build/receipts.
Actual Windows execution now passes the corrected fixture-specific two-record
assertion, exactly two dirty/value revision increments, stopped SaveState with
detach/zero-sample flush, stable capture revision, and fresh-instance Project-context
restore with exact component/controller bytes. This supersedes the earlier pending
state-restore evidence; it does not broaden the fixture's feedback contract.

The same native run passed stdout isolation, exact attach/resize/content-scale
request, trusted-HWND-validated programmatic interaction and ordered gestures,
repeat focus/owner rejection, repeated close/open, WM_CLOSE, owner loss,
unload/reload/no-editor, normal session completion, and independent Shutdown/EOF/
forced-termination cleanup. Full native acceptance still **FAILS**: both before/after
PrintWindow captures fail, including same-process diagnostic Button probes;
repaint comparison is **SKIP**. No speculative driver/security change, synthetic
paint replacement or overall-success reinterpretation is used.

[Preview 37893177401](https://github.com/wrench1997/DAW/actions/runs/37893177401) separately passed the full 167-Python-test,
optimized-build, static-CRT/PE, provenance/license, ZIP/hash/extraction and real
extracted-helper smoke lane, with upload disabled and zero artifacts. These results
do not establish physical pointer/keyboard input, actual DPI transitions, real-vendor
compatibility, forced detach/destruction failure injection, whole-app native GUI,
Linux native editors or hardware audio/MIDI acceptance.


At later exact `42393c66567f5f362e19230eabbba71b8d41a3d8`, [quality 37897795277](https://github.com/wrench1997/DAW/actions/runs/37897795277)
repeats the successful source/interaction/state/lifecycle stages with 988 application,
13 helper and 5 protocol Rust tests. The two PrintWindow stages still FAIL and repaint
comparison remains SKIP. [Preview37897795252](https://github.com/wrench1997/DAW/actions/runs/37897795252) fully passes all 167 Python tests and
the optimized provenance/package/extracted-helper lane, upload disabled. These are
terminal results for that checkpoint, not inferred results for newer Piano edits.


At exact `e6f117e18e2764f0c8b0e8ded3bee57fae46a3eb`, [quality 37902412259](https://github.com/wrench1997/DAW/actions/runs/37902412259)
again passes all source gates (1,025 application +13 helper +5 protocol tests) and
trusted interaction/state/lifecycle checks. Before/after paint remains FAIL and
repaint comparison SKIP. [Preview 37902412320](https://github.com/wrench1997/DAW/actions/runs/37902412320) fully passes the 167-Python-test,
optimized package/provenance/extracted-helper lane with upload disabled. The newer
Piano expression source requires a fresh run; these counts are not inherited.


At exact `b57076ad990869f4a421cc816d67409b5c69b694`, [quality 37905510854](https://github.com/wrench1997/DAW/actions/runs/37905510854)
again passes source gates (1,043 application +13 helper +5 protocol tests) and
trusted native state/interaction/lifecycle checks. Both paint captures still FAIL;
repaint comparison is SKIP. [Preview 37905510775](https://github.com/wrench1997/DAW/actions/runs/37905510775) fully passes 167 Python tests,
optimized provenance/package checks and actual extracted-helper smoke; upload is
disabled. This terminal receipt does not replace a fresh run for later range edits.


At `d9016e5066a8a974fc4be71d6fbc5cf147529010`, [quality 37910289005](https://github.com/wrench1997/DAW/actions/runs/37910289005)
again passed all source and independent native interaction/state/lifecycle/cleanup
checks. Only before/after PrintWindow capture failed; repaint comparison was skipped.
The overall quality result remains failure. [Preview 37910288992](https://github.com/wrench1997/DAW/actions/runs/37910288992)
passed completely with artifact upload skipped. Neither the passing package nor
offline genuine-plugin DSP receipts replace this native paint acceptance gate.
