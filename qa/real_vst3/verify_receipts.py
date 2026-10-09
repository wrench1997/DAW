#!/usr/bin/env python3
"""Read-only integrity and consistency checks; never loads or executes a plugin."""

import ast
import hashlib
import json
import pathlib
import re
import struct
import xml.etree.ElementTree as ET


ROOT = pathlib.Path(__file__).resolve().parent


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_json(name):
    return json.loads((ROOT / name).read_text())


def verify_inventory():
    manifest = ROOT / "CONTENTS-SHA256.txt"
    expected = {}
    raw = manifest.read_bytes()
    require(raw.endswith(b"\n"), "Inventory needs a final newline")
    for line in raw.decode("utf-8").splitlines(keepends=True):
        match = re.fullmatch(r"([0-9a-f]{64})  ([^\r\n]+)\n", line)
        require(match is not None, f"Invalid inventory line: {line!r}")
        sha, name = match.groups()
        relative = pathlib.PurePosixPath(name)
        require(not relative.is_absolute() and ".." not in relative.parts,
                f"Unsafe inventory path: {name}")
        require(str(relative) == name and "\\" not in name,
                f"Noncanonical inventory path: {name}")
        require(name not in expected and name != manifest.name,
                f"Duplicate or self-inventory entry: {name}")
        expected[name] = sha
    require(list(expected) == sorted(expected), "Inventory is not sorted")
    files = {}
    for path in sorted(ROOT.rglob("*")):
        require(not path.is_symlink(), f"Symlink in receipts: {path}")
        require(path.is_dir() or path.is_file(), f"Special file in receipts: {path}")
        if path.is_file() and path != manifest:
            files[path.relative_to(ROOT).as_posix()] = path
    require(set(files) == set(expected),
            f"Inventory mismatch; missing={sorted(set(expected) - set(files))}; "
            f"extra={sorted(set(files) - set(expected))}")
    for name, path in files.items():
        require(digest(path.read_bytes()) == expected[name], f"SHA256 mismatch: {name}")
        if path.suffix == ".json":
            json.loads(path.read_text())
        if path.suffix == ".py":
            ast.parse(path.read_text(), filename=name)
    return files


def verify_provenance():
    provenance = read_json("provenance.json")
    for entry in provenance["publication_copies"]:
        data = (ROOT / entry["path"]).read_bytes()
        require(digest(data) == entry["publication_sha256"],
                f"Publication provenance mismatch: {entry['path']}")
        if entry["treatment"] == "byte-for-byte":
            require(digest(data) == entry["original_sha256"], "Original hash mismatch")
        elif entry["treatment"] == "path-only normalization":
            require(re.fullmatch(r"[0-9a-f]{64}", entry["original_sha256"]),
                    "Invalid recorded original hash")
            for item in entry["path_normalization"]:
                require(re.fullmatch(r"[0-9a-f]{64}", item["original_prefix_sha256"]),
                        "Invalid original-prefix hash")
                require(data.count(item["publication_token"].encode()) == item["replacement_count"],
                        f"Path token count mismatch: {entry['path']}")
            # Raw prefix strings are deliberately not published. Their reversible
            # equivalence check is a local prepublication audit, not this check.
        elif entry["treatment"] in {
            "edited documentation", "adapted reproduction source, not measured-source bytes"
        }:
            require(re.fullmatch(r"[0-9a-f]{64}", entry["original_sha256"]),
                    "Invalid recorded pre-adaptation hash")
        else:
            raise ValueError(f"Unknown provenance treatment: {entry['treatment']}")


def note_entries(events, kind):
    return [(event["block"] * 256 + event["event"]["sample_offset"],
             event["event"]["data"][kind]["pitch"])
            for event in events if kind in event["event"]["data"]]


