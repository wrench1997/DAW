# Citrus isolated native-editor extension

## Exact upstream source and license

- Package: `vst3-host 0.9.0`, MIT; original `LICENSE` retained unchanged.
- Official registry archive: <https://static.crates.io/crates/vst3-host/vst3-host-0.9.0.crate>
- SHA-256: `6ec579d54bd13b83c60c1fd8bb756cf234e36ccbfb4833ff756b417e64db7fea`.
  This was checked against the original Citrus `Cargo.lock` registry checksum before extraction.
- Upstream repository: <https://github.com/HelgeSverre/rust-vst3-host>.
- Package `.cargo_vcs_info.json` identifies commit
  `ed054908cfe057694d8cf037d0c39dfb5eb4c2ca`, package directory `vst3-host`.
- The original package sources and manifests are retained. No binary plugin or vendor SDK
  is included. The application manifest explicitly selects this directory with `[patch.crates-io]`.
  The path override is recorded in the root lockfile; upstream version stays 0.9.0.

## Bounded upstream code changes

1. `src/plugin.rs`: additive `IsolatedEditorOwner`, `IsolatedEditorCommand`, and
   `IsolatedEditorState` types; `Plugin::isolated_editor` and an unsupported default internal
   implementation; checked `try_take_parameter_edits`/`try_take_host_notifications` drains so
   transport failure cannot masquerade as no native changes. No forged `WindowHandle` is needed for a helper-owned editor.
2. `src/process_isolation.rs`: additive `HostCommand::Editor` and
   `HostResponse::EditorState` wire variants. Existing variants are unchanged.
3. `src/internal/isolated_plugin_impl.rs`: typed forwarding and snapshot validation,
   updating legacy cached open/size state only after a valid reply; fallible edit/notification
   drains preserve transport and protocol errors. Existing infallible APIs remain unchanged.
4. `src/lib.rs`: re-export the three public types.
5. `src/bin/vst3-host-helper.rs`: the preserved upstream sample helper explicitly rejects the
   new command. The production Citrus helper lives at `src/bin/vst3-host-helper.rs` in the
   repository root and implements the Windows lifecycle; building the dependency's sample
   helper is not a substitute for building or shipping the production helper.

Existing APIs and `CreateGui`/`CloseGui` wire values remain compatible. A new client talking
with an old helper gets a normal protocol error rather than silently assuming GUI support.
The new types reject unknown fields. Native owner identity is data only, validated in the
helper against a live HWND and PID before use. It is logical association, not unsafe pointer
transport or cross-process embedding.

## Maintenance and test boundary

Do not replace this directory with a newer upstream package without porting/reviewing these
changes and rerunning Citrus helper/editor tests. Compare the directory to the exact registry
archive above; only the listed code files, this manifest, and the reviewable `CITRUS.patch` should differ.

The standalone source-only fixture under `tests/fixtures/vst3-editor` has separate provenance.
It is a test input, not a runtime dependency or release payload. Protocol tests do not prove a
real editor rendered. Windows lifecycle and interactive smoke must report their own results;
Linux typechecks and unsupported-desktop outcomes do not establish Windows GUI acceptance.
