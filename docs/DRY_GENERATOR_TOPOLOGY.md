# Dry generator topology prerequisite

Current revision: UNVALIDATED role-versus-participation source change, 2026-10-10.
The original prerequisite history and its executed results remain below. The current
admission section incorporates reviewed plan
00be9d0d0231270971ecf8c437426e75c4f4e846dffe64edb2ab5b4f64821084.
No new policy mocks or genuine qualification have executed for this revision.

Status: UNVALIDATED source checkpoint, 2026-10-10. No tests, genuine plug-ins,
editor/device or user-facing activation have executed for this change. It is a
prerequisite to a separately approved initialized Surge qualification, not evidence
that that qualification passed. The prior strict adapter source/mock checkpoint
is 7255e5f0. The approved proposal is aa82dc92af18c8001c035808f59c666bcc79ec12da108c0c45d4eafe7fa4e4f3.

## Admission and dry scope

Capture, dormant offline preparation and the strict renderer retain every ordered
audio descriptor as `BounceAudioBus { channel_count, is_main, active: Option<bool> }`.
The current bounded policy permits zero or one known mono/stereo input of either
preserved Main/Aux role, one known-active Main mono/stereo output at index zero,
and up to two known-inactive mono/stereo extra outputs of either preserved role.
Unknown activation, active extras, non-Main Output0, unsupported count or channels
are refused. Input and inactive-extra roles do not determine participation, but
remain part of exact metadata equality. Existing Instrument category, one known-active main
event input, no event output, exact factory identity, latency/tail and per-block
metadata equality remain required. No bus is filtered out of metadata equality.

Any participating input receives exact positive-zero samples, with no external
audio source. The closed client exposes only `process_silent_generator` counts
(0..2 inputs, 1..2 outputs, Q128); it does not accept caller sample buffers. A
borrowed fixed zero view uses the existing checkpointed count/emission codec and
charged encoded buffer. It does not allocate an unaccounted input Vec per call.
The existing zero-input method delegates to this path with count zero.

The unchanged flat processing kernel retains all declared SDK bus slots, channel
counts and channel-pointer arrays. Inactive buses have null sample pointers; active
input storage is zero-filled/copied each call. Only participating output channels
are flattened, so the admitted policy returns exactly the main output. This is
instrument dry output before channel/mixer/insert/send/master effects. No generic
routing or arbitrary external-input API is added.

## Acknowledgments are not prepared participation

The existing prepared bool vectors still control buffer participation. Separate
records describe successful owner-thread `activateBus(true/false)` calls and bind
them to observed media, direction, index, channel count, type, complete UTF-16 name
and flags. A skipped default-inactive activation stays unknown. Queries never
invent false or call activation. A failed activation clears its prior record;
setup/rebuild revalidates exact observed bindings and discards changed, missing,
reordered or unreadable sides. Event descriptors are now read during owner setup
for the same revalidation, without new default activation calls.

Known I/O/reload notifications, arrangement mutation, SDK setup failure and failed
state/lifecycle transactions invalidate evidence. State invalidation occurs only
after existing pure restore eligibility and decode guards, immediately before the
first potentially mutating SDK state call. A rejected reused Surge restore keeps
all evidence, descriptors, queues and lifecycle untouched. Successful unchanged
bindings may retain acknowledgments across ordinary buffer resizing; failed or
changed bindings cannot resurrect by returning to an earlier shape. This records
observed host operations, not unknowable plugin-private activation changes. It
cannot identify indistinguishable unsignaled bus replacement or prove physical
state beyond the SDK acknowledgment/observed binding contract.

## Fresh source setup and capture

