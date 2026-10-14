"""Small primitive/file regressions; these are not a full Saved13/SDK run."""
from contextlib import redirect_stderr, redirect_stdout
import copy
import hashlib
import io
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock

import validate_full_observation as audit


def wire_fixture():
    return [{"frame": 0, "block": 0, "offset": 0, "message": [0x90, 60, 91]},
            {"frame": 129, "block": 1, "offset": 1, "message": [0x80, 60, 0]},
            {"frame": 129, "block": 1, "offset": 1, "message": [0x90, 64, 71]}]


def actual_midi(wire):
    return [{"frame": row["frame"], "message": list(row["message"]), "block_offset": row["offset"]} for row in wire]


def geometry_fixture():
    return {"native_latency": 3, "musical_frames": 50, "retained_frames": 75, "tail_frames": 25,
            "generated_frames": 128, "blocks": 1, "raw_stereo_f32_bytes": 1024}


def geometry_observation(geometry):
    return {"native_latency": geometry["native_latency"], "musical_frames": geometry["musical_frames"],
            "retained_frames": geometry["retained_frames"], "fixed_tail": geometry["tail_frames"],
            "generated_frames": geometry["generated_frames"], "pcm_blocks": geometry["blocks"],
            "raw_bytes": geometry["raw_stereo_f32_bytes"]}


def owner(ordinal, phase, worker, source):
    return {"ordinal": ordinal, "phase": phase, "worker_id": worker, "source_instance_id": source}


def sidecar_fixture(path, *, source=b"source-original", capture=b"capture-original", clone=None):
    """Independent NSTATE01 binary fixture, exclusively small test-owned bytes."""
    if clone is None:
        clone = capture
    offset = 32 + 3 * 96
    ranges, rows = [], []
    for slot, (phase, worker, source_id, ordinal, record, timestamp, payload, capacity) in enumerate([
        (1, 0, 7, 1, 2, 11, source, len(source) + 3),
        (1, 0, 7, 1, 5, 19, capture, len(capture) + 7),
        (2, None, None, 2, 10, 25, clone, len(clone) + 11),
    ]):
        ranges.append({"slot": slot, "kind": 9 if slot == 1 else 2,
                       "record_instance": None if slot == 1 else owner(ordinal, phase, worker, source_id),
                       "phase": phase, "worker_id": worker, "source_id": source_id,
                       "instance_ordinal": ordinal, "record_ordinal": record,
                       "monotonic_elapsed_ns": timestamp, "unique_blob_ordinal": slot + 1,
                       "offset": offset, "length": len(payload), "original_owned_capacity": capacity})
        rows.append(struct.pack("<II11Q", phase, 0, int(worker is not None), worker or 0,
                                int(source_id is not None), source_id or 0, ordinal, record,
                                timestamp, slot + 1, offset, len(payload), capacity))
        offset += len(payload)
    header = struct.pack("<8sIIIIQ", b"NSTATE01", 1, 3, 3, 96, offset)
    path.write_bytes(header + b"".join(rows) + source + capture + clone)
    observation = {"records": 30, "state_ranges": ranges, "source_load_slot": 0, "capture_slot": 1,
                   "clone_load_slot": 2, "source_ordinal": 1, "source_worker_id": 0, "source_instance_id": 7,
                   "clone_ordinal": 2, "clone_worker_id": None, "clone_source_instance_id": None}
    return observation, source


def state_check(path, observation, source, report):
    return audit.audit_sidecar(path, observation, report, source_bytes=len(source),
                               source_sha=hashlib.sha256(source).hexdigest())


class TrackedReader:
    def __init__(self, delegate, callback=None):
        self.delegate, self.callback, self.requests = delegate, callback, []

    def read(self, length):
        if not 0 < length <= audit.BLOCK_BYTES:
            raise AssertionError("actual reader requested an unbounded block")
        self.requests.append(length)
        result = self.delegate.read(length)
        if self.callback is not None:
            callback, self.callback = self.callback, None
            callback()
        return result

    def __getattr__(self, name):
        return getattr(self.delegate, name)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.delegate.close()


