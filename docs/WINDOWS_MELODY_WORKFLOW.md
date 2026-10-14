# Windows melody workflow — support and verification

Updated: 2026-10-14 (Asia/Shanghai). Development branch:
`ci/windows-reliability-20261009`; this effort started from `b0d8d5c0`.
Citrus Studio remains a development DAW. The complete melody workflow is not yet qualified.
This document separates implemented behavior, actual local acceptance, failures and untested work.

## Current implementation and export boundaries

| Workflow | Implementation | Actual NOIRE status |
| --- | --- | --- |
| Real-time Master recording | Records the Master audio produced during live playback | Pure was heard and captured locally; not an offline export |
| Selected MIDI occurrence to audio | Experimental Linux/Windows strict clone, capture validation, native publication and history fences | Actual recapture mismatch refuses replacement; original MIDI is retained |
| Whole-song offline WAV | Built-in sources; experimental single-VST full arrangement/static routing/Master automation | Short Pure project passes; original 405-note song is not qualified |

These paths must retain their own receipts and must not be substituted for each other.
The ordinary build explicitly refuses unsupported active-VST offline export.
Experimental VST whole export requires `experimental-fresh-helper` on Linux or Windows.
It currently admits one active VST source and no insert effect slots; additional sources,
unsupported automation and unsupported graphs are rejected rather than silently omitted.
Master volume/pan/mute automation is wired through the complete arrangement path.
A fixed tail window requires explicit review; unknown/infinite tail retains its warning.
Effects, arbitrary graph latency compensation and complete natural tail release are not qualified.

Windows conversion uses an actual overlapped named-pipe helper and native file identity/publication.
It preserves MIDI until verified success, handles cancellation/failure and retains restoration owners.
Removing a platform condition was not treated as a Windows implementation.
Whole export shares the stopped-source capture actor but does not create a conversion history item.
Project/session, Timeline, asset, parameter and save fences remain active during pending work.

## Preservation and evidence policy

The initial uncommitted files were inspected and copied before editing:
`src/main.rs`, `scripts/qa_windows_kontakt.py`, `src/windows_kontakt_qa.rs`.
Their exact originals remain in the local `preexisting-changes` archive.
The two Rust files subsequently received reviewed formatting/window/QA wiring changes;
retaining their originals is not a claim that current Rust bytes never changed.
The original Kontakt script is preserved. No reset, work overwrite, security change or main merge
was performed. Test projects and new output directories were independent of user compositions.

Local evidence paths in this document belong to the original validation machine.
`target/` receipts, native projects, plugin state, PCM and binaries are not source-distribution inputs.
A clean clone is not expected to contain them; their absence is not a test failure or reproduced pass.
The pre-consolidation documents were archived byte-for-byte under
`target/backend-reliability-20261014/pre-push-docs-archive-01` with an exclusive manifest.
No NI opaque payload or private instrument-library download path is included here.

## Actual local plugin and audio results

| Check | Actual observation | Remaining limitation |
| --- | --- | --- |
| Scan/load | Kontakt 8.4.1 metadata and approximately 3-second empty-instance load passed | Empty-instance silence does not prove NOIRE audio |
| Native editor | User saw NOIRE Pure; old build opened, closed and reopened the separate Kontakt editor | Latest build and resize/playback-stall acceptance pending |
| Monitoring | User heard piano and reported normal volume | Specific sound-card model is not confirmed |
| Output identity | `扬声器 (2- CB4ND HW 1/2)`; manufacturer Icon ProAudio | Earlier default NVIDIA U2790B is not the intended output |
| Minimal persistence | Muted default-Kontakt project passed model save/reopen checks | Not NOIRE instrument or native restart verification |
| Pure persistence | Saved13 was normally saved, then cold opened in native14 | Not full internal NI parameter or Felt qualification |
| Song audio | Independent original song has actual Master recording and listening derivatives | These are real-time recordings, not conversion/offline receipts |

Computer Use succeeded for some local input/window observations and failed for particular cached
controls, click geometry and helper captures. These failures do not establish global local unavailability.
Cloud PrintWindow failure does not establish a broken plugin editor; native visibility is checked locally.
No new native window, listening or hardware operation is inferred from backend tests.

## Melody editing changes

