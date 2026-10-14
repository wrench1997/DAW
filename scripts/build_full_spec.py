"""Independent frozen Saved13 score ledger. No DAW/compiler/SDK imports."""
import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
import struct

SAVED_SHA = "90923c6384ff5bb3df7df2a6d31b0d7eea016e496a2588aaf2ed06e947cc3c23"
RATE, BPM, QUANTUM, MASTER = 48000, 84, 128, 0xFFFFFFFF


def require(condition, message):
    if not condition:
        raise ValueError(message)


def f32(value):
    require(math.isfinite(value), "nonfinite score operand")
    return struct.unpack("<f", struct.pack("<f", value))[0]


def sample_frame(beat):
    require(math.isfinite(beat) and beat >= 0, "invalid musical position")
    return math.floor(beat * 60 / BPM * RATE + 0.5)


def geometry(latency):
    require(isinstance(latency, int) and not isinstance(latency, bool)
            and 0 <= latency <= RATE * 30, "actual captured latency outside explicit spec bound")
    music, tail = sample_frame(160), RATE * 3
    retained = music + tail
    generated = ((max(latency + retained, music + 1) + QUANTUM - 1) // QUANTUM) * QUANTUM
    return {"native_latency": latency, "musical_frames": music, "tail_frames": tail,
            "retained_frames": retained, "generated_frames": generated,
            "blocks": generated // QUANTUM, "discarded_suffix_frames": generated - latency - retained,
            "raw_stereo_f32_bytes": generated * 8, "retained_stereo_f32_bytes": retained * 8}


def expand_score(project):
    """Expand this declared one-cycle score and retain every note identity boundary."""
    patterns = {p["id"]: p for p in project["patterns"]}
    require(len(patterns) == len(project["patterns"]), "duplicate pattern identity")
    boundaries, identities = [], set()
    for clip_order, clip in enumerate(project["clips"]):
        require(clip["kind"] == "Pattern" and not clip["muted"], "unsupported placement")
        start, length, offset, gain = (f32(clip[k]) for k in ("start", "length", "source_offset", "gain"))
        require(length > 0 and offset == 0 and gain >= 0, "unexpected one-cycle placement geometry")
        end = start + length
        pattern = patterns[clip["pattern_id"]]
        for note_order, note in enumerate(pattern["notes"]):
            require(not note["muted"] and note["channel_id"] == 1, "unexpected muted/foreign note")
            pitch = note["note"]
            require(isinstance(pitch, int) and 0 <= pitch < 128, "invalid MIDI pitch")
            raw_start = start + f32(note["start"]) - offset
            note_end = min(end, raw_start + f32(note["length"]))
            note_start = max(start, raw_start)
            if note_end <= note_start or note_start >= end:
                continue
            identity = (clip["id"], pattern["id"], note["id"])
            require(identity not in identities, "duplicate placed note identity")
            identities.add(identity)
            on = sample_frame(note_start)
            off = max(on + 1, sample_frame(note_end))
            velocity = max(1, math.floor(f32(max(0.0, min(1.0, f32(f32(note["velocity"]) * gain))) * 127) + .5))
            for frame, kind in ((on, "on"), (off, "off")):
                boundaries.append({"frame": frame, "kind": kind, "identity": list(identity),
                    "pitch": pitch, "velocity": velocity if kind == "on" else 0,
                    "source_order": [clip_order, note_order]})
    boundaries.sort(key=lambda row: (row["frame"], row["kind"] == "on", row["source_order"]))
    active_ids, pitches, wire = set(), Counter(), []
    for row in boundaries:
        identity, pitch = tuple(row["identity"]), row["pitch"]
        if row["kind"] == "on":
            require(identity not in active_ids, "duplicate active note identity")
            active_ids.add(identity)
            pitches[pitch] += 1
            eligible = True
        else:
            require(identity in active_ids and pitches[pitch] > 0, "unmatched identity closure")
            active_ids.remove(identity)
            pitches[pitch] -= 1
            eligible = pitches[pitch] == 0
        row["wire_emitted"] = eligible
        if eligible:
            wire.append({"frame": row["frame"], "block": row["frame"] // QUANTUM,
                "offset": row["frame"] % QUANTUM, "identity": row["identity"],
                "message": [0x90 if row["kind"] == "on" else 0x80, pitch, row["velocity"]]})
    require(not active_ids and not any(pitches.values()), "score ends with an unclosed note identity")
    return boundaries, wire


def build(project_bytes, latency):
    require(len(project_bytes) <= 2 * 1024 * 1024, "saved Project exceeds frozen input bound")
    require(hashlib.sha256(project_bytes).hexdigest() == SAVED_SHA, "full input differs from immutable native13")
    project = json.loads(project_bytes)
    require(project["tempo"] == BPM and project["swing"] == 0 and project["song_length_beats"] == 160,
            "unexpected original transport")
    require(len(project["channels"]) == 1 and len(project["plugin_instances"]) == 1
            and not project["audio_assets"] and not project["mixer_insert_slots"], "unexpected source/effect scope")
    channel = project["channels"][0]
    require(channel["id"] == 1 and channel["mixer_track_id"] == MASTER and channel["volume"] == .55
            and channel["pan"] == 0 and not channel["muted"] and not channel["solo"], "original direct MASTER channel changed")
    require(len(project["patterns"]) == len(project["clips"]) == 5, "not the complete five-pattern input")
    require([len(p["notes"]) for p in project["patterns"]] == [43, 106, 106, 107, 43], "original score changed")
    require([(c["start"], c["length"]) for c in project["clips"]]
            == [(0, 16), (16, 32), (48, 32), (80, 32), (112, 16)], "original placements changed")
    require(len(project["automation_lanes"]) == 1, "unexpected automation scope")
    lane = project["automation_lanes"][0]["lane"]
    points = [(p["position"], p["value"]) for p in lane["points"]]
    require(lane["enabled"] and lane["target"] == {"kind": "master_volume"} and lane["curve"] == "linear"
            and lane["loop_region"] is None and points == [(0, .7), (120, .7), (128, .56)], "original Master automation changed")
    boundaries, wire = expand_score(project)
    ons = sum(r["kind"] == "on" for r in boundaries)
    suppressed = sum(not r["wire_emitted"] for r in boundaries)
    require(len(boundaries) == 810 and len(wire) == 782 and ons == 405 and suppressed == 28, "full independent MIDI count differs")
    return {"schema": 1, "scope": "Independent complete original Saved13 analytical input, not actual SDK/audio evidence",
        "project_sha256": SAVED_SHA, "project_bytes": len(project_bytes), "actual_captured_latency_required": True,
        "geometry": geometry(latency), "sample_rate": RATE, "tempo": BPM, "quantum": QUANTUM,
        "static_channel_f32": f32(channel["volume"]), "source_mixer_track_id": MASTER,
        "master_points_f32": [[f32(p), f32(v)] for p, v in points],
        "master_automation_replaces_base": True, "output_gain": 1, "absolute_tolerance": 2e-6,
        "formula": "raw clamp+-8, f32 channel gain, direct MASTER addition, analytical f32 master replacing base, once tanh, Float32 gain1",
        "fixed_tail_may_be_silent": True, "unknown_metadata_tail_warning_must_remain": True,
        "note_identities": ons, "identity_boundaries": len(boundaries), "wire_messages": len(wire),
        "suppressed_same_pitch_closures": suppressed, "boundaries": boundaries, "wire": wire}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--actual-latency", type=int, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    with args.project.open("rb") as handle:
        project_bytes = handle.read(2 * 1024 * 1024 + 1)
    spec = build(project_bytes, args.actual_latency)
    with args.out.open("xb") as handle:
        handle.write((json.dumps(spec, indent=2) + "\n").encode())
    print(json.dumps({"path": str(args.out.resolve()), "identities": spec["note_identities"],
                      "boundaries": spec["identity_boundaries"], "wire": spec["wire_messages"],
                      "actual_sdk_verified": False}))
