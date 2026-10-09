# Linux native VST3 editor compatibility

## Scope

The production isolated helper owns a standalone X11 container and attaches the
real vendor IPlugView using `X11EmbedWindowID`. On a Wayland desktop the system's
XWayland server can provide that window. A DAW already running on Wayland does not
have to change renderer or backend. There is no cross-protocol embedding, forged
XID, shared process GUI, compositor launch, or new display server in the app.

The desktop must provide a working `DISPLAY`. Native Linux Wayland-only IPlugView
support is not implemented. The generic parameter editor remains available when
a plugin lacks an editor or native window creation fails. Windows keeps its existing
HWND/PID validation and logical owner-loss behavior.

## Lifecycle and protocol

- Linux `Open { owner: None }` opens once or requests focus of the existing window.
  A Windows owner payload is rejected. Repeated focus/open does not attach a second
  view. Generation increases only after a new successful attachment.
- `Query` reports actual helper state, including a titlebar close. `supported` means
  the helper implements this lifecycle; it does not promise a usable display.
- WM_DELETE records an intent. The main loop detaches the view before destroying
  the container on close, save/restore, unload/replacement, EOF and Shutdown.
- Display loss, externally destroyed host containers, or failed detach/cleanup
  terminate the isolated helper rather than returning a falsely clean closed state.
  A plugin rebuilding its own child window is handled separately from display loss.
- Existing dirty revisions, native-edit capture, zero-sample DSP flush, tagged state
  receipts and conservative app save barriers apply. Native Open still refuses an
  instance referenced by parameter automation, including stopped/unplaced lanes.
  Native gestures are not written into automation or individual undo operations.

Every plugin operation and native callback runs on the helper main thread. A
separate reader only parses bounded input and backpressures its four-entry queue.
The main thread alternates bounded command and X event batches and services run-loop
work while idle and while transport is stopped. A slow vendor callback can still
stall this helper; isolation is not a realtime or dropout-free guarantee.

## Run loop, focus and geometry

The context supplied to IPluginFactory3 and component/controller initialization
implements IRunLoop. The editor frame has a separate IRunLoop registry. Both service
fd readiness and timers on the creating thread, with bounded registrations and
per-registration identities. Callbacks and COM releases run outside the registry
lock. A callback cannot unregister/re-register another callback and receive its old
queued invocation. Each attachment gets a fresh frame; retired frames remain closed.
Factory callbacks remain active while an editor is closed and are retired before
module unload, including load-failure cleanup. The factory and host context outlive
the objects that require them.

The container negotiates XEmbed with one verified direct child. It handles client
reparent/destruction, mapped flags, activation and focus messages, and real key
press/release forwarding through a focus proxy. The WM grants focus through its
`WM_TAKE_FOCUS` timestamp; a stale queued FocusIn cannot steal focus back. Activation
requests remain subject to window-manager/compositor policy. Wayland owner,
minimize, z-order and activation relationships with the DAW are not promised.

User resizes pass through the plugin's constraints and `onSize`. Plugin-requested
resizeView already calls onSize synchronously, so it is not echoed again; stale WM
geometry is discarded when a plugin request takes precedence. Dimensions are bounded
to 1–16,384 pixels. Fixed-size plugins receive matching WM minimum/maximum hints.

The helper offers the X11 desktop's validated `Xft.dpi` resource as a content scale
and watches resource changes. It falls back to 1.0. It does not invent per-output
Wayland scale or multiply by the DAW scale. Fractional/per-output mixed-DPI behavior
requires actual compositor/plugin acceptance.

## Protocol isolation

Before loading plugin code, Unix helpers require a private close-on-exec duplicate
of writable protocol stdout. stdout is redirected to stderr; missing or aliased
stderr uses a private `/dev/null` sink. Duplication/redirection failure exits before
plugin load. The source fixture exercises Rust, POSIX and C stdout paths. This
protects accidental logging, not hostile code in a native plugin.

## Verification boundaries

The source-built Linux fixture and invocation are documented in
[the fixture README](../tests/fixtures/vst3-editor/README.md). Missing display or
interactive opt-in is an explicit exit-77 unsupported result, never GUI acceptance.

Keep evidence distinct:

1. Source/unit/protocol checks, including reentrant deregistration and fail-closed fd tests
2. Actual source-built fixture mouse/key events, resize/close/reopen and saved state
3. Genuine vendor UI paint, native edits and state, using identified plugin binaries
4. Generated-PCM processing while those windows are active, without a hardware claim
5. An actual Wayland DAW + XWayland plugin session, mixed DPI and real audio devices

Staging helper experiments are development evidence only. They do not replace
retesting the final integrated production helper. The tested source and binary
hashes must accompany any result; no third-party binaries or plugin assets belong
in the application release or source receipt package.

## Current component results

See [the exact production-helper validation](LINUX_VST3_EDITOR_VALIDATION.md).
Native fixture and genuine Surge/Stochas UI/state checks passed on an existing X11
desktop. Continuous Surge PCM remained finite, but native resize caused a measured
346.9 ms request stall. This is a functional preview, not realtime-continuity acceptance.

## References

- [Steinberg Linux run-loop contract](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical%2BDocumentation/Provide%2BA%2BRunloop%2BOn%2BLinux/Index.html)
- [Steinberg Linux editor-host window example](https://github.com/steinbergmedia/vst3_public_sdk/blob/master/samples/vst-hosting/editorhost/source/platform/linux/window.cpp)
- [XEmbed specification](https://specifications.freedesktop.org/xembed/latest-single/)
- [Wayland XWayland architecture](https://wayland.freedesktop.org/docs/book/Xwayland.html)
