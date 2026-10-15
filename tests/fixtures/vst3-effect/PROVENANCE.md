# Source-only Effect fixture

This independent fixture uses the repository's existing MIT VST3 COM fixture
conventions. Its minimal factory/interface boilerplate is adapted from
`../vst3-editor/src/lib.rs`, derived from Helge Sverre's
[`test-plugin/src/lib.rs`](https://github.com/HelgeSverre/rust-vst3-host/blob/ed054908cfe057694d8cf037d0c39dfb5eb4c2ca/test-plugin/src/lib.rs).
The upstream copyright and MIT terms are retained in [LICENSE](LICENSE).

The finite stereo delay DSP, state codec, Effect metadata and no-GUI acceptance
target are Citrus additions. They do not reuse the test instrument's oscillator,
MIDI, editor, advertised-only latency or 4800-sample tail.

The fixture depends on pinned registry `vst3 = 0.3.0`, whose generated SDK
interfaces and COM support are dual MIT/Apache-2.0 licensed. No separately
downloaded Steinberg SDK, commercial plugin, binary or installer is included.
The optional host-check target uses the same local MIT `vst3-host` implementation
as the production helper. Neither package is an application dependency or release
asset. Build output stays in this folder's ignored `target` directories.

`source-build.json` records the command and local output size. It is a build
record, not a signature. The documented acceptance starts with an explicit source
build and accepts only that fixture's fixed bundle path; it has no installed-plugin
fallback.
