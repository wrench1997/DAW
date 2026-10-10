# Selected-instrument bounce: dormant preparation checkpoint

Status: **DORMANT CAPTURE CANCELLATION, BOUNDED PLUGIN-FREE GATES PASSED**.
The cancellation/termination source passed its narrow gates at `90795d5`, as
detailed below. The earlier preparation slice was accepted at `0728a8c`, with
aggregate Rust receipts at `d91cb74` and provenance receipts at `5a2d0b3`. Those
older aggregate counts are not attributed to the new source. Native bounce,
App integration and rendering remain unqualified.
This branch starts from published
`dcdb2c14809b5e6f34af7db071db36f763d757f5`. No new App action is enabled. There is
no WAV renderer, project replacement, freeze, or complete-song plug-in export.
Existing whole-song WAV export refuses active VSTs exactly as before.

## First checkpoint

- `instrument_bounce.rs` prepares an immutable selection and owns one bounded,
  private capture receipt. Admission and completion require the same project
  fingerprint/session, selected channel/instance, endpoint, Timeline revision and
  stopped epoch, with no editor, edits, recording, MIDI or session transition.
- The current conservative eligibility rejects enabled nonempty automation,
  including Tempo; configured MIDI routing; missing/ambiguous placements;
  bypass/partial wet/muted source; Timeline diagnostics; unsupported rate/length.
  It compiles the original Project, retaining channel ordering. It does not
  strip unsupported content and present the result as a full mix.
- `PluginChainControl::request_instrument_bounce_state` uses one dedicated
  bounded request/reply. It does not use the regular SaveState publication,
  `RuntimeEvent::State`, or `NativeEditorSnapshot` capture serials.
- The worker requires the existing epoch to be quiescent. It does not perform
  the normal tagged-save catch-up reset. Open/pending editor, pending parameter
  receipt, existing slot fault, queue-full and stale epoch reject explicitly.
- VST3 capture queries the closed editor and durable native generation/revision,
  uses the production zero-sample SaveState flush, checks a freshly read catalog
  and reads current values for exactly the stored generic base IDs. Generation,
  revision, metadata and epoch are checked again. Nonempty state is mandatory;
  there is no default-preset fallback. Snapshot bytes/bases are never copied into
  the source Project by this path.
- Capture rejection is separate from a newly observed backend/IPC failure. The
  latter remains a normal visible source-worker fault. This is not a guarantee
  that arbitrary vendor `getState` implementations are side-effect free.
- `offline_instrument.rs` constructs a thread-affine fresh isolated VST3 owner
  through the existing backend/helper load route. It pins the actually captured
  factory class, verifies class/version, selects `kOffline` before restore/start,
  replays captured parameter bases, and validates current metadata/buses again.
  No editor is opened, no source LoadState occurs, and no positive warmup/reset
  or fresh re-save is used as a state-settlement claim.
- Actual instrument/MIDI capability metadata is required. No audio input and
  exactly one active mono/stereo output bus are accepted; even inactive extra
  buses are rejected in this first slice. Missing metadata never becomes zero.
  Prepared maximum is 2048. Future musical rendering is Q128, preserving the
  existing exact Surge 1.3.4 reset256 rule for clone cleanup when it is added.

## Deliberate pending work

App capture-fence integration, production note-ID/MIDI mapping, synchronous Q128
rendering, post-Process drift rejection, latency trimming, explicit fixed-tail
review, cancellation during helper setup/render/teardown, existing atomic WAV
writer reuse, and the user-facing review/action remain separate reviewed stages.
The existing source remains installed; the eventual operation is a pre-Channel/
Mixer instrument WAV, not a full mix or automatic project edit.

The preparation bound is 8000–192000 Hz, a requested tail of 0–30 seconds, no more
than ten minutes of song and 24 million total reserved frames including a maximum
one-second plug-in-latency allowance. Tail truncation consent and output-format
review are not implemented yet. Metadata is not binary authentication.

## Verification at checkpoint

Only source inspection/formatting and Git diff hygiene may have been performed
before this checkpoint archive. New tests have **not been executed**. The new
plugin-free test source covers immutable capture, exact current bases, source
identity drift, pending work/editor, queue-full and cancellation admission,
receipt timeout/disconnect, error/fault distinction and unsupported metadata.

