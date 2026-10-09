# Windows editor acceptance (developer-only)

This is trusted, MIT-licensed source, not a downloaded VST3 binary. See
[PROVENANCE.md](PROVENANCE.md). Build and run on an interactive Windows desktop
using an already installed Rust toolchain, Python 3.10+ and the desired target:

```powershell
cargo build --locked --bin vst3-host-helper
python tests/fixtures/vst3-editor/build_fixture.py --target x86_64-pc-windows-msvc
python scripts/smoke_vst3_editor.py --helper target/debug/vst3-host-helper.exe
```

The helper and fixture must use the same architecture. The two source-built
bundles stay in this folder's ignored `target/bundles` directory. The harness
requires build receipts matching the current source, lockfile and binary bytes;
it accepts no arbitrary `--plugin` path and has no commercial-plugin fallback.

The gate returns 0 only after all requested checks pass, 1 on failure, and 77
(`UNSUPPORTED / NOT VERIFIED`) when the machine lacks Windows or an accessible,
visible interactive input desktop. A service/session-0 runner cannot supply GUI
acceptance. Skipping this gate is not successful release acceptance.

Evidence is printed in separate stages:

1. Exact VST3 attach, self-resize to 560×400, content-scale and state/generation
   assertions against probes 1000–1003.
2. Real helper-owned HWND, native child/button visibility, nonuniform painted
   pixels, actual button event, parameter 0 = 0.25, begin/value/end callbacks,
   DirtyChanged(true), and a changed rendered caption.
3. Repeated open/focus/close, native title-bar close, rejected invalid/different
   owners, logical owner loss, unload/reload, no-editor capability, stdin EOF,
   Shutdown with stdin open, and forced helper termination with HWND cleanup.

The test owns its helper processes and temporary native owner windows only. It
never scans installed plugins, starts an audio device, changes system settings,
installs tools, or sends input to other applications. It does not use foreground
keyboard/mouse injection. BM_CLICK is posted directly to the verified fixture
button in the helper process. Forced termination simulates a helper crash; it
cannot establish graceful plugin detach after an actual crash.

Pure-Python harness regressions can run on any supported Python platform:

```sh
python -m unittest discover -s scripts -p test_smoke_vst3_editor.py -v
```

Fake helper results validate protocol/error/cleanup logic only. They are never
reported as native GUI validation. App-level dirty/undo/save propagation, audio
stability, multiple real plugins, DPI transitions and clean-package validation
remain distinct acceptance work.

The paint probe uses [Win32 PrintWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-printwindow)
in a disposable capture process with a five-second deadline. This API is
synchronous; the capture process is killed and reaped on timeout. A cleared bitmap
must contain nonuniform RGB pixels, and its RGB hash must change after the native
caption changes. This validates application-rendered control output rather than a
blank view or the mere existence of an HWND. It is not an on-screen screenshot or
a substitute for human visual review. Focus checks inspect the helper thread's
keyboard-focus HWND; they do not demand or bypass foreground-stealing permission.