class FullObservationTests(unittest.TestCase):
    def test_midi_primitive_compares_exact_order_scalar_and_offset(self):
        wire = wire_fixture()
        checked = audit.check_midi(actual_midi(wire), wire)
        self.assertTrue(checked["all_rows_equal"])
        self.assertEqual(checked["rows_compared"], 3)

    def test_one_midi_message_error_cannot_hide_behind_equal_count(self):
        wire = wire_fixture()
        actual = actual_midi(wire)
        actual[1]["message"][1] = 61
        with self.assertRaises(audit.AuditError) as caught:
            audit.check_midi(actual, wire)
        self.assertEqual(caught.exception.details["index"], 1)
        self.assertEqual(caught.exception.details["actual"]["message"], [0x80, 61, 0])
        self.assertEqual(caught.exception.details["expected"]["message"], [0x80, 60, 0])

    def test_offset_frame_and_same_frame_order_are_not_count_only_checks(self):
        wire = wire_fixture()
        for change in ("offset", "frame", "order"):
            actual = actual_midi(wire)
            if change == "offset":
                actual[1]["block_offset"] = 2
            elif change == "frame":
                actual[1]["frame"] = 130
            else:
                actual[1], actual[2] = actual[2], actual[1]
            with self.assertRaises(audit.AuditError):
                audit.check_midi(actual, wire)

    def test_midi_bool_and_more_than_4096_rows_are_refused(self):
        wire = wire_fixture()
        actual = actual_midi(wire)
        actual[0]["frame"] = False
        with self.assertRaises(audit.AuditError):
            audit.check_midi(actual, wire)
        with self.assertRaises(audit.AuditError):
            audit.check_midi([{}] * 4097, [])

    def test_each_complete_geometry_field_must_equal_independent_geometry(self):
        geometry = geometry_fixture()
        actual = geometry_observation(geometry)
        self.assertTrue(audit.check_geometry(actual, geometry)["all_fields_equal"])
        for field in actual:
            changed = dict(actual)
            changed[field] += 1
            with self.assertRaises(audit.AuditError) as caught:
                audit.check_geometry(changed, geometry)
            self.assertEqual(caught.exception.details["field"], field)

    def test_byte_primitive_compares_actual_small_files_and_all_state_fields(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state.bin"
            observation, source = sidecar_fixture(path)
            before = path.read_bytes()
            report = {}
            state_check(path, observation, source, report)
            self.assertTrue(report["state_byte_consistency_pass"])
            self.assertEqual(report["capture_clone_equal_bytes"], len(b"capture-original"))
            self.assertEqual(report["initial_bytes_read"], len(before))
            self.assertEqual(report["range_bytes_read"], len(source) + len(b"capture-original") * 2)
            self.assertTrue(report["eof_probe_attempted"] and report["eof_confirmed"])
            self.assertFalse(report["state_semantics_verified"] or report["sdk_verified"])
            self.assertEqual(path.read_bytes(), before)

    def test_wrong_original_source_hash_does_not_become_state_semantics_success(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state.bin"
            observation, source = sidecar_fixture(path)
            report = {}
            with self.assertRaises(audit.AuditError) as caught:
                audit.audit_sidecar(path, observation, report, source_bytes=len(source), source_sha="0" * 64)
            self.assertEqual(caught.exception.stage, "state_source")
            self.assertEqual(caught.exception.details["actual"], hashlib.sha256(source).hexdigest())
            self.assertTrue(report["failed"])
            self.assertNotIn("state_byte_consistency_pass", report)

    def test_small_fixture_cannot_pass_production_saved13_source_pin(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state.bin"
            observation, _ = sidecar_fixture(path)
            with self.assertRaises(audit.AuditError) as caught:
                audit.audit_sidecar(path, observation, {})
            self.assertEqual(caught.exception.details["expected"], audit.SOURCE_BYTES)

    def test_clone_payload_difference_keeps_exact_first_bad_offset(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state.bin"
            captured = b"capture-original"
            changed = captured[:7] + bytes([captured[7] ^ 1]) + captured[8:]
            observation, source = sidecar_fixture(path, capture=captured, clone=changed)
            report = {}
            with self.assertRaises(audit.AuditError) as caught:
                state_check(path, observation, source, report)
            self.assertEqual(caught.exception.stage, "state_clone")
            self.assertEqual(caught.exception.details["first_differing_payload_offset"], 7)
            self.assertNotIn("state_byte_consistency_pass", report)

    def test_range_optional_identity_time_capacity_and_header_are_exact(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state.bin"
            original, source = sidecar_fixture(path)
            for field, value in [("worker_id", None), ("source_id", 0), ("monotonic_elapsed_ns", 12),
                                 ("original_owned_capacity", len(source)), ("record_ordinal", 3)]:
                observation = copy.deepcopy(original)
                observation["state_ranges"][0][field] = value
                with self.assertRaises(audit.AuditError):
                    state_check(path, observation, source, {})
            observation = copy.deepcopy(original)
            observation["state_ranges"][1]["record_instance"] = owner(1, 1, 0, 7)
            with self.assertRaises(audit.AuditError):
                state_check(path, observation, source, {})
            observation = copy.deepcopy(original)
            observation["state_ranges"][2]["record_instance"]["worker_id"] = 0
            with self.assertRaises(audit.AuditError):
                state_check(path, observation, source, {})

    def test_bool_record_tuple_cannot_alias_integer_one_or_some_zero(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state.bin"
            original, source = sidecar_fixture(path)
            for field, value in [("ordinal", True), ("phase", True), ("worker_id", False)]:
                observation = copy.deepcopy(original)
                observation["state_ranges"][0]["record_instance"][field] = value
                with self.assertRaises(audit.AuditError):
                    state_check(path, observation, source, {})

    def test_actual_sidecar_growth_does_not_refresh_initial_extent_or_repeat_eof(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state.bin"
            observation, source = sidecar_fixture(path)
            initial_bytes = path.stat().st_size
            real_open, readers = Path.open, []
            def append():
                with real_open(path, "ab", buffering=0) as handle:
                    handle.write(b"actual-growth")
            def open_path(selected, *args, **kwargs):
                delegate = real_open(selected, *args, **kwargs)
                if selected == path and (args[0] if args else kwargs.get("mode")) == "rb":
                    traced = TrackedReader(delegate, append)
                    readers.append(traced)
                    return traced
                return delegate
            report = {}
            with mock.patch.object(Path, "open", open_path):
                with self.assertRaises(audit.AuditError):
                    state_check(path, observation, source, report)
            self.assertEqual(report["initial_bytes"], initial_bytes)
            self.assertEqual(report["initial_bytes_read"], initial_bytes)
            self.assertEqual(report["extra_bytes_observed"], 1)
            self.assertEqual(readers[0].requests.count(1), 1)
            self.assertFalse(report["eof_confirmed"])

    def test_change_after_frozen_hash_is_caught_before_seek_comparison(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "state.bin"
            observation, source = sidecar_fixture(path)
            original_hash = audit.FrozenSidecar.complete_hash
            def hash_then_append(reader):
                original_hash(reader)
                with path.open("ab", buffering=0) as handle:
                    handle.write(b"new-byte")
            report = {}
            with mock.patch.object(audit.FrozenSidecar, "complete_hash", hash_then_append):
                with self.assertRaises(ValueError):
                    state_check(path, observation, source, report)
            self.assertEqual(report["range_bytes_read"], 0)
            self.assertTrue(report["eof_confirmed"] and report["failed"])
            self.assertTrue(report["initial_extent_never_refreshed"])

    def test_json_duplicate_nonfinite_and_over_bound_files_are_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "observation.json"
            for content in (b'{"records":1,"records":2}', b'{"records":NaN}', b" " * (audit.MAX_JSON_BYTES + 1)):
                path.write_bytes(content)
                with self.assertRaises(ValueError):
                    audit.load_observation(path, {})

    def test_matching_silence_fails_sound_gate_but_real_silent_tail_is_allowed(self):
        report = {"file_consistency_matches": True, "frozen_input_verified": True,
                  "policy": {"absolute_tolerance": 2e-6}, "wav_audio": {"metrics": {
                      "musical": {"nonzero_frames": 0, "nonfinite_samples": 0},
                      "coda": {"nonzero_frames": 0}, "tail": {"nonzero_frames": 0}}}}
        self.assertFalse(audit.sound_gate(report))
        report["wav_audio"]["metrics"]["musical"]["nonzero_frames"] = 1
        self.assertTrue(audit.sound_gate(report))
        report["policy"]["absolute_tolerance"] = 1e-3
        self.assertFalse(audit.sound_gate(report))

    def test_cli_failure_creates_receipt_and_preserves_existing_out_and_inputs(self):
        with tempfile.TemporaryDirectory() as folder:
            directory = Path(folder)
            project, observation, wav, output = [directory / name for name in ("project.citrus", "observation.json", "output.wav", "receipt.json")]
            project.write_bytes(b"original-user-file")
            observation.write_bytes(b"{}")
            wav.write_bytes(b"original-audio-file")
            args = ["--project", str(project), "--observation", str(observation), "--wav", str(wav), "--out", str(output)]
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(audit.main(args), 2)
            original_receipt = output.read_bytes()
            report = json.loads(original_receipt)
            self.assertFalse(report["whole_sound_files_pass"] or report["sdk_verified"] or report["physical_listening"] or report["source_helper_strict_reap"])
            self.assertGreater(report["error_count"], 0)
            with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                self.assertEqual(audit.main(args), 2)
            self.assertEqual(output.read_bytes(), original_receipt)
            self.assertEqual(project.read_bytes(), b"original-user-file")
            self.assertEqual(wav.read_bytes(), b"original-audio-file")


if __name__ == "__main__":
    unittest.main()
