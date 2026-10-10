# Experimental strict fresh-helper foundation

Status: **SOURCE/MOCK TESTED**, first low-level Linux transport/mock checkpoint.
The feature is disabled by default and has no App, helper factory, bounce, or
plugin adapter caller. Existing strict metadata and legacy transport behavior
remain as before. This work follows the reviewed scoped-worker library checkpoint;
it does not activate GUI/audio separation.

## Closed initial contract

The crate-private `fresh_helper` module is compiled only on Linux with
`experimental-fresh-helper`. It owns a fresh child and its required nonblocking
stdin/stdout pipes on one host thread. It creates no reader, writer, or reaper
thread. A cloneable cancellation handle owns only a sticky atomic and shared
close-on-exec eventfd; it does not expose the session or its pipes.

The eight supported operations are LoadPlugin, LoadState, SaveState, Process,
ProcessBuses, GetParameter, LatencySamples and TailSamples. There is no implicit
LoadPlugin, cold-load retry, replay, transparent recovery, or unrestricted command
forwarding. Unsupported operations fail before encoding. Shutdown is private,
reply-free healthy cleanup traffic. Exact expected reply shape is supplied and
validated before writing, then checked against the actual response without
padding or truncating channels, frames, buses, or metadata counts.

Nonempty output-event arrays are rejected at the first element before decoding
an event payload. Empty-array loss and transport-applied flags survive unchanged.
State uses the current base64 form; legacy numeric state arrays are explicitly
unsupported. These restrictions exclude genuine MIDI-emitting plugins from this
initial subset. Audio bits, including nonfinite IEEE patterns, are preserved;
finite audio, required transport application, and acceptable event loss belong
to later caller policy.

## Delivery and deadlines

Each exchange uses its original absolute deadline across admission, encoding,
nonblocking write, response read, decode and final success validation. Payload
loops check cancellation/deadline at bounded checkpoints. No response becomes
success after observed expiry or cancellation. The timer is cooperative:
allocation, process creation, scheduling, system calls and process reap do not
have universal hard deadline guarantees.

Delivery is `NotSent` until the first positive write, then permanently
`MayHaveApplied` for that exchange. A well-formed helper Error is an operation
failure, not rollback proof, and seals this conservative session. Transport
corruption, timeout, cancellation, unexpected EOF, and uncertain writes also seal
it. A complete response followed by observed EOF is retained as diagnostic data
while the session becomes terminal. A later request cannot reset the fault or
retry the operation. A known earlier fault is not replaced by later cancellation. Once full bounded
validation plus its final budget check identifies Error/Crashed, only that known
failure can enter a <=64 KiB diagnostic-copy tail that ignores later time/cancel
signals. Allocation/size limits still apply. If the diagnostic cannot be copied,
the known failure category survives with diagnostic-unavailable. Successful
responses never use this exception.

The protocol remains untagged. A malicious or incorrect peer's delayed duplicate
with the same response variant cannot always be distinguished from a later
response. Tagged transport is a separate future checkpoint.

## Bounded payload handling

The new codec does not use the legacy derived HostResponse decoder, clamping
codecs, or eager whole-channel raw/base64 intermediate representation. Limits
are explicit: 96 MiB encoded line; 64 MiB + 28 state bytes; 8 buses; 32 aggregate
channels per direction; 4096 frames per channel; 64 KiB ordinary text; 128 bytes
per numeric token; JSON depth 16. Compatibility metadata keeps the real
`new_class_id` plus `old_class_ids` schema, with at most 256 entries, 256 old IDs
per entry and in total, and 64 bytes per class ID.

A 256 MiB charged per-exchange working-set budget covers owned encoded/decoded
buffers and codec scratch. Returned responses become caller-owned; retaining
multiple replies is outside this charge. Caller HostCommand and ReplyShape
allocations are also excluded. Fixed boxed failure/cleanup envelopes allocate
only after the session is terminal or the child is safely owned and are reported
separately from the variable payload budget. Actual Vec capacities are charged. The request buffer is
released before receiving/decoding the response. This budget excludes caller
input, child memory, allocator bookkeeping/rounding, stack data and kernel pipe
buffers; it is not a process-RSS or real-time allocation guarantee. Any
conservative retained scratch charge is disclosed by the codec implementation.

## Cleanup ownership

One **global process-wide reservation** is acquired before spawn. It remains
held until a concrete `try_wait(Some(status))` proves the direct child reaped.
A setup error after spawn returns the actual owned cleanup session. A pending
cleanup, kill error or wait error never takes-and-drops the Child.