No genuine plug-in, helper/native editor, OS device or WAV output has been run.
No previous source's receipts are attributed to this branch. Source plus Git
bundle must be durably saved and independently reviewed before long tests; native
plug-in qualification requires its own explicit gate.

## Pre-execution review corrections

The first frozen preparation checkpoint `d8c7bc0` was saved before execution.
Review found that a capture-time backend fault needed immediate reliable metadata
publication and that cached `PluginInfo` MIDI bits could not prove post-state bus
eligibility. Both are corrected in the next still-unvalidated source checkpoint:

- Capture failure republishes active-mask/latency/MIDI capability before its
  private reply, even if the best-effort SlotFault event ring is full.
- An additive, fallible `EventBusLayout` command queries actual component bus
  count/info on the owner thread. Unknown/invalid/unsupported/dead/wrong replies
  reject; there is no cached capability fallback or automatic recovery. Existing
  commands are unchanged. Activation is attested only by a matching successful
  host activation record, never by `kDefaultActive`; otherwise it is unknown.
- Bounce requires exactly one currently attested active main event-input bus and
  zero event-output buses, including inactive outputs. Enumeration is repeated
  after mode/state/base application and at both source capture fences.
- Added plugin-free component and line-protocol stand-ins exercise the actual
  new capture/factory sequence, post-state topology changes, unknown activation,
  missing parameter keys, empty state, and generation/revision changes. Those
  tests load no vendor plug-in or production helper. They are unexecuted here.

The dormant caller-supplied source stamp still requires an App adapter that binds
the control to the exact project-session/channel/endpoint probe and revalidates
that probe on completion; same-instance replacement must not pass by ID alone.
App integration is intentionally not enabled by this checkpoint.

## Strict audio-bus review correction (still unvalidated)

Review of `e0ea8aa` found that ordinary `AudioBusLayout` describes prepared buffers,
not a fresh component enumeration. The additive `CheckedAudioBusLayout` query now
checks actual audio count, index, channel count and main/aux descriptor against
those prepared buffers. The prepared main/aux descriptor is recorded from the
same SDK read that established each buffer's channel count; unknown fallback
metadata cannot attest consistency. The ordinary prepared-layout API is unchanged.

A non-draining restart flag observation rejects pending I/O/reload both before and
after enumeration. Neither source capture nor candidate admission services that
work, reconfigures buses, stops/resets the source, or infers active status from
bus default flags. The synchronous factory uses this strict query after state,
Start and parameter-base application. Later per-block rendering guards must use
the same query; no rendering method exists in this checkpoint.

A bounded `MetadataQueryRejection` enum distinguishes explicit Unsupported,
PendingTopology and InconsistentTopology refusal from faults. Only the new typed
response may produce harmless rejection. Generic remote Error (including an old
helper), wrong/malformed reply, SDK error, crash, timeout and poisoned state still
fail and fault the capture source. No failure can produce a valid layout, clear a
previous fault, or trigger automatic recovery. Fresh candidate failure disposes
only the candidate. Plugin-free tests cover Start/SetParameter/type changes,
unknown activation, pending/reentrant restart without flag drain, SDK errors and
protocol classification. All new tests remain unexecuted at source freeze.

The follow-on source correction enforces the offline owner's documented !Send/!Sync
boundary with a private Rc marker and compile-time negative-trait test. Failed fresh
candidate cleanup may issue the existing backend Drop's Editor Close; no Open/Focus
or source editor mutation is permitted. The protocol tests distinguish that teardown
from setup. Malformed audio layout/unknown rejection enums remain protocol faults.

## First execution findings and remaining capture gate

The first 71d0323 vendor compilation and five focused strict-audio tests passed.
Application compilation found a boxed panic-payload borrowing error, corrected in
this follow-on source checkpoint. Malformed-response fixtures now perform the
established readiness handshake before a one-second test-only timeout, rather than
waiting 120 seconds per deliberate malformed response. No-feature production builds
omit the unused VST3 render spec while keeping plugin-free capture tests available.
These corrections are not yet execution-validated at source freeze.

