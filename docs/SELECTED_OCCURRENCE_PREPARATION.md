# Pure selected-occurrence preparation and history checkpoint

Reference checkout: `/workspace/shared/DAW-recovered-cbbbe1f`, clean accepted
`ec47b63fff8bd33b7c09159c190735cde8df5785`. Renderer mechanics remain separately
accepted at `e79894920204610c4c5ca6d0d1ada1c08dc2fe4b`; do not alter that branch.
Status: BOUNDED PURE GATES PASS. Rust/manifests were tested at
`2fd8cd8a4269ec4adf4eb9dcae5913224acf8f84`; the final documentation freeze does
not change executable source. The independently cleared source plan is preserved by SHA-256
`c05b8c98d01eb08c0ae895a4b5a99ab2d312bde12183b53c765d63e0d94e1a95`.

Implementation: `src/app/clip_render.rs`, `clip_render_candidate.rs`, and
`clip_render_tests.rs`, registered as a dormant module in `src/app.rs`.
All twenty-two new plugin-free test functions passed in both App and no-default
core builds. The source adds no App action/call site. The separate e798949 renderer/capture branch is not
integrated into this ec47b63-based slice.

Exact snapshots are fallibly encoded into at most 64 MiB and must deserialize
without normalization. This is a serialization proof, not a filesystem save/reopen
or real App session proof. Session/revision tokens and publication IDs are data
supplied by private fixture seams; only a future qualified adapter can attest
actual endpoint ownership, durable publication bytes, and user transaction gates.

Additional explicit admission limits: source length at most ten minutes; rounded
N + fixed tail + reserved one-second native latency at most 24 million frames;
Channel volume in 0..=1, the valid source Channel domain, and centered pan. The original selected schedule preserves its compiled note IDs.
Muting a source renumbers later sequential runtime note IDs; the remaining-event
proof ignores only those transient IDs, retaining complete source identity,
frame/order, note/channel, velocity/gain and stable routing.

No native state/history, idle-silence, output files or real audio equivalence is
qualified by this source checkpoint. Older renderer/capture acceptance and its
failed attempts remain in their separate durable archives and Git history.

## Executed gates and attribution

At the tested Rust commit 2fd8cd8, all of these gates exited zero:
- all-feature/all-target compilation and no-default core compilation;
- occurrence 22/22 in App and core, including 100 exact candidate geometry cases;
- existing audio-import 10/10, history-filter 27/27 and export-filter 81/81;
- App/all-target and core/all-target Clippy, plus formatting;
- Python 239/239 with no skips and the explicitly copied current helper;
- package input/provenance/linked-document checks for 553 inputs.

The source-bound receipt is `DAW-selected-occurrence-qa/2fd8cd8/RESULTS.json`,
SHA-256 `a9860270a42bd5c228a5504281b7eac4b0d29dee299442e02132f33c6e19eb82`.
It records 706 source hashes, five copied binaries/rlib, four dep-info files,
twelve exact gate logs/commands and environment. The vendor rlib is byte-identical
to the forced current-worktree 7578047 compilation; no vendor Rust or manifest
changed between those checkpoints, and its verbose source/link proof is retained.
The helper Rust test binary was compiled/copied only; it was not run as a suite.

Full App/core aggregate suites and unchanged vendor/helper/protocol Rust suites
were not rerun. No genuine plugin, real offline adapter, native UI or new App
button was exercised. The original 7578047 gain-fixture failure remains preserved
and is superseded only by the explicit source-domain correction and these gates.

## Preserved pre-execution review finding

Initial source checkpoint `1ca990b225de1e4f43e9117b275404166c03bf13` omitted an
explicit fixed-tail consent token. It was durably saved but was never compiled
or tested. The correction adds `FixedTailAcknowledgment`, bound to the exact
serialized source snapshot, source context, Clip ID, sample rate and chosen tail frames.
Admission rejects missing/mismatched acknowledgment even for T=0; the media
receipt must carry the same admitted token before a candidate can exist. A
numeric `tail_frames` argument does not imply consent. Real UI presentation and
authoritative token issuance remain a later App adapter gate.

The same review found optional nonfinite fade-reference numbers can deserialize
from JSON null into absent fields. Snapshot capture now re-encodes the restored
Project through the same bounded writer and requires byte equality, rejecting
that lossy serialization even on ignored Pattern/muted references. No normalize
or repair pass is permitted. Both original findings are retained in this
checkpoint history; no execution preceded these corrections.

## Preserved first-execution failure

At `7578047e377b717450268524516b5fe79b78623e`, forced-source all-feature/all-target
compilation passed. The focused occurrence suite reported 21 passed / 1 failed.
The failing successful-control fixture supplied Channel volume 1.5; the existing
Timeline compiler correctly emitted `InvalidChannelVolume { value: 1.5 }`
(`timeline.rs:1334` accepts only 0..=1). The destination Audio Clip gain permits
1.5, but that does not widen the source domain. No candidate was installed.