Channel Rack layout/step visibility, Piano Roll horizontal and vertical view navigation,
Playlist ruler positioning/clip placement and button feedback have been adjusted in source.
Black/white key geometry, pressed/released state and playback feedback use actual audition state.
Audition cancellation, playback preparation and loop restart wait on their actual acknowledgements.
Existing copy, quantization, velocity, chord, snap and Undo/Redo functionality is retained.
These changes have source/headless regressions; latest-build FL-like interaction and compact layout
still require native use. A passing geometry or egui input test is not a physical playback acceptance.
For established editing behavior see [keyboard editing](PIANO_KEYBOARD_EDITING.md),
[mouse workflow](PIANO_MOUSE_WORKFLOW.md), [ranges/snap](PIANO_RANGES_AND_SNAP.md)
and the [FL parity inventory](FL_STUDIO_PARITY.md).

## Short actual whole-song qualification

Whole run02 used a real source, production App/controller and StrictBackend, with no mock renderer.
Its independent project has two patterns/two placements, two musical beats at 84 BPM and 48 kHz,
MasterVolume `.85 → .35`, and a reviewed three-second fixed tail.
The original Saved13 five-pattern/405-note/160-beat song was not rendered by this test.

The clone returned 1661 Q128 blocks. Natural exit0, concrete reap and destruction preceded typed
Complete/publication; actual source Join and cleanup/input pins passed.
The project, MIDI, history, dirty flag and saved path remained intact; playback remained paused.
The final Float32 stereo WAV contains 212571 frames (4.4285625 seconds).
An independent, pre-frozen analytical audit compared all 425142 final samples.
Maximum absolute error was approximately `2.05e-9` against fixed `2e-6` tolerance; no sample exceeded it.
Float32 bit differences were reported rather than called byte-identical.
All retained samples were finite, clipping count was zero, and all 37 padding frames were zero.
Tail audio was nonzero and the last 100 ms were silent; an internal exact-zero run was measured.
These numerical facts do not certify listening quality, absence of audible clicks or every natural release.
Plugin-reported tail remained unknown and its metadata warning was retained.

Evidence on the original machine:
`target/backend-reliability-20261014/windows-noire-whole-run-02/whole-song-master.wav`,
its immutable run result/terminal receipt and `whole-noire-independent-pcm-audit-01` artifacts.
The short test does not prove latest GUI, cold native save, clip Undo, Felt or physical monitoring.

## Conversion and opaque-state refusal

Actual run07/08 strict clones produced short audio and naturally exited/reaped, but conversion
was refused by `OccurrenceCaptureAttestation::match_recapture` before replacing the original MIDI.
Actual error: `The second actual source capture differs from the consumed render capture`.
In run08 the captures had equal host envelopes and lengths but 177 differing NI-component bytes.
Their source identity, metadata, native generation, dirty revision and generic bases were unchanged;
no original-source positive Process/ResetOrigin call occurred between captures.
A separate no-pump three-capture diagnostic also observed adjacent opaque differences.

The cause and semantic significance of NI serialization differences are not established.
Host generic parameter count zero does not mean NOIRE has no internal settings.
Do not interpret or canonicalize opaque NI fields merely to bypass this admission guard.
Original MIDI/project/state/history and output evidence remain preserved.
Actual NOIRE conversion restoration/Undo/Redo is still unqualified.
Source regressions cover refusal for opaque-only, generic-base-only and combined drift.

## Initial-source failure and retained fresh ownership

Whole run03 failed before source-ready with
`qualification poll/pump deadline or bound exceeded` (actual 55.33 seconds, exit101).
Only 18 observations existed: zero capture, strict clone, Process and output WAV.
Cleanup lacked a concrete successful source Join; later OS process absence does not supply one.
No valid processing-time estimate or full-song budget conclusion can be derived from this run.
Its result, log and terminal receipt are retained; the actual NI cause remains unknown.

Source29/31 fixes retain the exact marked fresh guard across installation queue refusal,
actual callback Installed=false, running/pending retirement, engine loss, session replacement and Drop.
New/Open clearing registries or reusing IDs does not misclassify an unrelated ordinary worker.
Early-return guard Drop hands the actual worker owner to retained polling; Pending never detaches as success.
Actually Joined/panicked terminal owners settle; ordinary legacy behavior is preserved.
Thread-creation failure keeps an unjoined owner and does not report successful cleanup.
Audio teardown is an attempt: timeout-retained callback resources are not claimed destroyed.
Bounded initial diagnostics retain existing call/runtime/owner outcomes without new SDK operations.
No production success predicate or 35/280/300-second budget was widened to hide the failure.

