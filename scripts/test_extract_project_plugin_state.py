"""Project state extraction regressions; no plug-in, audio device or GUI opens."""

import base64
import copy
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import extract_project_plugin_state as extract


def envelope(component=b"component", controller=None, version=1):
    controller_length = extract.NO_CONTROLLER_STATE if controller is None else len(controller)
    return (extract.STATE_MAGIC + struct.pack("<III", version, len(component), controller_length)
            + component + (controller or b""))


def project(state=None):
    return {
        "format_version": 12,
        "name": "Synthetic project",
        "channels": [
            {"id": 19, "name": "Unrelated", "instrument_plugin_instance_id": None},
            {"id": 7, "name": "Piano", "instrument_plugin_instance_id": 23},
        ],
        "plugin_instances": [
            {"id": 99, "name": "Other", "format": "vst3", "opaque_state": envelope(b"other").hex()},
            {"id": 23, "name": "Synth", "vendor": "Test", "format": "vst3", "role": "instrument",
             "path": "C:/plug-ins/test.vst3", "uid": "00112233445566778899AABBCCDDEEFF",
             "opaque_state": (state if state is not None else envelope()).hex()},
        ],
    }


class EncodingTests(unittest.TestCase):
    def test_valid_hex_is_not_base64_despite_ambiguous_alphabet(self):
        # 48 state bytes produce 96 hex characters: a base64 decoder accepts
        # this exact hex string without even needing padding.
        state = envelope(b"\0\xffvendor\x80!", b"controller")
        encoded = state.hex()
        self.assertNotEqual(base64.b64decode(encoded), state)
        self.assertEqual(extract.decode_project_hex(encoded), state)
        self.assertEqual(extract.decode_project_hex(encoded.upper()), state)
        self.assertEqual(extract.inspect_state_envelope(state)["component_bytes"], 10)
        with self.assertRaisesRegex(extract.ExtractionError, "magic"):
            extract.inspect_state_envelope(base64.b64decode(encoded))

    def test_base64_string_is_rejected_instead_of_guessed(self):
        encoded = base64.b64encode(envelope()).decode("ascii")
        with self.assertRaises(extract.ExtractionError):
            extract.decode_project_hex(encoded)

    def test_non_hex_empty_odd_whitespace_and_legacy_arrays_are_rejected(self):
        for value in ("", "123", "0xz1", "00 11", "0011\n", "éé", None, [], [0, 1, 255], 123):
            with self.subTest(value=value), self.assertRaises(extract.ExtractionError):
                extract.decode_project_hex(value)

    def test_state_size_limit_is_checked_before_decoding(self):
        with patch.object(extract, "MAX_STATE_PAYLOAD_BYTES", 1):
            with self.assertRaisesRegex(extract.ExtractionError, "64 MiB"):
                extract.decode_project_hex(envelope(b"ab").hex())


class EnvelopeTests(unittest.TestCase):
    def test_component_and_controller_lengths_are_exact(self):
        result = extract.inspect_state_envelope(envelope(b"abcd", b"efg"))
        self.assertEqual(result["version"], 1)
        self.assertEqual(result["component_bytes"], 4)
        self.assertEqual(result["controller_bytes"], 3)
        self.assertTrue(result["controller_present"])
        self.assertEqual(result["payload_bytes"], 7)
        self.assertEqual(result["total_bytes"], 35)

    def test_absent_controller_differs_from_empty_controller(self):
        absent = extract.inspect_state_envelope(envelope(b"", None))
        empty = extract.inspect_state_envelope(envelope(b"", b""))
        self.assertIsNone(absent["controller_bytes"])
        self.assertFalse(absent["controller_present"])
        self.assertEqual(empty["controller_bytes"], 0)
        self.assertTrue(empty["controller_present"])

    def test_wrong_magic_and_raw_legacy_payload_are_rejected(self):
        for value in (b"", b"old component blob", b"VST3HOST_STATF\0\0" + envelope()[16:]):
            with self.subTest(value=value), self.assertRaisesRegex(extract.ExtractionError, "magic"):
                extract.inspect_state_envelope(value)

    def test_truncated_headers_are_rejected(self):
        for length in range(16, 28):
            with self.subTest(length=length), self.assertRaisesRegex(extract.ExtractionError, "truncated"):
                extract.inspect_state_envelope(envelope()[:length])

    def test_unsupported_versions_are_rejected(self):
        for version in (0, 2, 0xFFFFFFFF):
            with self.subTest(version=version), self.assertRaisesRegex(extract.ExtractionError, "version"):
                extract.inspect_state_envelope(envelope(version=version))

    def test_truncated_and_trailing_payloads_are_rejected(self):
        valid = envelope(b"abc", b"def")
        for value in (valid[:-1], valid + b"x"):
            with self.subTest(value=value), self.assertRaisesRegex(extract.ExtractionError, "size"):
                extract.inspect_state_envelope(value)

    def test_combined_payload_limit_rejects_huge_lengths_without_allocation(self):
        header = extract.STATE_MAGIC + struct.pack("<III", 1, extract.MAX_STATE_PAYLOAD_BYTES, 1)
        with self.assertRaisesRegex(extract.ExtractionError, "combined payload"):
            extract.inspect_state_envelope(header)
        header = extract.STATE_MAGIC + struct.pack("<III", 1, 0xFFFFFFFF, extract.NO_CONTROLLER_STATE)
        with self.assertRaisesRegex(extract.ExtractionError, "combined payload"):
            extract.inspect_state_envelope(header)


