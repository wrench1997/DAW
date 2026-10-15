# Feed real stereo PCM through a source-built VST3 Effect

Build this no-editor test Effect, then run the explicitly opted-in host test.
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

Factory2 reports one audio class:

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

## What the tests establish

Four fixture tests exercise actual SDK Factory2/interface metadata, actual
`IAudioProcessor::process` over Q128 boundaries, independent stereo channels,
activation/processing prerequisites, output-clear detection and state validation.

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

This is focused backend evidence. Application Insert/Master routing, general FX
chains, whole-song PDC, arbitrary plugin tails and NOIRE remain separate work.

## Verification status

The f64 setter and the revised non-f32 base assertions are source-only changes
awaiting Root's next Windows/Linux cloud run. The passes below cover the earlier
f32-base target, not these new precision assertions. The fixture DSP is unchanged.
The App private-source target previously passed locally with Gain 0.375, source
DSP count 0, the original guard joined and natural clone reap; its new non-f32 source
initialization/restoration assertion is locally UNRUN. The current `effect_`
gate selects 29 cases (two new f64 cases), rather than the earlier 27.

The first combined candidate `689e0af` ran all29 Effect cases successfully in both Windows jobs; the preview root suite also passed2496/15 ignored. It then failed Root strict Clippy on the newly added admin-counter `fetch_update` deprecation, so Windows SDK targets and release/ZIP acceptance were not reached. The checked-CAS repair and new overflow case await their own cloud run; the source/clone precision SDK evidence remains separately required.

Local Windows: all four fixture tests passed; the x86_64 MSVC release bundle and
matching all-feature production helper built successfully. The first fixture
test compile found ambiguous integer inference in the analytical expectation
loop; its index is now explicitly `usize`, and the corrected run passed all four.
The real-helper target passed: one test, 1280 exact stereo samples, all 35 SDK
probe readbacks, opaque 40-byte host state round-trip, actual latency 32/last tap
160, exact silence from frame 416, and natural helper exit 0/reap. The host's
40-byte state wraps this plugin's 12-byte component payload. Strict fixture
all-target Clippy passed after replacing an indexed analytical test loop with
`iter().enumerate()`; all four tests passed again with unchanged expectations.
The unchanged DSP retains that earlier f32-base evidence; the revised precision
assertions still await execution.
Cloud quality run [38813369205](https://github.com/wrench1997/DAW/actions/runs/38813369205)
at `0c4872c42516a1d6cf7be7f421b1dbb20d7a5589` actually passed on Windows and Linux:
each ran all four SDK tests and the then-current real production-helper host target.
Both earlier receipts cover the f32-base response/state/transport/latency/tail
and natural-reap checks. The sticky-role reload regression also passed in both 27-case
Effect gates. Linux has not run locally. Detailed local terminal logs are under
`target/backend-reliability-20261014/strict-effect-input-source-candidate-01/`.
Full cloud logs and terminal metadata are under the same parent directory with
the `source118-quality-` prefix. No audio device or desktop was used by this target.
