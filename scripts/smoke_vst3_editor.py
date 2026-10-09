#!/usr/bin/env python3
"""Strict source-built Windows VST3 editor acceptance; exit 77 means NOT VERIFIED."""

import argparse
import base64
import binascii
from collections import deque
from contextlib import contextmanager
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import struct
import subprocess
import sys
import threading
import time

from smoke_vst3_helper import (
    SmokeError, _event, _remaining, _response, _stderr_reader, _stdout_reader,
)

FIXTURE_ROOT = Path(__file__).resolve().parents[1] / "tests" / "fixtures" / "vst3-editor"
SOURCE_FILES = ("Cargo.toml", "Cargo.lock", "LICENSE", "src/lib.rs", "src/native_windows.rs")
UPSTREAM_COMMIT = "ed054908cfe057694d8cf037d0c39dfb5eb4c2ca"
CONTAINER_CLASS = "CitrusVst3EditorContainerV1"
PANEL_CLASS = "CitrusVst3FixturePanelV1"
STATE_KEYS = {"supported", "has_editor", "open", "width", "height", "generation"}
STDOUT_MARKERS = frozenset(f"CITRUS_FIXTURE_STDOUT_{route}" for route in ("RUST", "WIN32", "CRT"))


class UnsupportedDesktop(SmokeError):
    """No interactive Windows desktop: explicitly not a passed acceptance gate."""


class AcceptanceStages:
    """Record bounded independent evidence without converting any failure into a pass."""

    def __init__(self, report=print):
        self.report = report
        self.results = {}

    @contextmanager
    def stage(self, name):
        try:
            yield
        except (SmokeError, OSError) as error:
            self.results[name] = "FAIL"
            self.report(f"FAIL {name}: {error}")
            raise
        else:
            self.results[name] = "PASS"
            self.report(f"PASS {name}")

    def attempt(self, name, action):
        try:
            with self.stage(name):
                value = action()
            return True, value
        except (SmokeError, OSError):
            return False, None

    def skip(self, name, reason):
        self.results[name] = "SKIP"
        self.report(f"SKIP {name}: {reason}")

    def finish(self, expected):
        for name in expected:
            if name not in self.results:
                self.skip(name, "a required earlier stage failed; dependent actions were not attempted")
        failures = [name for name, result in self.results.items() if result == "FAIL"]
        require(not failures, "Failed native acceptance stages: " + "; ".join(failures))
        require(all(result == "PASS" for result in self.results.values()),
                "Native acceptance contains unverified/skipped stages")


def require(condition, message):
    if not condition:
        raise SmokeError(message)


def bits(value):
    return struct.unpack("<Q", struct.pack("<d", value))[0]


def number(value):
    # lossless_f64 uses JSON numbers for finite values; only ParameterChanges uses bits.
    require(type(value) in (int, float), f"Invalid fixture parameter: {value!r}")
    result = float(value)
    require(math.isfinite(result), "Non-finite fixture parameter")
    return result


def response_payload(response, variant):
    require(isinstance(response, dict) and set(response) == {variant},
            f"Expected {variant}, got {response!r}")
    payload = response[variant]
    require(isinstance(payload, dict), f"Malformed {variant} payload: {payload!r}")
    return payload


def state_response(response, **expected):
    payload = response_payload(response, "EditorState")
    require(set(payload) == {"state"}, f"Malformed EditorState: {payload!r}")
    state = payload["state"]
    require(isinstance(state, dict) and set(state) == STATE_KEYS,
            f"Unexpected editor state fields: {state!r}")
    for field in ("supported", "has_editor", "open"):
        require(type(state[field]) is bool, f"Invalid editor {field}: {state[field]!r}")
    for field in ("width", "height", "generation"):
        require(type(state[field]) is int and state[field] >= 0,
                f"Invalid editor {field}: {state[field]!r}")
    if state["open"]:
        require(state["supported"] and state["has_editor"] and state["width"] > 0
                and state["height"] > 0 and state["generation"] > 0,
                f"Inconsistent open editor state: {state!r}")
    else:
        require(state["width"] == state["height"] == 0,
                f"Closed editor has stale dimensions: {state!r}")
    for field, value in expected.items():
        require(state[field] == value, f"Editor {field}: expected {value!r}, got {state[field]!r}")
    return state


def error_response(response, contains=None):
    payload = response_payload(response, "Error")
    require(set(payload) == {"message"} and isinstance(payload["message"], str) and payload["message"],
            f"Malformed Error: {payload!r}")
    if contains:
        require(contains in payload["message"], f"Unexpected error: {payload['message']}")


def verify_fixture(root, variant):
    """Accept only our named bundle with current source/binary receipt, never an arbitrary VST3."""
    name = {"editor": "CitrusEditorFixture", "no-editor": "CitrusNoEditorFixture"}[variant]
    bundle = root / "target" / "bundles" / f"{name}.vst3"
    receipt_path = bundle / "source-build.json"
    try:
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        require(receipt["schema"] == 1 and receipt["variant"] == variant
                and receipt["upstream_commit"] == UPSTREAM_COMMIT, "Wrong fixture build provenance")
        require(receipt["target"] in ("x86_64-pc-windows-msvc", "x86_64-pc-windows-gnullvm",
                                      "x86_64-pc-windows-gnu"), "Wrong fixture architecture/target")
        require(set(receipt["sources"]) == set(SOURCE_FILES), "Incomplete fixture source receipt")
        for name in SOURCE_FILES:
            actual = hashlib.sha256((root / name).read_bytes()).hexdigest()
            require(receipt["sources"][name] == actual, f"Stale fixture source: {name}; rebuild fixture")
        binary_name = f"Contents/x86_64-win/{bundle.stem}.vst3"
        require(receipt["binary"] == binary_name, "Unexpected fixture binary path")
        binary = bundle / binary_name
        require(hashlib.sha256(binary.read_bytes()).hexdigest() == receipt["binary_sha256"],
                "Fixture binary does not match source-build receipt")
        require(binary.read_bytes()[:2] == b"MZ", "Fixture is not a Windows PE binary")
    except (OSError, KeyError, TypeError, ValueError) as error:
        raise SmokeError(f"Missing/invalid source-built fixture at {receipt_path}: {error}") from error
    return bundle.resolve()


