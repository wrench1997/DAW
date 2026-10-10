# Selected-instrument renderer and Playlist conversion plan

Base: accepted 12af28b79b11d604e416799230a934952d62ac91, with capture/cancellation tested at 90795d5. The current source implements only the pure fake-backend slice described below, with focused plugin-free gates passed at b66a9a77aa033ce984995cfa21caf136f857761a. These are mechanics tests, not genuine-plugin or App qualification. The real adapter and Playlist conversion sections remain plans. Full-song export and App actions stay unchanged.

## Small supported result

One captured VST3 generator, arranged from song frame zero through the existing song duration, at its selected sample rate. Stereo float32 WAV, unity output gain; a mono generator is duplicated to stereo exactly as the current backend does. The scope is the instrument before Channel/Mixer gain, pan, inserts, sends and master processing. Other tracks and audio clips are intentionally outside this named scope, rather than purportedly included in a mix. Active automation, configured MIDI routing, unsupported source placement, buses outside the bounded dry-generator policy, unknown topology, MIDI output and current preparation limits continue to reject.

The request must explicitly acknowledge its fixed tail (0–30 seconds), even if the reported tail is longer or infinite. The report records captured identity, native latency, reported tail, chosen fixed tail, exact output frames and peak. No silence detector, auto-tail extension, loudness adjustment, freeze, project edits, source state reload, warmup or fresh re-save is introduced.

## Implementation stages

1. Current plugin-free-qualified mechanics: `instrument_bounce_render.rs` provides a pure selected-note schedule, fake-backend job ownership and bounded rendering engine; tests are in `instrument_bounce_render_tests.rs`, with paired actual callback fixtures in `audio.rs`. `export.rs` adds a crate-private checked float32/unity writer using the ordinary staged writer and cancel/commit CAS. `export_job.rs` adds an opt-in final deadline guard. Only test fixtures implement `RenderBackend`; there is deliberately no VST3 implementation or App callsite.
2. The fresh-clone adapter, only after a separately reviewed common strict-isolation failure/termination contract is agreed with the runtime owner. Reuse its contract/test corpus; do not implement a competing pipe transport or adopt its tagged framing. Connect the engine to real host calls only when these prerequisites have source/mock proof. Genuine VST qualification remains separately authorized.

Both checkpoints are dormant. Source plus full Git bundle is saved before tests, and each checkpoint receives independent review.

## Exact musical and sample contract

- Read the original CompiledTimeline.events() sequence from instrument_bounce.rs. Retain compiler ordering and stable note IDs; never rebuild the arrangement from channel-index shortcuts or the built-in oscillator exporter.
- Select NoteOn/NoteOff by the selected channel ID and verify the compiled generator route still names the captured instance. Require NoteOn in [0,N), and matched NoteOff in (on_frame,N]. Match channel, pitch, mixer track and full source descriptor, as well as stable ID. Require a valid, complete note-ID mapping. Reject duplicate IDs, unmatched/mismatched offs, non-finite note values, out-of-range offsets, unclosed notes and capacity overflow before loading a clone. Do not silently skip a selected event.
- Match audio.rs: velocity is max(1, round(clamp(velocity * Clip gain, 0, 1) * 127)); every same-pitch NoteOn retriggers; only the final active same-pitch binding emits NoteOff. Channel/Mixer gain is excluded. Keep callback production code unchanged in this slice; implement a small pure offline mapper with paired equivalence fixtures against the existing callback path so the duplicate mapping contract cannot drift unnoticed.
- Bound active note IDs at the existing 512 limit and emitted Timeline MIDI at the existing 96-events-per-Q128 Timeline budget. Preflight the whole selected schedule; an overfull quantum rejects atomically rather than splitting timing or dropping events. Keep the compiler's existing total-event/work budgets. Scan with one event cursor, checking cancellation/deadline during preflight, and one cursor during rendering.
- Use Q128 musical Process calls with prepared maximum 2048. Block offsets are event.frame minus input block start, in 0..127. Supply authoritative transport at every block: input frame index, constant-tempo quarter-note position, playing=true, and the existing 4/4 context. The musical clock continues through fixed post-roll; playing becomes false only for clone cleanup. No realtime bridge lookahead is added.
- Let N be song frames, T the explicit fixed tail, and L the admitted native latency. Retain exactly output interval [L, L+N+T). Native latency alone is trimmed; no realtime bridge/PDC delay is added. Generate ceil(max(N+T+L, N+1)/128)*128 frames, so a NoteOff exactly at N is delivered even for zero tail/latency at a quantum boundary. The extra closing quantum/padding is discarded and is explicitly reported; it is post-roll, never a pre-note warmup. Check all additions/rounding against frame/RIFF/allocation bounds before loading.
- Pin fresh metadata to the captured class/version and supported topology, then pin latency/tail/bus metadata for the run. Conservatively reject capture-versus-fresh metadata differences or any pre/post-Process drift, pending topology/reload, unexpected output MIDI/loss, malformed audio shape or non-finite audio anywhere in the complete reply, including trimmed latency and discarded closing/padding regions. Validate an already received reply before the post-call cancellation/deadline fence, so known malformed data cannot be masked as cancellation. Do not repair/reconfigure/recover the clone to make the render pass.

