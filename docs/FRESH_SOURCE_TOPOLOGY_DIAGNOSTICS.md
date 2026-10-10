# Fresh source topology refusal diagnostics

Status: UNVALIDATED source/mock checkpoint. No new compiler test or genuine run
has executed. Base dabcd337 remains the accepted topology source/mock checkpoint.
The genuine001 attempt failed source_setup before SlotReady; nine later stages
were unexecuted. Its exact evidence is retained in experimental Library v90.
That trace does not contain the rejected descriptor values. Static inspection of
the pinned Surge ELF suggests auxiliary input/all-main outputs; this is not a
runtime descriptor observation and does not justify changing admission.

The approved diagnostic scope is 60865b023fc39f888394f4b8295560b0237a139f715417aae09ba65d702716ca.
Only existing fresh-source validation failures gain an internal typed, bounded
record, converted to the existing SlotFault string envelope. The ordinary error
prefix remains byte-for-byte unchanged, followed by FreshBounceTopologyRefusalV1
JSON. The shared validator, successful path, default source construction and wire
protocol are unchanged. No extra SDK, activation, state, capture or metadata call
is added.

The record names ProposedPreflight, FinalPreStart or PostStart. raw_checked keeps
ordered channel_count/is_main/Option activation from the actual checked read;
proposal separately records the preflight-only intended false acknowledgments.
prepared, when available, is explicitly the existing initial_preflight_read and
contains channel_count/participation, not component-role evidence. PostStart has
no prepared snapshot because there was no such query; its prior metadata is
separately named expected_before_start. No unknown activation is changed to false
in raw_checked. No bus is relabeled or filtered for admission.

Each list is capped at16 while constructing the record, before materialization;
its full total_count and truncation flag remain. Error excerpts are capped at256
characters while the original rejection prefix is preserved. Only scalar bus
metadata and static phase/error text are serialized, never names, parameter
values, opaque state, PCM or object Debug. Diagnostic serialization failure keeps
the original error. The record is an error-path allocation, not a realtime or
no-allocation claim.

Five pure fixtures check raw/proposed separation, phases, absent prepared evidence,
unchanged prefix, original count/order/truncation and bounded projection visits.
Three actual-worker/backend protocol fixtures check exact refusal/success/post-
Start command ledgers, no unintended activation/Start/SlotReady, and unchanged
old metadata versus actual post-Start data. Existing capture/state/old-source
retention tests remain required. These are test intentions until result receipts
are appended.

A future one-source-only diagnostic attempt needs a separately frozen harness,
archive, independent review and root authorization. It may collect actual rejected
descriptors; it cannot qualify capture, current bases, strict clones or WAV output.
Any later role-versus-participation policy change remains a separate decision.
