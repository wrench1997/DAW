# Feed real stereo PCM through a source-built VST3 Effect

Build this no-editor test module, then run the explicitly opted-in host tests.
It uses the production helper and public `strict_offline::Client` API, not a
second renderer or fabricated protocol reply. No audio device or interactive
desktop is required. Windows x86_64 MSVC and Linux x86_64 are supported; helper
and fixture architectures must match.

```powershell
# From the repository root, after obtaining the shared CPU build window:
cargo build --locked --all-features --jobs 1 --bin vst3-host-helper
python tests/fixtures/vst3-effect/build_fixture.py --target x86_64-pc-windows-msvc
cargo test --locked --jobs 1 --manifest-path tests/fixtures/vst3-effect/Cargo.toml

# Both isolated packages have checked-in lockfiles; use --locked throughout.
$env:CITRUS_EFFECT_FIXTURE = (Resolve-Path tests/fixtures/vst3-effect/target/bundles/CitrusStereoDelayFixture.vst3).Path
$env:CITRUS_EFFECT_HELPER = (Resolve-Path target/debug/vst3-host-helper.exe).Path
cargo test --locked --jobs 1 --manifest-path tests/fixtures/vst3-effect/host-check/Cargo.toml --test real_effect real_sdk_effect_pcm_state_transport_delay_tail_and_natural_reap -- --ignored --exact --nocapture
```

On Linux, use `--target x86_64-unknown-linux-gnu`, `export` the same two absolute
paths, and omit `.exe`. Rust's normal Unix test harness ignores SIGPIPE, the
strict client's required host precondition. The build script bounds Cargo to
180 seconds; each helper session uses one original 20-second operation deadline
and bounded disposal. The outer test runner should impose its normal process
deadline. Timeout or forced cleanup is failure, not successful SDK acceptance.

The integration target is ignored by default because it needs explicit local
binary inputs. Running that exact ignored target without those inputs **fails**.
It accepts only this fixture's fixed bundle path, with no installed-plugin fallback.
It is not included in the root test suite or release assets.

## The response has real latency and a finite tail

Factory2 reports two audio classes. In the default build index0 remains this Effect:

- Class: `43545253454658440000000000000001`
- Name: `Citrus Stereo Delay Fixture`
- Vendor: `Citrus Studio Test Fixtures`
- Version: `1.0.0`
- Subcategory: `Fx|Delay` (not Instrument)
- One main stereo input, one main stereo output, no event buses, no editor.

Each channel is independent. With initially zero history:

`y[n] = gain × (x[n − 32] + 0.5 × x[n − 160])`

`getLatencySamples()` returns 32. `getTailSamples()` conservatively returns the
last-tap offset 160 **including** the 32-sample latency; the echo is 128 samples
after the latency-aligned direct tap. An impulse at input frame 255 has its last
response at frame 415 and is exactly silent from frame 416. Callers must retain
this convention when interpreting tail/PDC evidence.

Gain parameter 0 is normalized 0..1, default 0.5. Component state is `CE01` followed
by one little-endian f64 (12 bytes). Invalid/nonfinite/out-of-range values are
rejected, not clamped. Delay history is transient and resets on a fresh instance,
state restore, processing setup or deactivation. Stopped zero-frame parameter
flush is supported for normal host state capture. Only static, block-start Gain
changes are supported; later sample offsets are rejected.

## The second class generates MIDI audio through a real FIFO

`43545253454658440000000000000002` is the independent
`Citrus Lookahead MIDI Fixture`, category `Instrument|Synth`. It has no audio
input, one main stereo output, one main 16-channel event input and no editor or
event output. It uses the same vendor/version, Gain/state codec and read-only
probes. Probe1005's input activation is the event bus for this class.

At each sample, NoteOn/NoteOff is applied at its actual sample offset. Each
channel/pitch holds at most one velocity; velocity0 is NoteOff. The fixed-order
sum is `level = sum(velocity_f32 * ((pitch + 1) / 128))`. Undelayed left audio is
`gain_f32 * level`; right is `-0.5 * left`. A real stereo32-sample FIFO delays
this signal. Both reported latency and finite NoteOff tail are32 samples,
including that FIFO. CC120/123 clears the addressed MIDI channel. State restore,
setup and deactivation clear voices/FIFO; zero-frame parameter flush does not
advance it. Realtime source processing and Offline clone processing are supported.

