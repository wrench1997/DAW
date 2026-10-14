"""Independent Saved13 score/observation/state/audio consistency audit.

This does not load an SDK or authenticate a preset, device, helper reaping, or
human listening. The CLI never changes inputs and creates its receipt exclusively.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import sys

import build_full_spec as score
import validate_full_pcm as pcm

MAX_JSON_BYTES = 1024 * 1024
MAX_STATE_BYTES = 64 * 1024 * 1024
MAX_SIDECAR_BYTES = MAX_STATE_BYTES * 4 + 1024
BLOCK_BYTES = 32 * 1024
MAX_MIDI = 4096
MAX_RECORDS = 1_000_000
SOURCE_BYTES = 591772
SOURCE_SHA = "5d902e91f5eb267f4cf71bd3c39f12f8551076017708530a243015adc075502b"
TABLE_FIELDS = ("phase", "worker_id", "source_id", "instance_ordinal", "record_ordinal",
                "monotonic_elapsed_ns", "unique_blob_ordinal", "offset", "length", "original_owned_capacity")


class AuditError(ValueError):
    def __init__(self, stage, message, **details):
        super().__init__(message)
        self.stage, self.details = stage, details


def require(condition, stage, message, **details):
    if not condition:
        raise AuditError(stage, message, **details)


def integer(value, minimum, maximum, field):
    require(type(value) is int and minimum <= value <= maximum, "metadata",
            "integer outside declared bounds", field=field,
            actual=value if type(value) in (int, bool, type(None)) else type(value).__name__,
            minimum=minimum, maximum=maximum)
    return value


def optional_id(value, field):
    return None if value is None else integer(value, 0, (1 << 64) - 1, field)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "observation_json", "duplicate JSON field", field=key[:128])
        result[key] = value
    return result


def reject_constant(value):
    raise AuditError("observation_json", "nonfinite JSON constant", actual=value)


def load_observation(path, report):
    path = pcm.checked_path(path)
    path_stat = path.stat()
    with path.open("rb", buffering=0) as handle:
        stat = os.fstat(handle.fileno())
        identity = pcm.FileIdentity(path, handle, path_stat, stat, "observation JSON")
        integer(stat.st_size, 1, MAX_JSON_BYTES, "observation JSON bytes")
        reader = pcm.FrozenFileReader(handle, stat.st_size, BLOCK_BYTES, "observation JSON")
        content, digest = bytearray(), hashlib.sha256()
        report.update(path=str(path), initial_bytes=stat.st_size,
                      path_signature=list(identity.path_signature), handle_signature=list(identity.handle_signature))
        try:
            for block in reader.initial_blocks():
                content.extend(block)
                digest.update(block)
            reader.check_eof_once()
            identity.verify("during complete observation read")
            report.update(sha256=digest.hexdigest(), input_stable=True)
            result = json.loads(content, object_pairs_hook=unique_object, parse_constant=reject_constant)
            require(type(result) is dict, "observation_json", "root must be StreamReplayEvidence object")
            return result
        finally:
            report["read_contract"] = reader.snapshot()


def check_midi(actual, expected):
    """Small-table comparison only. This helper does not certify a full score."""
    require(type(actual) is list and len(actual) <= MAX_MIDI, "midi", "MIDI table exceeds finite bound")
    require(len(actual) == len(expected), "midi", "MIDI row count differs", actual=len(actual), expected=len(expected))
    for index, (row, wanted) in enumerate(zip(actual, expected)):
        require(type(row) is dict, "midi", "MIDI row must be an object", index=index)
        frame = integer(row.get("frame"), 0, pcm.MAX_FRAMES, "midi.frame")
        offset = integer(row.get("block_offset"), 0, 127, "midi.block_offset")
        message = row.get("message")
        require(type(message) is list and len(message) == 3, "midi", "MIDI scalar must have three bytes", index=index)
        for byte in message:
            integer(byte, 0, 255, "midi.message")
        observed = {"frame": frame, "message": message, "block_offset": offset}
        expected_row = {"frame": wanted["frame"], "message": wanted["message"], "block_offset": wanted["offset"]}
        require(observed == expected_row and frame // 128 == wanted["block"] and frame % 128 == offset,
                "midi", "actual MIDI scalar/order/offset differs from independent score",
                index=index, actual=observed, expected=expected_row)
    return {"rows_compared": len(actual), "all_rows_equal": True,
            "compared_fields": ["frame", "message", "block_offset", "block"]}


def check_geometry(observation, geometry):
    """Exact geometry primitive; a small test geometry is not Saved13 proof."""
    pairs = (("native_latency", "native_latency"), ("musical_frames", "musical_frames"),
             ("retained_frames", "retained_frames"), ("fixed_tail", "tail_frames"),
             ("generated_frames", "generated_frames"), ("pcm_blocks", "blocks"),
             ("raw_bytes", "raw_stereo_f32_bytes"))
    for field, expected_field in pairs:
        value = integer(observation.get(field), 0, pcm.MAX_FRAMES * 8, field)
        require(value == geometry[expected_field], "geometry", "complete render geometry differs",
                field=field, actual=value, expected=geometry[expected_field])
    require(observation["generated_frames"] == observation["pcm_blocks"] * 128,
            "geometry", "generated frames are not complete Q128 blocks")
    return {"all_fields_equal": True, "independent_expected": geometry}


def check_metadata(observation, spec):
    for field in ("actual_eof", "file_stamp_matches", "state_links_match", "instance_links_match",
                  "raw_flush_sync_and_length_passed"):
        require(observation.get(field) is True, "metadata", "required replay declaration is absent/false", field=field,
                actual=observation.get(field) if type(observation.get(field)) in (bool, int, type(None)) else type(observation.get(field)).__name__)
    records = integer(observation.get("records"), 1, MAX_RECORDS, "records")
    integer(observation.get("transcript_bytes"), 1, 192_000_000, "transcript_bytes")
    by_kind = observation.get("by_kind")
    require(type(by_kind) is list and len(by_kind) == 24, "metadata", "by_kind must contain all 24 positions")
    for count in by_kind:
        integer(count, 0, MAX_RECORDS, "by_kind count")
    require(by_kind[0] == 0 and sum(by_kind) == records, "metadata", "complete record counts disagree")
    require(by_kind[2] == 2 and by_kind[9] == 1, "metadata", "source/capture/clone state record counts differ",
            state_load_records=by_kind[2], capture_records=by_kind[9])
    require(by_kind[7] == observation.get("pcm_blocks") and by_kind[8] == len(spec["wire"]),
            "metadata", "typed PCM/MIDI counts differ from complete actual projections")
    require(integer(observation.get("source_ordinal"), 1, 4, "source_ordinal") == 1
            and integer(observation.get("clone_ordinal"), 1, 4, "clone_ordinal") == 2,
            "metadata", "full source/clone incarnation ordinals differ")
    source_worker = optional_id(observation.get("source_worker_id"), "source_worker_id")
    source_instance = optional_id(observation.get("source_instance_id"), "source_instance_id")
    require(source_worker is not None and source_instance is not None and source_instance > 0,
            "metadata", "actual source worker/instance identity missing")
    require(observation.get("clone_worker_id") is None and observation.get("clone_source_instance_id") is None,
            "metadata", "complete selected clone identity must preserve literal None fields")
    require(type(observation.get("source_worker_joined_records")) is int and observation["source_worker_joined_records"] == 1,
            "metadata", "actual source Join record count differs")
    capture, report = observation.get("capture_identity"), observation.get("report_identity")
    require(type(capture) is list and len(capture) == 7 and type(report) is list and len(report) == 7,
            "metadata", "capture/report identities must retain all seven fields")
    for value in capture + report:
        integer(value, 0, (1 << 64) - 1, "capture/report identity")
    require(capture == report and capture[5] == 1 and capture[6] == source_instance
            and all(capture[index] > 0 for index in (0, 2, 3, 4)), "metadata", "complete capture/report source identity differs",
            actual_capture=capture, actual_report=report)
    integer(observation.get("capture_receipt_identity"), 1, (1 << 64) - 1, "capture_receipt_identity")
    integer(observation.get("capture_parameter_bases"), 0, 4096, "capture_parameter_bases")
    for field in ("capture_native_generation", "capture_native_dirty_revision"):
        integer(observation.get(field), 0, (1 << 64) - 1, field)
    disposal = integer(observation.get("disposal_sequence"), 1, records, "disposal_sequence")
    destruction = integer(observation.get("destruction_sequence"), 1, records, "destruction_sequence")
    completed_report = integer(observation.get("report_sequence"), 1, records, "report_sequence")
    require(disposal < destruction < completed_report, "metadata", "report did not follow actual disposal/destruction")
    integer(observation.get("reported_tail"), 0, 0xFFFFFFFF, "reported_tail")
    return {"declarations_consistent": True, "records": records,
            "disposal_sequence": disposal, "destruction_sequence": destruction, "report_sequence": completed_report,
            "declarations_are_not_sdk_authentication": True}


def decode_reference(wire):
    phase, reserved = struct.unpack_from("<II", wire)
    values = struct.unpack_from("<11Q", wire, 8)
    worker_flag, worker, source_flag, source, ordinal, record, timestamp, unique, offset, length, capacity = values
    require(reserved == 0 and phase in (1, 2, 3) and worker_flag in (0, 1) and source_flag in (0, 1),
            "state_table", "invalid phase/reserved/optional flag")
    require((worker_flag or worker == 0) and (source_flag or source == 0), "state_table", "None optional field has nonzero wire value")
    return dict(zip(TABLE_FIELDS, (phase, worker if worker_flag else None, source if source_flag else None,
                                  ordinal, record, timestamp, unique, offset, length, capacity)))


class FrozenSidecar:
    """One frozen <=256MiB+1024 hash pass and one EOF probe; seeks stay inside it."""
    def __init__(self, path, handle, report):
        path_stat, stat = path.stat(), os.fstat(handle.fileno())
        self.identity = pcm.FileIdentity(path, handle, path_stat, stat, "state sidecar")
        self.initial_bytes = integer(stat.st_size, 32, MAX_SIDECAR_BYTES, "state sidecar bytes")
        self.handle, self.report = handle, report
        self.hasher, self.initial_read, self.range_read, self.calls = hashlib.sha256(), 0, 0, 0
        self.eof_attempted = self.eof_confirmed = self.failed = False
        report.update(path=str(path), initial_bytes=self.initial_bytes,
                      path_signature=list(self.identity.path_signature), handle_signature=list(self.identity.handle_signature),
                      input_stable=False)

    def read_exact(self, length, *, initial):
        require(not self.failed and 0 < length <= BLOCK_BYTES, "state_read", "failed reader or oversized request")
        if initial:
            require(not self.eof_attempted and self.initial_read + length <= self.initial_bytes,
                    "state_read", "initial frozen extent cannot be refreshed")
        self.calls += 1
        try:
            data = self.handle.read(length)
        except Exception:
            self.failed = True
            raise
        if initial:
            self.initial_read += len(data)
            self.hasher.update(data)
        else:
            self.range_read += len(data)
        if len(data) != length:
            self.failed = True
            raise AuditError("state_read", "short read inside frozen state extent", requested=length, observed=len(data))
        return data

    def complete_hash(self):
        while self.initial_read < self.initial_bytes:
            self.read_exact(min(BLOCK_BYTES, self.initial_bytes - self.initial_read), initial=True)
        require(not self.failed and not self.eof_attempted, "state_read", "state EOF cannot be retried")
        self.eof_attempted = True
        self.calls += 1
        try:
            extra = self.handle.read(1)
        except Exception:
            self.failed = True
            raise
        self.report["extra_bytes_observed"] = len(extra)
        if extra:
            self.failed = True
            raise AuditError("state_read", "sidecar grew beyond immutable initial extent")
        self.eof_confirmed = True
        self.identity.verify("after frozen hash and EOF")
        self.report.update(sha256=self.hasher.hexdigest(), frozen_hash_input_stable=True)

    def range_chunks(self, offset, length):
        require(not self.failed and self.eof_confirmed and offset >= 0 and length > 0
                and offset + length <= self.initial_bytes, "state_read", "seek comparison exceeds original frozen extent")
        self.identity.verify("before bounded range read")
        for consumed in range(0, length, BLOCK_BYTES):
            self.handle.seek(offset + consumed)
            yield self.read_exact(min(BLOCK_BYTES, length - consumed), initial=False)
        self.identity.verify("after bounded range read")

    def snapshot(self):
        self.report.update(initial_bytes_read=self.initial_read, range_bytes_read=self.range_read,
                          read_calls_attempted=self.calls, read_block_bytes=BLOCK_BYTES,
                          initial_extent_never_refreshed=True, eof_probe_attempted=self.eof_attempted,
                          eof_confirmed=self.eof_confirmed, failed=self.failed)


def check_state_table(references, observation, initial_bytes, unique_count):
    supplied = observation.get("state_ranges")
    require(type(supplied) is list and len(supplied) == len(references) and 1 <= len(supplied) <= 4,
            "state_table", "actual state range count differs")
    payload_offset = 32 + len(references) * 96
    uniques, previous_record, previous_time, owned_capacity = {}, 0, 0, 0
    for slot, (actual, declared) in enumerate(zip(references, supplied)):
        require(type(declared) is dict and type(declared.get("slot")) is int and declared["slot"] == slot,
                "state_table", "state slot declaration differs", slot=slot)
        for field in TABLE_FIELDS:
            if field in ("worker_id", "source_id"):
                optional_id(declared.get(field), "state_ranges." + field)
            else:
                integer(declared.get(field), 0, (1 << 64) - 1, "state_ranges." + field)
            require(field in declared and declared[field] == actual[field], "state_table", "complete state table field differs",
                    slot=slot, field=field, actual=actual[field], declared=declared.get(field))
        require(actual["record_ordinal"] > previous_record and actual["monotonic_elapsed_ns"] >= previous_time
                and actual["record_ordinal"] <= observation["records"],
                "state_table", "actual range records are unordered")
        previous_record, previous_time = actual["record_ordinal"], actual["monotonic_elapsed_ns"]
        require(actual["instance_ordinal"] > 0 and 0 < actual["length"] <= actual["original_owned_capacity"] <= MAX_STATE_BYTES,
                "state_table", "opaque length/capacity/identity bound differs", slot=slot)
        owned_capacity += actual["original_owned_capacity"]
        require(owned_capacity <= MAX_STATE_BYTES * 4, "state_table", "original state capacity total exceeds bound")
        ordinal = actual["unique_blob_ordinal"]
        owner = tuple(actual[key] for key in ("phase", "worker_id", "source_id", "instance_ordinal"))
        if ordinal not in uniques:
            require(ordinal == len(uniques) + 1 and actual["offset"] == payload_offset,
                    "state_table", "unique blob order/physical offset differs", slot=slot)
            uniques[ordinal] = (owner, actual["offset"], actual["length"])
            payload_offset += actual["length"]
        else:
            require(uniques[ordinal] == (owner, actual["offset"], actual["length"]), "state_table", "aliased range changed exact owner/extent", slot=slot)
        require(actual["offset"] + actual["length"] <= initial_bytes, "state_table", "opaque range exceeds immutable file extent", slot=slot)
    require(len(uniques) == unique_count and payload_offset == initial_bytes,
            "state_table", "physical unique blobs or final EOF extent differs")
    roles = {}
    for role in ("source_load", "capture", "clone_load"):
        slot = integer(observation.get(role + "_slot"), 0, len(references) - 1, role + "_slot")
        roles[role] = references[slot]
    require(len(set(observation[role + "_slot"] for role in roles)) == 3 and len(references) == 3,
            "state_table", "full workflow requires three distinct actual state slots")
    for role, expected_phase, expected_kind in (("source_load", 1, 2), ("capture", 1, 9), ("clone_load", 2, 2)):
        actual = roles[role]
        declared = supplied[observation[role + "_slot"]]
        prefix = "clone" if role == "clone_load" else "source"
        owner = {"ordinal": integer(observation[prefix + "_ordinal"], 1, 4, prefix + "_ordinal"), "phase": expected_phase,
                 "worker_id": optional_id(observation[prefix + "_worker_id"], prefix + "_worker_id"),
                 "source_instance_id": optional_id(observation[prefix + "_source_instance_id"] if prefix == "clone" else observation["source_instance_id"], prefix + " instance")}
        require(actual["phase"] == expected_phase and actual["instance_ordinal"] == owner["ordinal"]
                and actual["worker_id"] == owner["worker_id"] and actual["source_id"] == owner["source_instance_id"],
                "state_table", "state supplied owner differs from full source/clone", role=role)
        require(type(declared.get("kind")) is int and declared["kind"] == expected_kind,
                "state_table", "state kind differs from original recorded operation", role=role)
        recorded = declared.get("record_instance")
        if role != "capture":
            require(type(recorded) is dict and set(recorded) == {"ordinal", "phase", "worker_id", "source_instance_id"},
                    "state_table", "recorded instance tuple is incomplete", role=role)
            integer(recorded["ordinal"], 1, 4, "record_instance.ordinal")
            integer(recorded["phase"], 1, 3, "record_instance.phase")
            optional_id(recorded["worker_id"], "record_instance.worker_id")
            optional_id(recorded["source_instance_id"], "record_instance.source_instance_id")
        require("record_instance" in declared and declared["record_instance"] == (None if role == "capture" else owner),
                "state_table", "original state header identity differs (capture must retain None)", role=role,
                actual_record_instance=declared.get("record_instance"), expected_record_instance=None if role == "capture" else owner)
    return roles


def audit_sidecar(path, observation, report, *, source_bytes=SOURCE_BYTES, source_sha=SOURCE_SHA):
    """Byte/table primitive. Alternate pins are exclusively for small synthetic tests."""
    path = pcm.checked_path(path)
    with path.open("rb", buffering=0) as handle:
        reader = FrozenSidecar(path, handle, report)
        try:
            header = reader.read_exact(32, initial=True)
            magic, version, count, unique_count, entry_bytes, archive_bytes = struct.unpack("<8sIIIIQ", header)
            require(magic == b"NSTATE01" and version == 1 and entry_bytes == 96 and 1 <= count <= 4
                    and 1 <= unique_count <= count and archive_bytes == reader.initial_bytes,
                    "state_table", "state sidecar header/extent differs")
            references = [decode_reference(reader.read_exact(96, initial=True)) for _ in range(count)]
            roles = check_state_table(references, observation, reader.initial_bytes, unique_count)
            report.update(range_table_matches=True, reference_count=count, unique_blob_count=unique_count,
                          original_owned_capacity_bytes=sum(row["original_owned_capacity"] for row in references))
            reader.complete_hash()
            source = roles["source_load"]
            require(source["length"] == source_bytes, "state_source", "original source-load byte length differs",
                    actual=source["length"], expected=source_bytes)
            digest = hashlib.sha256()
            for chunk in reader.range_chunks(source["offset"], source["length"]):
                digest.update(chunk)
            report["source_load_sha256"] = digest.hexdigest()
            require(digest.hexdigest() == source_sha, "state_source", "original source-load SHA differs",
                    actual=digest.hexdigest(), expected=source_sha)
            capture, clone = roles["capture"], roles["clone_load"]
            require(capture["length"] == clone["length"], "state_clone", "capture/clone payload lengths differ",
                    capture_bytes=capture["length"], clone_bytes=clone["length"])
            compared, capture_hash, clone_hash = 0, hashlib.sha256(), hashlib.sha256()
            report["capture_clone_equal_bytes"] = 0
            for left, right in zip(reader.range_chunks(capture["offset"], capture["length"]),
                                   reader.range_chunks(clone["offset"], clone["length"])):
                capture_hash.update(left)
                clone_hash.update(right)
                if left != right:
                    first = next(index for index, (a, b) in enumerate(zip(left, right)) if a != b)
                    raise AuditError("state_clone", "actual clone-load bytes differ from captured bytes",
                                     first_differing_payload_offset=compared + first)
                compared += len(left)
                report["capture_clone_equal_bytes"] = compared
            require(compared == capture["length"], "state_clone", "not every capture/clone byte compared")
            reader.identity.verify("after source hash and complete capture/clone byte comparison")
            report.update(source_load_original_pin_matches=True, capture_clone_payload_equal=True,
                          capture_sha256=capture_hash.hexdigest(), clone_load_sha256=clone_hash.hexdigest(),
                          state_semantics_verified=False, sdk_verified=False, input_stable=True, state_byte_consistency_pass=True)
        except Exception:
            reader.failed = True
            raise
        finally:
            reader.snapshot()
    return report


def sound_gate(report):
    """Matching silent files are not a successful instrument-sound result."""
    wav = report.get("wav_audio") or {}
    musical = (wav.get("metrics") or {}).get("musical") or {}
    return (report.get("file_consistency_matches") is True and report.get("frozen_input_verified") is True
            and report.get("policy", {}).get("absolute_tolerance") == 2e-6
            and type(musical.get("nonzero_frames")) is int and musical["nonzero_frames"] > 0
            and musical.get("nonfinite_samples") == 0)


def error_row(stage, reason):
    message = str(reason).encode("utf-8", errors="replace")[:4096].decode("utf-8", errors="ignore")
    row = {"stage": reason.stage if isinstance(reason, AuditError) else stage,
           "type": type(reason).__name__, "error": message}
    if isinstance(reason, AuditError):
        row["details"] = reason.details
    if isinstance(reason, OSError):
        row.update(errno=reason.errno, winerror=getattr(reason, "winerror", None))
    return row


def validate_all(project_path, observation_path, wav_path, *, output_path=None):
    receipt = {"schema": 1, "scope": "complete original Saved13 file/observation consistency only",
               "sdk_verified": False, "physical_listening": False, "source_helper_strict_reap": False,
               "state_semantics_verified": False, "whole_sound_files_pass": False,
               "observation_json": {}, "state_sidecar": {}, "errors": [], "audio_audit": None}
    observation, spec = None, None
    try:
        project_path, observation_path, wav_path = map(pcm.checked_path, (project_path, observation_path, wav_path))
        observation = load_observation(observation_path, receipt["observation_json"])
        raw_path, sidecar_path = map(pcm.checked_path, (observation["raw_path"], observation["state_sidecar_path"]))
        paths = [project_path, observation_path, wav_path, raw_path, sidecar_path]
        require(len(set(path.resolve() for path in paths)) == len(paths), "inputs", "score/observation/raw/WAV/state paths must differ")
        if output_path is not None:
            require(pcm.checked_path(output_path).resolve() not in {path.resolve() for path in paths}, "inputs", "receipt must not alias any input")
        latency = integer(observation.get("native_latency"), 0, score.RATE * 30, "native_latency")
        project_bytes = pcm.bounded_project(project_path)
        spec = score.build(project_bytes, latency)
        # The existing PCM Plan also freezes and hashes the independent builder
        # source; this new wrapper cannot substitute a fitted musical policy.
        pcm.Plan.saved13(project_bytes, latency)
        receipt["independent_score"] = {"project_sha256": spec["project_sha256"],
            "note_identities": spec["note_identities"], "identity_boundaries": spec["identity_boundaries"],
            "wire_messages": spec["wire_messages"], "geometry": spec["geometry"]}
        receipt["observation"] = check_metadata(observation, spec)
        receipt["geometry"] = check_geometry(observation, spec["geometry"])
        receipt["midi"] = check_midi(observation.get("midi"), spec["wire"])
    except Exception as reason:
        receipt["errors"].append(error_row("inputs_and_observation", reason))
    if observation is not None:
        try:
            audit_sidecar(observation["state_sidecar_path"], observation, receipt["state_sidecar"])
        except Exception as reason:
            receipt["errors"].append(error_row("state_sidecar", reason))
    if spec is not None and observation is not None:
        try:
            receipt["audio_audit"] = pcm.validate_saved13(project_path, observation["raw_path"], wav_path,
                observation["native_latency"], block_bytes=BLOCK_BYTES, reported_tail=observation["reported_tail"])
            if not sound_gate(receipt["audio_audit"]):
                receipt["errors"].append({"stage": "whole_sound", "error": "complete sample consistency or nonzero musical-sound gate failed"})
        except Exception as reason:
            receipt["errors"].append(error_row("all_raw_and_wav_samples", reason))
    receipt["whole_sound_files_pass"] = (not receipt["errors"] and receipt["state_sidecar"].get("state_byte_consistency_pass") is True
                                         and receipt["audio_audit"] is not None and sound_gate(receipt["audio_audit"]))
    receipt["error_count"] = len(receipt["errors"])
    return receipt


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("project", "observation", "wav", "out"):
        parser.add_argument("--" + option, type=Path, required=True)
    args = parser.parse_args(argv)
    output = pcm.checked_path(args.out)
    try:
        with output.open("xb") as handle:
            receipt = validate_all(args.project, args.observation, args.wav, output_path=output)
            handle.write((json.dumps(receipt, indent=2, allow_nan=False) + "\n").encode("utf-8"))
            handle.flush()
            os.fsync(handle.fileno())
    except Exception as reason:
        print(json.dumps({"whole_sound_files_pass": False, "receipt_created": False,
                          "error": error_row("exclusive_receipt", reason)}, allow_nan=False), file=sys.stderr)
        return 2
    print(json.dumps({"out": str(output), "whole_sound_files_pass": receipt["whole_sound_files_pass"],
                      "error_count": receipt["error_count"], "sdk_verified": False, "physical_listening": False}))
    return 0 if receipt["whole_sound_files_pass"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