## Executed source checks and current gate

| Source/check | Actual terminal | Scope |
| --- | --- | --- |
| source22 historical complete all-target cohort | 1963 passed, 0 failed; main 1932, helper/protocol 31; five NI ignores | Historical source; not acceptance of later changes |
| source29 clip-glue | 50 passed, 1 failed; all 12 new fresh cases passed | Original full-ring fixture failure retained |
| source30 compile | E0425 /Cargo101 | Diagnostic helper was private in a sibling module; no tests ran |
| source31 corrected clip-glue | 51 passed, 0 failed in 5.30 seconds | Entire corrected group, not whole current suite |
| source31 quality | Strict ordinary/all-feature all-target Clippy, root/include fmt and diff exit0 | Exact 243 source inputs; later changes excluded |
| Private spool/sidecar | 24 +24 selected tests passed; formatted copies passed fmt | Std-only foundations, not root/SDK/native integration |
| source32 compile | Four E0433 errors /Cargo101 | Missing test-only `strict` import; no tests/Clippy ran at that checkpoint |
| source33 regression | Compile passed; strict backend 29 passed /2 failed | Stale standalone IPC assertion and unrecorded Windows mock stdout error |
| source34 complete root all-target cohort | 2015 passed, 0 failed; main 1977, helper 25, protocols 6/5/2; seven explicit local/NI ignores | Actual terminal exit0; unchanged main/protocol results reused later, not native acceptance |
| source34 unfiltered vendor first run | 574 passed, 1 failed | Missing external upstream Dexed metadata fixture; original failure retained |
| source34 vendor single-exclusion rerun | 575 passed, 0 failed; one exact Dexed test filtered; probe has zero tests; exit0 | 574 library +1 legacy helper; not an unfiltered pass |
| source34 fixture | Check exit0; separate fmt-check exit1 | Three formatting blocks in native_windows.rs; original failure and bytes retained |
| source35 completed gates | Fixture check/fmt exit0; Python 259 passed (package 170, extraction 28, editor 61) | Actually executed here; root 2015/vendor 575 still belong to source34 |
| source35 strict all-feature Clippy | exit101: two result_large_err diagnostics | src/bin/strict_metadata_bundle/mod.rs:50/67; Err HostResponse at least 160B; original failure retained |
| source36 current helper/quality | Helper 25 passed, 0 failed; strict all-feature/default Clippy, root/vendor/include fmt and diff all exit0 | Compact internal Failure representation tested; these 25 are not added again to source34's 2015 |
| source36 actual Saved13 load-only | 1 passed, 0 failed; body 0.40s, including compile 18.79s | Reads the intact original project; no SDK/NOIRE instance, native reopen or export |
| source37 full owner | 14 targeted tests passed; strict Clippy exit101 (`new_ret_no_self`) | Original failure retained; model/binding/retention-policy tests, not a real NOIRE worker |
| source38 current owner/quality | 14 passed, 0 failed; body 1.76s; strict all-feature/all-target Clippy, root/include fmt and diff exit0 | Constructor renamed and callback released before App; 248 source inputs pinned, no original-song native claim |
| Private auditors candidate02 | PCM/WAV 35 and state sidecar 28 tests passed | Actual Windows file fixtures; not SDK, writer Join or NOIRE sound acceptance |
| source39 full-source fixture | 8 evidence tests passed; Clippy exit101 with five `cloned_ref_to_slice_refs` findings | Original failure preserved; no native source acceptance |
| source40 source fixture/quality/build | 8 passed; strict all-feature/all-target Clippy, fmt/diff and production helper build exit0 | Borrowed scan path/single-record slices; 249 source inputs frozen |
| source40 full-source native run01 | Actual 28.4428s, exit101; production scan timed out after 5s | Zero source workers/StateLoad/clone/Process/WAV; intact original 405 notes/160 beats and binary/project SHA unchanged |
| source41 pre-commit regression | 34 full-project tests passed, 2 ignored; 35 helper tests passed; Clippy exit101 (`write_literal`) | One format reference in a new diagnostic test; original failure preserved, no NI |
| source42 commit gates | 35 helper tests passed; strict default/all-feature/all-target Clippy, root/include fmt, diff and helper build exit0 | Only the test's dynamic format reference changed; source41's unchanged 34 full-project tests retained, not a whole-suite or native audio acceptance |
| source42 complete source native run02 | Actual 57.10s, exit0; 102 observations, original state acknowledged, one actual source worker Join | Intact 405-note/160-beat model/history/routes/automation and file/binary pins; no listening, new cold save, conversion or WAV acceptance |
| source43 source stream foundations | 115 passed (1.26s); strict Clippy exit101 with five decoder/style/enum findings | Original failure retained; no native throughput or complete-song claim |
| source44 stream foundation fixes | 116 passed (1.27s); strict default/all-feature Clippy, root/include fmt and diff exit0 | 257 source pins; fixed 64×2KiB pool, 64KiB whole operand, healthy backpressure and actual writer Join tested; active streaming sink not installed |
| source45 first complete stream-module gate | 198 passed (1.57s); strict Clippy exit101 with 15 unused-code/style findings | 263 source pins; instance dictionary, state sidecar and stream sink registered as test modules; original failure logs retained, no NI/full-song acceptance |
| source46 foundation commit gates | 200 passed (1.46s); strict root default/all-feature/all-target Clippy, root/include fmt and diff exit0 | Removed the unused FinalSource phase; actual cancel/recovery/same-writer Join regressions; 263 source pins, no NI/full-song acceptance |
| source47 independent full audit source port | 42 passed (1.14s); complete 405-note/810-boundary/782-wire input and 35 file/PCM regressions | Frozen builder bytes retain their original SHA; silent matching music is not NOIRE sound; no actual complete raw/WAV supplied |
| source47 first active stream integration compile | exit101: `E0063`, missing `WavExportOptions.sample_rate` in the full-song entry | 274 source pins; new Rust regressions and NI not run; failure retained and explicit 48k configuration added separately |
| source48 integration Clippy | exit101: unused replay-test helper and raw-reader loop style | Compilation passed; new Rust tests had not run; original failure retained |
| source49 active integration gates | 235 stream +34 full-owner tests passed, 3 explicit native cases ignored; strict default/all-feature/all-target Clippy, root/include fmt and diff exit0 | Default caps, all-kind replay and actual whole-owner Drop/recovery/Join fixtures; separate total auditor 16 passed, no native audio acceptance |
| source50 complete original native run01 | Actual 27.767s, test exit101; production scan again timed out waiting 5s for the helper response | No source/clone/stream/WAV started; complete model/history/592KB original state and binary pins unchanged; no full-song export pass |

