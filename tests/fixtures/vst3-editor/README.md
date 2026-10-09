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

1. Deliberate valid fake protocol replies written by the fixture through Rust,
   Win32 and CRT stdout must appear on stderr, without corrupting real replies.
   This checks all three routes for the fixture's selected toolchain/runtime;
   it does not establish compatibility with every third-party static CRT.
2. Exact VST3 attach, self-resize to 560×400, content-scale and state/generation
   assertions against probes 1000–1003.
3. Real helper-owned HWND, native child/button visibility, nonuniform painted
   pixels, actual button event, parameter 0 = 0.25, begin/value/end callbacks,
   DirtyChanged(true), and a changed rendered caption.
4. Repeated open/focus/close, native title-bar close, rejected invalid/different
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


## Linux standalone editor acceptance

Use an existing X11 desktop or a compositor-provided XWayland DISPLAY. This test
never starts a display server and does not require changing the DAW renderer.
Build on x86_64 Linux with the installed Rust target and normal XCB development
link library. No downloaded VST3 fixture or vendor SDK is used.

```sh
cargo build --locked --bin vst3-host-helper
python3 tests/fixtures/vst3-editor/build_fixture.py --target x86_64-unknown-linux-gnu
python3 scripts/smoke_vst3_editor_linux.py --helper target/debug/vst3-host-helper --interactive
```

Follow the real-window prompts: click the button, switch focus away/back, move
the pointer outside the plugin, press/release Space, and close from the titlebar.
The source-built probes must observe actual mouse and both key transitions;
factory/frame fd callbacks and timers must run on the creating thread. State must
survive replacement and restore; closing must stop frame timers while factory
timers continue. Repeated Open/Close, no-editor rejection, EOF, Shutdown and forced
helper termination are also covered. Keep real before/after screenshots separately
for visual paint review. The script does not synthesize desktop input or claim to
observe pixels. Missing DISPLAY or omitted --interactive returns 77, not a pass.

Linux and Windows acceptance remain separate. A Linux fixture success is neither
real-plugin compatibility nor proof of a Wayland DAW and XWayland plugin working
together. Per-output/fractional scaling and long audio-load tests remain distinct.
