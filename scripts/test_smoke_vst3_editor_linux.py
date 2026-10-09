import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import smoke_vst3_editor_linux as smoke


class LinuxFixtureReceiptTests(unittest.TestCase):
    def fixture(self, root):
        for name in smoke.SOURCE_FILES:
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(name.encode())
        bundle = root / "target/bundles/CitrusEditorFixture.vst3"
        binary = bundle / "Contents/x86_64-linux/CitrusEditorFixture.so"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"\x7fELFsource fixture test")
        receipt = {"schema": 1, "variant": "editor", "target": "x86_64-unknown-linux-gnu",
                   "upstream_commit": smoke.UPSTREAM_COMMIT,
                   "sources": {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in smoke.SOURCE_FILES},
                   "binary": binary.relative_to(bundle).as_posix(), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
        (bundle / "source-build.json").write_text(json.dumps(receipt))
        return bundle, binary

    def test_receipt_matches_exact_sources_and_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, _ = self.fixture(root)
            self.assertEqual(smoke.verify_fixture(root, "editor"), bundle)
            (root / "src/native_linux.rs").write_text("changed")
            with self.assertRaisesRegex(smoke.SmokeError, "Stale fixture"):
                smoke.verify_fixture(root, "editor")

    def test_binary_and_platform_mismatch_are_not_acceptance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, binary = self.fixture(root)
            binary.write_bytes(b"MZwrong target")
            with self.assertRaisesRegex(smoke.SmokeError, "ELF bytes"):
                smoke.verify_fixture(root, "editor")

    def test_receipt_path_is_portable_and_host_separators_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bundle, _ = self.fixture(root)
            receipt_path = bundle / "source-build.json"
            receipt = json.loads(receipt_path.read_text())
            self.assertEqual(receipt["binary"],
                             "Contents/x86_64-linux/CitrusEditorFixture.so")
            self.assertEqual(smoke.verify_fixture(root, "editor"), bundle)
            receipt["binary"] = receipt["binary"].replace("/", "\\")
            receipt_path.write_text(json.dumps(receipt))
            with self.assertRaisesRegex(smoke.SmokeError, "identity/source set"):
                smoke.verify_fixture(root, "editor")

    def test_missing_interactive_opt_in_reports_unsupported(self):
        result = subprocess.run([sys.executable, str(Path(smoke.__file__)), "--helper", "/not/a/helper"],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 77)
        self.assertIn("UNSUPPORTED / NOT VERIFIED", result.stdout)


if __name__ == "__main__":
    unittest.main()
