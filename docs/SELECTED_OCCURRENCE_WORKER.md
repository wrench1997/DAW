# Dormant selected-occurrence worker integration (Stop A)

Status: source implemented, UNVALIDATED. No new tests or genuine plug-in runs have been executed for this checkpoint. The accepted combined baseline is `fde61cdf5a019477256a7e6b86992054fde16abf`. This is the pure/worker stop in reviewed plan `22a784bc15a7d3415bd88f56eed68dc4d1b524369b9816e2acf69498c435582b`; it does not install an App action/controller.

## Closed request and actual capture

The pure planner compiles the original Project, checks the exact selected occurrence and sole generator use, and creates one immutable request bound to the reviewed canonical snapshot, explicit tail acknowledgment, exact structured source/runtime stamp, original source IDs, absolute A/B and local event schedule. It rejects other placed or latent persisted Pattern uses, including muted/unplaced notes; nonpersisted preview/live/legacy-piano admission remains the later App fence's responsibility.

The stamp contains the App-relative native revision. Raw backend generation/revision evidence comes only from the actual dedicated capture receipt; raw revision is never inferred from the relative value or a job ID. Each completed receipt gains a private token. One copied state/base attestation is retained while the first capture is consumed by the renderer. Final matching requires a distinct second completed receipt, the same immutable request and exact identity, full metadata, generation, raw revision, opaque bytes and base float bits. Reusing the first receipt or merely building another equivalent request fails. Obtaining that second capture after media verification under a held stopped App fence remains a caller ordering requirement; the pure seal does not infer an App fence.

Occurrence preparation uses load identity without cloning stale Project state. Strict restore continues to borrow the actual captured state and uses the captured factory UID. No additional source query, reset, restore, warmup or vendor command is introduced.

## Renderer and publication

Occurrence preparation retains N = B - A, local MIDI and N + T retained frames. Each Process receives absolute A + local block start and matching PPQ; native latency is trimmed exactly once. The absolute signed transport range is checked. The ordinary whole-song entry rejects occurrence captures, preventing accidental full-song rendering from the wrong entry point.

Both entries share the existing checked render/disposal/destructor kernel and first-fault precedence. The ordinary writer remains on its existing path. The new dormant entry publishes through a Linux create-new hard link and returns a separate typed publication result. Destination ownership starts only after link succeeds, independently of ExportControl's earlier file-commit CAS. Post-link errors and caught panic retain destination/stage identity; required parent sync is attempted even after stage-unlink failure. A finalization failure cannot mint a success receipt. Existing destinations are never replaced or deleted by this path.

Generated PCM and the consumed capture are dropped before the render termination acknowledgment permits media verification. The renderer's ExportControl retains its existing final file-CAS semantics; a separate later App insertion cancellation/CommitAccepted gate is still required.

## Verification and retained owner

The verifier checks the producer's exact Float32 RIFF/fmt/fact/data shape and file identity, every raw finite sample, unit-range reload compatibility, exact N + T extent, sample rate and prewrite peak. It does not use the ordinary clamping decoder. A >1 Float32 render may remain on disk but cannot become insertion-capable media. File identity checks detect observed drift; they are not universal protection against a hostile concurrent writer.

Exactly one verifier can be admitted process-wide. A slot is reserved before spawning and retains the same JoinHandle, cancellation token, ticket and bounded path/provenance evidence. Receipt or admission Drop requests cancellation and marks abandonment; it does not detach, release admission or erase the published file. Polling joins only an observed finished handle outside the slot lock. Abandoned polling uses the same slot and returns retained evidence. An OS file read can remain pending; this is not a hard I/O deadline. Worker panic and spawn failure remain explicit and cannot yield a verified receipt.

## Authoritative history preparation

Candidate preparation requires producer-verified media and the matching second-capture seal. It reconstructs an authoritative before-image, changing only target opaque state and existing captured base IDs, then creates the after-image that disables the retained generator, mutes the selected MIDI Clip and appends the AudioAsset/Audio Clip. Original live Project/history remain untouched during preparation. MIDI geometry and all unrelated persisted content remain checked.

The prepared transaction owns three complete images: authoritative before, after and next history snapshot. It reserves history capacity before removal admission, preserves any preceding pending edit as a separate history step, and rechecks exact Project/context plus expected history state before the move-only swap. Undo restores the captured source state/bases and enabled generator; redo restores the disabled source and retained audio path. These pure tests do not exercise actual source reconstruction, callback removal, media registration, playback, save-file I/O or UI.

