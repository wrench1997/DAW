# Reproduce genuine VST3 checks

These are **Linux x86-64, controlled offline** checks. Python uses only its
standard library. Install the normal Rust/Cargo and native Linux build/runtime
dependencies separately. No plugin, factory content, compiler, compiled probe,
helper executable, audio file or linker shim is supplied. The owned Stochas
test-state data is supplied as evidence; it is not a plugin or factory preset.

## 1. Verify, then make a disposable copy

From the repository root:

```sh
python3 qa/real_vst3/verify_receipts.py
QA_RUN="$(mktemp -d)"
cp -R qa/real_vst3/. "$QA_RUN/"
cd "$QA_RUN"
mkdir -p bin downloads home/config home/data plugins
```

Run all following commands in this copy. Scripts overwrite their corresponding
`receipts/` files and generate new outputs; never run them over the checked-in
historical evidence. The pristine inventory is intentionally no longer valid
after a reproduction. Retain new results separately with their build provenance.
The harness directs its receipts, captured source and HOME/XDG data into this
copy, with Cargo build outputs in the selected external target. This is not a
security sandbox for third-party plugin code. The harness does not install
plugins or edit the application checkout.

## 2. Obtain and verify the genuine plugins

Download archives from the official release pages in [receipts/README.md](receipts/README.md).
Verify their SHA-256 values there before loading them. Retain the original Surge
archive at `downloads/surge-xt-linux-1.3.4-pluginsonly.tar.gz`: `validate.py`
hashes it. Extract only the two VST3 bundles into these exact locations:

```text
plugins/Surge XT.vst3/
plugins/Surge XT Effects.vst3/
```

For MIDI-generator checks, also obtain the official Stochas 1.3.13 Linux archive
and extract `plugins/Stochas.vst3/`. Check the inner binary SHA-256 values in the
receipt guide. Inspect archive paths before extraction and preserve bundle
structure. Do not run an installer, copy into system plugin folders, download
factory assets or open an editor. Downloads/extraction are manual prerequisites;
the scripts never fetch or execute installers.

## 3. Build the matching production helper and dependencies

Choose a separate clean source checkout and an idle debug target directory. Set
`REPO`, `TARGET` and `RUSTC` to their absolute paths. `RUSTC` must be the compiler
used for that target. For the historical runtime test use source
`b57076ad990869f4a421cc816d67409b5c69b694`; for the historical corrected scanner
use `1c673eb47ace331b4b05d147dd8d29747670d99e`.

A fresh debug target can be populated from the chosen source checkout with:

```sh
cargo build --manifest-path "$REPO/Cargo.toml" --locked \
  --no-default-features --features vst3 --bins --target-dir "$TARGET"
```

Use the normal system development libraries required by the repository. The
historical compiler/linker command templates in the receipts record that run,
not portable commands to paste verbatim. Historical local linker shim directories
are not shipped. No exact reproducible compiler/toolchain installation is claimed;
compiler, native libraries and dependency features must match the chosen cache.

If an existing cache contains multiple rlibs for a dependency, the builder refuses
to guess. Pass repeated `--extern NAME=/absolute/path/to/exact.rlib` selections
from the matching build, or use a fresh target. Never build against a target
another process is modifying. `--native-library-path /path/to/libraries` may be
repeated when a normal native link requires it.

The builder captures unmodified `src/plugins.rs` and `src/plugin_runtime.rs`
with `git show`, records their hashes, the exact compiler command, dependency
hashes, compiler version and resulting binary/helper hashes. It invokes `rustc`,
not Cargo, and does not itself run a plugin. A newly built helper is not expected
to have the historical helper's executable hash unless all build inputs match.
Source identity and the new executable hashes must be recorded separately.

## 4. Helper and historical production runtime

