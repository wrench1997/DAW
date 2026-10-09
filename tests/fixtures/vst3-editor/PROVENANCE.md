# Source-only VST3 editor acceptance fixture

Upstream: https://github.com/HelgeSverre/rust-vst3-host

Exact commit: `ed054908cfe057694d8cf037d0c39dfb5eb4c2ca`.

The fixture's `src/lib.rs` derives from upstream `test-plugin/src/lib.rs`. The
upstream MIT license is preserved verbatim in `LICENSE`. No third-party plugin
binary, commercial plugin, installer, or additional agreement is included.

Original upstream source SHA-256 values, before Citrus modifications:

- `test-plugin/src/lib.rs`: `79fc4188adf4049b7a1bb5708bf153f2e79741a8ca6fd1da0a839ad010a3901d`
- `test-plugin/Cargo.toml`: `f9f779e75d35d8419588551f3ac1afeb370190dbc8220c96ffc8c9071d794a9d`
- `LICENSE`: `a65a537295910b776a8b2edb2e7410c3b0e975ca6388994e032c4d1842b4952d`
- `vst3-host/examples/editor_smoke.rs` (consulted for probe encodings; not copied):
  `503e18988fe14b7c607194d295a3f016107a3e200be9de1e14bf6624c1bd41e1`

Citrus modifications:

1. Standalone developer-only package/workspace, `publish = false`, pinned official
   registry `vst3 = 0.3.0` and Windows-only `winapi = 0.3.9`.
2. `no-editor` feature makes the default audio class's `createView` return null.
3. Windows `IPlugView` owns a native panel and a standard, visible Win32 button.
   The button sets Cutoff parameter 0 to exactly 0.25 and publishes the normal
   begin/value/end gesture and `IComponentHandler2::setDirty(true)` callbacks.
   Native window resources are destroyed during detach/drop before module unload.
4. Controller values, revision and component handler are shared with that view;
   upstream IDs and editor attach/size/scale probes are preserved.
5. Source-build script produces two local bundles and SHA-256 receipts. Receipts
   detect stale/wrong artifacts; they are not cryptographic builder attestation.
6. A narrow newer-Clippy style-lint allowance preserves upstream state parsing
   unchanged; non-Windows native-editor shared fields allow dead-code warnings.
7. Windows attach deliberately writes valid fake protocol replies to stdout through
   Rust, direct Win32 standard-handle writes, and the target C runtime. The harness
   requires all three markers on stderr and unaffected real protocol replies. This
   checks accidental stdout routing, not isolation against a hostile native plugin.

The original fixture is lifecycle-complete but draws nothing. The lifecycle
assertions and the Citrus-added native drawing/interaction assertions are separate
evidence levels. Neither establishes commercial-plugin compatibility, DAW audio
quality, full app UI correctness, or clean-machine release readiness.

This folder is not a root Cargo workspace member, application dependency, or
release asset. Do not copy it or its locally built bundles into shipping packages.