## Existing APIs that cannot yet prove the strict path

1. plugin_runtime.rs Vst3Backend::load uses Vst3Host::load_plugin_class. process_isolation.rs PluginHostProcess::send_command independently retries a crashed cold LoadPlugin once even when auto_recover_plugins(false) is set. An off-default explicit no-replay load policy is required.
2. PluginHostProcess::exchange serializes and writes synchronously, then starts its response deadline. Large LoadState can block before that timer; stale-line draining is also currently unbounded against continuous noise. The common strict contract must cover serialization/queue/write/read with one absolute deadline, bounded stale/noise handling and supervised termination. A wrapper around the current call does not establish this.
3. Vst3Backend::prepare bundles reset-support, optional stop/reconfigure, LoadState, Start and native revision calls. Add a clone-only controlled setup path or a shared no-op-default checkpoint seam so cancellation/deadline is checked between each operation and each base setter. Real in-flight errors retain precedence over cancellation. No source path changes.
4. IsolatedPluginImpl::process copies only min(reply length, requested length) and zero-pads missing output channels. Add an opt-in strict response path that checks exact declared output channel count, 128 frames per channel, finite samples, transport_applied and no MIDI/loss before accepting/copying audio. Configure the clone with the separately reviewed count-only silent-generator input policy and the declared main mono/stereo output; preserve every declared inactive bus in metadata and SDK storage. Ordinary Process behavior stays unchanged.
5. Vst3Backend::Drop ignores Editor Close/Stop errors. PluginHostProcess::shutdown performs blocking write/flush and unchecked kill/wait/reader-detach cleanup. Neither is checked termination evidence. A consuming clone close/dispose operation must expose checked helper exit/reap and I/O-worker completion, with no silently detached worker or live process beneath success. Unsupported checked disposal refuses the strict adapter.
6. The pure slice now provides the crate-private float32/unity wrapper and shared StereoFrame, preserving the ordinary same-directory create_new staging, sync, atomic rename and ExportControl cancel/commit CAS. The opt-in checked writer reaches a deadline guard after sync and immediately before the CAS, and preserves typed cancellation/deadline causes plus any checked-abort failure and retained path. Ordinary export behavior and its active-VST refusal remain. File I/O is cooperatively cancellable between calls; arbitrary OS filesystem calls cannot be forcibly interrupted by this wrapper. Native teardown must still complete before a real adapter invokes it.

The runtime owner confirmed the minimal shared seam: off-default fresh-helper failure policy, absolute ExchangeDeadline through encode/admission/write/read, owner-only strict shape/finite/transport validation, cloneable cancel/terminal control with no Plugin/COM/Child access, and dispose_checked evidence for child reaping plus I/O-worker joining. Zero output MIDI/loss is renderer policy, not a global restriction on the later tagged worker. Linux needs interruptible/nonblocking pipes; merely moving blocking writes to threads is insufficient. The shared foundation owner is preparing a synchronous owner-thread nonblocking poll pump with cancel wake FD, one outstanding untagged exchange and no initial I/O threads. Two-phase spawn/load retains Child cleanup ownership on failure; unproven reap cannot drop the token or permit safe replacement. This foundation is separately scoped; the pure renderer proceeds independently. This renderer retains the existing untagged wire commands. Platforms without the strict contract must explicitly reject rather than fall back. No new wire command is needed merely to select strict host-side validation; any necessary facade/query change is separately reviewed before implementation.

## Lifetime, cleanup and publication

The caller transfers immutable capture and cancellation control to one background job; the !Send/!Sync clone is created, used and destroyed there. Cancellation stops new musical/setup requests after the current bounded operation. Dropping a result receiver cannot release the job's lease. The worker acknowledges completion only after all clone resources are disposed; real operation/cleanup failures remain visible.

