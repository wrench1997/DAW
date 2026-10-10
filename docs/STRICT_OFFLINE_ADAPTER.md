# Dormant strict offline clone adapter

Status: **UNVALIDATED SOURCE, MOCK EXECUTION PENDING REVIEW AND ARCHIVE**.
The Linux `experimental-fresh-helper` feature is disabled by default. This slice
connects the reviewed closed client to the dormant selected-instrument renderer.
It adds no App button, selected-occurrence installation, native editor, device,
observed-age metadata mode or tagged transport. Genuine plugin acceptance requires
a separate fixed qualification plan. The legacy offline factory is not reused as
strict transport or disposal evidence.

## Owner and immutable inputs

`plugins::strict_backend::StrictBackend` is empty at construction and remains
!Send/!Sync through its owning Client field. The RenderAdmission factory creates
it on the renderer worker. Spawn, restore, MIDI, Process, metadata, checked reap
and destruction all remain on that owner. No loaded owner crosses threads.
A failed post-spawn setup stores the returned cleanup Client before converting
or reporting its error. Pending cleanup retains the same owner and job admission.
The client's single process-wide reservation blocks another strict clone until
actual reap; the source's existing legacy helper is separate and untouched.

The adapter borrows the capture's exact load identity and requested base-ID set.
It does not call the legacy render_spec path that clones opaque state. Captured
state is borrowed synchronously by LoadState. There is no source endpoint handle,
source SaveState/LoadState/reset, mutable Project, state fallback or fresh retry.
The existing installed-helper/explicit-helper path policy is retained; this is
path/identity validation, not cryptographic authentication of a binary.

## Ordered preparation and rendering

1. Validate authoritative nonempty state, exact requested finite base IDs/values,
   VST3 role/configuration, sample rate and original absolute job budget.
2. Spawn once. Install closed cancellation registration before LoadPlugin; a
   cancellation during spawn is re-observed before the first exchange.
3. Load the selected factory class at the captured rate and configured maximum
   2048. Check actual class/version; select Offline.
4. Query reset support: exact contract 1, configured maximum 2048, processing
   false. This query is configuration evidence, not proof of prepared buffers.
5. Restore once, Start, record dirty revision, read the bounded complete catalog.
   Dirty revision is evidence only; it is not a freshness or edit-generation fence.
6. Validate every requested base is present and writable before any setter. Apply
   all bases in deterministic BTreeMap order, then read all back. Raw f64 must be
   finite in [0,1] and numerically equal at the explicitly captured f32 precision.
   Signed zero is numerically equal; no tolerance or clamp is used. Readback is
   controller evidence, not a claim that SDK Process has applied a queued edit.
7. Read owner-only checked audio/event layout and latency/tail. Validate raw main
   and Option<bool> activation while retaining every full ordered audio descriptor. The dry-topology
   prerequisite permits at most one known main mono/stereo input, one known-active
   main mono/stereo output, and up to two known-inactive auxiliary mono/stereo outputs.
   Participating input channels receive exact zeros through the closed count-only
   client. Require one known-active main event input and no event output. Compare
   complete captured metadata. See DRY_GENERATOR_TOPOLOGY.md for the unvalidated
   prerequisite and observational capture/fresh-source acknowledgment boundaries.
8. The existing renderer reads strict metadata before and after every Q128 block,
   sends only planned ordinary notes at offsets 0..127, and processes once with
   explicit transport. Every returned sample, including discarded latency/padding,
   must be finite with exact shape, transport acknowledgment and zero event/loss.
   No output enters the retained WAV interval before these checks.

Catalog titles are not compared after positive processing. Factory class/version,
name/category, exact bus topology, latency and tail retain their existing equality
policy. No title-settlement exception changes state/base or timing acceptance.
This remains the whole-song selected-generator engine. Selected occurrence origin,
clip identity, media ownership and history installation are separate integration.

## Cancellation and typed evidence

Every accepted ExportControl cancellation, through any clone or receipt Drop,
publishes the existing sticky CAS before cloning a registered closed handle under
a short lock. The wake runs after unlock. Registration installs then rereads the
same sticky state. Its Arc identity guard cannot unregister another job or a
replacement registration; handle drops also happen outside the registration lock.
No arbitrary callback, raw sender, process owner or pipe is registered. Before
every internal exchange and before Stop or Shutdown, the owner also synchronizes
the canonical sticky flag into Client, closing the interval while the cancelling
thread has published the flag but has not yet delivered its wake.

RenderBackend now reports closed Cancelled/Deadline/Backend categories, optional
NotSent/MayHaveApplied delivery, bounded diagnostic and exact strict transport
kind when present. Complete helper failures and metadata refusals retain their
observed category rather than being inferred from text or a later cancel flag.
An established render fault remains primary through cleanup. A cleanup cancellation
or deadline after successful rendering remains typed and forbids publication.
Rust panic is a fault, not rollback or automatic-retry authority. Cleanup reporting
grace and kill/wait/nonzero-exit diagnostics remain separately bounded evidence.

## Revised one-shot cleanup oracle

