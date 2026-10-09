# Reproducing the debug/source-linked tests

This archive is portable evidence plus source harnesses, not a self-contained executable or plugin distribution. Run new tests in a new writable working copy. Keep this extracted receipt archive immutable. Do not rerun `summarize_runs.py` over historical normalized receipts: it would replace historical original raw-log hashes with publication-file hashes.

## Required external inputs

1. A Citrus Studio source checkout at `f68b9a0210e95f1d26ec3c67ca0e389bcdfa798b`, including its locked dependencies and vendor source. Compare its 121 production input files against `receipts/source-snapshot.json`. Earlier f164 attempts require their own exact commit and historical harness; do not claim a current harness reproduces those original bytes.
2. Independently obtained official Linux VST3 bundles: Stochas 1.3.13, Surge XT 1.3.4, and Surge XT Effects 1.3.4. Official release URLs and module SHA256 values are in `receipts/plugin-provenance.json`. Supply them under `$VST3_VALIDATION_ROOT/plugins/` with their recorded bundle names. Obtain any factory resources from the official distributions, not this archive.
3. The user's own generated/re-exported Stochas blank state at `$VST3_VALIDATION_ROOT/receipts/stochas-initial-state.bin` and test-pattern state at `$VST3_VALIDATION_ROOT/receipts/stochas-qa-pattern.state`. The test pattern emits 60/64/67/60 at 6000-frame spacing; the blank state emits no events. This package excludes state payloads and the earlier state builder is not included. Independent provision of these inputs is required before execution.
4. The captured Surge edit state at `inputs/surge-native-edited.state` in the new working directory for the independent fresh-instance test. Its SHA256 must be `104bc8ab0945eda2b2873ac43da12dfff67f1e4cbb4863b6827a220740d35a42`; the harness checks controller value 0.8691863417625427 and component volume −6.27905654907227 dB. It is intentionally not distributed. A regenerated state is a new input, not a byte-identical recreation of the historical state.
5. A compatible Linux x86_64 Rust/Cargo/linker and audio-library development environment. `receipts/build-environment.json` records rustc 1.99.0 (`b940084d7`, 2026-09-28), LLVM 23.1.1, offline/locked/all-features flags, and relevant environment settings. Provision dependency caches before offline building. Machine/scheduler differences can change deadline outcomes.

## Fresh working-directory procedure

Copy only `native_timing_tests.rs`, `prepare_snapshot.py`, `build_and_copy.py`, `run_profiles.sh`, and `summarize_runs.py` from this archive to a new QA work directory. Create its empty `receipts` and `inputs` directories. Provide the external input state above. Set environment variables explicitly; neutral `__...__` path defaults are placeholders, not installed locations.

```sh
export DAW_SOURCE="$HOME/src/citrus-studio-f68b9a0"
export DAW_SOURCE_COMMIT=f68b9a0210e95f1d26ec3c67ca0e389bcdfa798b
export REQUIRED_TIMING_SOURCE_COMMIT="$DAW_SOURCE_COMMIT"
export VST3_VALIDATION_ROOT="$HOME/vst3-validation-inputs"
export CARGO_TARGET_DIR="$HOME/build/callback-debug-target"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
# Supply toolchain PATH, Cargo cache, PKG_CONFIG_PATH, LIBRARY_PATH and TMPDIR
# for this machine. Reserve the target against concurrent builds.
mkdir -p receipts inputs
python3 build_and_copy.py
# Release the target after exact compiler-artifact executables have been copied.
export NATIVE_TIMING_RUN_CONDITION='Declared quiet window; debug/source-linked'
bash run_profiles.sh reproduction-quiet
# The exit code is a result: preserve failures, raw log, and all generated files.
```

The build script creates a disposable source copy, appends the harness, runs the unoptimized Cargo test/build profiles, and copies exact compiler-artifact paths by JSON attribution. It does not pick executables by modification time. It will overwrite only the new working directory's generated `source` and build receipts; never point `DAW_SOURCE` at that disposable destination. Clear any optimization-profile overrides if the goal is the recorded unoptimized profile. Inspect actual `compiler_artifact.profile` values afterward.

For the same-binary CPU-load matrix, do not rebuild. Use a second unique directory and the matrix-only test filter:

```sh
export NATIVE_CPU_LOAD_THREADS=4
export NATIVE_TIMING_RUN_CONDITION='Same binary; four CPU workers per measured segment'
bash run_profiles.sh reproduction-cpu4 native_timing_all_exposed_profiles
```

Each load worker performs bounded numerical work for 1.1 seconds. `NATIVE_TIMING_CASE` optionally filters case labels; leave it unset for the full 16-cell matrix. The runner refuses to overwrite an existing run name, requires the source commit, verifies executable and source-manifest bindings, and records host resources. It writes WAV/state-related evidence locally; do not publish excluded payloads without separate review.

## Historical integrity limitations

Path-normalized source copies have different bytes and hashes from the raw historical files. The environment-variable paths make the neutral defaults irrelevant to a correctly configured new run; still, a new build must generate new bindings and must not reuse historical manifests as though they identify its executable. Bit-identical rebuilds are not claimed.

run001 did not retain its harness source text. run001–003 did not retain their snapshot-builder source files. Their original source/harness/builder hashes, exact executable attribution, raw logs, and measurements remain preserved. The current builder/harness must not be substituted as an exact reconstruction of those missing historical files. Full run004–006 harness/builder snapshots are retained and verified against their raw manifests.

Separate optimized qualification must use separately attributed artifacts and results. No optimized result is included in this archive.