Revised one-shot oracle (approved 2026-10-10): a healthy disposable fresh clone uses checked Stop and reply-free Shutdown followed by proven child reap. It is never reused and has no external MIDI output. This explicitly supersedes the earlier panic/reset128 or Surge256 cleanup requirement for this clone only; no reset audio is sent. Reusable realtime reset behavior is unchanged, and its historical acceptance is not reused as proof of this revised oracle. Both Stop and Shutdown use min(original job deadline, cleanup grace); an expired job sends no new protocol while pipe-close/reap remains mandatory. A failed/poisoned transport receives no musical retry or state reload; supervised disposal must prove termination. If termination cannot be proved, the cleanup owner and job admission remain retained/quarantined; a failure report cannot masquerade as a termination acknowledgment or reopen admission. Cancellation does not skip mandatory disposal; cleanup has its own bounded grace budget and failures prevent publication.

Prefer completing rendering into one bounded stereo allocation, then checked clone cleanup, then the existing staged WAV writer. This avoids exposing a destination while clone teardown can still fail. On cancel/error, discard audio and any temporary file; an existing destination remains byte-identical. The only publication boundary is the existing commit CAS immediately before rename. An accepted cancel before that CAS forbids publication; cancel after commit starts reports too late. Job ownership remains held through final file outcome/cleanup.

## Bounded acceptance tests

- Successful controls precede every injected failure. Typed stand-ins assert exact error category and final ledger stage, not merely any Err or a timeout.
- Mapping: step and Piano Roll repeats, source offsets, swing already compiled to frames, clipped song-end notes, simultaneous off/on ordering, same-pitch overlaps, gain rounding, stable IDs, exact offsets 0/1/127/128, and immutable source bytes/history/dirty state. Test 512/513 active notes and 96/97 emitted events; unsupported cases reject before clone load or file creation.
- Audio oracle: a fake fixed-latency impulse instrument for L=0/1/127/128/129 and non-quantum-aligned song lengths, T=0 and explicit T>0. Assert exact output length, first sample placement, closing NoteOff, discarded padding, unity peak, mono duplication, metadata drift and non-finite/short/extra-channel replies.
- Lifecycle: exact current setup order: Offline -> reset-support query -> captured LoadState -> Start -> native revision -> fresh catalog -> each current base setter -> strict metadata (no positive pre-render Process, reset, editor open or SaveState). Cancel/expire before load, during each setup/base/MIDI/Process/query stage, and after render before cleanup. Assert no later musical request; concurrent real IPC failure wins.
- Transport prerequisite corpus: a helper that never reads a large LoadState; continuous unsolicited/noise output; crash during cold load; malformed Process output; hanging/failing shutdown; kill/reap failure; inherited open stdout/I/O-worker termination. Assert absolute deadline, no replay, no false successful disposal and no early job admission release.
- Cleanup/publication: healthy one-shot Stop/Shutdown/reap, cancellation before/during Stop and before Shutdown, terminal no-protocol cleanup, Stop/disposal errors, dropped receiver, attempted immediate retry while teardown waits, error precedence, cancellation during encoding and at the commit race, write/sync/rename failure, existing and absent destinations, and no temporary-file residue on successful abort; injected removal failure must surface the cleanup error and retained path.

A plugin-free renderer pass proves these mechanics only. Genuine initialized state fidelity, audio audibility/repeatability, platform helper teardown and Windows atomic-output behavior require their own fixed qualification plan. App endpoint/session binding and user review remain later work.


## Later user-facing selected Playlist occurrence conversion

The user requested Playlist automation clips and conversion of Playlist Piano
Roll/MIDI content to audio. Automation is a separate implementation task. The
current pure engine still renders one selected generator from frame zero through
song end; it does not yet implement the occurrence range or Project edit below.

1. Admit one selected, unmuted Pattern Clip occurrence by stable Clip ID, with
   exact Project session/fingerprint, Pattern ID, original start/length/source
   offset, selected channel and instrument instance. Inspect all compiled note
   sources belonging to that occurrence. Reject a Pattern containing other
   sounding channels/instruments rather than silently keeping one channel.
   Reject enabled active automation and configured MIDI routing with the exact
   unsupported item in the UI. A same-instance replacement endpoint is still a
   different source and must reject through the future App control binding.
