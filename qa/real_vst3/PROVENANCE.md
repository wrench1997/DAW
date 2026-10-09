# Publication provenance and normalization

## Evidence identity

All measurements date to 9 October 2026. Initial publication preparation copied
the reviewed portable Surge/Stochas receipts and historical scanner-only receipts.
The integration owner then performed a separate exact-integrated-source scanner
rerun, preserved under `scanner-integrated/`. Only metadata was requested in that
fresh run. No substitute measurements were created. The original raw files remain
outside the repository unchanged.

The input `vst3-validation-scripts-receipts.tar.gz` archive had SHA-256
`8a11f97588cfcf139aaebaf272d352aa79ce17a56b0ac3ec948262f39552e5a2`.
Its original portable inventory had SHA-256
`4aa880025fa0a9729067e7127ba76e3a77ef6c943c50e4aa64516fe65dbdfea1`.
[provenance.json](provenance.json) records the original and published SHA-256 of
each imported file, including edited documentation and adapted scripts.
[CONTENTS-SHA256.txt](CONTENTS-SHA256.txt) is the strict sorted inventory of every
regular file in this QA directory except the inventory itself. The separate
repository guide and the inventory are covered by the containing repository or
preview package, not recursively by their own hashes.

Measurements, JSON event records, XML export and the owned binary
`stochas-qa-pattern.state` are byte-for-byte copies. The state must never undergo
newline normalization or decode/re-encode. It is data created by our deterministic
test and re-exported by the real plugin, not a redistributed executable or factory
preset. Plugin archive/binary hashes identify inputs that are intentionally not
included. Generated audio is likewise omitted.

## Path-only receipt normalization

Only these seven receipt files have path substitutions:

- `receipts/production-runtime.log`
- `receipts/runtime-build-command.json`
- `scanner/actual-scan.log`
- `scanner/compile-command.json`
- `scanner/source-and-hashes.json`
- `scanner-integrated/actual-scan.log`
- `scanner-integrated/scanner-reproduction-build.json`

Semantic publication tokens, when present:

| Publication token | Original location meaning |
| --- | --- |
| `${VALIDATION_ROOT}` | Surge/Stochas validation working directory |
| `${SCANNER_ROOT}` | Historical scanner working directory |
| `${INTEGRATED_SCANNER_ROOT}` | Separate integrated-source scanner rerun directory |
| `${TARGET}` | Matching Cargo target directory |
| `${RUSTC}` | Compiler executable |
| `${NATIVE_LIBRARY_PATH_1}` | First local native linker-library directory |
| `${NATIVE_LIBRARY_PATH_2}` | Second local native linker-library directory |

These tokens are legible historical location placeholders, not claims that the
log used those literal paths and not executable shell commands. Path components
in descriptor IDs and cache JSON are replaced consistently. Numbers, categories,
class IDs, parameters, events, state, errors and timing data are unchanged.
Before publication, a local audit reversed every literal replacement and verified
the original raw SHA-256. The full reversible map is retained only outside this
repository. Public provenance records each original prefix's SHA-256, semantic
token and substitution count, plus original/raw and publication file hashes.
`verify_receipts.py` checks the published hashes and token counts, and checks
byte-for-byte copies against their original hashes. It cannot reconstruct hidden
original paths or independently repeat the private reversal audit.

Hashes inside `scanner/source-and-hashes.json` still identify the original
measured source, executable and raw log bytes. Its raw-log digest will therefore
differ from the normalized publication digest. Both identities are explicit in
`provenance.json`; no historical digest was silently updated to fit an adaptation.
Unshipped executable/rlib names in compile-command templates are historical inputs,
not missing promised payload files. Use the reproduction builder instead of
trying to run those templates directly.

## Measured source versus reproduction source

### Historical runtime

The measured runtime imports exact production modules from
`b57076ad990869f4a421cc816d67409b5c69b694`:

- `src/plugins.rs`: `e9b16f89261b8540d4ef15d5a0f568e7975a32672e83d9deabdb88661718329f`
- `src/plugin_runtime.rs`: `0d818b698c4becabb6982e6c7942f48fff2dff08240e80213f81066abf01dbac`

These match `git show` at that commit. The original checksum record reports
measured `probe.py` as
`27810387c1f4937bbfa362bf66519ffb3f268abec9961771faf5784786c24b0c` and
measured `runtime_probe.rs` as
`2168e341c8ef63ba09af349bccdc7019601b1a15cc83a5b6c6a15b9518019862`.
The later portable versions supplied here have different hashes. They are
reproduction adaptations; the original measured script/source bytes are not
included and the historical executable cannot be attributed to these new bytes.
The original recorded executable hash is retained in `provenance.json`.

`render_probe.py`, `validate.py` and the Stochas scripts are copied unchanged from
the reviewed portable input. `build_runtime_probe.py` is a newly reviewed
reproduction utility: it adds scanner selection, unambiguous rlib selection and
explicit build provenance. It is not the script used for the old measurement.

### Corrected scanner

The original scanner probe source hash is
`ec127640d00fe8fabc083946589ff0b2d3ccc299b962fcd7d9d569552a24f019`.
Its production modules come from
`1c673eb47ace331b4b05d147dd8d29747670d99e`; source hashes are retained in
[scanner/source-and-hashes.json](scanner/source-and-hashes.json).
The publication `scanner_probe.rs` changes only:

1. Absolute production include to `production-src/plugins.rs`, populated using
   `git show` by the builder
2. Fixed plugin directory to `VALIDATION_ROOT/plugins`, or `./plugins` when unset

The measured-source and adapted-source hashes are separate in `provenance.json`.
Assertions and real production-scanner calls are unchanged. The scanner metadata
receipt is still the original result, not fabricated output of this adapted copy.

An independent diff found no changes between the reviewed scanner commit and
integration `e6bd216863f6180c2745de05c6701c6300d41cf2` in `src/plugins.rs`,
`src/plugin_runtime.rs`, `src/bin/vst3-host-helper.rs`, `vendor`, `Cargo.toml`,
`Cargo.lock` or `build.rs`. This establishes relevant source equivalence; it does
not turn the scanner-only load/teardown check into a new integrated DSP test.

### Fresh integrated-source metadata rerun

On 9 October 2026, the integration owner compiled and ran the exact shipped
`scanner_probe.rs` and `build_runtime_probe.py` against production commit
`e6bd216863f6180c2745de05c6701c6300d41cf2`. The measured probe-source SHA-256 is
`45907d40467950c7ebf55b0b674b1724eafb3fc744b9144bae3ae89a3be165ee`, matching this
publication. Its [build receipt](scanner-integrated/scanner-reproduction-build.json)
records production-module, compiler, selected rlib, helper and executable hashes.
The selected dependencies came from the exact Cargo compiler-artifact output,
with serde also checked against the application fingerprint; no newest-file
guessing was used. The freshly built helper has the same `ca91f242...` hash as
the historical helper.

The [actual output](scanner-integrated/actual-scan.log) passes instrument/effect
identity, category, event capabilities and cache round-trip. The
[stderr receipt](scanner-integrated/actual-scan.stderr.log) shows two normal helper
starts/shutdowns. The original scanner receipt remains separate. This fresh run
validates the portable scanner adaptation against the integrated production
source, but requests no DSP, editor, device or downstream MIDI routing.

## Editorial and reproduction boundaries

The receipt guide and reproduction guide are rewritten summaries. They correct
stale references to omitted WAVs/checksum files, explain the archive prerequisite
and distinguish historical runtime versus later metadata-only results. The
original Stochas summary is preserved unchanged, including its chronological
blank-default negative followed by the owned-pattern positive result.

Historical full-suite counts from an external scanner work log are not adopted
as evidence in this bundle; source gates must be reported separately by the
integration owner. No native GUI, hardware, realtime device, Windows runtime,
Harmony Blueprint, downstream DAW MIDI routing or real multi-class plugin result
is created by packaging these files.
