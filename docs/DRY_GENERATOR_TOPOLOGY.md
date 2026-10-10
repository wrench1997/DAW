# Dry generator topology prerequisite

Status: UNVALIDATED source checkpoint, 2026-10-10. No tests, genuine plug-ins,
editor/device or user-facing activation have executed for this change. It is a
prerequisite to a separately approved initialized Surge qualification, not evidence
that that qualification passed. The prior strict adapter source/mock checkpoint
is 7255e5f0. The approved proposal is aa82dc92af18c8001c035808f59c666bcc79ec12da108c0c45d4eafe7fa4e4f3.

## Admission and dry scope

Capture, dormant offline preparation and the strict renderer retain every ordered
audio descriptor as `BounceAudioBus { channel_count, is_main, active: Option<bool> }`.
The bounded policy permits zero or one known main mono/stereo input, one known-active
main mono/stereo output at index zero, and up to two known-inactive auxiliary
mono/stereo outputs. Unknown activation, active auxiliaries, unsupported role,
count or channels are refused. Existing Instrument category, one known-active main
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
only for already nonparticipating, unknown buses. A known-active auxiliary is
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