class FixtureDiagnostics(deque):
    """Bound diagnostics while retaining evidence that every stdout route reached stderr."""

    def __init__(self):
        super().__init__(maxlen=8)
        self._markers = set()
        self._carry = ""
        self._lock = threading.Lock()
        self.stdout_rerouted = threading.Event()

    def append(self, chunk):
        with self._lock:
            # readline is capped, so a marker can straddle two bounded chunks.
            scan = self._carry + chunk
            self._markers.update(marker for marker in STDOUT_MARKERS if marker in scan)
            self._carry = scan[-(max(map(len, STDOUT_MARKERS)) - 1):]
            if self._markers == STDOUT_MARKERS:
                self.stdout_rerouted.set()
            super().append(chunk)

    def missing_stdout_markers(self):
        with self._lock:
            return sorted(STDOUT_MARKERS - self._markers)


class Session:
    """One bounded helper process. Fake-process tests exercise this layer without claiming GUI QA."""

    def __init__(self, command, timeout=30.0):
        if not math.isfinite(timeout) or timeout <= 0:
            raise ValueError("timeout must be finite and greater than zero")
        self.deadline = time.monotonic() + timeout
        self.events = queue.Queue(maxsize=16)
        self.diagnostics = FixtureDiagnostics()
        self.stopped = threading.Event()
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.PIPE, text=True, encoding="utf-8",
                                        errors="replace", bufsize=1,
                                        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        self.readers = [
            threading.Thread(target=_stdout_reader,
                             args=(self.process.stdout, self.events, self.stopped), daemon=True),
            threading.Thread(target=_stderr_reader,
                             args=(self.process.stderr, self.diagnostics), daemon=True),
        ]
        for reader in self.readers:
            reader.start()

    def __enter__(self):
        return self

    def send(self, command):
        _remaining(self.deadline, "command write")
        self.process.stdin.write(json.dumps(command, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def request(self, command):
        self.send(command)
        return _response(self.events, self.deadline)

    def editor(self, command, **expected):
        return state_response(self.request({"Editor": {"command": command}}), **expected)

    def assert_stdout_rerouted(self):
        require(self.diagnostics.stdout_rerouted.wait(_remaining(self.deadline, "fixture stdout routing")),
                "Fixture stdout routes did not reach stderr: "
                + ", ".join(self.diagnostics.missing_stdout_markers()))

    def load(self, path, has_editor=True):
        info = response_payload(self.request({"LoadPlugin": {
            "path": str(path), "sample_rate": 48000.0, "block_size": 512,
            "tempo": 120.0, "time_sig_numerator": 4, "time_sig_denominator": 4,
            "class_id": None,
        }}), "PluginInfo")
        require(info.get("name") == "VST3 Host Test Synth", f"Unexpected fixture class: {info!r}")
        require(info.get("has_gui") is has_editor, f"Unexpected fixture editor capability: {info!r}")

    def parameter(self, param_id):
        payload = response_payload(self.request({"GetParameter": {"id": param_id}}), "ParameterValue")
        require(set(payload) == {"value"}, f"Malformed ParameterValue: {payload!r}")
        return number(payload["value"])

    def native_revision(self):
        payload = response_payload(self.request("NativeDirtyRevision"), "NativeDirtyRevision")
        require(set(payload) == {"revision"} and type(payload["revision"]) is int
                and payload["revision"] >= 0, f"Malformed native dirty revision: {payload!r}")
        return payload["revision"]

    def save_state(self):
        payload = response_payload(self.request("SaveState"), "State")
        require(set(payload) == {"data"} and isinstance(payload["data"], str)
                and 0 < len(payload["data"]) <= 65536, "Invalid fixture state payload")
        try:
            decoded = base64.b64decode(payload["data"], validate=True)
        except (binascii.Error, ValueError) as error:
            raise SmokeError("Malformed base64 fixture state") from error
        require(bool(decoded), "Empty fixture state")
        return payload["data"]

    def finish(self, mode="Shutdown"):
        if mode == "EOF":
            self.process.stdin.close()
        elif mode == "crash":
            self.process.kill()
        else:
            self.send("Shutdown")  # Keep stdin open so EOF cannot falsely satisfy this test.
        try:
            code = self.process.wait(timeout=_remaining(self.deadline, mode))
        except subprocess.TimeoutExpired as error:
            raise SmokeError(f"Timed out during {mode}") from error
        if mode != "crash":
            require(code == 0, f"Helper exited with code {code}")
        kind, value = _event(self.events, self.deadline, f"stdout close after {mode}")
        require(kind == "eof", f"Unexpected extra stdout after {mode}: {value!r}")

    def __exit__(self, kind, value, traceback):
        cleanup_error = None
        if self.process.poll() is None:
            self.process.kill()
            try:
                self.process.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                cleanup_error = "Could not reap editor helper"
        self.stopped.set()
        deadline = time.monotonic() + 2.0
        for reader in self.readers:
            reader.join(timeout=max(0, deadline - time.monotonic()))
        if any(reader.is_alive() for reader in self.readers):
            cleanup_error = "Editor helper output reader did not stop"
        for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
            try:
                stream.close()
            except (BrokenPipeError, OSError):
                pass
        if cleanup_error:
            raise SmokeError(cleanup_error)
        if kind is not None:
            tail = "".join(self.diagnostics).strip()
            raise SmokeError(f"{value}\nHelper stderr (tail):\n{tail}") from value
        return False


class WindowsDesktop:
    """Bounded ctypes calls against only harness/helper-owned HWNDs; no global input injection."""

    def __init__(self):
        if os.name != "nt":
            raise UnsupportedDesktop("Windows interactive desktop required")
        from ctypes import wintypes as w
        self.w = w
        self.user = ctypes.WinDLL("user32", use_last_error=True)
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.gdi = ctypes.WinDLL("gdi32", use_last_error=True)
        self.enum_proc = ctypes.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
        class GuiThreadInfo(ctypes.Structure):
            _fields_ = [("cbSize", w.DWORD), ("flags", w.DWORD),
                        ("hwndActive", w.HWND), ("hwndFocus", w.HWND),
                        ("hwndCapture", w.HWND), ("hwndMenuOwner", w.HWND),
                        ("hwndMoveSize", w.HWND), ("hwndCaret", w.HWND), ("rcCaret", w.RECT)]
        self.gui_info = GuiThreadInfo
        signatures = {
            "OpenInputDesktop": ([w.DWORD, w.BOOL, w.DWORD], w.HANDLE),
            "CloseDesktop": ([w.HANDLE], w.BOOL),
            "GetThreadDesktop": ([w.DWORD], w.HANDLE),
            "GetProcessWindowStation": ([], w.HANDLE),
            "GetUserObjectInformationW": ([w.HANDLE, ctypes.c_int, w.LPVOID, w.DWORD, ctypes.POINTER(w.DWORD)], w.BOOL),
            "CreateWindowExW": ([w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD, ctypes.c_int, ctypes.c_int,
                                 ctypes.c_int, ctypes.c_int, w.HWND, w.HMENU, w.HINSTANCE, w.LPVOID], w.HWND),
            "DestroyWindow": ([w.HWND], w.BOOL),
            "IsWindow": ([w.HWND], w.BOOL),
            "IsWindowVisible": ([w.HWND], w.BOOL),
            "EnumWindows": ([self.enum_proc, w.LPARAM], w.BOOL),
            "EnumChildWindows": ([w.HWND, self.enum_proc, w.LPARAM], w.BOOL),
            "GetWindowThreadProcessId": ([w.HWND, ctypes.POINTER(w.DWORD)], w.DWORD),
            "GetClassNameW": ([w.HWND, w.LPWSTR, ctypes.c_int], ctypes.c_int),
            "GetDlgItem": ([w.HWND, ctypes.c_int], w.HWND),
            "GetClientRect": ([w.HWND, ctypes.POINTER(w.RECT)], w.BOOL),
            "GetParent": ([w.HWND], w.HWND),
            "IsChild": ([w.HWND, w.HWND], w.BOOL),
            "GetGUIThreadInfo": ([w.DWORD, ctypes.POINTER(self.gui_info)], w.BOOL),
            "PrintWindow": ([w.HWND, w.HDC, w.UINT], w.BOOL),
            "GetDC": ([w.HWND], w.HDC),
            "ReleaseDC": ([w.HWND, w.HDC], ctypes.c_int),
            "PostMessageW": ([w.HWND, w.UINT, w.WPARAM, w.LPARAM], w.BOOL),
            "SendMessageTimeoutW": ([w.HWND, w.UINT, w.WPARAM, w.LPARAM, w.UINT, w.UINT,
                                     ctypes.POINTER(ctypes.c_size_t)], ctypes.c_ssize_t),
            "PeekMessageW": ([ctypes.POINTER(w.MSG), w.HWND, w.UINT, w.UINT, w.UINT], w.BOOL),
            "TranslateMessage": ([ctypes.POINTER(w.MSG)], w.BOOL),
            "DispatchMessageW": ([ctypes.POINTER(w.MSG)], ctypes.c_ssize_t),
        }
        for name, (args, result) in signatures.items():
            function = getattr(self.user, name)
            function.argtypes, function.restype = args, result
        self.kernel.GetCurrentThreadId.restype = w.DWORD
        self.kernel.GetModuleHandleW.argtypes = [w.LPCWSTR]
        self.kernel.GetModuleHandleW.restype = w.HMODULE
        for name, args, result in (
            ("CreateCompatibleDC", [w.HDC], w.HDC),
            ("CreateCompatibleBitmap", [w.HDC, ctypes.c_int, ctypes.c_int], w.HBITMAP),
            ("SelectObject", [w.HDC, w.HGDIOBJ], w.HGDIOBJ),
            ("DeleteObject", [w.HGDIOBJ], w.BOOL),
            ("DeleteDC", [w.HDC], w.BOOL),
            ("PatBlt", [w.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, w.DWORD], w.BOOL),
            ("GetDIBits", [w.HDC, w.HBITMAP, w.UINT, w.UINT, w.LPVOID, w.LPVOID, w.UINT], ctypes.c_int),
        ):
            function = getattr(self.gdi, name)
            function.argtypes, function.restype = args, result
        self.owners = []
        self.previous_dpi_context = None
        self.check_interactive()
        self.set_thread_dpi = getattr(self.user, "SetThreadDpiAwarenessContext", None)
        require(self.set_thread_dpi is not None, "Windows per-monitor DPI API unavailable")
        self.set_thread_dpi.argtypes, self.set_thread_dpi.restype = [w.HANDLE], w.HANDLE
        self.previous_dpi_context = self.set_thread_dpi(ctypes.c_void_p(-4))
        require(bool(self.previous_dpi_context), "Cannot select physical-pixel DPI context for test thread")

    def check_interactive(self):
        desktop = self.user.OpenInputDesktop(0, False, 0x101)
        if not desktop:
            raise UnsupportedDesktop("No accessible interactive input desktop (service/session-0/locked desktop)")
        try:
            # The process's window station must actually be visible, and its thread must
            # already be on the input desktop. Never switch desktops or alter permissions.
            flags = (self.w.DWORD * 3)()
            needed = self.w.DWORD()
            station = self.user.GetProcessWindowStation()
            visible = self.user.GetUserObjectInformationW(station, 1, flags, ctypes.sizeof(flags), ctypes.byref(needed))
            if not visible or not flags[2] & 1:
                raise UnsupportedDesktop("Process window station is not visible")
            names = []
            for handle in (desktop, self.user.GetThreadDesktop(self.kernel.GetCurrentThreadId())):
                name = ctypes.create_unicode_buffer(256)
                if not self.user.GetUserObjectInformationW(handle, 2, name, ctypes.sizeof(name), ctypes.byref(needed)):
                    raise UnsupportedDesktop("Cannot inspect input desktop identity")
                names.append(name.value)
            if names[0] != names[1]:
                raise UnsupportedDesktop("Process is not on the active input desktop")
        finally:
            self.user.CloseDesktop(desktop)

    def owner(self):
        hwnd = self.user.CreateWindowExW(0, "STATIC", "Citrus fixture acceptance owner", 0x10CF0000,
                                        20, 20, 320, 100, None, None,
                                        self.kernel.GetModuleHandleW(None), None)
        require(bool(hwnd), "Failed to create native test owner window")
        self.owners.append(hwnd)
        self.pump()
        return {"window": hwnd, "process_id": os.getpid()}

    def destroy_owner(self, owner):
        hwnd = owner["window"]
        require(bool(self.user.DestroyWindow(hwnd)), "Failed to destroy native test owner")
        self.owners.remove(hwnd)

    def cleanup(self):
        for hwnd in self.owners[:]:
            self.user.DestroyWindow(hwnd)
            self.owners.remove(hwnd)
        if self.previous_dpi_context:
            self.set_thread_dpi(self.previous_dpi_context)
            self.previous_dpi_context = None

    def pump(self):
        msg = self.w.MSG()
        while self.user.PeekMessageW(ctypes.byref(msg), None, 0, 0, 1):
            self.user.TranslateMessage(ctypes.byref(msg))
            self.user.DispatchMessageW(ctypes.byref(msg))

    def wait(self, predicate, deadline, description):
        while True:
            self.pump()
            result = predicate()
            if result:
                return result
            _remaining(deadline, description)
            time.sleep(0.02)

    def pid(self, hwnd):
        pid = self.w.DWORD()
        self.user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        return pid.value

    def class_name(self, hwnd):
        name = ctypes.create_unicode_buffer(256)
        self.user.GetClassNameW(hwnd, name, len(name))
        return name.value

    def windows(self, pid, parent=None):
        found = []
        def visit(hwnd, unused):
            if self.pid(hwnd) == pid:
                found.append(hwnd)
            return True
        callback = self.enum_proc(visit)
        if parent is None:
            self.user.EnumWindows(callback, 0)
        else:
            self.user.EnumChildWindows(parent, callback, 0)
        return found

    def container(self, session):
        matches = [hwnd for hwnd in self.windows(session.process.pid)
                   if self.class_name(hwnd) == CONTAINER_CLASS]
        require(len(matches) <= 1, f"Multiple editor containers: {matches}")
        return matches[0] if matches else None

    def focused_within(self, hwnd):
        info = self.gui_info()
        info.cbSize = ctypes.sizeof(info)
        thread = self.user.GetWindowThreadProcessId(hwnd, None)
        require(bool(self.user.GetGUIThreadInfo(thread, ctypes.byref(info))), "Cannot inspect helper focus")
        # Foreground activation is subject to Windows focus-stealing rules. Assert only
        # the helper's own keyboard-focus target, without attaching input queues.
        return info.hwndFocus == hwnd or bool(self.user.IsChild(hwnd, info.hwndFocus))

    def size(self, hwnd):
        rect = self.w.RECT()
        require(bool(self.user.GetClientRect(hwnd, ctypes.byref(rect))), "GetClientRect failed")
        return rect.right - rect.left, rect.bottom - rect.top

    def send_timeout(self, hwnd, message, wparam=0, lparam=0):
        result = ctypes.c_size_t()
        okay = self.user.SendMessageTimeoutW(hwnd, message, wparam, lparam, 2, 2000, ctypes.byref(result))
        require(bool(okay), f"Native control message {message:#x} failed/timed out")
        return result.value

    def text(self, hwnd):
        text = ctypes.create_unicode_buffer(256)
        self.send_timeout(hwnd, 0x000D, len(text), ctypes.addressof(text))  # WM_GETTEXT
        return text.value

    def painted_pixels(self, hwnd):
        # PrintWindow is synchronous and has no timeout. A disposable Python child owns
        # the GDI capture resources so timeout can kill/reap it without leaking them or
        # freeing a DC while Windows still paints. No HDC is manually sent cross-process.
        source = ("import sys; sys.path.insert(0, sys.argv[1]); "
                  "from smoke_vst3_editor import capture_native_button; "
                  "print(capture_native_button(int(sys.argv[2]), int(sys.argv[3])))")
        try:
            result = subprocess.run([sys.executable, "-B", "-c", source,
                                     str(Path(__file__).resolve().parent), str(hwnd), str(self.pid(hwnd))],
                                    capture_output=True, text=True, timeout=5.0,
                                    creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        except subprocess.TimeoutExpired as error:
            diagnostics = error.stderr or ""
            if isinstance(diagnostics, bytes):
                diagnostics = diagnostics.decode("utf-8", errors="replace")
            raise SmokeError("Native paint capture timed out; capture child terminated\n"
                             + diagnostics[-4000:]) from error
        require(result.returncode == 0, f"Native paint capture failed: {result.stderr[-4000:]}")
        digest = result.stdout.strip()
        require(len(digest) == 64 and all(c in "0123456789abcdef" for c in digest),
                f"Invalid native paint capture result: {digest!r}")
        return digest

    def _print_window_probe(self, hwnd, memory, flags):
        # PrintWindow does not document a GetLastError contract. Clear stale error
        # state and record it only as advisory evidence, never as success/failure.
        ctypes.set_last_error(0)
        succeeded = bool(self.user.PrintWindow(hwnd, memory, flags))
        error = ctypes.get_last_error()
        return {"flags": flags, "succeeded": succeeded, "last_error_advisory": error}

    def _diagnose_print_failure(self, hwnd, memory):
        """Failure-only probes; none can satisfy the original paint assertion.

        They share the capture child's existing five-second kill/reap deadline.
        No unrelated HWND, screen pixels, permissions or desktop settings are used.
        """
        def record(probe, **details):
            print("PRINTWINDOW_DIAGNOSTIC " + json.dumps({"probe": probe, **details}),
                  file=sys.stderr, flush=True)

        parent = self.user.GetParent(hwnd)
        record("target", capture_pid=os.getpid(), target_pid=self.pid(hwnd),
               valid=bool(self.user.IsWindow(hwnd)), visible=bool(self.user.IsWindowVisible(hwnd)),
               window_class=self.class_name(hwnd), parent_pid=self.pid(parent),
               parent_class=self.class_name(parent), client_size=self.size(hwnd))
        # The temporary parent is registered for cleanup; destroying it also
        # destroys its same-process standard button, including on probe failure.
        owner = self.owner()
        button = self.user.CreateWindowExW(
            0, "BUTTON", "Citrus diagnostic button", 0x50010000,  # CHILD|VISIBLE|TABSTOP
            0, 0, 256, 48, owner["window"], None, self.kernel.GetModuleHandleW(None), None)
        record("same-process-button-created", succeeded=bool(button))
        if button:
            self.pump()
            for flags in (1, 0):  # PW_CLIENTONLY and documented default whole-window mode.
                record("same-process-button", **self._print_window_probe(button, memory, flags))
        record("target-whole-window", **self._print_window_probe(hwnd, memory, 0))

    def _paint_in_capture_process(self, hwnd):
        width, height = self.size(hwnd)
        require(0 < width <= 1024 and 0 < height <= 1024, "Unbounded control bitmap dimensions")
        screen = self.user.GetDC(None)
        memory = self.gdi.CreateCompatibleDC(screen)
        bitmap = self.gdi.CreateCompatibleBitmap(screen, width, height)
        previous = self.gdi.SelectObject(memory, bitmap) if memory and bitmap else None
        try:
            require(bool(screen and memory and bitmap and previous), "Cannot allocate native paint probe")
            require(bool(self.gdi.PatBlt(memory, 0, 0, width, height, 0x00000042)), "Cannot clear paint probe")
            # PrintWindow performs the OS-supported interprocess rendering request.
            # The caller owns a bounded child-process timeout around this entire method.
            printed = self._print_window_probe(hwnd, memory, 1)
            if not printed["succeeded"]:
                print("PRINTWINDOW_ACCEPTANCE_FAILURE " + json.dumps(printed),
                      file=sys.stderr, flush=True)
                try:
                    self._diagnose_print_failure(hwnd, memory)
                except Exception as error:
                    print(f"PRINTWINDOW_DIAGNOSTIC failed: {error}", file=sys.stderr, flush=True)
            require(printed["succeeded"], "PrintWindow failed; diagnostic probes cannot satisfy acceptance")
            self.gdi.SelectObject(memory, previous)
            previous = None
            header = struct.pack("<IiiHHIIiiII", 40, width, -height, 1, 32, 0, width * height * 4, 0, 0, 0, 0)
            info = ctypes.create_string_buffer(header + b"\0" * 16)
            pixels = ctypes.create_string_buffer(width * height * 4)
            rows = self.gdi.GetDIBits(memory, bitmap, 0, height, pixels, info, 0)
            require(rows == height, "Could not read native painted pixels")
            data = pixels.raw
            colors = {data[i:i + 3] for i in range(0, len(data), 4)}
            require(len(colors) >= 2, "Native fixture control painted a blank/uniform bitmap")
            # The 32-bit DIB alpha byte is unspecified for ordinary GDI painting.
            rgb = b"".join(data[i:i + 3] for i in range(0, len(data), 4))
            return hashlib.sha256(rgb).hexdigest()
        finally:
            if previous:
                self.gdi.SelectObject(memory, previous)
            if bitmap:
                self.gdi.DeleteObject(bitmap)
            if memory:
                self.gdi.DeleteDC(memory)
            if screen:
                self.user.ReleaseDC(None, screen)

    def post(self, hwnd, message):
        require(bool(self.user.PostMessageW(hwnd, message, 0, 0)), "Could not post native test event")

    def gone(self, handles, deadline):
        self.wait(lambda: all(not self.user.IsWindow(hwnd) for hwnd in handles), deadline,
                  "native HWND teardown")


def capture_native_button(hwnd, expected_pid):
    desktop = WindowsDesktop()
    parent = desktop.user.GetParent(hwnd)
    require(expected_pid != os.getpid() and desktop.pid(hwnd) == expected_pid
            and desktop.pid(parent) == expected_pid
            and desktop.class_name(hwnd).lower() == "button"
            and desktop.class_name(parent) == PANEL_CLASS
            and desktop.user.GetDlgItem(parent, 4101) == hwnd,
            "Capture target is not the verified fixture button")
    try:
        return desktop._paint_in_capture_process(hwnd)
    finally:
        desktop.cleanup()


def wait_open(session, desktop):
    def ready():
        state = session.editor("Query", supported=True, has_editor=True, open=True)
        return state if (state["width"], state["height"]) == (560, 400) else None
    return desktop.wait(ready, session.deadline, "plugin self-resize to 560x400")


def assert_handshake(session, desktop):
    state = wait_open(session, desktop)
    hwnd = desktop.wait(lambda: desktop.container(session), session.deadline, "native container creation")
    require(desktop.user.IsWindowVisible(hwnd), "Native container is not visible")
    require(desktop.size(hwnd) == (560, 400), f"Native container client size is {desktop.size(hwnd)!r}")
    require(session.parameter(1000) == 1.0, "Plugin never observed IPlugView::attached")
    require(session.parameter(1001) * 4096 == 560, "Plugin onSize width is not 560")
    require(session.parameter(1002) * 4096 == 400, "Plugin onSize height is not 400")
    scale = session.parameter(1003) * 8
    require(0.25 <= scale <= 8, f"Plugin never received a valid content scale: {scale}")
    get_dpi = getattr(desktop.user, "GetDpiForWindow", None)
    if get_dpi is not None:
        get_dpi.argtypes, get_dpi.restype = [desktop.w.HWND], desktop.w.UINT
        dpi = get_dpi(hwnd)
        require(dpi > 0 and scale == round(dpi / 96 * 256) / 256,
                f"Plugin scale {scale} does not match container DPI {dpi}")
    return state, hwnd


def verified_fixture_button(session, desktop, hwnd, expected=None):
    """Validate the live trusted hierarchy afresh before sending native control events."""
    require(session.process.poll() is None and desktop.container(session) == hwnd
            and desktop.pid(hwnd) == session.process.pid
            and desktop.user.IsWindow(hwnd) and desktop.user.IsWindowVisible(hwnd)
            and desktop.class_name(hwnd) == CONTAINER_CLASS and desktop.size(hwnd) == (560, 400),
            "Native fixture container identity/geometry changed")
    children = desktop.windows(session.process.pid, hwnd)
    panels = [child for child in children if desktop.class_name(child) == PANEL_CLASS]
    require(len(panels) == 1, f"Expected one native fixture panel, got {panels}")
    panel = panels[0]
    require(desktop.pid(panel) == session.process.pid and desktop.user.IsWindow(panel)
            and desktop.user.IsWindowVisible(panel) and desktop.user.GetParent(panel) == hwnd
            and desktop.size(panel) == (560, 400),
            "Plugin child is not correctly attached/resized inside helper container")
    button = desktop.user.GetDlgItem(panel, 4101)
    require(button and desktop.user.IsWindow(button) and desktop.pid(button) == session.process.pid
            and desktop.class_name(button).lower() == "button"
            and desktop.user.GetParent(button) == panel and desktop.user.IsChild(hwnd, button)
            and desktop.size(button) == (256, 48) and desktop.user.IsWindowVisible(button),
            "Real native fixture button identity/geometry is missing or changed")
    handles = [hwnd, panel, button]
    require(expected is None or handles == expected, "Native fixture HWND hierarchy changed before event")
    return handles


def assert_interaction(session, desktop, hwnd, stages):
    handles = verified_fixture_button(session, desktop, hwnd)
    button = handles[-1]
    require(desktop.text(button) == "Set Cutoff to 0.25", "Unexpected native fixture caption")
    before_ok, before = stages.attempt("native paint before edit", lambda: desktop.painted_pixels(button))
    require(session.parameter(0) == 1.0, "Fixture did not start at default Cutoff")
    revision = session.native_revision()
    require(response_payload(session.request("TakeParameterEdits"), "ParameterEdits").get("edits") == [],
            "Unexpected pre-click parameter gestures")
    require(response_payload(session.request("TakeHostNotifications"), "HostNotifications").get("notifications") == [],
            "Unexpected pre-click host notifications")
    # A failed/timed-out pixel capture does not authorize using a stale HWND.
    # Recheck PID, class, ancestry, control ID and exact client sizes immediately
    # before BM_CLICK, even when the earlier capture succeeded.
    verified_fixture_button(session, desktop, hwnd, expected=handles)
    desktop.post(button, 0x00F5)  # BM_CLICK; delivered by real helper GUI message pump.
    desktop.wait(lambda: desktop.text(button) == "Cutoff = 0.25 (edited)", session.deadline,
                 "native button interaction")
    require(session.parameter(0) == 0.25, "Native click did not edit controller Cutoff")
    edits = response_payload(session.request("TakeParameterEdits"), "ParameterEdits").get("edits")
    require(edits == [
        {"id": 0, "kind": "BeginGesture", "value": None},
        {"id": 0, "kind": "ValueChange", "value": 0.25},
        {"id": 0, "kind": "EndGesture", "value": None},
    ], f"Native edit callback sequence mismatch: {edits!r}")
    # Keep stopped-transport DSP edits queued until SaveState's zero-sample flush.
    # Draining the legacy display queue here would deliberately invalidate capture.
    notifications = response_payload(session.request("TakeHostNotifications"), "HostNotifications").get("notifications")
    require(notifications == [{"DirtyChanged": True}], f"Missing native DirtyChanged(true): {notifications!r}")
    require(session.native_revision() > revision, "Native dirty revision did not survive feedback draining")
    verified_fixture_button(session, desktop, hwnd, expected=handles)
    after_ok, after = stages.attempt("native paint after edit", lambda: desktop.painted_pixels(button))
    if before_ok and after_ok:
        stages.attempt("native repaint change", lambda: require(after != before, "Native edited caption did not repaint"))
    else:
        stages.skip("native repaint change", "baseline or edited pixels unavailable; no repaint claim")
    return handles


def assert_state_roundtrip(session, desktop, fixture, handles):
    """Capture the stopped native edit, then restore into a fresh fixture instance."""
    revision = session.native_revision()
    captured = session.save_state()  # Must detach and flush the pending native DSP edit.
    session.editor("Query", supported=True, has_editor=True, open=False)
    desktop.gone(handles, session.deadline)
    require(session.parameter(1000) == 0.0, "SaveState did not detach the native view")
    changes = response_payload(session.request("TakeParameterChanges"), "ParameterChanges").get("changes")
    require(changes == [[0, bits(0.25)]], f"Native parameter-change feedback mismatch: {changes!r}")
    require(session.native_revision() == revision, "Native dirty revision changed during state capture")
    response_payload(session.request("UnloadPlugin"), "Success")
    session.load(fixture)
    require(session.parameter(0) == 1.0, "Fresh fixture did not reset Cutoff before restore")
    response_payload(session.request({"LoadState": {"data": captured, "context": "Project"}}), "Success")
    require(session.parameter(0) == 0.25, "Native Cutoff edit did not survive opaque state restore")
    require(session.parameter(1004) == 0.5, "Fixture did not observe project state context")
    require(session.save_state() == captured, "Fixture component/controller state changed after round-trip")


def exercise_lifecycle(command, fixture, no_editor, desktop, timeout=60.0, report=print):
    """Real acceptance with failed paint retained and independent evidence completed."""
    stages = AcceptanceStages(report)
    expected = (
        "protocol stdout isolation (Rust/Win32/CRT)", "native attach/resize/DPI",
        "native paint before edit", "native control interaction/gesture/dirty feedback",
        "native paint after edit", "native repaint change", "repeat focus and owner rejection",
        "native stopped-edit state round-trip", "native lifecycle cleanup", "helper session completion",
        "process cleanup Shutdown", "process cleanup EOF", "process cleanup crash",
    )
    try:
        with stages.stage("helper session completion"):
            with Session(command, timeout) as session:
                with stages.stage('protocol stdout isolation (Rust/Win32/CRT)'):
                    owner = desktop.owner()
                    session.editor("Query", supported=True, has_editor=False, open=False)
                    error_response(session.request({"Editor": {"command": {"Open": {"owner": None}}}}), "No plugin loaded")
                    session.load(fixture)
                    session.editor("Query", supported=True, has_editor=True, open=False)
                    error_response(session.request({"Editor": {"command": "Focus"}}), "not open")
                    for invalid in ({"window": 0, "process_id": os.getpid()},
                                    {"window": owner["window"], "process_id": 0}):
                        error_response(session.request({"Editor": {"command": {"Open": {"owner": invalid}}}}))
                    session.editor({"Open": {"owner": owner}}, supported=True, has_editor=True, open=True)
                    session.assert_stdout_rerouted()
                with stages.stage('native attach/resize/DPI'):
                    state, hwnd = assert_handshake(session, desktop)
                with stages.stage('native control interaction/gesture/dirty feedback'):
                    handles = assert_interaction(session, desktop, hwnd, stages)
                with stages.stage('repeat focus and owner rejection'):
                    for _ in range(3):
                        same = session.editor({"Open": {"owner": owner}}, open=True)
                        require(same == state, "Repeated Open changed editor state/generation")
                        require(session.editor("Focus", open=True) == state, "Focus changed editor state/generation")
                        desktop.wait(lambda: desktop.focused_within(hwnd), session.deadline, "helper keyboard focus")
                        require(desktop.container(session) == hwnd, "Repeated Open replaced native container")
                    other = desktop.owner()
                    error_response(session.request({"Editor": {"command": {"Open": {"owner": other}}}}))
                    require(session.editor("Query") == state, "Rejected owner change modified editor state")
                    desktop.destroy_owner(other)
                with stages.stage('native stopped-edit state round-trip'):
                    assert_state_roundtrip(session, desktop, fixture, handles)
                with stages.stage('native lifecycle cleanup'):
                    session.editor({"Open": {"owner": owner}}, open=True)
                    state, hwnd = assert_handshake(session, desktop)
                    handles = [hwnd] + desktop.windows(session.process.pid, hwnd)
                    for _ in range(3):
                        session.editor("Close", open=False, has_editor=True)
                        desktop.gone(handles, session.deadline)
                        require(session.parameter(1000) == 0.0, "Closed editor did not detach plugin view")
                        closed = session.editor("Close", open=False)
                        require(session.editor("Close") == closed, "Repeated Close changed closed state")
                        session.editor({"Open": {"owner": owner}}, open=True)
                        reopened, hwnd = assert_handshake(session, desktop)
                        require(reopened["generation"] > state["generation"], "Reopen did not advance generation")
                        state, handles = reopened, [hwnd] + desktop.windows(session.process.pid, hwnd)
                    verified_fixture_button(session, desktop, hwnd)
                    desktop.post(hwnd, 0x0010)  # WM_CLOSE, same path as the native title-bar close button.
                    desktop.wait(lambda: not session.editor("Query")["open"], session.deadline, "title-bar close")
                    desktop.gone(handles, session.deadline)
                    require(session.parameter(1000) == 0, "Title-bar close did not detach")
                    session.editor({"Open": {"owner": owner}}, open=True)
                    _, hwnd = assert_handshake(session, desktop)
                    handles = [hwnd] + desktop.windows(session.process.pid, hwnd)
                    desktop.destroy_owner(owner)
                    desktop.wait(lambda: not session.editor("Query")["open"], session.deadline, "logical owner loss")
                    desktop.gone(handles, session.deadline)
                    require(session.parameter(1000) == 0, "Owner loss did not detach")
                    session.editor({"Open": {"owner": None}}, open=True)
                    _, hwnd = assert_handshake(session, desktop)
                    handles = [hwnd] + desktop.windows(session.process.pid, hwnd)
                    response_payload(session.request("UnloadPlugin"), "Success")
                    desktop.gone(handles, session.deadline)
                    session.editor("Query", has_editor=False, open=False)
                    session.load(fixture)
                    session.editor({"Open": {"owner": None}}, open=True)
                    _, hwnd = assert_handshake(session, desktop)
                    handles = [hwnd] + desktop.windows(session.process.pid, hwnd)
                    session.load(no_editor, has_editor=False)  # Reload while an editor is still open.
                    desktop.gone(handles, session.deadline)
                    session.editor("Query", supported=True, has_editor=False, open=False)
                    error_response(session.request({"Editor": {"command": {"Open": {"owner": None}}}}),
                                   "does not have a GUI editor")
                    session.editor("Close", open=False)
                    session.finish()
    except (SmokeError, OSError):
        # Any non-paint failure aborts dependent operations in this session.
        # The context manager still kills/reaps the helper before fresh sessions.
        pass
    for ending in ("Shutdown", "EOF", "crash"):
        try:
            with stages.stage(f"process cleanup {ending}"):
                with Session(command, timeout) as session:
                    session.load(fixture)
                    session.editor({"Open": {"owner": None}}, open=True)
                    _, hwnd = assert_handshake(session, desktop)
                    handles = [hwnd] + desktop.windows(session.process.pid, hwnd)
                    session.finish(ending)
                    desktop.gone(handles, session.deadline)
                    require(not desktop.windows(session.process.pid), f"Helper native windows leaked after {ending}")
        except (SmokeError, OSError):
            pass
    stages.finish(expected)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", required=True, type=Path)
    parser.add_argument("--timeout", type=float, default=60.0, help="deadline per helper process, seconds")
    args = parser.parse_args(argv)
    desktop = None
    try:
        if not math.isfinite(args.timeout) or args.timeout <= 0:
            raise SmokeError("timeout must be finite and greater than zero")
        desktop = WindowsDesktop()
        require(args.helper.is_file(), f"Helper not found: {args.helper}")
        require(ctypes.sizeof(ctypes.c_void_p) == 8, "Use 64-bit Python for the x86_64 fixture")
        fixture = verify_fixture(FIXTURE_ROOT, "editor")
        no_editor = verify_fixture(FIXTURE_ROOT, "no-editor")
        exercise_lifecycle([str(args.helper.resolve())], fixture, no_editor, desktop, args.timeout)
        print("EDITOR ACCEPTANCE OK: source fixture lifecycle + native Windows drawing/interaction verified")
        return 0
    except UnsupportedDesktop as error:
        print(f"UNSUPPORTED / NOT VERIFIED: {error}", file=sys.stderr)
        return 77
    except (SmokeError, OSError) as error:
        print(f"EDITOR ACCEPTANCE FAILED: {error}", file=sys.stderr)
        return 1
    finally:
        if desktop is not None:
            desktop.cleanup()


if __name__ == "__main__":
    sys.exit(main())