This first slice was **not App-cancellation-qualified**. At that checkpoint, receiver-drop/
35-second coordinator timeout did not stop a capture already reading current bases
and released admission before worker termination. The separate reviewed checkpoint
must carry cancellation/deadline checks before and after every state/metadata/base
IPC operation, preserve genuine in-flight failure as a backend fault, and retain a
worker-owned admission lease until terminal acknowledgment. Queued cancellation,
slow getter, elapsed-deadline success, concurrent IPC failure, dropped receiver,
worker teardown and immediate retry must be tested before user-visible integration.
No source/UI ownership may be released merely because the reply was abandoned.

The d192277 application/all-target build then passed, as did ten coordinator tests
and the existing helper/protocol targets. Its new backend/factory suite had three
passes and four failures: handwritten parameter replies omitted the required
`Parameter.flags`, causing unrelated protocol timeouts. Broad `is_err()` assertions
could also falsely pass negative scenarios that never reached the intended fault.
Those failed receipts remain preserved; they are not acceptance evidence.

The next test-only correction serializes complete `HostResponse` templates using
production Rust types and round-trips them through the real decoder before launch.
Each negative group first runs a successful control fixture; every source rejection
then asserts the intended fault/rejection category, reason and exact request prefix.
Fresh-candidate failures assert the intended error and terminal setup stage, ordered
Offline/LoadState/Start/base application, and cleanup-only Close. Open-editor injection
now supplies valid open geometry so it tests editor admission rather than malformed
wire rejection. No production behavior or error policy is weakened. These corrected
fixture tests remain unexecuted at their source checkpoint.

## Plugin-free results at d91cb74

The unchanged source passed 1209 App tests, 1186 no-default core tests, 409 available
vendor tests, 26 vendor doctests, App/core/vendor Clippy, formatting, helper build,
and the focused coordinator/capture/factory tests. One vendor test requiring an
external SDK-generated fixture was explicitly filtered, not counted as passing.
The independent review matched all 140 Rust/manifest hashes and eight saved binary
hashes to their receipts. These are plugin-free/headless tests, not native bounce
or user-facing rendering qualification.

Python had 238/239 passes: its separate provenance-constant test still expected the
old vendor patch digest. Before changing that assertion, the official crate digest
was checked and the reviewed patch was applied to pristine upstream with fuzz zero;
all 38 resulting source files matched the vendored source byte-for-byte. The next
correction updates the hardcoded test pin and adds the reviewed patch-document pin;
no Rust or production packaging code changes. Its bounded Python/package rerun is
still pending at source freeze. All previous failed receipts remain preserved.

## Final preparation checkpoint (2026-10-10)

The bounded provenance rerun at `5a2d0b3` passed all 239 Python tests and 552 package
input/provenance/document-policy checks. Rust source is unchanged from `d91cb74`:
1209 App tests, 1186 no-default core tests, 409 available vendor tests, 26 vendor
doctests, 25 helper tests and 13 transport/editor/reset protocol tests passed.
Focused coordinator10 and capture/factory/affinity7 are included in the App total.
App/core/vendor Clippy, formatting and helper build passed. Direct vendor Clippy
retains the established deprecated/drain_collect baseline allowances. Exactly one
external SDK-generated metadata fixture remains explicitly excluded.

Source-linked compiler commands, copied executable hashes, 140 source/manifest
hashes, patch reconstruction and the earlier failures are retained together in the
checkpoint archive. This final documentation-only freeze does not change tested
Rust, the helper wire protocol, or packaging behavior. It does not qualify genuine
VST processing, audible/WAV output, native UI, Windows, cancellation or App actions.
The next bounded checkpoint must close worker-owned cancellation/deadline admission
before any App integration or renderer acceptance. Full-song VST export still refuses.

## Cancellation/termination checkpoint (source freeze)

This follow-on changes only the dormant capture coordinator, runtime capture path,
and plugin-free fixtures. It does not add an App adapter or renderer. No tests
had run at its initial freeze; the later source-bound results are recorded below.
Earlier accepted receipts remain bound to their original source hashes.