The narrow correction tightens both planner/candidate admission to 0..=1, uses
only source-valid positive gains and explicitly refuses 1.0001. It does not
weaken or change the compiler. Original compile/test logs, source hashes, copied
binaries and dep-info remain preserved under the 7578047 QA receipt. No other
suite or native action was executed before this corrected checkpoint.

## One small implementation slice

Create a separate worktree from ec47b63. Add a dormant pure module beside
`src/app/audio_import.rs` and its tests. It may prepare a selected occurrence,
validate an already-produced fake media receipt, construct a whole-Project
candidate, and call the existing explicit history helper from a guarded wrapper.
It must make no filesystem, plugin, audio-runtime or UI-button calls. No new
Project schema or general history/media manager is proposed. Real producer,
source endpoint and filesystem verification remain later adapter responsibilities.

Suggested boundaries (names may be adjusted without broadening behavior):

- `OccurrencePlan::prepare(project, context, clip_id, sample_rate, tail_frames, acknowledgment)`.
  Own the exact fallible serialized Project bytes and immutable context rather
  than relying solely on a hash. The context includes the actual project session
  plus source/control revision tokens; the future App adapter must bind those
  tokens to real state, not invent them from a selected channel. Pure tests can
  prove equality/rejection only. Preserve the relevant native capture identity
  when this module is integrated with the accepted capture/renderer branch.
- `prepare_rendered_occurrence_candidate(project, current_context, plan, media)`.
  This validates everything before cloning/changing a Project. The media is a
  data-only receipt bound to the same plan/context: published absolute UTF-8
  path, immutable producer/file identity, exact PCM/metadata, and prewrite dry
  peak. Tests construct it through a fixture seam; no production filesystem
  attestation is claimed or supplied by this pure module.
- `commit_rendered_occurrence_history_transaction(current_context, project,
  history_snapshot, history_fingerprint, undo, redo, dirty, candidate)`.
  Repeat the context and exact before-snapshot check immediately before invoking
  `commit_explicit_project_history_transaction` (`app.rs:4164`). Return source,
  audio Clip and asset IDs. A mismatch or no-op failure touches none of those
  mutable values. This wrapper remains dormant outside tests.

## Source admission and compiled interval

Resolve exactly one unmuted Pattern Clip, one Pattern, one Channel and one enabled
fully-wet unbypassed VST3 Instrument instance. Stable IDs must be nonzero and
unique. Pattern membership is checked from persisted enabled steps and unmuted
Piano notes, using original Channel ordering: reject multiple addressed Channels
rather than rendering one and muting the others' occurrence. Reject missing or
unassigned note channels and empty selected musical content. Reject a muted or
non-solo-audible Channel, configured MIDI routing, active nonempty automation,
nonfinite/out-of-range controls, and noncenter Channel pan in this first subset.
Conservative rejection can name an otherwise valid excluded item; it cannot
silently omit it. Follow existing bounce metadata/state restrictions only at the
later real-capture boundary; the pure planner does not fake native eligibility.

Validate the Mixer graph and pin the source Channel's stable `mixer_track` ID,
not the Playlist row or a mutable runtime index. Compile the original Project
with `TempoMap::from_project` and `CompiledTimeline::from_project` using existing
default options. Reject all diagnostics, suppressed diagnostics, invalid/skipped
content, collapsed ranges and out-of-song/truncated Clip ranges. Do not normalize
the Project to repair a candidate or silently extend the song.

Let A = beat_to_frame(f64(source.start)), B = beat_to_frame(f64(source.start) +
f64(source.length)), and N = B-A. Require A<B within the compiled song. Select
NoteOn/NoteOff by full `NoteSourceDescriptor` Clip and Pattern IDs (`timeline.rs:
382`), retaining all IDs, source descriptors, pitch, Channel, mixer track and
compiler order. Assert each selected on is in [A,B) and its matching off in
(on,B]; never fabricate a chased/pre-start note or infer the pattern period.
Only then subtract A from frames for a local [0,N] schedule. This uses the real
compiler for source offset, separate step/Piano repetition periods, swing and
clipped ends. Record A independently so a future renderer supplies absolute song
transport while its file/schedule origin is zero. No MIDI mapping duplication:
a later integration consumes this schedule through the accepted pure mapper.

The first plan should explicitly reject same-generator note activity from a
different occurrence overlapping or crossing this interval/tail (resolve by
instance identity, not only Channel ID) rather than claim an
isolated clone reproduces shared-voice context. Native state/history, random
behavior and idle output still require genuine qualification. A different
source/control revision invalidates reinsertion even if visible values were
edited back. Any changed Project bytes, including unrelated persisted edits,
conservatively reject the delayed result rather than overwrite them.

## Audio candidate, gain and geometry

The receipt must match plan/context, sample rate, stereo IEEE float32 format,
exact N+T frame count, finite samples, a computed peak matching the receipt
and explicit fixed-tail consent. Rate is
8000–192000 Hz and the established frame/tail limits apply. Reject prewrite dry
peak >1 before Project/history mutation: `wav.rs:600` currently clamps Float32
on import, whereas export preserves headroom. Do not normalize or silently clip;
retain any already-published WAV as a recoverable artifact. Decode/cache data
must equal the same file's reload representation, not separate higher-headroom
PCM. File-content verification itself belongs to the later producing adapter.