For pitch63/velocity127/Gain0.5, NoteOn at raw frames0 and127, with NoteOff at1
and129, yields left0.25/right−0.125 only at frames32,159,160. The exact production
helper target checks all768 stereo samples, actual layouts/state/probes and
natural helper cleanup:

```powershell
# Same explicit fixture/helper environment as above; no installed fallback.
cargo test --locked --jobs 1 --manifest-path tests/fixtures/vst3-effect/host-check/Cargo.toml --target-dir target --test real_effect real_sdk_instrument_midi_has_actual_q128_lookahead_tail_and_natural_reap -- --ignored --exact --nocapture
```

This real signal is an input for Whole serial-stage/PDC validation. A reported
latency alone, or this isolated fixture pass, does not establish Whole PDC.

## What the tests establish

Eight default-build fixture tests exercise actual SDK Factory2/interface metadata, actual
`IAudioProcessor::process` over Q128 boundaries, independent stereo channels,
activation/processing prerequisites, output-clear detection and state validation,
plus the Instrument's physical FIFO, MIDI offset boundaries and reset/refusal.
The Effect accepts Realtime and Offline only when setup/data modes match. The
additional mode unit checks the full stereo FIR across Q128 boundaries and that
unknown/mismatched modes cannot advance its output, history or probes.

The separate real-helper target captures opaque state through a real source SDK
instance, drops it, cold-loads another source instance, checks exact state/readback,
then loads a fresh production helper. It restores opaque state (Gain 0.25), applies
a non-f32 captured base (`0.75000004_f64`) through `set_parameter_f64`, verifies
its exact f64 readback bits, selects/admits the actual stereo buses and starts
processing. The fixture retains f64 controller/state precision but intentionally
casts gain to f32 in its DSP; the independent PCM expectation models that actual
conversion after checking transport precision, not before sending the base.
Five Q128 blocks are checked sample-for-sample against an independent two-tap
convolution, including nonzero input at frame 127 and values above full scale.
Three silent blocks flush the finite tail; the last block must remain silent.
Actual SDK probes verify every supplied block's transport and cleared output
buffers. Stop and natural helper exit/reap must succeed.

The separate App private-source SDK target additionally captures the actual
stopped Effect worker chain, requires source positive-frame DSP count0 and joins
the original source guard before creating the strict clone. Its current source
uses non-f32 Gain `0.12345678912345678`, exact source/opaque/base readback, a clone
perturbation before f64 restoration, and three Q128 stereo blocks. This Windows
exact target is wired into the same existing 10-minute no-GUI quality step; it
does not replace the independent helper target above, which also runs on Linux.

Read-only probes never send dirty callbacks or modify persisted state:

| ID | Actual SDK observation | Encoding |
| --- | --- | --- |
| 1000 | Nonempty Process calls | clamp(count / 65536, 0, 1) |
| 1001 | Last project sample position | clamp(samples / 16777216, 0, 1) |
| 1002 | Last project quarter-note position | clamp(quarters / 65536, 0, 1) |
| 1003 | Last tempo | clamp(BPM / 1024, 0, 1) |
| 1004 | Last playing flag | 0 or 1 |
| 1005 | Active component + Process + both buses + Offline | 0 or 1 |
| 1006 | Both output buffers initially all zero | 0 or 1 |

The optional `instrument-first` build additionally exposes read-only 1007
(clamped accepted SDK state loads /65536) and 1008 (last accepted state gain). Only actual
successful `IComponent::setState` updates these observations; generic replay,
parameter flush and history reset cannot. They do not enter writable bases or
change CE01/state bytes. Its additional feature-only actual-SDK unit checks this
distinction. A separate unit checks negative/ordinary/beyond-scale real Process
contexts and unchanged raw observations. Private seeded counters only exercise
normalization boundaries, not executed Process or state-load receipts.

