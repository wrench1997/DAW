#!/usr/bin/env python3
"""Source-built Linux editor interaction gate. Exit 77 is UNSUPPORTED / NOT VERIFIED.

Requires an existing desktop DISPLAY and explicit --interactive. It never starts a
compositor, moves the DAW to X11, installs software, or synthesizes desktop input.
Follow the terminal prompts using the real visible fixture. Retain screenshots of
paint and interactions separately. This is helper-component acceptance only.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import time

from smoke_vst3_editor import Session, SmokeError, SOURCE_FILES, UPSTREAM_COMMIT

ROOT = Path(__file__).resolve().parents[1] / "tests/fixtures/vst3-editor"


def verify_fixture(root, variant):
    name = "CitrusEditorFixture" if variant == "editor" else "CitrusNoEditorFixture"
    bundle = root / "target/bundles" / f"{name}.vst3"
    receipt = json.loads((bundle / "source-build.json").read_text(encoding="utf-8"))
    expected = f"Contents/x86_64-linux/{name}.so"
    if (receipt.get("schema") != 1 or receipt.get("variant") != variant
            or receipt.get("target") != "x86_64-unknown-linux-gnu"
            or receipt.get("upstream_commit") != UPSTREAM_COMMIT
            or receipt.get("binary") != expected or set(receipt.get("sources", {})) != set(SOURCE_FILES)):
        raise SmokeError("Fixture receipt identity/source set does not match")
    for source in SOURCE_FILES:
        if hashlib.sha256((root / source).read_bytes()).hexdigest() != receipt["sources"][source]:
            raise SmokeError(f"Stale fixture source: {source}; rebuild fixture")
    binary = bundle / expected
    if binary.read_bytes()[:4] != b"\x7fELF" or hashlib.sha256(binary.read_bytes()).hexdigest() != receipt["binary_sha256"]:
        raise SmokeError("Fixture ELF bytes do not match source receipt")
    return bundle


def counter(session, param):
    return round(session.parameter(param) * 1_000_000)


def wait_until(session, predicate, message):
    while time.monotonic() < session.deadline:
        if predicate():
            return
        time.sleep(0.025)
    raise SmokeError(message)


def exercise(helper, fixture, no_editor, timeout):
    with Session([str(helper)], timeout=timeout) as session:
        session.load(fixture)
        session.editor("Query", supported=True, has_editor=True, open=False)
        rejected = session.request({"Editor": {"command": {"Open": {"owner": {"window": 1, "process_id": 1}}}}})
        if "Error" not in rejected:
            raise SmokeError("Linux accepted a Windows HWND owner")
        state = session.editor({"Open": {"owner": None}}, supported=True, has_editor=True, open=True)
        generation = state["generation"]
        again = session.editor({"Open": {"owner": None}}, open=True)
        if again["generation"] != generation:
            raise SmokeError("Repeated Open attached a duplicate view")
        wait_until(session, lambda: all(marker in "".join(session.diagnostics) for marker in
                   ("CITRUS_FIXTURE_STDOUT_RUST", "CITRUS_FIXTURE_STDOUT_POSIX", "CITRUS_FIXTURE_STDOUT_CRT")),
                   "Fixture stdout routes did not all arrive on stderr")
        mouse = counter(session, 1026)
        print("CLICK the orange fixture button once. Retain before/after screenshots.", flush=True)
        wait_until(session, lambda: counter(session, 1026) > mouse, "No real native mouse event")
        if session.parameter(0) != 0.25:
            raise SmokeError("Native mouse edit was not delivered")
        press, release = counter(session, 1024), counter(session, 1025)
        print("Switch focus away and back using the titlebar. Move the mouse outside the plugin, then press and release Space.", flush=True)
        wait_until(session, lambda: counter(session, 1024) > press and counter(session, 1025) > release,
                   "Missing native key press or release through XEmbed")
        if session.native_revision() == 0:
            raise SmokeError("Native edit did not mark state dirty")
        edits = session.request("TakeParameterEdits")["ParameterEdits"]["edits"]
        if not {"BeginGesture", "ValueChange", "EndGesture"}.issubset({edit["kind"] for edit in edits}):
            raise SmokeError("Incomplete native gesture callbacks")
        # Wait for both factory and frame callbacks; neither fixture creates an event thread.
        wait_until(session, lambda: counter(session, 1020) > 0 and counter(session, 1021) > 0
                   and counter(session, 1022) > 0 and counter(session, 1023) > 0,
                   "Factory/frame fd or timer callback missing")
        saved = session.save_state()  # detach + zero-sample native flush
        session.editor("Query", open=False, width=0, height=0)
        frame, factory = counter(session, 1023), counter(session, 1021)
        time.sleep(0.1)
        if counter(session, 1023) != frame or counter(session, 1021) <= factory:
            raise SmokeError("Close did not retire frame callbacks while preserving factory timers")
        session.editor("Close", open=False)
        session.load(fixture)
        if session.parameter(0) != 1.0:
            raise SmokeError("Fresh fixture did not start from default state")
        response = session.request({"LoadState": {"data": saved, "context": "Project"}})
        if "Success" not in response or session.parameter(0) != 0.25:
            raise SmokeError("Native edit did not survive fresh-instance state restore")
        reopened = session.editor({"Open": {"owner": None}}, open=True)
        if reopened["generation"] <= generation:
            raise SmokeError("Reopen did not advance attachment generation")
        print("CLOSE the fixture using its actual titlebar close button.", flush=True)
        wait_until(session, lambda: not session.editor("Query")["open"], "Titlebar close was not observed")
        session.load(no_editor, has_editor=False)
        session.editor("Query", supported=True, has_editor=False, open=False)
        if "Error" not in session.request({"Editor": {"command": {"Open": {"owner": None}}}}):
            raise SmokeError("No-editor fixture reported a false native open")
        session.finish()
    for mode in ("EOF", "Shutdown", "crash"):
        with Session([str(helper)], timeout=30) as session:
            session.load(fixture)
            session.editor({"Open": {"owner": None}}, open=True)
            session.finish(mode)
    print("PASS: source fixture native mouse/key events, run-loop, state and lifecycle. Review retained paint screenshots separately.")
    print("This does not establish DAW Wayland + XWayland integration, real-plugin compatibility, audio hardware or realtime stability.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--interactive", action="store_true")
    parser.add_argument("--timeout", type=float, default=180)
    args = parser.parse_args()
    if not sys.platform.startswith("linux") or not os.environ.get("DISPLAY") or not args.interactive:
        print("UNSUPPORTED / NOT VERIFIED: requires Linux, an existing DISPLAY and --interactive; no display server is started.")
        return 77
    try:
        exercise(args.helper.resolve(), verify_fixture(ROOT, "editor"), verify_fixture(ROOT, "no-editor"), args.timeout)
    except (SmokeError, OSError, ValueError, KeyError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
