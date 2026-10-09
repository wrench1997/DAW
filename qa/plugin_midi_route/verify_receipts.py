#!/usr/bin/env python3
"""Read-only published receipt validation; never builds or executes a plugin."""
import argparse
import ast
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess

ROOT = Path(__file__).resolve().parent


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_json(name):
    return json.loads((ROOT / name).read_text())


def verify_inventory():
    inventory = ROOT / "CONTENTS-SHA256.txt"
    expected = {}
    for line in inventory.read_text().splitlines(keepends=True):
        match = re.fullmatch(r"([0-9a-f]{64})  ([^\r\n]+)\n", line)
        require(match is not None, "Malformed inventory line")
        sha, name = match.groups()
        path = PurePosixPath(name)
        require(not path.is_absolute() and ".." not in path.parts and str(path) == name
                and "\\" not in name and name not in expected and name != inventory.name,
                "Unsafe, duplicate or self-inventory path")
        expected[name] = sha
    require(list(expected) == sorted(expected), "Unsorted inventory")
    files = {}
    for path in ROOT.rglob("*"):
        require(not path.is_symlink() and (path.is_file() or path.is_dir()), "Nonregular QA entry")
        if path.is_file() and path != inventory:
            name = path.relative_to(ROOT).as_posix()
            data = path.read_bytes()
            require(data, "Empty QA payload")
            files[name] = data
    require(set(files) == set(expected), "QA inventory names differ")
    for name, data in files.items():
        require(digest(data) == expected[name], "QA hash mismatch: " + name)
        if name.endswith(".json"):
            json.loads(data)
        if name.endswith(".py"):
            ast.parse(data, filename=name)
    return files


def verify_imported(files):
    publication = read_json("PUBLICATION.json")
    require(digest(files["INVENTORY.json"]) == publication["original_inventory_sha256"],
            "Imported inventory identity changed")
    adapted = {item["path"]: item for item in publication["adaptations"]}
    require(set(adapted) == {"REPRODUCE.md"}, "Unexpected source adaptation")
    entries = read_json("INVENTORY.json")["files"]
    require(len(entries) == 45 and len({item["path"] for item in entries}) == 45,
            "Unexpected imported inventory")
    for item in entries:
        name = item["path"]
        require(name in files, "Missing imported evidence")
        if name in adapted:
            require(item["publication_sha256"] == adapted[name]["imported_sha256"] and
                    digest(files[name]) == adapted[name]["publication_sha256"],
                    "Adapted guide attribution changed")
        else:
            require(digest(files[name]) == item["publication_sha256"] and
                    len(files[name]) == item["publication_bytes"], "Imported bytes changed: " + name)
            for replacement in item["path_only_normalizations"]:
                require(files[name].count(replacement["semantic_token"].encode()) >=
                        replacement["replacement_count"], "Missing normalized path token")
    # Original raw hashes remain attestations; their private prefix map is not shipped.


def stats(text):
    return {name: int(value) for name, value in re.findall(r"([a-z_]+): (\d+)", text)}