This is focused backend evidence. The static serial Whole consumer separately
passed its real-Generator/two-Effect PCM/latency oracle and final local code gates.
Its final Windows cloud suite and real serial SDK target passed at b9 below.
General FX graphs,
non-Master/plug-in automation, arbitrary tails and real NOIRE acceptance remain separate from these
fixture results.

## Verification status

### Disk Save and cold App Open with the default Instrument

The optional `instrument-first` feature reverses the two existing Factory
class entries for a separately built bundle. In the new Q128 candidate it also
marks only that build's Instrument Gain as `kCanAutomate`; the default Effect
metadata and all read-only probes retain their old flags. Actual Factory-created
controllers and host-catalog assertions cover the distinction. The normal App can therefore load
and reopen the Instrument as its default class without a test-only class override
or a copied old runtime identity. The default Effect build/path is unchanged.

```powershell
python -B tests/fixtures/vst3-effect/build_fixture.py --target x86_64-pc-windows-msvc --variant instrument-first
cargo test --locked --jobs 1 --manifest-path tests/fixtures/vst3-effect/Cargo.toml --features instrument-first
python -B -m unittest discover -s tests/fixtures/vst3-effect -p test_build_fixture.py -v
$env:CITRUS_MELODY_FIXTURE = (Resolve-Path tests/fixtures/vst3-effect/target/bundles/CitrusLookaheadMIDIFixture.vst3).Path
$env:CITRUS_MELODY_HELPER = (Resolve-Path target/debug/vst3-host-helper.exe).Path
# Also keep CITRUS_EFFECT_FIXTURE set to the default Effect bundle above.
```

The four App targets share the prefix
`app::whole_song_export_job::melody_workflow_sdk_tests::`:
- `actual_sdk_catalog_play_stop_manual_save_cold_open_genuine_whole_factory_matches_pcm`
- `actual_sdk_catalog_play_stop_manual_save_cold_open_whole_master_matches_pcm`
- `actual_sdk_whole_generator_parameter_q128_automation_matches_full_pcm`
- `actual_sdk_generator_parameter_q128_manual_save_cold_open_genuine_whole_matches_full_pcm`

The third target has passed local CPU qualification, separately from the prior
two-target acceptance below. Published4cffe0b passed quality38893797179 and
preview38893797217:2574 root/20ignored, Quality SDK6 including Q12810.29s/fullPCM,
real PrintWindow and preview release/ZIP/extracted helper; upload=false.
Rebuild the feature bundle before running it. It authors a
Generator Gain lane after cold Open and compares all96,768 stereo output frames
against gain-before-FIFO32 plus two FIR stages and one96-frame crop. Hold points
127/130 and layer end385 exercise Q128 sampling at128/256/512. It does not prove
that new lane's Save/Open persistence; separate255/256/257-frame traces check
tail-write absence. The new SDK passed in11.79s; the rebuilt feature's9 units,
actual default-controller metadata negative control1, feature Clippy,64 directed
cases (19 new), retained genuine7.98s/audit7.99s and Root strict Clippy/no-default/
fmt all passed. The actual two-size Review WGPU also passed1/5.54s and Root
inspected both frames. The first E0046 Backend omission and cursor5PASS/2FAIL remain:
the repaired new tests use valid binary loop/slip inputs and the compiler's
existing2048-frame Tension spans, with no production compiler change. The seven
cursor cases then passed. Logs, actual project/WAV and frozen inputs are under
`generator-q128-automation-source-candidate-01/` in the workflow evidence parent.

Stage the actual `cargo test --no-run --message-format=json` App test executable
and that matching helper into a new private directory as siblings, then run the
executable separately with each selector and
`--exact --ignored --nocapture --test-threads=1`.
Set `CITRUS_MELODY_HELPER` to the staged helper. Do not overwrite the common
`target/debug/deps` helper or a user preview. CI performs this staging explicitly;
the production `current_exe`-sibling helper resolver is unchanged.