- One non-cloneable queued/worker lease owns per-source admission. Receiver drop
  requests cancellation but cannot release that lease. Queue rejection, worker
  teardown and panic use the same RAII release path; an old receiver cannot clear
  a later job's busy state. Release reopens admission before publishing termination
  acknowledgment, so an acknowledged caller can retry immediately. Replies remain
  unreadable until that acknowledgment.
- The immutable capture deadline starts at request admission. The shared fence is
  checked before and after editor/native-revision, strict audio/event/latency/tail,
  zero-sample SaveState, catalog and every current-base IPC. An already-dispatched
  call keeps its existing helper timeout. It is never retried or interrupted via
  source reset, reload, editor close, or positive-sample processing.
- A successful in-flight call followed by cancel/deadline becomes harmless refusal,
  and no later capture request is dispatched. The actual result of a failed IPC
  or backend panic takes precedence over simultaneous cancellation. Such failures
  still publish the reliable source-fault metadata before termination.
- The coordinator retains its immutable plan and receipt during cancellation,
  stale-source rejection or deadline expiry until worker acknowledgment. A future
  App adapter must retain its corresponding session/job ownership while polling,
  bind the supplied stamp to the exact live control endpoint, and test same-instance
  endpoint replacement. Dropping the coordinator still cancels without releasing
  the worker's source lease, but does not implement the future App session guard.

Plugin-free tests cover queued cancellation/expiry without backend calls, dropped
receiver, full/disconnected queue, teardown/panic, immediate retry while pending,
old-receiver versus new-job ownership, acknowledgment-gated success/error receipts,
and real-fault precedence. A typed line-protocol stand-in blocks each of the 15
source-capture IPC stages separately: cancellation must stop at that exact ledger
prefix. A slow successful first base getter crosses the deadline with a second
base waiting; no second request is permitted. State/getter faults concurrent with
cancel remain faults. Every group first exercises the successful control sequence.
The source owner stays alive for ledger assertions, separating capture termination
from ordinary later source teardown. These interruption fixtures execute no vendor
module or production helper.

Cancellation during fresh helper construction, restore, rendering and teardown is
a later stage. This capture-only patch cannot qualify App actions, audible output,
WAV files, latency trimming, tail consent or native/Windows behavior.

Pre-execution review of `01750d5` found a terminal-publication ordering edge: the
old lease published acknowledgment just before clearing busy, so an immediate
retry could spuriously fail. The corrective source releases admission first and
publishes the Release acknowledgment last. A per-fence test-only barrier forces
that inter-store boundary and checks both states, then retries after Acquire
acknowledgment without first joining the worker. The initial checkpoint remains
preserved; no tests had run on either cancellation source at that correction.


## Bounded cancellation results at 90795d5 (2026-10-10)

The corrected source passed all-target compilation, 115 runtime-module tests
(including 15 coordinator/lease and 11 capture/factory/affinity tests), 15 no-default
core coordinator tests, 25 helper tests and 13 editor/transport/reset protocol
tests. App and no-default core Clippy and formatting passed. The protocol fixture
exercised all 15 cancellation stop stages, elapsed-deadline success before a second
base getter, and real state/getter failures concurrent with cancellation.

The Python rerun passed 239 tests with the copied current helper explicitly bound;
552 package-input/provenance/document checks passed. An initial Python invocation
without that binding ran 235 tests with one skipped helper class; its log is
preserved separately and is not counted as the full pass. No native plug-in was
loaded. Current source/manifest hashes, seven copied executable hashes, verbose
current-worktree compiler linkage, dep-info and terminal logs are retained.

Full App/core aggregate and unchanged vendor suites were not repeated for this
narrow patch. Their older accepted preparation receipts remain separate. This
final documentation-only freeze does not change Rust, manifests, helper protocol
or packaging behavior relative to tested `90795d5`. The initial acknowledgment
ordering review finding and earlier failed source/fixture/provenance receipts are
preserved. Exact App control binding and session ownership, cancellation during
fresh factory/render/teardown, note mapping, WAV output and native/Windows/UI
qualification remain open. Existing full-song VST export still refuses.