For this disposable never-reused fresh clone with no external MIDI output, the
approved oracle is healthy checked Stop, reply-free Shutdown, and proven direct-
child reap. It explicitly replaces the earlier bounce plan's panic/reset128 or
Surge256 cleanup sequence. No reset audio is sent or appended. Reusable realtime
reset behavior and its accepted tests are unchanged; those tests do not qualify
this new cleanup path.

Both Stop and Shutdown use min(original job deadline, original cleanup deadline).
Cancellation or expiry may reject them NotSent; cleanup must still close pipes
and observe/force/reap within its separate reporting grace. No expired job gets a
new protocol budget. A terminal/partial exchange skips all later protocol,
including Stop and Shutdown. Semantic admission rejection of a valid reply is
different: its still-healthy client may perform the ordered teardown.

The adapter attempts Stop at most once, remembers whether Start succeeded, and
uses the same checked cleanup kernel. Once disposal returns, later polls use only
nonwaiting same-child continuation. A Rust panic after closed-pipe disposal cannot
cause a second Stop or Shutdown. Successful kill is Pending; only observed reap
releases ownership. Forced kill, abnormal exit or cleanup error prevents WAV
success. Publication occurs only after reap, backend destruction and the existing
writer's final cancel/deadline/commit fence. No universal hard bound is claimed for
spawn, allocation, OS scheduling/syscalls or reap.

## Planned mock evidence and attribution

Closed test-only owner actions pin cancellation before registration/Load and
between Stop/Shutdown, plus one Rust panic after disposal returns. Real client
mock-child ledgers exercise preparation stages, exact note/Process sequence,
semantic versus terminal failure, raw topology, base readback, per-block output,
cancellation and cleanup-before-WAV publication. Shared wake tests cover both
sides of registration, all clones, replacement guards and publication fences.
Typed fake-backend tests separately cover fault/cleanup precedence and bounded
secondary evidence. Their results will not be mislabeled as actual child behavior.

Stop A's deterministic partial-write, kill/wait-error, spawn-setup ownership and
quarantine syscall fixtures remain separately attributed. This adapter does not
add public FD/process authority merely to reproduce those injections. The final
receipt will identify executed cases and unexercised OS/platform paths explicitly.

## Executed Stop B source/mock checkpoint

The unvalidated source above is now exercised within its dormant Linux/mock
boundary. The first source-bound runs on `b84466ee` passed 230 distinct selected
all-feature App tests: 13 actual-adapter mock-child tests (including the 104-case
preparation/processing fault matrix), 24 typed renderer tests, 24 export-control
and registration tests, 60 ordinary-export tests, three option tests, 15 capture
runtime tests, three MIDI-equivalence tests, 11 offline factory tests and 77
runtime tests. Helper 25 and protocol 13 also passed. Those are focused selections,
not a complete App-suite result.

The first QA driver deliberately rejected a zero-match capture selector; its
continuation selected the actual `instrument_bounce_tests::` suite and ran only
unexecuted gates. All-feature Clippy then rejected one new test's constant
`chunks_exact(2)` expression. Both failures remain in the evidence. The sole Rust
correction at `8c657fc9` uses `as_chunks::<2>()` and explicitly rejects a remainder;
production Rust is identical. That exact success fixture was freshly rerun, and
current all-feature/default/core Clippy, all-target compilation and formatting
passed. A separate current feature-off/core build passed 92 selected tests:
renderer 22, export control seven, ordinary export 60 and options three.

The same corrected revision passed two fresh external client positive controls,
239 Python tests (zero skips), and the 556-input source/package closure. Forced
current-worktree vendor compilation, exact rlib linkage and copied test/helper
identities are retained. The default helper SHA-256 remains
`7865876366d2eb608fc2b685d79256b71ca4d9d27683a3e33ec7afa2469b3f69`,
identical to Stop A and the combined baseline.

Unchanged vendor 575 tests, 26 docs, direct vendor Clippy and the prior 68-unique
client compiler outcomes are qualified historical reuse, not new executions.
The receipt retains the vendor harness's standalone 190-package lock/profile,
all 64 vendor files, 62 relevant dependency/fixture inputs and 68 fixture byte
sequences. It explicitly lists changed App sources and eight differences in the
old full compiler input map. The original compiler report remains failed 67/68;
the preserved targeted 3/3 produces the already-reviewed 65+3 unique union.
Fresh current App linkage and client positives are recorded separately.

The evidence verifier recovered ten nested `bin/*.rs` QA snapshots omitted by an
older evidence-copy filter. Every recovered copy matches the sealed hash and the
original archive's full-source member; the source itself was already preserved.
The new copier retains source directories named `bin` and excludes only actual
compiled-artifact directories. Its initial report-schema mismatch is retained
alongside the corrected successful binding; no test was rerun for that QA fix.

This closes only the source/mock adapter gate. The actual child is a deterministic
mock helper, not a VST3 plugin. Injected low-level spawn/partial-write/kill/wait
faults remain Stop A evidence; adapter-level ownership, cancellation, cleanup and
publication paths are fresh mock evidence. Genuine plugin sound/state fidelity,
selected-occurrence/media/history integration and any UI/device activation still
require separately reviewed qualification. Cooperative deadline, single global
strict-child reservation and checked-reap limits remain in force.