class SelectionTests(unittest.TestCase):
    def test_id_uses_global_plugin_reference_and_index_is_explicit(self):
        channel, plugin, selection = extract.select_channel_plugin(project(), channel_id=7)
        self.assertEqual(channel["id"], 7)
        self.assertEqual(plugin["id"], 23)
        self.assertEqual(selection["channel_index"], 1)
        by_index = extract.select_channel_plugin(project(), channel_index=1)
        self.assertEqual(by_index[:2], (channel, plugin))
        self.assertEqual(by_index[2]["method"], "channel_index")

    def test_missing_and_duplicate_id_are_rejected(self):
        with self.assertRaisesRegex(extract.ExtractionError, "got 0"):
            extract.select_channel_plugin(project(), channel_id=999)
        data = project()
        data["channels"].append(copy.deepcopy(data["channels"][1]))
        for selector in ({"channel_id": 7}, {"channel_index": 1}):
            with self.subTest(selector=selector), self.assertRaises(extract.ExtractionError):
                extract.select_channel_plugin(data, **selector)

    def test_selection_requires_one_selector_and_integer_identity(self):
        for selector in ({}, {"channel_id": 7, "channel_index": 1}, {"channel_id": True},
                         {"channel_id": -1}, {"channel_index": -1}, {"channel_index": 2},
                         {"channel_index": True}):
            with self.subTest(selector=selector), self.assertRaises(extract.ExtractionError):
                extract.select_channel_plugin(project(), **selector)

    def test_unassigned_missing_duplicate_and_invalid_plugin_references_fail(self):
        data = project()
        with self.assertRaisesRegex(extract.ExtractionError, "instrument_plugin_instance_id"):
            extract.select_channel_plugin(data, channel_id=19)
        for reference in (None, True, -1, 1 << 64, 1234):
            data = project()
            data["channels"][1]["instrument_plugin_instance_id"] = reference
            with self.subTest(reference=reference), self.assertRaises(extract.ExtractionError):
                extract.select_channel_plugin(data, channel_id=7)
        data = project()
        data["plugin_instances"].append(copy.deepcopy(data["plugin_instances"][1]))
        with self.assertRaisesRegex(extract.ExtractionError, "got 2"):
            extract.select_channel_plugin(data, channel_id=7)

    def test_missing_state_and_wrong_plugin_format_fail(self):
        for field, value in (("format", "vst2"), ("uid", 123)):
            data = project()
            data["plugin_instances"][1][field] = value
            with self.subTest(field=field), self.assertRaises(extract.ExtractionError):
                extract.select_channel_plugin(data, channel_id=7)
        data = project()
        del data["plugin_instances"][1]["opaque_state"]
        with self.assertRaisesRegex(extract.ExtractionError, "no opaque_state"):
            extract.select_channel_plugin(data, channel_id=7)

    def test_malformed_collections_and_boolean_ids_fail(self):
        for key, value in (("channels", {}), ("channels", [None]),
                           ("plugin_instances", None), ("plugin_instances", [123])):
            data = project()
            data[key] = value
            with self.subTest(key=key, value=value), self.assertRaises(extract.ExtractionError):
                extract.select_channel_plugin(data, channel_id=7)
        for key in ("channels", "plugin_instances"):
            data = project()
            data[key][0]["id"] = True
            with self.subTest(key=key), self.assertRaisesRegex(extract.ExtractionError, "unsigned"):
                extract.select_channel_plugin(data, channel_id=7)


class FileAndCliTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.source = self.root / "source.citrus"
        self.state = envelope(b"\0\xffvendor-state", b"controller")
        self.source.write_text(json.dumps(project(self.state)), encoding="utf-8")
        self.output = self.root / "state.bin"
        self.receipt = self.root / "state.json"

    def extract(self, **kwargs):
        return extract.extract_state(self.source, channel_id=7, output_path=self.output, **kwargs)

    def test_success_preserves_project_and_binds_actual_state_and_identity(self):
        original = self.source.read_bytes()
        result = self.extract()
        self.assertEqual(self.source.read_bytes(), original)
        self.assertEqual(self.output.read_bytes(), self.state)
        self.assertEqual(json.loads(self.receipt.read_text(encoding="utf-8")), result)
        self.assertEqual(result["source_project"]["sha256"], hashlib.sha256(original).hexdigest())
        self.assertEqual(result["source_project"]["bytes"], len(original))
        self.assertEqual(result["channel"]["id"], 7)
        self.assertEqual(result["plugin"]["id"], 23)
        self.assertEqual(result["plugin"]["class_uid"], "00112233445566778899AABBCCDDEEFF")
        self.assertEqual(result["state"]["bytes"], len(self.state))
        self.assertEqual(result["state"]["sha256"], hashlib.sha256(self.state).hexdigest())
        self.assertEqual(result["envelope"]["controller_bytes"], len(b"controller"))

    def test_existing_state_or_receipt_is_preserved_with_no_other_output(self):
        for existing, other in ((self.output, self.receipt), (self.receipt, self.output)):
            with self.subTest(existing=existing):
                existing.write_bytes(b"preserve user evidence")
                with self.assertRaisesRegex(extract.ExtractionError, "refusing to overwrite"):
                    self.extract()
                self.assertEqual(existing.read_bytes(), b"preserve user evidence")
                self.assertFalse(other.exists())
                existing.unlink()

    def test_no_output_is_created_for_invalid_hex_or_envelope(self):
        for value in ("not hex", envelope()[:-1].hex(), base64.b64encode(self.state).decode("ascii")):
            data = project()
            data["plugin_instances"][1]["opaque_state"] = value
            self.source.write_text(json.dumps(data), encoding="utf-8")
            with self.subTest(value=value), self.assertRaises(extract.ExtractionError):
                self.extract()
            self.assertFalse(self.output.exists())
            self.assertFalse(self.receipt.exists())

    def test_duplicate_json_state_field_is_rejected(self):
        value = self.source.read_text(encoding="utf-8").replace(
            '"opaque_state":', '"opaque_state": "", "opaque_state":', 1
        )
        self.source.write_text(value, encoding="utf-8")
        with self.assertRaisesRegex(extract.ExtractionError, "duplicate JSON field"):
            self.extract()
        self.assertFalse(self.output.exists())

    def test_nonfinite_json_and_nonobject_roots_are_rejected(self):
        for value in ('{"channels": NaN}', "[]", "not JSON"):
            self.source.write_text(value, encoding="utf-8")
            with self.subTest(value=value), self.assertRaises(extract.ExtractionError):
                self.extract()
        self.assertFalse(self.output.exists())

    def test_output_extension_and_missing_parent_fail_without_creation(self):
        with self.assertRaisesRegex(extract.ExtractionError, "end in"):
            extract.extract_state(self.source, channel_id=7, output_path=self.root / "wrong.txt")
        with self.assertRaisesRegex(extract.ExtractionError, "parent directory"):
            extract.extract_state(self.source, channel_id=7, output_path=self.root / "absent" / "state.bin")
        self.assertFalse((self.root / "absent").exists())

    def test_exclusive_create_closes_check_open_race_without_overwriting(self):
        real_open = Path.open

        def racing_open(path, mode="r", *args, **kwargs):
            if path == self.output and mode == "xb":
                with real_open(path, "wb") as stream:
                    stream.write(b"other writer")
            return real_open(path, mode, *args, **kwargs)

        with patch.object(Path, "open", racing_open):
            with self.assertRaises(FileExistsError):
                self.extract()
        self.assertEqual(self.output.read_bytes(), b"other writer")
        self.assertFalse(self.receipt.exists())

    def test_receipt_create_race_preserves_existing_receipt_and_removes_owned_state(self):
        real_open = Path.open

        def racing_open(path, mode="r", *args, **kwargs):
            if path == self.receipt and mode == "xb":
                with real_open(path, "wb") as stream:
                    stream.write(b"other receipt")
            return real_open(path, mode, *args, **kwargs)

        with patch.object(Path, "open", racing_open):
            with self.assertRaises(FileExistsError):
                self.extract()
        self.assertEqual(self.receipt.read_bytes(), b"other receipt")
        self.assertFalse(self.output.exists())

    def test_write_failure_removes_both_owned_outputs_and_preserves_source(self):
        original = self.source.read_bytes()
        with patch.object(extract.os, "fsync", side_effect=OSError("synthetic disk failure")):
            with self.assertRaisesRegex(OSError, "synthetic disk failure"):
                self.extract()
        self.assertFalse(self.output.exists())
        self.assertFalse(self.receipt.exists())
        self.assertEqual(self.source.read_bytes(), original)

    def test_cli_explicit_selector_and_new_custom_receipt(self):
        custom = self.root / "identity.json"
        script = Path(extract.__file__)
        result = subprocess.run(
            [sys.executable, str(script), str(self.source), "--channel-index", "1",
             "--output", str(self.output), "--receipt", str(custom)],
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["state"]["sha256"], hashlib.sha256(self.state).hexdigest())
        self.assertEqual(json.loads(custom.read_text(encoding="utf-8"))["selection"]["method"], "channel_index")
        again = subprocess.run(
            [sys.executable, str(script), str(self.source), "--channel-id", "7",
             "--output", str(self.output)],
            capture_output=True, text=True, timeout=10,
        )
        self.assertNotEqual(again.returncode, 0)
        self.assertIn("refusing to overwrite", again.stderr)
        self.assertEqual(self.output.read_bytes(), self.state)

    def test_cli_requires_selector(self):
        result = subprocess.run(
            [sys.executable, str(Path(extract.__file__)), str(self.source), "--output", str(self.output)],
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 2)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
