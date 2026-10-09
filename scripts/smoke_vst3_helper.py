#!/usr/bin/env python3
"""Exercise the built helper without a plugin. Python is a developer-only tool."""

import argparse
from collections import deque
import json
import math
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time


class SmokeError(RuntimeError):
    """A protocol, process, or cleanup check failed."""


def _remaining(deadline, phase):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise SmokeError(f"Timed out during {phase}")
    return remaining


def _stdout_reader(stream, events, stopped):
    def emit(event):
        while not stopped.is_set():
            try:
                events.put(event, timeout=0.05)
                return
            except queue.Full:
                pass

    try:
        while not stopped.is_set():
            line = stream.readline(65537)
            if not line:
                emit(("eof", None))
                return
            if len(line) > 65536:
                emit(("error", "Protocol line exceeded 64 KiB"))
                return
            emit(("line", line))
    except (OSError, ValueError) as error:
        emit(("error", f"Cannot read protocol output: {error}"))


def _stderr_reader(stream, tail):
    # Drain concurrently so diagnostics cannot fill the pipe and deadlock replies.
    # Bound retained diagnostics, including output with no newline.
    try:
        while chunk := stream.readline(4096):
            tail.append(chunk)
    except (OSError, ValueError) as error:
        tail.append(f"Cannot read diagnostics: {error}")


def _event(events, deadline, phase):
    try:
        kind, value = events.get(timeout=_remaining(deadline, phase))
    except queue.Empty as error:
        raise SmokeError(f"Timed out during {phase}") from error
    if kind == "error":
        raise SmokeError(value)
    return kind, value


def _response(events, deadline):
    kind, line = _event(events, deadline, "protocol response")
    if kind != "line":
        raise SmokeError("Helper closed stdout before its protocol response")
    try:
        return json.loads(line)
    except json.JSONDecodeError as error:
        raise SmokeError(f"Malformed JSON on stdout: {line[:200]!r}") from error


def smoke_command(command, timeout=5.0, cwd=None):
    """Return after three checked replies and explicit Shutdown, or raise.

    A single deadline covers startup, exchanges and graceful exit. Failure cleanup
    kills and reaps this child with a separate bounded wait. No command can load a
    plugin, spawn a plugin child, access audio devices, or write application data.
    """
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("timeout must be finite and greater than zero")
    deadline = time.monotonic() + timeout
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    events = queue.Queue(maxsize=16)
    diagnostics = deque(maxlen=8)
    stopped = threading.Event()
    readers = [
        threading.Thread(
            target=_stdout_reader, args=(process.stdout, events, stopped), daemon=True
        ),
        threading.Thread(
            target=_stderr_reader, args=(process.stderr, diagnostics), daemon=True
        ),
    ]
    failure = None
    try:
        for reader in readers:
            reader.start()

        def send(line):
            # All writes combined are < 100 bytes, well below the pipe capacity.
            process.stdin.write(line + "\n")
            process.stdin.flush()

        send("")  # Blank lines must not produce an extra reply.
        send('"GetAllParameters"')
        expected = {"Error": {"message": "No plugin loaded"}}
        if _response(events, deadline) != expected:
            raise SmokeError("Unexpected response to GetAllParameters")

        send("not-json")
        response = _response(events, deadline)
        if (
            not isinstance(response, dict)
            or set(response) != {"Error"}
            or not isinstance(response["Error"], dict)
            or set(response["Error"]) != {"message"}
            or not isinstance(response["Error"]["message"], str)
            or not response["Error"]["message"].startswith("Invalid command:")
        ):
            raise SmokeError(f"Unexpected invalid-command response: {response!r}")

        send('"GetAllParameters"')
        if _response(events, deadline) != expected:
            raise SmokeError("Protocol did not recover after an invalid command")

        send('"Shutdown"')
        # Deliberately keep stdin open: EOF alone must not satisfy this check.
        try:
            code = process.wait(timeout=_remaining(deadline, "Shutdown"))
        except subprocess.TimeoutExpired as error:
            raise SmokeError("Timed out during Shutdown with stdin still open") from error
        if code != 0:
            raise SmokeError(f"Helper exited with code {code}")
        kind, value = _event(events, deadline, "stdout close after Shutdown")
        if kind != "eof":
            raise SmokeError(f"Unexpected extra stdout after Shutdown: {value[:200]!r}")
    except (SmokeError, OSError, ValueError) as error:
        failure = str(error)
    finally:
        if process.poll() is None:
            process.kill()
            try:
                process.wait(timeout=2.0)
            except subprocess.TimeoutExpired as error:
                raise SmokeError("Could not reap helper after forced termination") from error
        stopped.set()
        cleanup_deadline = time.monotonic() + 2.0
        for reader in readers:
            if reader.ident is not None:
                reader.join(timeout=max(0, cleanup_deadline - time.monotonic()))
        if any(reader.is_alive() for reader in readers):
            raise SmokeError("Helper exited but an output reader did not stop")
        for stream in (process.stdin, process.stdout, process.stderr):
            try:
                stream.close()
            except BrokenPipeError:
                pass
    if failure:
        tail = "".join(diagnostics).strip()
        raise SmokeError(f"{failure}\nHelper stderr (tail):\n{tail}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("helper", type=Path, help="path to the built helper executable")
    parser.add_argument("--timeout", type=float, default=5.0, help="total deadline in seconds")
    args = parser.parse_args()
    helper = args.helper.resolve()
    if not helper.is_file():
        parser.error(f"helper executable does not exist: {helper}")
    try:
        smoke_command([str(helper)], timeout=args.timeout, cwd=helper.parent)
    except (SmokeError, OSError, ValueError) as error:
        print(f"VST3 helper smoke FAILED: {error}", file=sys.stderr)
        return 1
    print("VST3 helper smoke PASS: 3 JSON replies, invalid-command recovery, "
          "Shutdown exit 0 with stdin open; helper reaped; no plugin loaded")
    return 0


if __name__ == "__main__":
    sys.exit(main())