The source34 root command actually completed in 805.36 seconds; main tests took 635.87 seconds.
The first source42 runner mistakenly retained the source41 manifest reference and failed its precheck before Cargo started. That failure is preserved; the corrected runner passed. Source42 gates cover its helper/quality and unchanged source41 full-project regressions. Source49 is the current active-integration gate; earlier full-suite results remain attributed to their original source batches.
The vendor failure is `internal::module_info::tests::reads_sdk_generated_bundle_metadata`, which requires the absent external `vendor/test_plugins/Dexed.vst3`; production read returns `Ok(None)`.
The existing [processor domains](PLUGIN_PROCESSOR_DOMAINS.md) documents this single-fixture boundary. That boundary is not evidence of an unfiltered vendor pass or an exact historical command invocation.
Source35 changes only fixture whitespace and the `MB_OK` call's trailing comma; root/vendor source is unchanged. Source34's actual complete results are retained without claiming a new full-suite rerun.
Source35 also verifies 49 historical MIDI payloads (inventory only); Kontakt `--help` exits0 without spawning a plugin.
Source36 changes only the helper's internal Failure representation and preserves wire/query order/admission guards.
The unchanged source34 implementation coverage, source35 fixture/Python and source36 helper checks retain their original attribution. Source38 adds the targeted full-owner regression and current all-feature quality gates; it does not claim a new complete source34-suite execution. Its initial runner referenced the old manifest, so preflight exited1 before any Cargo command; the corrected runner then completed all checks.
None of these checks is new acoustic/NI acceptance.