```sh
python3 build_runtime_probe.py --repo "$REPO" --target "$TARGET" --rustc "$RUSTC" \
  --probe runtime --ref b57076ad990869f4a421cc816d67409b5c69b694
python3 probe.py > receipts/helper-probe.log
python3 render_probe.py > receipts/render.log
python3 validate.py > receipts/validation-summary.log
env -u DISPLAY -u WAYLAND_DISPLAY VALIDATION_ROOT="$PWD" HOME="$PWD/home" \
  XDG_CONFIG_HOME="$PWD/home/config" XDG_DATA_HOME="$PWD/home/data" \
  bin/runtime_probe > receipts/production-runtime.log 2> receipts/production-runtime.stderr.log
```

Check every exit status and inspect stdout/stderr; stop on failure. `probe.py`
records identity, buses, parameters and units. `render_probe.py` creates the
initialized C4 sample; `validate.py` checks parameters, state, note release,
effect dry/wet and bypass and generates dry/delay WAVs. Program selection returns
an error in the historical receipt and is not claimed as supported.

The runtime probe uses the production scanner and PluginChain. It checks an
empty-chain dry baseline, real instrument, real effect, ordered instrument-to-FX
**audio** chain and shutdown. Its explicit block-result wait makes it an offline
acceptance test, not proof of real-time audio-device deadlines or underrun safety.
The old scanner's Effects-as-Instrument label remains in this historical log;
do not replace it with the later corrected result.

## 5. Stochas default and owned-pattern MIDI output

Using the matching production helper already copied into `bin/`:

```sh
python3 stochas_probe.py > receipts/stochas-probe.log
python3 stochas_pattern.py > receipts/stochas-pattern.log
python3 stochas_tempo.py > receipts/stochas-tempo-check.log
```

The first check records a silent, event-free blank default. The second starts
from genuine SaveState output, validates source-defined host/JUCE containers,
authors four notes, loads that state and requires the actual plugin's re-export
to match. It tests 400 playing blocks and a mid-note stop. The final script checks
60/120 BPM spacing and note balance. No injected notes generate the positive
pattern sequence. See [the historical MIDI findings](receipts/stochas-summary.md).

## 6. Corrected default-class production scanner

Build dependencies for the scanner source revision in its own matching target,
then run:

```sh
python3 build_runtime_probe.py --repo "$REPO" --target "$TARGET" --rustc "$RUSTC" \
  --probe scanner --ref 1c673eb47ace331b4b05d147dd8d29747670d99e
env -u DISPLAY -u WAYLAND_DISPLAY VALIDATION_ROOT="$PWD" HOME="$PWD/home" \
  XDG_CONFIG_HOME="$PWD/home/config" XDG_DATA_HOME="$PWD/home/data" \
  bin/scanner_probe > receipts/scanner-reproduction.log 2> receipts/scanner-reproduction.stderr.log
```

The portable scanner source differs from the measured probe only in production
source/plugin-root lookup; see [PROVENANCE.md](PROVENANCE.md). It imports the actual
production scanner, explicitly selects only the two Surge bundles, asserts
verified identity/category/event capabilities and cache round-trip, and uses the
production helper beside the probe. It requests metadata loading and shutdown,
not audio processing. Main's integration commit
`e6bd216863f6180c2745de05c6701c6300d41cf2` has byte-identical relevant source; use
that explicit `--ref` with matching build inputs to check the integrated source.
Do not silently relabel the historical `1c673eb` output as a new run.
The integration owner performed that separate e6bd216 metadata-only rerun on
9 October 2026 using the exact shipped scanner/build source. Its output and exact
build provenance are preserved under [scanner-integrated/](scanner-integrated/actual-scan.log).

## Boundaries

No command here demonstrates downstream DAW MIDI routing, Harmony Blueprint,
native plugin GUI rendering, physical MIDI/audio hardware, speaker output,
Windows runtime compatibility or device/realtime performance. Real multi-class
plugin selection also remains unvalidated. A new source revision, plugin version
or environment requires a new run, kept separate from these historical receipts.