Default proposed tail is T=0, explicitly described as cutting at the Clip end.
An explicitly chosen fixed T>0 extends only the Audio Clip; reject B+T beyond
song end. The UI can later review overlap, but this pure slice does not auto-grow
the Project or shorten a requested tail. Require recompiled candidate audio
start=A, requested stop=B+T and actual stop=B+T, with native source frame zero.

Preserve the source Clip start and visual row. Keep its original displayed length
for T=0; for T>0 calculate a checked finite display length. Use existing
`AudioLengthReference` (`model.rs:256`, `audio_clip.rs:217`) to pin the exact
end-frame-derived f64 extent for both realtime and ordinary export paths; do not
reuse import's tempo/snap-rounded duration (`audio_import.rs:77`). Compile the
candidate again and inspect the actual `AudioClipDescriptor` to prove placement,
length, source offset, gain and stable routing. Round-trip serialization must
preserve this proof. If exact geometry cannot be represented, reject.

Channel/Clip gain mapping is grounded in the current callback:
- Pattern Clip gain already scales VST MIDI velocity (`audio.rs:6595`).
- Generator PCM is multiplied by Channel volume and linear Channel pan before
  being added to the Mixer input (`audio.rs:11336`, `11444`).
- Audio Clip PCM is multiplied by Clip gain/fade before that input (`10938`).
Therefore, with Channel pan exactly zero, retain dry WAV PCM and set new Audio
Clip gain to the captured constant Channel volume exactly once. Set fades zero;
do not multiply the original Pattern Clip gain again. The admitted peak<=1
also stays below the existing safe-plugin/sample clamps. Add one explicit
`AudioClipMixerDestination` using the same stable Mixer ID. Downstream inserts,
fader/pan, sends and Master processing stay live and are not baked. A later user
edit to Channel volume will not modify the converted Audio Clip's frozen gain;
that is part of rendering a snapshot, not a claim of continued channel linkage.

## Atomic change and media lifetime

Allocate fresh checked Clip/asset IDs before mutation. Clone once. Change only
the selected source Clip's muted bit; preserve its ID, Pattern, notes, group,
placement, source offset, gain and plugin data. Append one AudioAsset and Audio
Clip, ungrouped, source frame zero, no trim/fade references except the exact
length reference, and the explicit stable Mixer destination. Leave every other
Pattern occurrence and entity unchanged. Revalidate graph, compile and expected
source-note absence before constructing a candidate.

Commit exactly once through the existing history helper after the final exact
source/context check. The helper may first save an already-unrecorded preceding
edit as its own earlier history entry, as it does today; the conversion itself
is one undo step. First undo restores the exact pre-conversion Project; redo
restores the same audio/source mute/asset path. Failure never partially mutes the
source, installs an asset or changes history/dirty state.

`Project` and undo snapshots contain paths/metadata, not file ownership. Undo
prunes runtime PCM caches (`app.rs:18780`), so redo/reopen needs the durable WAV.
The pure layer never creates, moves, overwrites or deletes a published file,
even for stale, unsupported or headroom-rejected candidates. A later adapter
must publish to a unique permanent path, not a disposable cache/temp path, and
retain it across undo/redo/history eviction/save/reopen. Orphan cleanup is
explicitly deferred. A saved-project/managed-media policy is an App gate.

Muting the source Clip suppresses its compiled MIDI events only. The generator
still processes without notes (`audio.rs:11358`); an arbitrary plugin may emit
idle audio. No model test can claim that muted MIDI silences that generator.
Native idle-silence/context qualification remains mandatory before a real action.

## Plugin-free acceptance and stop condition

Successful controls must precede injected refusal cases, asserting exact error
category/reason and unchanged serialized Project/history/dirty state. Cover:
- unique versus missing/duplicate Clip/Pattern/Channel/instance, multichannel
  steps/Piano, active automation, MIDI routing, inaudible source and pan refusal;
- start A=0/1/127/128/129, short/nonaligned intervals, source offsets, swing,
  separate step/Piano repeats, same-pitch overlap, clipped end and empty range;
- another Pattern occurrence untouched, exact descriptor filtering, overlapping
  same-generator context refusal, and full note identity/order preservation;
- wrong session, source/control revision, exact snapshot, candidate media stamp,
  rate/channels/encoding/frame count, NaN/headroom, exhausted IDs and tail bounds;
- recompiled candidate audio starts/stops at exact frames and correct stable
  Mixer ID; constant Channel gain appears once while Pattern gain stays in MIDI;
- one-step undo/redo, pending preceding edit, stale completion, preservation of
  original source/plugin data and file path through history, and no filesystem
  activity by the pure layer.

Freeze source plus complete Git bundle and explicit unvalidated status before
execution, get independent source clearance and coordinate shared Cargo. Stop
this slice at reviewed source/mock proof and a durable checkpoint. No real App
button, native producer, media registration or output-file operation is included.