Source29's isolated full-ring retry passed but did not erase the original group failure or prove its cause.
Source31 additionally inherits unchanged source29 retained-shutdown (7) and pure NOIRE (62) checks; those cohorts were not unnecessarily rerun. Their actual NI cases remained explicitly ignored.
Current exact receipts are `source31-quality-receipt-01.json`, `source32-strict-backend-tests-terminal-01.json`, `source33-strict-backend-tests-terminal-01.json`,
`source34-pre-push-root-all-target-tests-terminal-01.json`, `source34-pre-push-vendor-all-target-tests-terminal-01.json`, its precisely filtered rest receipt,
`source36-pre-push-*-terminal-01.json` and `pure-stream-foundations-quality-03.json` under the local backend directory. Each receipt retains its actual source/stage attribution.
New owner receipts are `source37-owner-tests-terminal-01.json`, its failed Clippy receipt, and `source38-*-terminal-01.json`. Auditor receipts are `independent-full-pcm-audit02-tests-terminal-01.json` and `independent-state-sidecar-audit02-tests-terminal-01.json`.

## Newly applied and remaining full-song foundations

Selected metadata bundle code admits the actual selected Kontakt 8.4.1 factory identity and preserves complete bus/event/latency/tail snapshots, failure fields and query order.
Source-level accounting changes wrapper IPC from 11 to 5 per Q128 while retaining 149 SDK calls.
This is not a measured speed-up; source regressions, current quality and package gates pass, while actual native processing/cancellation timing and full-song feasibility remain unmeasured.
The separate legacy helper explicitly refuses the new request rather than silently handling it.
Vendor replay/provenance and packaging pins are refreshed independently of native acceptance.

The additive full-input loader preserves the complete pinned Saved13 through production Project::load and refuses normalization changes; the short scratch path stays separate.
Original 405 notes, five placements and 160 beats including coda remain intact.
The source routes directly to stable MASTER, channel `.55`, Master base `.7`, global `.7 → .56`.
The driven Master value replaces its base; it must not be multiplied by a second `.7`.
The explicit Windows test module now has a full-project owner using production source initialization, actual callback installation and stopped-source readiness checks. Its model/history image preserves every persisted field and skipped selection/waveform/migration data, allowing only the live source's session runtime status to change.
The owner requires an actual caller-supplied scan descriptor with an exact `Instrument` category token. A retained wrapper keeps the entire owner in failure quarantine on unfinished cleanup; this has no continuation/recovery interface yet and is not successful retirement. Resource completion is distinct from final model equality. Complete images are checked at stage boundaries rather than copied every 8ms while loading.
Source42 native run02 has now initialized the intact original project with a real source instance; complete-song rendering remains unqualified.
The source40 init-only fixture was actually attempted once using private frozen test/helper binaries. It failed in the separate default-instance scan with `Timed out after 5s waiting for helper response (plugin may have hung)`, before any full source worker was constructed. The 28.4428s test duration is not LoadPlugin duration, and NI's own initialization log interval does not prove the whole host load returned. Post-initialization controller/state synchronization, GUI capability, MIDI/program/parameter enumeration, setup and the Windows `plugin_changed`/reply chain remain to be timed. No existing timeout or success predicate was expanded.
The exact outcome is in `windows-noire-full-source-run-01/source-result.json` and its external terminal receipt. Full input/model/history and executable/helper pins passed; cleanup of the absent source resources is not a source worker Join or scan helper reap claim.
Windows helper diagnostics now have an exact process-start opt-in: `CITRUS_VST3_HELPER_LOAD_DIAGNOSTIC=windows-load-response-timing-v1`. Disabled paths retain the original serializer/write/flush, SDK order, retries and deadlines without diagnostic clock, TLS or stderr work. Enabled records correlate common load-handler entry/return, Windows `plugin_changed`, serialization and actual write/flush outcomes using one monotonic load clock. Fixed stderr fields omit paths, state, parameters and audio. Accepted write bytes and successful flush do not prove receipt by the parent. Ten pure diagnostic regressions passed. Actual run02 succeeded: the scan and source handler-entry-to-flush intervals were 3.404s and 2.367s, both 879-byte writes succeeded, and the actual descriptor/source readiness and state acknowledgement passed. This does not explain run01's timeout or establish repeated stability. The actual `cannot resolve resource: resources_ENG` and `lib_ENG` messages remain recorded; no NI resources were edited.
Its ordinary regressions are covered by source34; the explicit source36 actual Saved13 load-only check passes, preserving original notes/state/routing/automation without creating a native instance.

