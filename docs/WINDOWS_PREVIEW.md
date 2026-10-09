# Windows MSVC developer PREVIEW package

This lane creates an **unsigned developer PREVIEW**, separate from the official
[pinned gnullvm release contract](BUILD_AND_RELEASE.md). It is intended to make
reviewed application/helper builds testable as an extracted package. It does not
assert commercial readiness or replace clean-Windows, GUI, hardware, plugin,
security, signing or license acceptance.

## Inputs and runtime policy

- Rust/Cargo **1.99.0**, installed with official `rustup`; explicit target
  `x86_64-pc-windows-msvc`, `--locked`, all features (`vst2`, `vst3`).
- Optimized `release` profile, `RUSTFLAGS=-C target-feature=+crt-static` and
  `CARGO_INCREMENTAL=0`. The explicit target keeps target CRT flags out of host
  proc-macro builds. Rust documents this option in its
  [linkage reference](https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes).
- The **installed** Visual Studio C++ toolchain on the `windows-2025` hosted
  runner. The wrapper uses the installed `Launch-VsDevShell.ps1` with explicit
  x64 host/target architecture to load its development environment, and pins Cargo's
  linker to that installation. The MSVC toolset/linker version and linker SHA-256,
  Windows SDK version and runner image version are recorded. The hosted image
  remains a moving input, so this is not a hermetic or bit-for-bit build promise.
- Python 3.11+ standard library is only a packaging/test tool, never an application
  runtime dependency. No new executable download source, package registry,
  signing certificate, account or credential is introduced by the scripts.

The main application and `vst3-host-helper.exe` must both be AMD64 PE32+ Windows
GUI executables. The packager parses **normal and delay imports**, rejects unknown
DLLs and rejects dynamic VC runtime or `libunwind.dll` imports. Accepted imports
are an explicit Windows 10/11 OS-component allowlist plus Windows API-set names.
This policy avoids silently relying on a developer's PATH or redistributing
unreviewed DLLs. Investigate a rejected DLL; do not copy DLLs from System32, a
random download or another toolchain to make the test pass. The parser follows
Microsoft's [PE format reference](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format).

The allowlist includes `ComBase.dll`, the Windows Runtime/COM OS component
identified in Microsoft's [WindowsPreallocateStringBuffer requirements](https://learn.microsoft.com/en-us/windows/win32/api/winstring/nf-winstring-windowspreallocatestringbuffer).
A normal/delay-import regression covers this exact name. Dynamic VC runtimes and
unknown DLLs remain rejected; no system DLL is copied into the archive.

The preview includes **no `libunwind.dll`**. Do not mix these two MSVC executables
into the gnullvm release package, which still requires the matching LLVM-MinGW
20260616 runtime. PE import auditing does not discover every `LoadLibrary` call,
third-party plugin dependency, driver or GPU-runtime requirement.

## Running the lane

The workflow `.github/workflows/windows-preview.yml` validates pushes to the
existing independent `ci/windows-reliability-20261009` branch when code, build
inputs, packaging scripts, workflow, licenses or primary package instructions
change. Routine roadmap/work-log-only updates do not start another preview build.
The ordinary Windows quality workflow remains unchanged.

A manual dispatch is also available **after the workflow exists on the default
branch**. It is not the only validation route: branch-scoped push works before a
main merge. Both routes default to **no artifact upload**. A later, explicitly
reviewed change to `UPLOAD_PREVIEW_ARTIFACT` can enable branch uploads; the manual
`upload_preview` input is an alternative after dispatch becomes available. Upload
is limited to the final ZIP and its outer checksum, with seven-day retention.
There is no GitHub Release, tag, installer, deployment, signing or attestation.

The runner performs:

1. Python packaging and helper-harness regression tests.
2. Formatting, locked all-feature/all-target tests, Clippy with denied warnings,
   and a no-default-features check using the pinned MSVC/static-CRT configuration.
3. Optimized all-feature builds of both binary targets.
4. PE/import checks, exact-whitelist packaging, checksums and archive validation.
5. Fresh extraction of the **final ZIP**, followed by the existing bounded
   plugin-free helper protocol smoke with PATH reduced to Windows system paths.
   The smoke has a five-second protocol deadline and a one-minute CI step limit.

The source tree must be clean. Source commit/time and Cargo.lock hash are captured
before building and checked again during packaging. Cargo metadata is filtered
for the MSVC target. Only sanitized package name/version/source, locked checksum,
manifest license expression and selected features enter `DEPENDENCIES.json`.
Raw metadata with local paths stays in ignored build output and is not uploaded.

To run locally on an authorized Windows development machine with PowerShell 7+:

    rustup toolchain install 1.99.0-x86_64-pc-windows-msvc --profile minimal --component rustfmt --component clippy
    python -B -m unittest discover -s scripts -p "test_*.py" -v
    ./scripts/build_windows_preview.ps1
    ./scripts/smoke_windows_preview.ps1