The fourth target separately authors its lane before Manual Save and never
recreates it after cold Open. Full disk/cold payload, three initialization ACKs,
actual catalog/current.625 and accepted opaque.625 are checked before genuine
Whole rendering. It passed1/11.81s locally with all96,768 stereo frames and
original/cold3+3 joins. Its legal1500-frame placement restores base at Q1536;
the third target retains385/512 and the .25 first point. The Saved case uses a
.625 first point so stopped legacy dispatch preserves the acknowledged opaque
base. First E0689 and disk-normalization failures remain recorded; only test
type/minimum-window premises changed, not production/model/payload assertions.
All four shared-flow SDK cases and current Root strict Clippy/no-default/fmt
passed. Existing fixture/helper/UI inputs are unchanged; new cloud qualification
is pending. The actual Saved Project16309B and WAV774202B are retained under
`generator-q128-save-cold-open-source-candidate-01/accepted-saved-sdk02-output/`.

All four tests require actual audible callback Play, manual Stop/Save, original worker
Join, fresh App disk Open, accepted opaque-state and generic-ACK checks, then the
complete independent one-bar Whole PCM oracle: 96,000 music +768 fixed-tail frames,
96 native-latency frames cropped once, 96,768 final stereo frames. The genuine
selector uses the unoverridden default Whole production factory, checking both
starters None before every Whole tick and typed Complete after termination ACK
and admission drain. It does not independently audit strict child PIDs. The
retained audit selector keeps its closed real-backend wrappers and false genuine
factory marker; strict3 natural exit0/reap/order/max-live1 and Effect1005=1 remain
that separate case's direct observations. This is device-free SDK evidence, separate from NI,
OS program restart, realtime/offline PDC equivalence and hardware listening.

Current default-factory slice: first local genuine exact1/8.69s and retained audit
exact1/8.47s passed; actual Cargo artifact, strict Root Clippy/no-default and fmt
all exited0 with all phases terminated/reaped and no resource guard. Both actual
WAVs/projects are retained. Production/helper/fixture inputs were unchanged, so
their accepted binaries and fixture evidence were reused. Both exacts now run
explicitly in the existing10-minute Windows CI step with immediate failure
propagation; CI-contract7/actual PowerShell parse passed. First6050d36 cloud
quality38872515281/preview38872515243 failed old clip/identity/catalog cases
before this new SDK gate or release/ZIP. Linux independently passed
fixture8/9/builder11/helper2. Original failures remain recorded. The isolated four-file repair passed276
focused local tests and strict Clippy/no-default/fmt, retaining real failure and
healthy-retirement rules. Clip timing has bounded diagnostics but no proved cause
fix. Repair code `820cddb098969b35c98a5d976220feff340e160d` passed
quality38880415806/preview38880415815 on attempt1:2553 root/19 ignored,
all five explicit Windows App SDK targets, genuine12.64s and audit12.32s each
with complete96,768-frame PCM; strict gates and real eligible PrintWindow passed.
Linux again passed fixture8/9/builder11/helper2. Preview passed372 Python/12
conditional skips, static-CRT release8m25s/ZIP/extracted helper, upload=false.
Local SDK PCM was reused; these are separate actual cloud executions. Evidence: `melody-genuine-whole-source-candidate-01/`
under the workflow record's target directory.