The dormant Linux `experimental-fresh-helper` constructor
`PluginChain::spawn_fresh_bounce_source(instance_id, spec, config)` builds its
candidate inside the worker. It consumes a one-shot setup marker before any
fallible prepare guards. After optional authoritative restore and before Start,
it preflights the complete actual/prepared topology and proposes acknowledgments
only for already nonparticipating, unknown buses. A known-active extra output is
never disabled to make a candidate fit. Each actual false acknowledgment is
followed by complete descriptor/participation equality checks; Start is followed
by another metadata equality fence. Failure never publishes SlotReady or an active
usable processing slot. Endpoint object creation alone is not that admission.

Existing default source construction keeps its activation policy. Capture itself
is strictly observational and never activates, deactivates or reconfigures a
working source. A source without sufficient evidence is refused with a fresh-load
requirement. A used marker cannot retry setup or change an already published source.
The strict clone's existing state-refresh false calls are recorded truthfully;
no extra clone protocol command or hidden warmup is introduced.

## Source-visible proof obligations

New pure/mock fixtures cover bounds and complete raw descriptor drift; unknown
source refusal; all-source preflight before any activation; failed one-shot setup
and post-Start admission; exact default/fresh helper command ledgers; dry input wire
shape and positive-zero bits; codec cancellation/deadline and allocation accounting;
actual f32/f64 SDK buffers with poisoned input storage overwritten on repeated calls;
failed/mid-refresh activation, changed descriptors during activation, state/setup/
negotiation failures, I/O service failure before refresh, audio/event count/reorder
rebinding, and pure used-Surge restore equality. These are source intentions until
executed receipts are appended. They are not genuine plugin or timing acceptance.

The source archive and independent exact-source reviews must precede mock execution.
Relevant vendor/state/kernel tests must run freshly because vendor behavior changed;
the previous Stop B unchanged-vendor reuse receipt cannot qualify these bytes.
Genuine capture, MIDI/WAV audio and cancellation qualification remains separately
frozen, archived and authorized. No realtime reset policy, COM thread ownership,
strict fresh metadata or checked one-shot teardown contract is relaxed.

## Executed source/mock checkpoint, 2026-10-10

Bounded gates pass with the original failed runs retained. Source progression:
3868c2fb (topology) → fb86e1aa (mock polling/unwind evidence only) → c3aabe0f
(two convenience-method cfg guards only). No production algorithm was changed
after the first source freeze; all 66 vendor entries stayed byte-identical.

- 3868c2fb: 596 available vendor tests, 27 doc tests and direct vendor Clippy pass.
  The single pre-existing absent SDK metadata fixture was explicitly excluded.
  Nine actual topology COM tests, three pure binding tests, eight selected silent
  codec/client tests and the pure used-Surge rejection also passed as focused gates.
  The new doc compile-fail excludes arbitrary caller sample input from the client.
- The all-feature App proof consists of 247 unique selected cases across runs:
  59 bounce cases on 3868c2fb; corrected offline24, runtime77, export-control24,
  ordinary export60 and export-options3 on fb86e1aa. This is not the whole App suite.
  Default and core selected suites each passed109 on fb86e1aa. Helper25 plus13
  protocol cases passed there too. Prior-source outcomes are not relabeled as
  executions of the final cfg revision.
- c3aabe0f: fresh core Clippy and two affected pure audio-policy tests pass. Fresh
  public-client compiler contracts pass68/68 with stable inputs/fixtures and both
  positive controls; no compiled Rust fixture or plugin was executed by that runner.
  Python239, formatting and package557 checks pass; patch replay reconstructed all
  58 vendor source/manifest inputs with zero fuzz. Feature/default Clippy passed on
  fb86e1aa; the two cfg guards leave their enabled method bodies unchanged. Direct
  vendor Clippy uses the recorded existing deprecated/drain_collect allowances.
- Current-worktree forced vendor compilation, exact linked rlibs, caller commands,
  copied test executables and helper are bound. The current default helper SHA256 is
  bafce0dcf4c24411651d84df0877638b77b051019002a6cfbb06ce38ec96b76d.
  Dep-info uses relative vendor paths; its source identity is resolved with the
  recorded absolute Cargo manifest, source target and compiler working directory.