def verify_measurements():
    summary = read_json("receipts/summary.json")
    matrix = read_json("receipts/native-production-graph.json")
    require(summary["matrix"] == matrix, "Summary/matrix mismatch")
    active = [case for case in matrix if case.get("routed") is True]
    require({(case["bpm"], case["callback_frames"]) for case in active} ==
            {(120.0, 128), (120.0, 256), (120.0, 512), (120.0, 2048), (60.0, 512)}
            and len(active) == 5, "Missing paced routed cases")
    for case in active:
        require(case["strict_realtime_provenance_gate"] and case["sink_peak"] > .01,
                "Routed case lacks output/provenance")
        require(len(case["adapter_provenance"]) == len(case["worker_stats"]) == 2,
                "Wrong endpoint count")
        for endpoint in case["adapter_provenance"]:
            require(endpoint["completed_quanta"] == endpoint["submitted_quanta"] == 750 and
                    endpoint["exact_plugin_quanta"] == 734 and
                    endpoint["delayed_dry_quanta"] == endpoint["expected_lookahead_quanta"] == 16 and
                    endpoint["nonstartup_legacy_fallback_quanta"] == 0, "Routed output provenance changed")
        for worker in case["worker_stats"]:
            values = stats(worker)
            require(values["completed"] == values["submitted"] == 750 and
                    all(values[name] == 0 for name in ["deadline_misses", "input_overflows",
                        "output_overflows", "faults", "reset_faults", "dropped_rt_events"]),
                    "Unexpected routed worker loss/fault")
        ons = [(frame, message[1]) for frame, message in case["source_events"]
               if message[0] & 0xf0 == 0x90 and message[2] != 0]
        require(len(ons) >= 4 and ons[0][0] == 2176, "Missing source onset")
        require([pitch for _, pitch in ons] == ([60, 64, 67, 60] * ((len(ons) + 3) // 4))[:len(ons)],
                "Generated pitch ordering changed")
        interval = 48000 * 60 / case["bpm"] / 4
        require(all(abs(b[0] - a[0] - interval) <= 1 for a, b in zip(ons, ons[1:])),
                "Generated tempo spacing changed")
    off = [case for case in matrix if case.get("routed") is False]
    require(len(off) == 1 and off[0]["sink_peak"] == 0 and
            not off[0]["strict_realtime_provenance_gate"], "Off baseline scope changed")
    require([stats(worker)["deadline_misses"] for worker in off[0]["worker_stats"]] == [15, 10],
            "Legacy Off deadline observations lost")
    cb128 = next(case for case in active if case["callback_frames"] == 128)
    require(cb128["max_callback_render_runtime_us"] == 3116 and 3116 > 128 / 48000 * 1e6,
            "Debug callback over-budget observation lost")
    require(summary["concurrent_load_observation"]["outcome"] == "failed" and
            "not yet isolated" in summary["concurrent_load_observation"]["cause"],
            "Unresolved concurrent-load failure lost")
    for name in ["single_note_latch", "retrigger_chord_latch"]:
        case = summary[name]
        require(case["held_note_peak"] > 0 and case["last_second_peak"] == 0 and
                case["no_explicit_note_off_sent_by_test"], "Safety/release evidence changed")
    require(summary["retrigger_chord_latch"]["note_on_pitches"] == [60, 60, 64, 67],
            "Retrigger/chord case changed")
    fx = summary["fx_recovery"]
    require(fx["route_fault"] == fx["new_execution_failures"] == 0 and fx["sink_peak"] > .01,
            "FX fresh-epoch recovery failed")
    overload = summary["controlled_overload"]
    require(overload["published_faulted_destinations"] != 0 and
            overload["published_faults_after_restart"] == 0 and overload["sink_peak_after_restart"] > .01,
            "Deliberate overload/restart evidence changed")
    require("test result: ok. 5 passed; 0 failed; 0 ignored" in
            (ROOT / "receipts/native-tests-final.log").read_text(), "Final five-test result missing")


def verify_source(repo=None, ref=None):
    binding = read_json("receipts/final-source-binding.json")
    snapshot_bytes = (ROOT / "receipts/source-snapshot.json").read_bytes()
    snapshot = json.loads(snapshot_bytes)
    require(digest(snapshot_bytes) == binding["source_manifest_sha256"] and
            snapshot["base"] == binding["candidate_commit"] ==
            "e54a6e49fe02b78ff299ce56dddd781925f7fe3c" and
            binding["all_recorded_production_source_files_match_candidate"] and
            binding["git_status_porcelain"] == "" and
            len(snapshot["files"]) == binding["file_count"] == 117,
            "Final source binding inconsistent")
    require(read_json("receipts/summary.json")["source"] == snapshot, "Summary/source mismatch")
    if repo is not None:
        require(ref is not None and re.fullmatch(r"[0-9a-fA-F]{40}", ref),
                "An explicit full historical source commit is required")
        for name, sha in snapshot["files"].items():
            data = subprocess.check_output(["git", "-C", str(repo), "show", ref + ":" + name])
            require(digest(data) == sha, "Production source differs: " + name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, help="Optional Git checkout for source-byte verification")
    parser.add_argument("--source-ref", help="Explicit measured or byte-equivalent integrated commit")
    args = parser.parse_args()
    require((args.repo is None) == (args.source_ref is None), "Use both --repo and --source-ref")
    files = verify_inventory()
    verify_imported(files)
    verify_measurements()
    verify_source(args.repo, args.source_ref)
    print(f"PASS: {len(files)} inventoried payloads; original import, source binding and positive/negative measurements")
    print("Read-only receipt validation; no plugin, GUI, hardware or new paced execution")


if __name__ == "__main__":
    main()