Drop writes no protocol and performs no blocking wait. It transfers the child
and pipes into its already-reserved process-lifetime quarantine slot. There is
no eviction, overwrite, background reaper, or second slot. A new strict spawn
is refused while either a live session or abandoned child occupies the
reservation. Thus simultaneous strict sessions and fresh replacement of an
existing strict session are not supported by this first stop. Later caller
integration must address this limit explicitly.

Checked direct disposal closes admission. Healthy disposal can send exactly one
bounded Shutdown. Terminal/partial exchanges never send another protocol line.
Pipe closure starts a single 100 ms grace within the caller's cleanup deadline;
repeated short cleanup calls do not renew it. Forced kill and observation errors
are recorded separately from verified reap. Abandoned quarantine cleanup can
force-kill directly. A guard restores quarantined ownership across Rust unwind;
no global mutex spans protocol I/O, kill, wait or callbacks. Direct-child exit is checked during blocked polls and before successful reply
publication, even if another writer keeps stdout open. The exit status is
retained for checked cleanup. Only the direct child is qualified by this reap receipt; descendants are not implicitly killed.

## Supported Linux host and tests

The actual SIGPIPE disposition must be ignored before spawn and before writes.
The foundation checks it read-only and never installs a handler or changes a
thread mask. Embedding code must not change the disposition during a live
session. This explicit precondition is not a universal guarantee for arbitrary
library hosts. Cancellation stores its sticky bit first, attempts at most four
nonblocking wake writes, and poll also rechecks at most every 25 ms; a missed
wake cannot defer observation until the full exchange deadline.

Mock tests serialize the global reservation. Cooperative child ledgers must
exit successfully without force-kill, so Python assertion failures cannot pass
as merely reaped. Deliberately hung/faulted child cases use separately labeled
force-cleanup assertions. Disposable host tests isolate unsupported SIGPIPE and
actual EPIPE conditions. Test unwind cleanup retains and reaps mock children
without a second panic. Codec/pump/cleanup fault tests and current-source linkage passed on the
archived source revision described below.

No genuine-plugin, native editor, audio-device, performance, bounce-rendering,
tagged transport, or observed-age metadata acceptance is implied.

## Executed checkpoint and retained qualifications

Source revision `927ef43b` passed all 532 available vendor tests, including all
63 new codec, pipe, cancellation, failure-precedence and retained-cleanup tests.
The one existing SDK-generated module-info fixture was explicitly excluded because
its external bundle is unavailable. Current gates also passed 26 doctests,
77 runtime tests, 25 helper tests, 13 protocol tests, all-feature/default/core
Clippy, formatting, 239 Python tests with no skips, and the 553-input package
closure/source-provenance gate. Upstream patch reconstruction has zero mismatches.

The three older private compiler corpora retain their qualified prior outcomes
under the disabled-feature boundary. Their six positive controls were rebuilt
on this source using the corrected 19-module shims; negative diagnostics are
reused, not newly executed. Prior full App/core executions retain their original
source and feature attribution. Current all-feature callers were freshly compiled
and linked, and the runtime/helper/protocol tests above were freshly executed.

Forced current-worktree compilation binds both the vendor feature test executable
and the linked default helper. The default helper SHA-256 is
`d325d65df4267f8b4bff07701971596a042dc61a56671bd3cdf2af91e03fb64c`,
byte-identical to the previous default helper. Its features exclude both
experimental worker and fresh-helper paths. This byte identity does not replace
the separately recorded new feature mock results.

All intermediate evidence is retained: the initial 62/63 result with the wrong
`"Infinity"` negative expectation, the later manual-clamp Clippy failure, the
compiler runner's rejected binary-bearing QA manifest, and the first Python run
that explicitly skipped its unset helper path. The corrected runs are separately
recorded. The first two source corrections preserved parser/growth behavior; the
remaining corrections changed only external QA inputs. Historical unvalidated
source-checkpoint sections remain historical records.

These results qualify this private Linux foundation and mocks only. No genuine
plugin, bounce adapter, parallel helper sessions, tagged transport or production
audio path uses this module yet. No Windows source or runtime acceptance is added.

## Following Stop A source checkpoint

The original eight-operation source/mock qualification above remains historical.
The following closed offline client and ten additional existing wire operations
are currently **unvalidated source**; see [Strict offline client](STRICT_OFFLINE_CLIENT.md).
It adds synchronous borrowed requests and retained nonwaiting cleanup, with no
App/RenderBackend or real-plugin activation. Its new results require their own
frozen source/mock receipt rather than inheriting the first foundation pass.