The first offline run remains FAILED22/23: the mock lacked three normal native
polling replies. Its original SlotFault text/ledger was not retained, and cleanup
emitted FileNotFoundError. Source inspection established the missing first polling
command; the corrected24-case run separately proves complete polling and successful
admission. Its intentional Rust-unwind case preserves the original panic, records
actual Joined, retains terminal Shutdown evidence, and inspects files before removal.
Legacy Joined still does not establish the strict clone's full child-reap contract.
The second run's core Clippy failure remains visible; the two cfg guards correct
unused helpers only in the no-VST3/non-test binary. Two QA binder failures are also
retained: assuming absolute dep-info paths and stripping every matching cfg attribute
rather than the exact two. Only the receipt binder was corrected/rerun for those.

These results qualify source/mock behavior and the bounded admission policy.
There was no genuine plugin, editor, device, WAV qualification or user-facing action
activation. Fresh real capture/state/base/MIDI/audio/reap acceptance remains a
separately frozen plan and authorized run. Unseen plugin-internal changes, synchronous
strict metadata cost and legacy source teardown limits are unchanged and explicit.

## Observed roles and minimal policy revision, 2026-10-10

The accepted diagnostic source/mock checkpoint is 28b3e38. The first three-instance
qualification attempt remained FAILED before its source became Ready; no capture,
strict clone or audio stage was reached. A separately frozen one-source diagnostic
then observed actual stereo Aux input/known true and three stereo Main outputs with
raw activation true/null/null. Prepared participation was true/[true,false,false].
Its proposed false values were not acknowledged or applied. The source was still
rejected by the old role-only predicates, and qualification_success remained false.
Diagnostic outer result c5c5e111d2f2bf3efac2f03ea93c3671f2cb33b2ad5a4152482d6cea1589e2af
and events 47d6e5da27a364b55b29762dda586a82574bb2e47048e900bc34757eb26fc0be
are preserved in the separately reviewed durable evidence archive; neither historical
outcome is relabeled by this revision.

The new source removes only the input-Main and extra-output-Aux admission checks.
Output0 still must be Main/known true. The same complete fresh-only preflight,
actual false acknowledgments, descriptor rebinding and post-Start fences remain.
Capture is observational, and a working source with unknown evidence still fails.
The closed input API supplies only positive zero; no external audio authority is
added. Full role/order/channel/count/activation equality still rejects drift,
including individually supported role changes before or after a Process call.
No vendor/kernel, helper wire, default activation or lifecycle behavior is changed.

New pure and mock fixtures will test roles independently, the observed role shape,
exact zero input, acknowledgment before SlotReady, read-only capture, and role
changes as equality failures. Prior role-only rejection fixtures now use explicit
unsupported channels or active extra outputs; their assertions are not weakened.
All new gates remain unexecuted pending source archive and independent review.
Existing full-slot f32/f64 SDK zero-input evidence is unchanged and will be checked
again. A later actual source capture/strict-clone/WAV qualification needs its own
updated harness and separate execution authorization; this source change does not
establish that state restore or DSP will succeed on the initialized fixture.


## Role-policy source/mock closure, 2026-10-10

The UNVALIDATED role-policy notes above preserve the source-freeze stage. Bounded
source/mock gates are now complete, with both intervening failures retained.
Production behavior has not changed since 57aedcfb. Final tested 843653da changes only
a capture fixture's iterator/exact-count assertion and its WORK_LOG explanation.
No genuine instrument qualification or user-facing activation is included.

Executed on 57aedcfb, with source maps unchanged within each run:

- Vendor topology 9, binding 3, silent-input codec/client 8 and used-restore guard 1.
  These include the actual f32/f64 repeated zero-input/full-declared-slot checks.
- All-feature App: offline 32, bounce 60 and runtime 77. The bounce suite includes
  the 378 successful topology cases and 360 exact fault cases inside two test
  functions; those case counts are not additional test-function counts.