def verify_measurements():
    validation = read_json("receipts/validation.json")
    require(validation["all_assertions_passed"] is True, "Helper validation failed")
    require(validation["sample_rate"] == 48000 and validation["block_frames"] == 256,
            "Unexpected DSP configuration")
    instrument = validation["checks"]["instrument"]
    require(instrument["parameter_count"] == 2855, "Unexpected instrument parameter count")
    require(instrument["saved_state_bytes"] == 51929, "Unexpected instrument state size")
    require(instrument["pre_note"]["peak"] == 0, "Unexpected pre-note audio")
    require(instrument["note_on"]["finite"] and instrument["note_on"]["rms"] > .0001,
            "Instrument is not finite/nonzero")
    require(instrument["late_release"]["peak"] < 1e-6, "Note release did not decay")
    for name in ["dry", "bypass"]:
        require(validation["checks"]["effects_" + name]["delta_from_input"]["peak"] < 1e-5,
                f"Effect {name} does not match input")
    wet = validation["checks"]["effects_delay"]
    require(wet["delta_from_input"]["rms"] > .001 and
            wet["tail_after_input_stops"]["rms"] > .0001, "Missing wet delay/tail")
    default = read_json("receipts/stochas-probe.json")
    require(not default["default_pattern_events"] and not default["note_on_output"] and
            not default["note_off_output"], "Blank default is no longer a negative result")
    pattern = read_json("receipts/stochas-pattern-output.json")
    state = (ROOT / "receipts/stochas-qa-pattern.state").read_bytes()
    require(digest(state) == pattern["state_sha256"], "Pattern state does not match event receipt")
    require(state[:16] == b"VST3HOST_STATE\0\0", "Wrong state envelope")
    version, size, controller = struct.unpack("<III", state[16:28])
    require(version == 1 and controller == 0xffffffff and len(state) == 28 + size,
            "Wrong state lengths/version")
    magic, xml_size = struct.unpack("<II", state[28:36])
    require(magic == 0x21324356, "Wrong JUCE XML magic")
    xml = ET.fromstring(state[36:36 + xml_size])
    exported = ET.parse(ROOT / "receipts/stochas-qa-pattern.xml").getroot()
    require(ET.tostring(xml) == ET.tostring(exported), "State/XML exports disagree")
    require(pattern["source_schema_roundtrip_verified"] and not pattern["preplay_output"],
            "Pattern round-trip/pre-roll failed")
    require(len(pattern["playing_output"]) == 35, "Unexpected pattern event count")
    stop = pattern["stop_output"]
    require(len(stop) == 1 and stop[0]["block"] == 0 and
            stop[0]["event"]["sample_offset"] == 0 and
            "NoteOff" in stop[0]["event"]["data"], "Missing immediate stop note-off")
    for bpm, report in read_json("receipts/stochas-tempo-check.json").items():
        ons = note_entries(report["events"], "NoteOn")
        intervals = [b[0] - a[0] for a, b in zip(ons[1:], ons[2:])]
        expected = 48000 * 60 / int(bpm) / 4
        require(intervals == report["steady_intervals_samples"] and
                all(abs(value - expected) <= 1 for value in intervals), "Wrong tempo spacing")
        require([pitch for _, pitch in ons] ==
                ([60, 64, 67, 60] * ((len(ons) + 3) // 4))[:len(ons)], "Wrong pitch sequence")
        require(not note_entries(report["stop_events"], "NoteOn"), "Note-on after stop")
        balance = {}
        for event in report["events"] + report["stop_events"]:
            for kind, sign in [("NoteOn", 1), ("NoteOff", -1)]:
                data = event["event"]["data"]
                if kind in data:
                    pitch = data[kind]["pitch"]
                    balance[pitch] = balance.get(pitch, 0) + sign
                    require(balance[pitch] >= 0, "Note-off without matching note-on")
        require(all(value == 0 for value in balance.values()), "Unbalanced notes after stop")
    for directory in ["scanner", "scanner-integrated"]:
        scan = (ROOT / directory / "actual-scan.log").read_text()
        cache = json.loads(scan.split("CACHE_JSON_START\n", 1)[1].split("\nCACHE_JSON_END", 1)[0])
        require(len(cache) == 2, "Unexpected scanner descriptor count")
        by_name = {entry["name"]: entry for entry in cache}
        require(by_name["Surge XT"]["category"] == "Instrument", "Wrong instrument category")
        require(by_name["Surge XT Effects"]["category"] == "Effect", "Wrong effect category")
        for name, factory, midi_input in [
            ("Surge XT", "Instrument|Synth", True), ("Surge XT Effects", "Fx", False)
        ]:
            descriptor = by_name[name]
            metadata = descriptor["vst3_metadata"]
            require(descriptor["verified"] and descriptor["vendor"] == "Surge Synth Team",
                    "Unverified identity")
            require(metadata["category"] == factory and metadata["has_midi_input"] == midi_input
                    and not metadata["has_midi_output"], "Wrong factory/event metadata")
    build = read_json("scanner-integrated/scanner-reproduction-build.json")
    require(build["commit"] == "e6bd216863f6180c2745de05c6701c6300d41cf2" and
            build["compile_succeeded"] is True, "Wrong integrated build provenance")
    require(build["probe_source_sha256"] == digest((ROOT / "scanner_probe.rs").read_bytes()),
            "Shipped scanner source differs from integrated measured source")
    require(build["helper_sha256"] == validation["helper_sha256"],
            "Integrated helper hash differs from recorded historical helper")
    rerun = read_json("provenance.json")["integrated_scanner_rerun"]
    require(rerun["build_script_sha256"] == digest((ROOT / "build_runtime_probe.py").read_bytes()),
            "Shipped builder differs from integrated rerun source")


def main():
    files = verify_inventory()
    verify_provenance()
    verify_measurements()
    print(f"PASS: {len(files)} inventoried files; hashes, provenance, JSON, Python syntax, "
          "state/XML, DSP/MIDI receipts and corrected scanner metadata are consistent")
    print("Read-only receipt validation; no new plugin, device, GUI or current-source DSP run")


if __name__ == "__main__":
    main()