The scripts refuse to overwrite an existing output or extraction directory.
Move an earlier output aside before a new run. They do not delete an existing
build, terminate the DAW, launch the main GUI, load a plugin or access audio/MIDI
hardware.

## Archive contract

The ZIP has one top-level directory:
`Citrus-Studio-<version>-Windows-x64-MSVC-PREVIEW-<commit-prefix>/`.

Its required files are:

- `citrus-studio.exe`, `vst3-host-helper.exe`
- `LICENSE`, `THIRD_PARTY_NOTICES.md`, `README.md`, `DEV_STATE.md`
- `START_HERE_PREVIEW.txt`, `BUILD_PROVENANCE.json`, `DEPENDENCIES.json`
- `SHA256SUMS.txt`
- `docs/FL_STUDIO_PARITY.md`, `docs/BUILD_AND_RELEASE.md`
- `docs/DEVELOPMENT_ROADMAP.md`, `docs/WORK_LOG.md`
- `docs/HISTORICAL_DEV_STATE.md`, `docs/WINDOWS_PREVIEW.md`

The explicitly reviewed optional `docs/PROJECT_MEDIA.md` and
`docs/OFFLINE_EXPORT_WORKFLOW.md`, `docs/AUDIO_SPLIT_FIDELITY.md` and
`docs/WAV_EXPORT_OPTIONS.md` and `docs/MIXER_METERING.md` are included
when present in the source checkout.
This supports their independent implementation branches without requiring an
unrelated code merge to test the packager. Any new
package document must be added explicitly to `SOURCE_FILES` or
`OPTIONAL_SOURCE_FILES`; no directory is copied recursively. Relative Markdown
links are checked against the packaged files and a missing target fails the build.

`BUILD_PROVENANCE.json` records the full source SHA, exact Cargo.lock hash, Rust,
Cargo, MSVC/SDK/runner inputs, target, features, profile, package-document manifest
and the audited import sets of both binaries. It is an engineering record, not a
cryptographic publisher attestation. `SHA256SUMS.txt` hashes every other package
file. The adjacent `.zip.sha256` hashes the final ZIP. Signing or other binary
changes require rebuilding the hashes and rerunning checks.

Entries are sorted with a fixed source-commit-based ZIP timestamp, permissions and
compression settings. Tests establish identical archive bytes for identical
payloads in one Python environment; this is **not** a claim that different MSVC,
SDK, Python/zlib or runner installations reproduce identical application bytes.

The verifier rejects missing, extra, duplicate, traversing, absolute-path,
second-root, symbolic-link, encrypted and oversized entries, as well as corrupted
checksums or mismatched provenance. It validates all entries before extraction:

    python -B scripts/package_windows_preview.py verify <preview.zip> --extract-to <new-directory>

No PDBs, source tree, toolchain, tests, private Cargo metadata, user profiles,
Autosave.citrus, plugin cache, recordings, projects, third-party plugins or
presets are allowed in the archive. Extra files beside the built executables are
ignored rather than copied.

## Licenses and acceptance limits

`LICENSE` and the existing `THIRD_PARTY_NOTICES.md` are shipped unchanged. That
notice file describes a historical gnullvm distribution: its libunwind/MinGW
component statements are **not claims about this MSVC preview**. The generated
start-here notice makes this distinction visible before launch. `DEPENDENCIES.json`
provides the current resolved Cargo graph and license expressions; a manifest
inventory alone does not establish full license compliance or identify every
piece of native linked object code. Before external commercial distribution,
review the actual Rust, embedded crate/font/native code and Microsoft static CRT
redistribution terms and all required notices. This pipeline does not certify
that review.

A green preview run proves packaging checks and the extracted helper's three JSON
responses, error recovery and explicit Shutdown. A hosted runner with sanitized
PATH is **not a clean Windows installation**. Clean Windows 10/11 startup, real
GUI workflows, audio/MIDI devices, real plugins, project recovery/export, malware
scanning, signing/reputation handling and commercial acceptance remain separate
checks. Use copies of projects while evaluating the preview. Do not disable
Windows security controls to run an unsigned build.

## Implementation evidence

The packager has synthetic PE and ZIP regression coverage, including malicious
entry names, duplicate/symlink entries, payload tampering, runtime imports,
metadata sanitization, source-lock changes, documentation links and deterministic
archives. These tests run on Linux without executing any Windows binary.
PowerShell orchestration, actual optimized MSVC binaries and their import tables
must be verified by a Windows workflow run at the integrated source SHA. Do not
turn synthetic/local test success into a claim that a downloadable preview has
already been produced or accepted.