The future controller must retain the actual factory UID/module/version restoration binding; Project descriptor UID is not reinterpreted as that UID. It must perform stopped/epoch fencing, the second actual capture, separate insertion cancellation, stamped callback removal, retaining source-worker join and asynchronous media/timeline acknowledgment. Those remain Stop B.

## Owned memory ledger and limits

- One shared canonical Project snapshot, at most 64 MiB; comparison streams against it rather than retaining another canonical buffer.
- First captured state C plus one attestation C during render. The occurrence load spec does not retain a stale Project-state copy. After render termination, only the attestation remains until second C arrives; matching drops second C before Project-image preparation.
- Metadata copies have bounded text/list sizes and state/base copies use checked reservations. Immutable request/provenance clones copy handles rather than state or PCM.
- Render PCM is bounded by the existing 24-million-frame limit. It is destroyed before verification owns one raw file buffer plus one decoded stereo buffer; the raw buffer is destroyed before the verified outcome returns.
- Three full Project images are prepared before removal; save/reopen checking is sequenced to reuse the decoded third image. Every Image owns its own state Vec. Explicit added buffers/reservations are bounded, but existing serde, Project clones and allocator behavior are not a universal recoverable-OOM or global-heap guarantee. In particular, the existing state serializer can make a temporary hexadecimal string.

## Source/mock evidence intended for this checkpoint

Authored tests include actual coordinator request/lease/termination and dedicated worker capture-kernel calls with a mock PluginBackend, followed by the asynchronous occurrence renderer, real temporary-directory writer, retained verification join, authoritative candidate and history helper. They cover raw revision 9 versus App-relative 1, absolute A/local MIDI around quantum boundaries, latency trim, two distinct actual capture receipts, exact recapture rejection, cancellation/timeout/helper-fault draining, source/file retention, no-clobber/finalization failures, verification identity/format/headroom faults and owner abandonment/panic.

The join fixture has no actual source worker thread, SDK, plug-in binary, audio device or App controller. Its history checks are pure model moves, not runtime Undo re-instantiation. Source/mock gates and exact linkage have yet to run. Earlier initialized-Surge qualification remains separately archived and does not qualify this new occurrence path.

## Reviewed source/mock result (2026-10-10)

This appendix supersedes the initial unvalidated status above. The bounded dormant Stop A gates have completed, with exact revision attribution. `ea5c05fc` passed feature offline49 and60 old bounce cases, while six new joined fixtures refused a wrong synthetic stable route before capture. The fixture-only correction on `224e0f03` passed those six, runtime77, control24 and planner13; fourteen candidate fixtures then refused another wrong stable route before media. Correcting only that fixture context and asserting its actual Channel route yielded all14 passes on `8826f433`, followed by feature/default export71/media21, all default occurrence27/bounce49/offline15/runtime77/control7, automation10/6/4/1 in both profiles, helper25 and protocols6+5+2.

The feature Clippy failure on `8826f433` records all three diagnostics. Last tested `8504a17` adds documented allowances only to the dormant occurrence entry and three retained publication evidence fields, plus the equivalent test-only pause-gate let-chain. Fresh media21, all three Clippy modes, fmt, ordinary helper build, Python239 and package557 pass. Original failures remain failed records; these results are not represented as one full suite on a single source revision. The inherited vendor deprecation warning remains visible.

The original before/after maps are stable for all750 source files in each of four runs. Artifact binding `3a7de269` retains36 distinct root executable copies,6 current-worktree vendor rlibs,42 dep-info copies and exact verbose compiler/extern provenance. The ordinary helper hash is unchanged; feature and test artifacts have separate identities. No fresh broad vendor, whole-App, negative-compiler or genuine plug-in claim is added.

Actual mock joins and pure history tests establish the stated dormant building blocks and failure ownership, including two distinct capture receipts, absolute A/local MIDI, matched recapture, no-clobber typed media, retained verifier join and authoritative model Undo/Redo. They do not establish the later App fence, receipt-retention UI path, stamped callback removal, retained source-worker shutdown, runtime class restoration or playback/residual-audio behavior. Those remain the separately reviewed Stop B and App qualification gates. No real plug-in, App window or audio device was invoked in this checkpoint.

Receipt wording qualification: original artifact binding `3a7de269` remains preserved; qualified binding `fa74598f` corrects its helper non-launch phrase. The Python239 suite launches the exact ordinary helper in four plugin-free Unix descriptor tests (three successful Editor Query/Shutdown cases and one expected read-only-stdout startup refusal), with no LoadPlugin. Count attribution comes from the unchanged test source plus the unskipped suite; no separate per-process ledger or genuine execution claim. Source/test/link evidence is unchanged.