Private bounded spool and opaque sidecar foundations have executed 48 selected tests. They retain bounded storage, partial-write/cancel/unwind state and actual writer Join obligations.
Source44 registers typed control/App/native codecs, complete identity framing and the reusable spool under the test-only observer. It adds borrowed encoding/decoding, inline PCM bits and a whole-operand header budget without changing the default memory collector. Its 116 actual tests include 44,000 synthetic PCM-sized records reusing one buffer; this is not native filesystem/NI throughput proof. Source46 also registers and tests the supplied-instance dictionary, state sidecar and stream sink. Cancellation preserves original allocations and cannot publish success after the same actual writer joins. Source49 adds all 23 live borrowed taps, terminal replay and the full-song driver; the source50 native attempt did not reach streaming or rendering.
The reviewed commit scope is 19 source/test/document/attribute files covering the qualified source49/50 integration and independent auditors. Five new vendor load-phase diagnostic files have not passed formatting, compilation or native checks and remain uncommitted without being reset. Existing ignore rules retain 14 local composition files and target receipts, opaque state, PCM and binaries outside Git; no user work is deleted.
Typed-control replay and streamed observation integration pass their targeted source tests; actual complete-song retirement and audio qualification remain open. The independent state-file audit first ran 23 cases with 11 failures/6 errors; that original result remains. Candidate02 subsequently executed 28 tests successfully, retaining separate Path.stat/fstat baselines and comparing only common identity fields across APIs.
Source49 passes targeted gates for all 23 borrowed taps, whole-owner early Drop/recovery, record-by-record typed replay, raw PCM projection and the original complete-project production driver. Default memory caps and the original immutable prelude are neither increased nor reset. Stream budgets are separate and successful hot-path records avoid owned PCM/RecordKind copies. Source50 actually attempted the full native entry but failed during the separate production scan, before source initialization or streaming. The real complete render, retirement and full-sample audit remain untested.
Complete PCM/WAV auditor candidate02 executed 35 tests successfully. Actual Windows stat/fstat ctime differences are covered without weakening each API's own mutation checks; a short read locks WaveCursor failure and forbids further draining. The independent builder and numerical policy remain unchanged.
These are private file fixtures. They do not certify real SDK identity, source retirement, writer Join or full NOIRE audio.
Old finite observation/reply/state quotas cannot be bypassed by resetting counts or dropping evidence.

## Reproduction and next acceptance

Use private new directories and explicit opt-in ignored QA only after the coordinated desktop/build window is available. Preserve the original project and identify the exact test/helper/source inputs.
Normal source checks can be reproduced independently; real NI tests are intentionally excluded:

```powershell
cargo test --locked --jobs 1 --all-features --all-targets -- --test-threads=1
cargo test --locked --jobs 1 --manifest-path vendor/vst3-host-0.9.0/Cargo.toml --all-features --all-targets -- --test-threads=1 --skip internal::module_info::tests::reads_sdk_generated_bundle_metadata
cargo clippy --locked --jobs 1 --all-features --all-targets -- -D warnings
cargo clippy --locked --jobs 1 --all-targets -- -D warnings
cargo fmt --all -- --check
```

Source33's two failures were the stale `LatencySamples:1` command assertion and
`OSError: [Errno 22] Invalid argument` while the late mock reply was written.
Source34 fixes only the fixture: bundled getters retain their exact order and late write failure
records its actual type/errno/message without claiming delivery or a closed-pipe diagnosis.
Source35's Clippy failure remains historical; source36 helper tests and both strict checks now pass.
Source34 root/filtered vendor and source35 fixture/Python retain their original execution attribution.
The source50 native attempt failed in scanning and produced no complete-song audio. Historical hardware listening and short-export results remain separately attributed; actual project load is not native restore.
Then complete one intact original-song playback/save/cold-reopen/offline flow, checking preset,
notes, parameters, routing, automation, finite PCM, duration, silence, clipping and tail.
Repeat editor open/resize/close during playback locally and verify Icon monitoring explicitly.
Keep failing conversion-state restoration separate from successful short whole export.
Qualify effects/general PDC and Felt only after the supported core flow is dependable.
Related established contracts: [offline workflow](OFFLINE_EXPORT_WORKFLOW.md),
[WAV options](WAV_EXPORT_OPTIONS.md), [plugin restore limits](PLUGIN_STATE_RESTORE_LIMITS.md),
[native editor](NATIVE_VST3_EDITORS.md) and [build/release](BUILD_AND_RELEASE.md).