Previous audited acceptance: 8 default/9 feature fixture tests, both strict fixture Clippy
modes, both rebuilt bundles and joint App exact 05 passed. The App exact completed
in 7.94 s with complete 96,768-frame PCM, source joins and three naturally reaped
strict children; both Effects actually reported Offline 1005=1. Three retained SDK
cases, initialization8/Save1/catalog1, Root strict Clippy/no-default and builder11 passed.
Initial cloud preview failed a shipped guide's link to this unshipped README;
the guide now links the exact source instead of adding fixtures to the package.
Quality failed an old immediate receipt-current assertion; its original-deadline
ACK→Ready test repair passed all 11 Effect cases locally with eight threads.
Code `bdf6901418f89fe5d0f4ac763a498c432156dacb` passed
[quality38863519248](https://github.com/wrench1997/DAW/actions/runs/38863519248)
and [preview38863519233](https://github.com/wrench1997/DAW/actions/runs/38863519233)
on attempt 1. Both platforms ran fixture8/9, builder11 and real helper2;
Windows explicitly passed all four App SDK targets, including this joint case
in 12.64 s with the full 96,768-frame oracle. Root2552/18 ignored, strict gates
and eligible real PrintWindow passed. Preview passed Python372/12 conditional
skips, static-CRT/ZIP/extracted helper; upload was disabled.
The four original joint workflow failures and
queue/mode/one-bar/invalid-probe fixture repairs are retained in the
[workflow record](../../../docs/WINDOWS_MELODY_WORKFLOW.md#sdk-melody-play-disk-save-cold-app-open-and-whole-wav-2026-10-16).

### Current Whole Master-automation consumer

The new Whole consumer uses the same real Instrument FIFO and two Effect stages,
without adding fixture DSP or sample-offset plug-in automation. Run both the
retained static oracle and the new Master oracle with the explicit inputs above:

```powershell
cargo test --locked --all-features --jobs 1 --bin citrus-studio plugins::plugin_runtime::whole_serial_sdk_tests::actual_sdk_serial_generator_two_effects_preserves_raw_latency_tail_and_natural_reap -- --ignored --exact --nocapture --test-threads=1
cargo test --locked --all-features --jobs 1 --bin citrus-studio plugins::plugin_runtime::whole_serial_sdk_tests::actual_sdk_serial_generator_two_effects_master_automation_uses_retained_project_frames -- --ignored --exact --nocapture --test-threads=1
```

Both exact targets actually passed locally. The new target checks all1280 retained
stereo frames after96 samples of cumulative native latency, including Master
Volume/Pan aliases, a saved-muted/zero Master becoming audible, a later mute
interval and tail hold at music frame255. Three original sources have DSP0,
state40 and actual guard joins; three strict clones naturally exit0/reap with
max live owner1. Exact f64 Effect bases, one latency crop and one final tanh remain
part of the independent PCM expectation. The same existing10-minute Windows
no-GUI quality step now explicitly runs both targets. Code89ef2cb passed
quality38846965192 and preview38846965091 on their first attempts:2551 root/17
ignored, all19 serial App cases, all7 commit boundaries, strict Clippy/bins and
no-default passed. Windows explicitly executed the private source target and
both Whole SDK targets; Linux newly passed fixture6/real host2. Real eligible
PrintWindow also passed. Preview passed371 Python cases (12 conditional skips),
static-CRT release8m41s, exact ZIP/PE/import/provenance and the extracted helper;
artifact upload was disabled. Source/publication and test receipts are under
`target/backend-reliability-20261014/whole-serial-master-automation-source-candidate-01/`.

### Previously accepted static consumer

Code candidate `b9f27091fbf5dd91e70ee318dcd897434fb5f6cf` passed
quality38834734658 attempt2 and preview38834734476. Windows passed2529 root
cases/16 ignored, all8 serial App cases, strict Clippy/build/no-default, fixture6
and both explicit host targets. The private source and real Whole SDK targets
also ran explicitly and passed; usable Default/session2 PrintWindow actually
passed. Linux fixture6/host2 success is the same-SHA original execution, not a
new attempt2 execution. Preview passed static-CRT release8m53s, ZIP/PE/import
audits and the extracted helper protocol/natural-reap smoke; upload was disabled.
The first quality attempt's unchanged factory mock hit its3s deadline and stays
recorded as FAIL; one same-SHA failed-job rerun passed without changing the
timeout or assertions. Its precise startup cause remains unproven. Full actual
receipts/logs are under `target/backend-reliability-20261014/source123-*`.

The current Whole fixture candidate passed all6 local Windows unit
tests, strict all-target fixture Clippy, the release bundle and matching production
helper build. Both explicit SDK host targets passed: the non-f32 Effect case
checked1280 samples/35 probes, and the new Instrument case checked768 samples with
real32-sample FIFO, state40, finite tail32 and natural helper exit/reap. All owned
native phases ended0 without resource guards. Evidence is in
`target/backend-reliability-20261014/serial-lookahead-instrument-fixture-01/`.
At `8d8e94d1`, Linux quality38829430252 passed all6 fixture tests and both
explicit real helper targets, including the Instrument's768 samples/32-frame FIFO.
Windows App tests failed on a new Whole cancel/join-observation assertion before
its SDK, Clippy/build/no-default or PrintWindow steps. Preview38829430476 failed
on the related App-drop observation, then retained-fixture admission failures;
release/ZIP/helper validation did not execute. The exact candidate is not cloud
accepted. Full logs are under `target/backend-reliability-20261014/source121-*`.

The first repair, `eae18b7`, then failed cloud compilation because the fixture
incorrectly tried to clone `PluginChainControl`; its exclusive consumers are not
Clone. Linux again passed fixture6/host2, but Windows App tests and later gates
did not execute. The corrected test-only repair pins the existing
`PluginControlIdentity`, checks it matches each original control/worker ID, and
reads the same real guard-return witness through cfg(test) helpers. Production,
global collector limits, old weak identity witnesses and failure rules are
unchanged. All8 Serial App cases (including the new scoped foreign-collector
failure case) passed locally in2.01s; old identity2 passed in0.26s, and strict
root Clippy/no-default checks exited0. That corrected API is the version accepted at b9 above.
Evidence: `whole-serial-effects-source-candidate-01/cloudeae18b7-compile-repair-01/`
under the same target directory.
At `b73b57a`, quality38824285643 actually passed: both platforms ran29 Effect
cases and the real non-f32 helper SDK target. Windows additionally ran the App
private-source exact target with Gain0.12345678912345678, sourceDSP0, original
worker joined, opaque40, three exact stereo Q128 blocks and natural clone reap.
Root2497/15 ignored, strict Clippy/build/no-default and eligible real PrintWindow
also passed. Preview38824285597 also completed successfully: the static-CRT
release build took7m32s; exact ZIP/integrity, AMD64 GUI images, OS-only imports and
the extracted helper's sanitized-PATH protocol/natural-reap check all passed.
Artifact upload was disabled, so that upload step was skipped.
These results cover the first Fx class and capture/CAS source, not the new second
Instrument class or current Whole consumer. The earlier local Gain0.375 pass
is retained only as historical evidence.

The first combined candidate `689e0af` ran all29 Effect cases successfully in both Windows jobs; the preview root suite also passed2496/15 ignored. It then failed Root strict Clippy on the new admin-counter `fetch_update` deprecation, before Windows SDK or release/ZIP acceptance. The checked-CAS repair, near-MAX real-worker regression and source/clone precision assertions passed the subsequent `b73b57a` quality run above; the original failure logs remain under `source119-*`.

Earlier Fx-only local Windows: all four fixture tests passed; the x86_64 MSVC release bundle and
matching all-feature production helper built successfully. The first fixture
test compile found ambiguous integer inference in the analytical expectation
loop; its index is now explicitly `usize`, and the corrected run passed all four.
The real-helper target passed: one test, 1280 exact stereo samples, all 35 SDK
probe readbacks, opaque 40-byte host state round-trip, actual latency 32/last tap
160, exact silence from frame 416, and natural helper exit 0/reap. The host's
40-byte state wraps this plugin's 12-byte component payload. Strict fixture
all-target Clippy passed after replacing an indexed analytical test loop with
`iter().enumerate()`; all four tests passed again with unchanged expectations.
The unchanged first-class DSP retains that earlier f32-base evidence; the later
non-f32 precision assertions passed at `b73b57a` as recorded above.
Cloud quality run [38813369205](https://github.com/wrench1997/DAW/actions/runs/38813369205)
at `0c4872c42516a1d6cf7be7f421b1dbb20d7a5589` actually passed on Windows and Linux:
each ran all four SDK tests and the then-current real production-helper host target.
Both earlier receipts cover the f32-base response/state/transport/latency/tail
and natural-reap checks. The sticky-role reload regression also passed in both 27-case
Effect gates. Linux has not run locally. Detailed local terminal logs are under
`target/backend-reliability-20261014/strict-effect-input-source-candidate-01/`.
Full cloud logs and terminal metadata are under the same parent directory with
the `source118-quality-` prefix. No audio device or desktop was used by this target.