2. Use the original compiled Timeline to retain source offset, Pattern/Piano
   repeats, swing, clipped note ends, full source identity and stable ordering.
   Filter the exact Clip occurrence, not every instance of its Pattern. Convert
   absolute event frames to a file-local interval with zero at the Clip start.
   Supply absolute song transport to a future clone even though WAV time is
   local. Validate every selected note against the admitted interval. Rounding
   at beat/frame boundaries must be pinned in the placement receipt, so insertion
   never drifts a sample or reconstructs duration from a rounded display value.
3. Explicitly call the first result a dry instrument render before Channel and
   Mixer gain/pan, insert effects, sends and Master effects. Clip gain is already
   encoded into velocity and must not be applied a second time to the inserted
   audio. Reinsert at unity Clip gain and route through the original approved
   channel/mixer path only after an exact no-double-processing policy is reviewed.
   Current immutable opaque state and current native base values do not prove
   historical state at a later Clip start, random reproducibility or earlier
   note/controller context. Qualify those cases explicitly or reject them;
   never hide preroll, source reset/reload, automation omission or preset fallback.
4. Proposed conservative default: fixed tail T=0, described as “cut at the end of
   this clip,” with explicit truncation consent. An optional fixed tail of 0–30
   seconds extends only the inserted audio duration. A future UI must preview
   the extension. Until an explicit song-extension operation is designed, reject
   any chosen tail extending beyond song end; do not silently truncate it or
   auto-grow the Project. Existing content may overlap the new tail, so show
   that fact before conversion. The source MIDI range remains unchanged.
5. Publish a unique managed WAV only after checked clone termination. Stage the
   decoded asset and edit off-Project, then revalidate the exact session/source
   stamp. Apply one atomic undoable command that adds the audio asset/Clip at the
   admitted placement and mutes/retains the original selected MIDI occurrence.
   Keep all original notes, Pattern data, other Pattern occurrences and plugin
   state untouched. Undo removes the audio occurrence and restores the original
   mute state; redo reuses the same retained asset. Do not delete an asset needed
   by undo/redo or another occurrence. Failed/stale insertion leaves the Project,
   history and dirty flag untouched and reports any retained staged file.
6. Acceptance must cover nonzero Clip start/source offset, repeats/swing, shared
   Pattern occurrences, multi-instrument and active-automation refusal, exact
   sample placement, tail overlap/end-of-song policy, existing muted state,
   stale selection/session/endpoint, file/asset-load failure, and atomic undo/redo
   without double playback. Genuine dry audio equivalence and contextual state
   limitations require their own native qualification before enabling the action.

Official FL reference: [Playlist consolidation documentation](https://www.image-line.com/fl-studio-learning-content/fl-studio-online-manual/html/playlist.htm).
FL Playlist consolidation accounts for contextual automation and mutes sources;
Picker Render and replace uses fixed automation values and replaces Pattern
instances. Consolidation normally includes insert effects while excluding Master
effects to avoid double processing. Our first dry, one-occurrence instrument
conversion has the narrower scope above and must not claim contextual FL
consolidation parity. These reference distinctions were supplied by the root's
verified documentation review; no FL behavior is implemented by this checkpoint.

## Pure-slice review correction boundary

The current fake-only job exposes Running, Disposing, CleanupPending and
Terminated status. After a five-second cleanup grace, it retains the same owner
and admission and reports sticky nonterminal diagnostics while continuing
cooperative 250 ms cleanup polling. Completed audio is freed as soon as cleanup
forbids publication. This status remains visible after result-receiver drop;
only the per-job Acquire termination acknowledgment permits terminal receipt and
readmission. Disposal polls receive the absolute grace deadline and must return
promptly, using only nonblocking cleanup progress after expiry. This is bounded
visibility and retained ownership, not proof that the engine can forcibly stop
a blocking OS call. The strict adapter supplies actual deadline/reap guarantees.

Writer errors retain typed Cancelled/Deadline categories and the full checked
abort failure/path chain. Final deadline expiry is checked after sync and before
commit CAS. Native Offline/state/base and the revised one-shot Stop/Shutdown/reap order, plus any real helper
cleanup remain adapter-only acceptance, not evidence from opaque fake methods.


## Strict adapter source stop

The disabled Linux strict adapter is now prepared as unvalidated source under
[its dedicated contract](STRICT_OFFLINE_ADAPTER.md). Source/mock review and
archival gate its first execution. Genuine plugin qualification and App/UI
activation remain separate; the fake-only results above keep their attribution.
