"""Harness regressions use a fake process, never a third-party plugin."""

import subprocess
import sys
import time
import unittest
from unittest import mock

import smoke_vst3_helper as smoke


FAKE_HELPER = r'''
import json
import sys
import time

mode = sys.argv[1]
if mode == "oversized_stdout":
    print("x" * 65537, flush=True)
if mode == "stderr_flood":
    sys.stderr.write("diagnostic " * 100000 + "\n")
    sys.stderr.flush()
queries = 0
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    if mode == "no_reply":
        time.sleep(60)
    if mode == "early_eof":
        sys.exit(0)
    if line == '"Shutdown"':
        if mode == "stdout_flood":
            for _ in range(100000):
                print('{}', flush=True)
        if mode == "ignore_shutdown":
            continue  # Would succeed incorrectly if the harness closed stdin.
        if mode == "nonzero":
            print("deliberate failure", file=sys.stderr, flush=True)
            sys.exit(7)
        if mode == "extra_stdout":
            print('"unexpected"', flush=True)
        sys.exit(0)
    if mode == "bad_json":
        print("not valid JSON", flush=True)
        continue
    if mode == "wrong_response":
        print('{}', flush=True)
        continue
    if line == '"GetAllParameters"':
        queries += 1
        if mode == "no_recovery" and queries == 2:
            print('{}', flush=True)
        else:
            print(json.dumps({"Error": {"message": "No plugin loaded"}}), flush=True)
    else:
        message = "wrong error" if mode == "wrong_invalid" else "Invalid command: fake"
        print(json.dumps({"Error": {"message": message}}), flush=True)
'''


class SmokeHarnessTests(unittest.TestCase):
    def run_helper(self, mode, error=None, timeout=5.0):
        started = time.monotonic()
        children = []
        real_popen = subprocess.Popen

        def capture(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            children.append(child)
            return child

        with mock.patch.object(smoke.subprocess, "Popen", side_effect=capture):
            try:
                command = [sys.executable, "-u", "-c", FAKE_HELPER, mode]
                if error is None:
                    smoke.smoke_command(command, timeout=timeout)
                else:
                    with self.assertRaisesRegex(smoke.SmokeError, error):
                        smoke.smoke_command(command, timeout=timeout)
            finally:
                self.assertLess(time.monotonic() - started, timeout + 6.0)
                self.assertEqual(len(children), 1)
                self.assertIsNotNone(children[0].poll(), "child was not reaped")
                for stream in (children[0].stdin, children[0].stdout, children[0].stderr):
                    self.assertTrue(stream.closed, "pipe was not closed")

    def test_valid_protocol_and_shutdown(self):
        self.run_helper("valid")

    def test_stderr_flood_does_not_block_stdout(self):
        self.run_helper("stderr_flood")

    def test_oversized_stdout_is_rejected_and_child_reaped(self):
        self.run_helper("oversized_stdout", "Protocol line exceeded")

    def test_stdout_flood_is_rejected_and_child_reaped(self):
        self.run_helper("stdout_flood", "Timed out during Shutdown", timeout=2.0)

    def test_malformed_json_is_rejected_and_child_reaped(self):
        self.run_helper("bad_json", "Malformed JSON")

    def test_wrong_response_is_rejected(self):
        self.run_helper("wrong_response", "Unexpected response")

    def test_invalid_command_must_have_expected_error(self):
        self.run_helper("wrong_invalid", "Unexpected invalid-command response")

    def test_protocol_must_recover_after_invalid_command(self):
        self.run_helper("no_recovery", "did not recover")

    def test_early_eof_is_rejected(self):
        self.run_helper("early_eof", "closed stdout before")

    def test_response_timeout_kills_and_reaps_child(self):
        self.run_helper("no_reply", "Timed out during protocol response", timeout=1.0)

    def test_shutdown_must_work_without_closing_stdin(self):
        self.run_helper("ignore_shutdown", "Timed out during Shutdown", timeout=2.0)

    def test_nonzero_exit_includes_stderr(self):
        self.run_helper("nonzero", "(?s)code 7.*deliberate failure")

    def test_unexpected_extra_stdout_is_rejected(self):
        self.run_helper("extra_stdout", "Unexpected extra stdout")

    def test_invalid_timeouts_never_launch_a_child(self):
        with mock.patch.object(smoke.subprocess, "Popen") as popen:
            for timeout in (0, -1, float("inf"), float("nan")):
                with self.subTest(timeout=timeout), self.assertRaises(ValueError):
                    smoke.smoke_command(["unused"], timeout=timeout)
            popen.assert_not_called()


if __name__ == "__main__":
    unittest.main()
