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

Factory2 reports two audio classes. Index0 remains this default Effect:

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

Six fixture tests exercise actual SDK Factory2/interface metadata, actual
`IAudioProcessor::process` over Q128 boundaries, independent stereo channels,
activation/processing prerequisites, output-clear detection and state validation,
plus the Instrument's physical FIFO, MIDI offset boundaries and reset/refusal.

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
| 1000 | Nonempty Process calls | count / 65536 |
| 1001 | Last project sample position | samples / 16777216 |
| 1002 | Last project quarter-note position | quarters / 65536 |
| 1003 | Last tempo | BPM / 1024 |
| 1004 | Last playing flag | 0 or 1 |
| 1005 | Active component + Process + both buses + Offline | 0 or 1 |
| 1006 | Both output buffers initially all zero | 0 or 1 |

This is focused backend evidence. The static serial Whole consumer separately
passed its real-Generator/two-Effect PCM/latency oracle and final local code gates.
Its final Windows cloud suite and real serial SDK target passed at b9 below.
General FX graphs,
automation, arbitrary tails and real NOIRE acceptance remain separate from these
fixture results.

## Verification status

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
