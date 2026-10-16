#!/usr/bin/env python3
"""Test bundle/receipt assembly with Cargo replaced by fake binary bytes."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock


_SPEC = importlib.util.spec_from_file_location(
    "vst3_effect_fixture_builder", Path(__file__).with_name("build_fixture.py")
)
builder = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(builder)

WINDOWS = "x86_64-pc-windows-msvc"
LINUX = "x86_64-unknown-linux-gnu"
PLATFORMS = {
    WINDOWS: ("win32", "x86_64-win", "citrus_vst3_effect_fixture.dll", ".vst3"),
    LINUX: ("linux", "x86_64-linux", "libcitrus_vst3_effect_fixture.so", ".so"),
}
EFFECT_BYTES = b"mock effect fixture binary\n"
INSTRUMENT_BYTES = b"mock instrument-first fixture binary\n"
EFFECT_CLASS = {
    "class_id": "43545253454658440000000000000001",
    "name": "Citrus Stereo Delay Fixture",
    "category": "Fx|Delay",
    "latency_samples": 32,
    "last_tap_samples": 160,
}
INSTRUMENT_CLASS = {
    "class_id": "43545253454658440000000000000002",
    "name": "Citrus Lookahead MIDI Fixture",
    "category": "Instrument|Synth",
    "latency_samples": 32,
    "tail_samples": 32,
    "real_lookahead_fifo": True,
}


class BuildFixtureTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        root_patch = mock.patch.object(builder, "ROOT", self.root)
        root_patch.start()
        self.addCleanup(root_patch.stop)
        # Every test, including argument failures, installs this mock before
        # calling main. No test is permitted to launch Cargo or a native tool.
        run_patch = mock.patch.object(
            builder.subprocess, "run", side_effect=self._write_fake_binary
        )
        self.run = run_patch.start()
        self.addCleanup(run_patch.stop)

    def _write_fake_binary(self, command, *, check, timeout):
        target = command[command.index("--target") + 1]
        directory = Path(command[command.index("--target-dir") + 1])
        binary = directory / target / "release" / PLATFORMS[target][2]
        binary.parent.mkdir(parents=True, exist_ok=True)
        binary.write_bytes(
            INSTRUMENT_BYTES if "--features" in command else EFFECT_BYTES
        )

    def _invoke(self, target, *arguments, platform=None):
        with (
            mock.patch.object(builder.sys, "argv", ["build_fixture.py", "--target", target, *arguments]),
            mock.patch.object(builder.sys, "platform", platform or PLATFORMS[target][0]),
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            return builder.main()

    def _command(self, target, variant="effect"):
        command = [
            "cargo", "build", "--locked", "--jobs", "1", "--release",
            "--manifest-path", str(self.root / "Cargo.toml"),
            "--target", target,
            "--target-dir", str(self.root / "target" / (
                "build" if variant == "effect" else "build-instrument-first"
            )),
        ]
        if variant == "instrument-first":
            command.extend(["--features", "instrument-first"])
        return command

    def _bundle(self, variant="effect"):
        stem = (
            "CitrusStereoDelayFixture" if variant == "effect"
            else "CitrusLookaheadMIDIFixture"
        )
        return self.root / "target" / "bundles" / (stem + ".vst3")

    def _read_receipt(self, variant="effect"):
        return json.loads((self._bundle(variant) / "source-build.json").read_text(encoding="utf-8"))

    def _assert_bundle(self, target, variant="effect"):
        bundle = self._bundle(variant)
        _, architecture, _, suffix = PLATFORMS[target]
        binary_relative = "Contents/" + architecture + "/" + bundle.stem + suffix
        expected_bytes = EFFECT_BYTES if variant == "effect" else INSTRUMENT_BYTES
        self.assertEqual((bundle / binary_relative).read_bytes(), expected_bytes)
        receipt = self._read_receipt(variant)
        self.assertEqual(receipt["binary"], binary_relative)
        self.assertEqual(receipt["binary_bytes"], len(expected_bytes))
        self.assertEqual(receipt["build_command"], self._command(target, variant))
        self.assertEqual(receipt["target"], target)
        self.assertEqual(receipt["schema"], 1)
        self.assertIs(receipt["source_only"], True)
        self.assertEqual(receipt["vendor"], "Citrus Studio Test Fixtures")
        self.assertEqual(receipt["version"], "1.0.0")
        return receipt

    def _assert_legacy_receipt(self, target):
        receipt = self._assert_bundle(target)
        expected = {
            "schema": 1,
            "source_only": True,
            "target": target,
            "build_command": self._command(target),
            "binary": receipt["binary"],
            "binary_bytes": len(EFFECT_BYTES),
            "vendor": "Citrus Studio Test Fixtures",
            "version": "1.0.0",
            **EFFECT_CLASS,
            "additional_classes": [INSTRUMENT_CLASS],
        }
        self.assertEqual(receipt, expected)

    def _assert_instrument_receipt(self, target):
        receipt = self._assert_bundle(target, "instrument-first")
        expected = {
            "schema": 1,
            "source_only": True,
            "target": target,
            "build_command": self._command(target, "instrument-first"),
            "binary": receipt["binary"],
            "binary_bytes": len(INSTRUMENT_BYTES),
            "vendor": "Citrus Studio Test Fixtures",
            "version": "1.0.0",
            **INSTRUMENT_CLASS,
            "additional_classes": [EFFECT_CLASS],
            "variant": "instrument-first",
        }
        self.assertEqual(receipt, expected)

    def test_default_variant_preserves_windows_command_and_receipt(self):
        self.assertEqual(self._invoke(WINDOWS), 0)
        self.run.assert_called_once_with(self._command(WINDOWS), check=True, timeout=180.0)
        self._assert_legacy_receipt(WINDOWS)

    def test_default_variant_preserves_linux_command_and_receipt(self):
        self.assertEqual(self._invoke(LINUX), 0)
        self.run.assert_called_once_with(self._command(LINUX), check=True, timeout=180.0)
        self._assert_legacy_receipt(LINUX)

    def test_explicit_effect_variant_matches_default_on_each_platform(self):
        for target in PLATFORMS:
            with self.subTest(target=target):
                self.run.reset_mock()
                self.assertEqual(self._invoke(target, "--variant", "effect"), 0)
                self.run.assert_called_once_with(self._command(target), check=True, timeout=180.0)
                self._assert_legacy_receipt(target)

    def test_instrument_first_windows_command_and_receipt(self):
        self.assertEqual(self._invoke(WINDOWS, "--variant", "instrument-first"), 0)
        self.run.assert_called_once_with(
            self._command(WINDOWS, "instrument-first"), check=True, timeout=180.0
        )
        self._assert_instrument_receipt(WINDOWS)

    def test_instrument_first_linux_command_and_receipt(self):
        self.assertEqual(self._invoke(LINUX, "--variant", "instrument-first"), 0)
        self.run.assert_called_once_with(
            self._command(LINUX, "instrument-first"), check=True, timeout=180.0
        )
        self._assert_instrument_receipt(LINUX)

    def test_variant_builds_and_bundles_do_not_overwrite_each_other(self):
        for target in PLATFORMS:
            with self.subTest(target=target):
                self.assertEqual(self._invoke(target), 0)
                legacy_receipt = (self._bundle() / "source-build.json").read_bytes()
                self.assertEqual(self._invoke(target, "--variant", "instrument-first"), 0)
                self.assertEqual((self._bundle() / "source-build.json").read_bytes(), legacy_receipt)
                self._assert_legacy_receipt(target)
                self._assert_instrument_receipt(target)
                instrument_receipt = (self._bundle("instrument-first") / "source-build.json").read_bytes()
                self.assertEqual(self._invoke(target), 0)
                self.assertEqual(
                    (self._bundle("instrument-first") / "source-build.json").read_bytes(),
                    instrument_receipt,
                )
                for variant, expected_bytes in (
                    ("effect", EFFECT_BYTES), ("instrument-first", INSTRUMENT_BYTES)
                ):
                    command = self._command(target, variant)
                    build_directory = Path(command[command.index("--target-dir") + 1])
                    source_binary = build_directory / target / "release" / PLATFORMS[target][2]
                    self.assertEqual(source_binary.read_bytes(), expected_bytes)

    def test_positive_timeout_is_forwarded_to_mocked_build(self):
        self.assertEqual(self._invoke(LINUX, "--timeout", "7.25"), 0)
        self.run.assert_called_once_with(self._command(LINUX), check=True, timeout=7.25)

    def test_unknown_variant_is_rejected_before_build(self):
        with self.assertRaises(SystemExit) as error:
            self._invoke(LINUX, "--variant", "unknown")
        self.assertEqual(error.exception.code, 2)
        self.run.assert_not_called()
        self.assertFalse((self.root / "target").exists())

    def test_invalid_timeout_is_rejected_before_build(self):
        for timeout in ("0", "-1", "nan", "inf", "-inf", "not-a-number"):
            with self.subTest(timeout=timeout):
                with self.assertRaises(SystemExit) as error:
                    self._invoke(LINUX, "--timeout=" + timeout)
                self.assertEqual(error.exception.code, 2)
                self.run.assert_not_called()
                self.assertFalse((self.root / "target").exists())

    def test_unsupported_target_is_rejected_before_build(self):
        with self.assertRaises(SystemExit) as error:
            self._invoke("aarch64-apple-darwin", platform="darwin")
        self.assertEqual(error.exception.code, 2)
        self.run.assert_not_called()

    def test_platform_mismatch_is_rejected_before_build(self):
        for target, platform in ((WINDOWS, "linux"), (LINUX, "win32")):
            with self.subTest(target=target, platform=platform):
                with self.assertRaises(SystemExit) as error:
                    self._invoke(target, platform=platform)
                self.assertEqual(error.exception.code, 2)
                self.run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
