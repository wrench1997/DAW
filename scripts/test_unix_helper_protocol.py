"""Linux/Unix helper descriptor acceptance. Set CITRUS_VST3_HELPER to the built helper.

These tests do not load a plugin or access a display/audio device. Missing helper is
an explicit skip, never evidence for GUI or descriptor acceptance.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


@unittest.skipUnless(os.name == "posix", "Unix descriptor tests")
class UnixHelperProtocolTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        candidate = Path(os.environ.get("CITRUS_VST3_HELPER", "target/debug/vst3-host-helper"))
        if not candidate.is_file():
            raise unittest.SkipTest("build helper or set CITRUS_VST3_HELPER")
        cls.helper = str(candidate.resolve())

    def run_helper(self, **kwargs):
        return subprocess.run([self.helper], input='{"Editor":{"command":"Query"}}\n"Shutdown"\n',
                              text=True, timeout=10, **kwargs)

    def assert_query(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(len(lines), 1, result.stdout)
        state = json.loads(lines[0])["EditorState"]["state"]
        self.assertFalse(state["open"])

    def test_protocol_and_diagnostics_have_separate_descriptors(self):
        result = self.run_helper(stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assert_query(result)
        self.assertIn("VST3 Host Helper Process Started", result.stderr)

    def test_merged_inherited_stderr_is_redirected_away_from_protocol(self):
        result = self.run_helper(stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.assert_query(result)
        self.assertNotIn("Process Started", result.stdout)

    def test_closed_stderr_has_a_safe_null_sink(self):
        result = self.run_helper(stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 preexec_fn=lambda: os.close(2))
        self.assert_query(result)

    def test_read_only_stdout_is_rejected_before_claiming_success(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "read-only-protocol"
            path.write_bytes(b"unchanged")
            with path.open("rb") as stream:
                result = self.run_helper(stdout=stream, stderr=subprocess.PIPE)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("could not isolate helper protocol output", result.stderr)
            self.assertNotIn("Process Started", result.stderr)
            self.assertEqual(path.read_bytes(), b"unchanged")


if __name__ == "__main__":
    unittest.main()
