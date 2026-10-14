#!/usr/bin/env python3
"""Bounded, device-free Kontakt VST3/helper QA; never loads an NKI by guessing its state format.

Opaque state stays in the private output directory. A successful default-instance test
does not qualify NOIRE, licensing, native UI, physical playback, or song export.
"""
import argparse
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import shutil
import struct
import subprocess
import threading
import time


def digest(data):
    return hashlib.sha256(data).hexdigest()


class Session:
    def __init__(self, helper, out, label, transcript, timeout):
        self.label, self.transcript, self.timeout = label, transcript, timeout
        self.events = queue.Queue(maxsize=64)
        self.stderr = (out / f"{label}.stderr.log").open("wb")
        self.process = subprocess.Popen(
            [str(helper)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=self.stderr, cwd=helper.parent,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.closed = False

    def _read(self):
        try:
            while True:
                line = self.process.stdout.readline(96 * 1024 * 1024 + 1)
                if not line:
                    self.events.put(("eof", None), timeout=2)
                    return
                if len(line) > 96 * 1024 * 1024:
                    self.events.put(("error", "protocol line over limit"), timeout=2)
                    return
                try:
                    event = ("reply", json.loads(line))
                except (ValueError, UnicodeDecodeError):
                    event = ("error", "non-JSON helper stdout")
                self.events.put(event, timeout=2)
        except Exception as error:
            try:
                self.events.put(("error", str(error)), timeout=1)
            except queue.Full:
                pass

    def command(self, command):
        label = command if isinstance(command, str) else next(iter(command))
        started = time.perf_counter()
        self.process.stdin.write((json.dumps(command, separators=(",", ":")) + "\n").encode())
        self.process.stdin.flush()
        try:
            kind, response = self.events.get(timeout=self.timeout)
        except queue.Empty:
            entry = {"session": self.label, "command": label, "seconds": time.perf_counter()-started,
                     "status": "failed", "error": f"deadline {self.timeout}s"}
            self.transcript.append(entry)
            raise RuntimeError(f"{self.label}/{label}: deadline {self.timeout}s")
        entry = {"session": self.label, "command": label, "seconds": time.perf_counter()-started}
        if kind != "reply" or not isinstance(response, dict) or len(response) != 1:
            entry.update(status="failed", error=str(response or kind))
            self.transcript.append(entry)
            raise RuntimeError(f"{self.label}/{label}: {entry['error']}")
        reply_type = next(iter(response))
        entry.update(response=reply_type, status="failed" if reply_type in ("Error", "Crashed") else "passed")
        if entry["status"] == "failed":
            entry["error"] = response[reply_type].get("message", reply_type)
        self.transcript.append(entry)
        if entry["status"] == "failed":
            raise RuntimeError(f"{self.label}/{label}: {entry['error']}")
        return response[reply_type]

    def close(self):
        if self.closed:
            return
        self.closed = True
        started = time.perf_counter()
        forced = False
        try:
            if self.process.poll() is None:
                self.process.stdin.write(b'"Shutdown"\n')
                self.process.stdin.flush()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    forced = True
                    self.process.kill()
                    self.process.wait(timeout=5)
        except (OSError, BrokenPipeError):
            if self.process.poll() is None:
                forced = True
                self.process.kill()
                self.process.wait(timeout=5)
        finally:
            self.reader.join(timeout=3)
            self.stderr.close()
            for stream in (self.process.stdin, self.process.stdout):
                stream.close()
        self.transcript.append({"session": self.label, "command": "Shutdown", "seconds":time.perf_counter()-started,
                                "exit_code":self.process.returncode, "forced":forced,
                                "status":"passed" if self.process.returncode == 0 and not forced else "failed"})


def load(session, path):
    return session.command({"LoadPlugin": {"path":str(path), "sample_rate":48000.0, "block_size":2048,
        "tempo":80.0, "time_sig_numerator":4, "time_sig_denominator":4, "class_id":None}})


def state_bytes(response):
    raw = response["data"]
    return base64.b64decode(raw, validate=True) if isinstance(raw, str) else bytes(raw)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--plugin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="a NEW private output directory")
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()
    if not 0 < args.timeout <= 60:
        parser.error("timeout must be within (0,60] seconds")
    helper, plugin, out = args.helper.resolve(), args.plugin.resolve(), args.output.resolve()
    if not helper.is_file() or not plugin.exists():
        parser.error("helper or plugin missing")
    out.mkdir(parents=True, exist_ok=False)
    copied_helper = out / helper.name
    shutil.copyfile(helper, copied_helper)
    transcript = []
    report = {"scope":"production helper + fresh default Kontakt instance (no NKI load attempted); no device output or native editor input",
              "helper_sha256":digest(copied_helper.read_bytes()), "plugin":str(plugin),
              "noire_loading":"not tested: requires supported native UI or an existing verified Kontakt state",
              "license":"not tested: no licensing credentials read or changed",
              "native_ui":"not tested", "physical_playback":"not tested", "song_export":"not implemented for active VSTs",
              "phases":transcript}
    session = None
    success = False
    try:
        session = Session(copied_helper, out, "original", transcript, args.timeout)
        report["plugin_info"] = load(session, plugin)
        report["layout"] = session.command("AudioBusLayout")
        report["editor_query"] = session.command({"Editor":{"command":"Query"}})
        params = session.command("GetAllParameters")["params"]
        report["parameter_count"] = len(params)
        report["parameter_sample"] = params[:8]
        units = session.command("GetUnits")["units"]
        report["units"] = [{"id":unit["id"], "name":unit["name"],
                            "program_count":len(unit.get("programs", [])),
                            "program_sample":unit.get("programs", [])[:2]} for unit in units]
        initial_state = state_bytes(session.command("SaveState"))
        (out / "default-kontakt.state.bin").write_bytes(initial_state)
        report["state"] = {"bytes":len(initial_state), "sha256":digest(initial_state)}
        session.command("StartProcessing")
        session.command({"SetPlaying":{"playing":True}})
        note = session.command({"NoteOn":{"channel":0,"note":60,"velocity":32,"sample_offset":0}})["note_id"]
        peak, non_finite, sample_count = 0.0, 0, 0
        for block in range(4):
            output = session.command({"Process":{"inputs":[],"frames":2048}})["outputs"]
            for channel in output:
                raw = base64.b64decode(channel, validate=True)
                for (sample,) in struct.iter_unpack("<f", raw):
                    sample_count += 1
                    if math.isfinite(sample):
                        peak = max(peak, abs(sample))
                    else:
                        non_finite += 1
        session.command({"NoteOff":{"note_id":note,"sample_offset":0}})
        session.command("MidiPanic")
        session.command("StopProcessing")
        report["default_instance_pcm"] = {"peak":peak,"samples":sample_count,"non_finite":non_finite,
                                    "interpretation":"no NKI was loaded by this harness; silence of an empty rack is expected; this is not a NOIRE audition"}
        if non_finite:
            raise RuntimeError("non-finite PCM from default Kontakt instance")
        session.close()
        session = Session(copied_helper, out, "fresh-restoration", transcript, args.timeout)
        restored_info = load(session, plugin)
        if restored_info.get("uid") != report["plugin_info"].get("uid"):
            raise RuntimeError("fresh class identity differs")
        session.command({"LoadState":{"data":base64.b64encode(initial_state).decode("ascii")}})
        restored_state = state_bytes(session.command("SaveState"))
        (out / "fresh-restored-default-kontakt.state.bin").write_bytes(restored_state)
        report["restored_state"] = {"bytes":len(restored_state),"sha256":digest(restored_state),
                                   "byte_identical":initial_state == restored_state}
        restored_params = session.command("GetAllParameters")["params"]
        report["restored_parameters_equal"] = params == restored_params
        success = True
    except Exception as error:
        report["error"] = str(error)
    finally:
        if session is not None:
            session.close()
        report["status"] = "passed" if success and all(p["status"] == "passed" for p in transcript) else "failed"
        (out / "result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