- Helper 25, default offline 15/bounce 43/runtime 77, core bounce 43/runtime 76.
- All-feature/default/core test executables were source-linked and copied.

Run001 stopped at a QA artifact-copy collision after its passing App suites:
test/ordinary helper names shared a destination, and the ordinary helper reached
--list, emitted its startup line, received no protocol request and listed zero
tests. The selector rejected it. The App's earlier ordinary copy also shared its
later test destination, but the executed App copy matches the real test hash
b992d736. Both raw overwritten records remain. Exact original-hash artifacts and
dep-info were recovered separately; the correction uses unique profile names,
duplicate refusal and selected test-hash checks before list/run. No helper-test
pass or plugin load is inferred from the erroneous startup/EOF invocation.

Run002 completed the remaining mock suites, then root Clippy rejected a test-only
needless_range_loop. The corrected 843653da fixture binds the output array, asserts
exactly 3 elements and checks every unchanged role/channel assertion. Run003 freshly
compiled current all-feature callers and passed that affected capture test 1,
strict root Clippy in all-feature/default/core profiles, fmt, Python 239 and package
input validation 557. It also rebuilt the ordinary helper from this worktree and
reproduced bafce0dcf4c24411651d84df0877638b77b051019002a6cfbb06ce38ec96b76d.
The earlier full selected mocks are not relabeled as executions of the correction.

Artifact binding 4b4cfbaf25c6b1173d65593346031a558e0d1ee4b57e3e96af9b85892d99f774
binds 23 root artifact records, 4 copied vendor rlibs, the vendor test executable and
the continued exact helper test, with current package paths, exact compiler externs
and per-run before/after source hashes. The original collision and Clippy failures
remain terminal historical results. No full-App, whole-vendor, doc-test or compiler-
negative rerun is claimed for this small role-policy slice.

These mocks establish bounded role admission and complete descriptor fidelity,
explicit silent input, actual mock acknowledgment ordering, post-Start publication
fencing and unchanged strict output/cleanup checks. They do not establish genuine
Surge state restore, capture/base fidelity, MIDI/audio, WAV or cancellation behavior.
The earlier genuine001 remains FAILED and the one-source diagnostic remains only
an observed refusal with qualification_success=false. A new fixed qualification
must use the actual observed roles and newly observed known false acknowledgments,
with separate frozen harness, review, archive and root authorization.

## Fresh-only initial timing settlement, unvalidated source, 2026-10-10

The actual complete Start diagnostic observed only tail88,200→96,000, with all
ordered topology/activation/identity and latency fields equal. It did not admit a
source or qualify rendering. Approved plan145ff381 now permits an initial tail
change only across the first unpublished fresh-source Start. Both receipts still
pass policy and every other field remains exact; all pre-Start acknowledgment
comparisons remain full equality. The post-Start tail is the actual u32 value,
never a fixture-specific conversion or cached fallback.

A fresh-only scalar lifetime pin guards later live Ready/snapshot/poll/capture
reads. A mismatch faults; no renewed settlement is available. Capture's full
metadata equality and strict clone/renderer per-block checks are untouched.
Ordinary loaders and working-source defaults retain their behavior. Ready's first
reread must pass before its event; the second initial-snapshot reread can fault
after Ready was queued and then publishes inactive state. This ordering is not
changed or represented as atomic admission. No extra query or positive audio is
introduced. Source/archive/review/mock gates remain pending, and a new genuine
run is separately gated.

The fresh timing source/mock follow-on is now tested on b1e62cbb: feature49/60/77
and default15/43/77 selected suites, both all-target compiles, three strict Clippy
profiles, fmt, Python239 and package557 pass. Initial48/49 marker-publication
failure remains preserved; a reviewed test-only atomic marker fix preceded the
full corrected49 run. Actual plugin readiness/capture/render remains unqualified.
Current linkage and exact source attribution are in receipt d93750de; no topology,
SDK or default activation change accompanies the timing exception.
