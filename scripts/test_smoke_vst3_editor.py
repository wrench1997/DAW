"""Fake protocol/build-receipt regressions only; these never claim native Windows GUI QA."""

from contextlib import redirect_stderr
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

import smoke_vst3_editor as smoke


FAKE_HELPER = r'''
import json
import sys
import time

mode = sys.argv[1]
loaded = False
has_editor = True
opened = False
generation = 0
owner = None
if mode == "oversized_stdout":
    print("x" * 65537, flush=True)
if mode == "stderr_flood":
    sys.stderr.write("diagnostic " * 100000 + "\n")
    sys.stderr.flush()

def state():
    answer = dict(supported=True, has_editor=loaded and has_editor, open=opened,
                  width=560 if opened else 0, height=400 if opened else 0, generation=generation)
    if mode == "bad_width":
        answer["width"] = 0 if opened else 12
    if mode == "bool_generation":
        answer["generation"] = True
    if mode == "missing_field":
        del answer["supported"]
    return {"EditorState": {"state": answer}}

for line in sys.stdin:
    if mode == "no_reply":
        time.sleep(60)
    if mode == "early_eof":
        sys.exit(0)
    command = json.loads(line)
    if command == "Shutdown":
        if mode == "ignore_shutdown":
            continue
        if mode == "nonzero":
            print("deliberate fixture failure", file=sys.stderr, flush=True)
            sys.exit(7)
        if mode == "extra_stdout":
            print('{}', flush=True)
        if mode == "stdout_flood":
            for i in range(100000):
                print('{}', flush=True)
        sys.exit(0)
    if mode == "bad_json":
        print('not-json', flush=True)
        continue
    if mode == "wrong_response":
        print('{}', flush=True)
        continue
    if isinstance(command, dict) and "LoadPlugin" in command:
        load = command["LoadPlugin"]
        if load["sample_rate"] != 48000.0 or load["tempo"] != 120.0:
            raise RuntimeError("f64 wire numbers must not be encoded as bits")
        loaded = True
        has_editor = "no-editor" not in load["path"]
        opened = False
        response = {"PluginInfo": {"name": "VST3 Host Test Synth", "has_gui": has_editor}}
    elif command == "UnloadPlugin":
        loaded = opened = False
        response = {"Success": {"message": "Plugin unloaded"}}
    elif isinstance(command, dict) and "GetParameter" in command:
        param = command["GetParameter"]["id"]
        value = {1000: float(opened), 1001: 560 / 4096, 1002: 400 / 4096, 1003: 1.0 / 8, 0: 1.0}[param]
        response = {"ParameterValue": {"value": value}}
    elif isinstance(command, dict) and "Editor" in command:
        editor = command["Editor"]["command"]
        error = None
        if isinstance(editor, dict) and "Open" in editor:
            candidate = editor["Open"]["owner"]
            if not loaded:
                error = "No plugin loaded"
            elif not has_editor:
                error = "Plugin does not have a GUI editor"
            elif candidate is not None and (candidate["window"] == 0 or candidate["process_id"] == 0):
                error = "Invalid editor owner"
            elif opened and owner != candidate:
                error = "Owner differs from the open editor"
            elif not opened:
                opened = True
                owner = candidate
                generation += 1
                if mode in ("pollution_stderr", "pollution_stdout", "missing_pollution"):
                    stream = sys.stdout if mode == "pollution_stdout" else sys.stderr
                    routes = ("RUST", "WIN32") if mode == "missing_pollution" else ("RUST", "WIN32", "CRT")
                    for route in routes:
                        print(json.dumps({"Error": {"message": "CITRUS_FIXTURE_STDOUT_" + route}}),
                              file=stream, flush=True)
            elif mode == "unstable_generation":
                generation += 1
        elif editor == "Close":
            if opened:
                opened = False
        elif editor == "Focus":
            if not opened:
                error = "Plugin editor is not open"
        elif editor != "Query":
            error = "Invalid command"
        response = {"Error": {"message": error}} if error else state()
    else:
        response = {"Error": {"message": "Invalid command"}}
    print(json.dumps(response), flush=True)
'''


class SessionTests(unittest.TestCase):
    def run_session(self, callback, mode="valid", error=None, timeout=3.0):
        children = []
        original_popen = subprocess.Popen
        started = time.monotonic()
        def capture(*args, **kwargs):
            process = original_popen(*args, **kwargs)
            children.append(process)
            return process
        def run():
            with smoke.Session([sys.executable, "-u", "-c", FAKE_HELPER, mode], timeout) as session:
                callback(session)
        with mock.patch.object(smoke.subprocess, "Popen", side_effect=capture):
            if error:
                with self.assertRaisesRegex(smoke.SmokeError, error):
                    run()
            else:
                run()
        self.assertLess(time.monotonic() - started, timeout + 5)
        self.assertEqual(len(children), 1)
        self.assertIsNotNone(children[0].poll(), "child not reaped")
        for pipe in (children[0].stdin, children[0].stdout, children[0].stderr):
            self.assertTrue(pipe.closed, "pipe not closed")

    @staticmethod
    def protocol(session):
        session.editor("Query", open=False, has_editor=False, supported=True)
        smoke.error_response(session.request({"Editor": {"command": {"Open": {"owner": None}}}}), "No plugin loaded")
        session.load("source-built-editor")
        smoke.error_response(session.request({"Editor": {"command": "Focus"}}), "not open")
        owner = {"window": 2**40, "process_id": 1234}
        initial = session.editor({"Open": {"owner": owner}}, open=True, width=560, height=400)
        for _ in range(3):
            smoke.require(session.editor({"Open": {"owner": owner}}) == initial, "Repeated Open changed state")
            smoke.require(session.editor("Focus") == initial, "Focus changed state")
        smoke.require(session.parameter(1000) == 1.0, "Attach probe mismatch")
        smoke.require(session.parameter(1001) == 560 / 4096, "Width probe mismatch")
        smoke.require(session.parameter(1003) == 1 / 8, "Scale probe mismatch")
        closed = session.editor("Close", open=False, width=0, height=0)
        smoke.require(session.editor("Close") == closed, "Close is not idempotent")
        opened = session.editor({"Open": {"owner": None}}, open=True)
        smoke.require(opened["generation"] > initial["generation"], "Reopen generation did not advance")
        smoke.response_payload(session.request("UnloadPlugin"), "Success")
        session.editor("Query", open=False, has_editor=False)
        session.load("source-built-no-editor", has_editor=False)
        session.editor("Query", open=False, has_editor=False)
        smoke.error_response(session.request({"Editor": {"command": {"Open": {"owner": None}}}}), "does not have a GUI editor")
        session.finish()

    def test_editor_protocol_lifecycle(self):
        self.run_session(self.protocol)

    def test_repeat_open_must_not_change_generation(self):
        self.run_session(self.protocol, "unstable_generation", "Repeated Open changed state")

    def test_stderr_flood_is_drained(self):
        self.run_session(self.protocol, "stderr_flood")

    @staticmethod
    def pollution_probe(session):
        session.load("source-built-editor")
        session.editor({"Open": {"owner": None}}, open=True)
        session.assert_stdout_rerouted()
        session.editor("Query", open=True)  # Reject any delayed fake response, too.
        session.finish()

    def test_fixture_stdout_markers_must_be_observed_on_stderr(self):
        self.run_session(self.pollution_probe, "pollution_stderr")

    def test_valid_json_plugin_stdout_is_not_accepted_as_editor_reply(self):
        self.run_session(self.pollution_probe, "pollution_stdout", "Expected EditorState")

    def test_missing_stdout_route_cannot_silently_pass(self):
        self.run_session(self.pollution_probe, "missing_pollution",
                         "did not reach stderr: CITRUS_FIXTURE_STDOUT_CRT", timeout=0.3)

    def test_wrong_size_fails(self):
        self.run_session(self.protocol, "bad_width", "Closed editor has stale dimensions")

    def test_bool_generation_fails(self):
        self.run_session(self.protocol, "bool_generation", "Invalid editor generation")

    def test_missing_field_fails(self):
        self.run_session(self.protocol, "missing_field", "Unexpected editor state fields")

    def test_malformed_json_fails_and_reaps(self):
        self.run_session(self.protocol, "bad_json", "Malformed JSON")

    def test_oversize_reply_fails_and_reaps(self):
        self.run_session(self.protocol, "oversized_stdout", "Protocol line exceeded")

    def test_wrong_response_fails(self):
        self.run_session(self.protocol, "wrong_response", "Expected EditorState")

    def test_early_eof_fails(self):
        self.run_session(self.protocol, "early_eof", "closed stdout before")

    def test_timeout_fails_and_reaps(self):
        self.run_session(self.protocol, "no_reply", "Timed out during protocol response", timeout=0.3)

    def test_shutdown_requires_exit_with_stdin_open(self):
        self.run_session(lambda s: s.finish(), "ignore_shutdown", "Timed out during Shutdown", timeout=0.3)

    def test_shutdown_extra_stdout_fails(self):
        self.run_session(lambda s: s.finish(), "extra_stdout", "Unexpected extra stdout")

    def test_stdout_flood_is_bounded(self):
        self.run_session(lambda s: s.finish(), "stdout_flood", "Timed out during Shutdown", timeout=0.3)

    def test_nonzero_exit_includes_diagnostics(self):
        self.run_session(lambda s: s.finish(), "nonzero", "(?s)code 7.*deliberate fixture failure")

    def test_eof_process_cleanup(self):
        self.run_session(lambda s: s.finish("EOF"))

    def test_forced_termination_cleanup(self):
        self.run_session(lambda s: s.finish("crash"))

    def test_invalid_timeouts_never_launch(self):
        with mock.patch.object(smoke.subprocess, "Popen") as popen:
            for timeout in (0, -1, float("nan"), float("inf")):
                with self.subTest(timeout=timeout), self.assertRaises(ValueError):
                    smoke.Session(["unused"], timeout)
            popen.assert_not_called()

    def test_rejected_owner_actions_do_not_mutate_state(self):
        def run(session):
            session.load("source-built-editor")
            state = session.editor("Query")
            smoke.error_response(session.request({"Editor": {"command": {"Open": {"owner": {"window": 0, "process_id": 5}}}}}))
            self.assertEqual(session.editor("Query"), state)
            session.editor({"Open": {"owner": {"window": 2, "process_id": 5}}})
            state = session.editor("Query")
            smoke.error_response(session.request({"Editor": {"command": {"Open": {"owner": None}}}}))
            self.assertEqual(session.editor("Query"), state)
            session.finish()
        self.run_session(run)


class ValidationTests(unittest.TestCase):
    def test_fixture_state_requires_nonempty_base64(self):
        session = smoke.Session.__new__(smoke.Session)
        session.request = mock.Mock(return_value={"State": {"data": "c3RhdGU="}})
        self.assertEqual(session.save_state(), "c3RhdGU=")
        session.request.assert_called_once_with("SaveState")
        for payload in ({"data": ""}, {"data": "===="}, {"data": "bad!"},
                        {"data": "\N{SNOWMAN}"}, {"data": []}, {"data": True},
                        {"data": "A" * 65540}, {"data": "c3RhdGU=", "extra": 1}, {}):
            with self.subTest(payload=payload), self.assertRaises(smoke.SmokeError):
                session.request.return_value = {"State": payload}
                session.save_state()

    def test_native_revision_is_an_exact_nonnegative_integer(self):
        session = smoke.Session.__new__(smoke.Session)
        session.request = mock.Mock(return_value={"NativeDirtyRevision": {"revision": 3}})
        self.assertEqual(session.native_revision(), 3)
        session.request.assert_called_once_with("NativeDirtyRevision")
        for payload in ({"revision": True}, {"revision": -1}, {"revision": 3.0},
                        {"revision": "3"}, {"revision": 3, "extra": 1}, {}):
            with self.subTest(payload=payload), self.assertRaises(smoke.SmokeError):
                session.request.return_value = {"NativeDirtyRevision": payload}
                session.native_revision()

    def test_stdout_evidence_survives_bounded_diagnostic_eviction(self):
        diagnostics = smoke.FixtureDiagnostics()
        for marker in smoke.STDOUT_MARKERS:
            # Simulate a bounded reader splitting a marker between chunks.
            midpoint = len(marker) // 2
            diagnostics.append(marker[:midpoint])
            diagnostics.append(marker[midpoint:] + "\n")
        for _ in range(40):
            diagnostics.append("later unrelated diagnostic\n")
        self.assertEqual(len(diagnostics), 8)
        self.assertTrue(diagnostics.stdout_rerouted.is_set())
        self.assertEqual(diagnostics.missing_stdout_markers(), [])

    def test_unrelated_stderr_is_not_stdout_routing_evidence(self):
        diagnostics = smoke.FixtureDiagnostics()
        diagnostics.append("plugin loaded successfully\n")
        self.assertFalse(diagnostics.stdout_rerouted.is_set())
        self.assertEqual(diagnostics.missing_stdout_markers(), sorted(smoke.STDOUT_MARKERS))

    def test_state_must_be_internally_consistent(self):
        valid = dict(supported=True, has_editor=True, open=True, width=560, height=400, generation=2)
        for change in ({"supported": False}, {"has_editor": False}, {"width": 0}, {"height": -1},
                       {"open": 1}, {"generation": 2.0}, {"generation": -1}, {"surprise": True}):
            with self.subTest(change=change), self.assertRaises(smoke.SmokeError):
                smoke.state_response({"EditorState": {"state": valid | change}})

    def test_open_editor_requires_nonzero_attachment_generation(self):
        state = dict(supported=True, has_editor=True, open=True, width=560, height=400, generation=0)
        with self.assertRaisesRegex(smoke.SmokeError, "Inconsistent open editor state"):
            smoke.state_response({"EditorState": {"state": state}})

    def test_parameters_reject_nonfinite_strings_and_bools(self):
        self.assertEqual(smoke.number(0.25), 0.25)
        self.assertEqual(smoke.number(1), 1.0)
        for value in ("NaN", float("nan"), float("inf"), None, True):
            with self.subTest(value=value), self.assertRaises(smoke.SmokeError):
                smoke.number(value)

    def test_no_desktop_is_explicit_exit_77_and_does_not_launch(self):
        with mock.patch.object(smoke, "WindowsDesktop", side_effect=smoke.UnsupportedDesktop("locked")), \
             mock.patch.object(smoke.subprocess, "Popen") as popen, redirect_stderr(io.StringIO()) as stderr:
            self.assertEqual(smoke.main(["--helper", "unused"]), 77)
            self.assertIn("UNSUPPORTED / NOT VERIFIED", stderr.getvalue())
            popen.assert_not_called()

    def test_bad_timeout_is_failure_not_unsupported(self):
        with mock.patch.object(smoke, "WindowsDesktop") as desktop, redirect_stderr(io.StringIO()):
            self.assertEqual(smoke.main(["--helper", "unused", "--timeout", "nan"]), 1)
            desktop.assert_not_called()

    def test_paint_probe_uses_bounded_reaped_capture_process(self):
        desktop = smoke.WindowsDesktop.__new__(smoke.WindowsDesktop)
        desktop.pid = lambda hwnd: 123
        result = subprocess.CompletedProcess([], 0, "a" * 64 + "\n", "")
        with mock.patch.object(smoke.subprocess, "run", return_value=result) as run:
            self.assertEqual(desktop.painted_pixels(456), "a" * 64)
            self.assertEqual(run.call_args.kwargs["timeout"], 5.0)
            self.assertEqual(run.call_args.args[0][-2:], ["456", "123"])

    def test_paint_timeout_is_failure(self):
        desktop = smoke.WindowsDesktop.__new__(smoke.WindowsDesktop)
        desktop.pid = lambda hwnd: 123
        with mock.patch.object(smoke.subprocess, "run", side_effect=subprocess.TimeoutExpired("capture", 5)):
            with self.assertRaisesRegex(smoke.SmokeError, "capture timed out"):
                desktop.painted_pixels(456)

    def test_invalid_paint_results_are_failures(self):
        desktop = smoke.WindowsDesktop.__new__(smoke.WindowsDesktop)
        desktop.pid = lambda hwnd: 123
        for result in (subprocess.CompletedProcess([], 1, "", "capture failed"),
                       subprocess.CompletedProcess([], 0, "not-a-digest", "")):
            with self.subTest(result=result), mock.patch.object(smoke.subprocess, "run", return_value=result):
                with self.assertRaises(smoke.SmokeError):
                    desktop.painted_pixels(456)

    def make_fixture(self, root):
        for filename in smoke.SOURCE_FILES:
            path = root / filename
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(filename.encode())
        bundle = root / "target/bundles/CitrusEditorFixture.vst3"
        binary = bundle / "Contents/x86_64-win/CitrusEditorFixture.vst3"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"MZtest fixture bytes")
        receipt = dict(schema=1, variant="editor", upstream_commit=smoke.UPSTREAM_COMMIT,
                       target="x86_64-pc-windows-msvc",
                       sources={name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in smoke.SOURCE_FILES},
                       binary="Contents/x86_64-win/CitrusEditorFixture.vst3",
                       binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
        path = bundle / "source-build.json"
        path.write_text(json.dumps(receipt))
        return bundle, binary, path, receipt

    def test_current_source_receipt(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            bundle, _, _, _ = self.make_fixture(root)
            self.assertEqual(smoke.verify_fixture(root, "editor"), bundle.resolve())

    def test_stale_source_receipt_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            self.make_fixture(root)
            (root / "src/lib.rs").write_text("modified")
            with self.assertRaisesRegex(smoke.SmokeError, "Stale fixture source"):
                smoke.verify_fixture(root, "editor")

    def test_replaced_binary_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            _, binary, _, _ = self.make_fixture(root)
            binary.write_bytes(b"MZdifferent")
            with self.assertRaisesRegex(smoke.SmokeError, "does not match"):
                smoke.verify_fixture(root, "editor")

    def test_arbitrary_binary_path_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            _, _, path, receipt = self.make_fixture(root)
            receipt["binary"] = "../../commercial.vst3"
            path.write_text(json.dumps(receipt))
            with self.assertRaisesRegex(smoke.SmokeError, "Unexpected fixture binary path"):
                smoke.verify_fixture(root, "editor")

    def test_missing_receipt_is_not_a_skip(self):
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaisesRegex(smoke.SmokeError, "Missing/invalid source-built fixture"):
                smoke.verify_fixture(Path(temp), "editor")


class PaintDiagnosticTests(unittest.TestCase):
    """Failure diagnostics do not turn a rejected capture into an acceptance pass."""

    def test_print_result_not_advisory_last_error_controls_success(self):
        desktop = smoke.WindowsDesktop.__new__(smoke.WindowsDesktop)
        desktop.user = mock.Mock()
        for returned, last_error in ((0, 0), (0, 5), (1, 5)):
            with self.subTest(returned=returned, last_error=last_error), \
                 mock.patch.object(smoke.ctypes, "set_last_error", create=True) as reset, \
                 mock.patch.object(smoke.ctypes, "get_last_error", return_value=last_error, create=True) as read:
                desktop.user.PrintWindow.return_value = returned
                calls = mock.Mock()
                calls.attach_mock(reset, "reset")
                calls.attach_mock(desktop.user.PrintWindow, "print_window")
                calls.attach_mock(read, "read_error")
                self.assertEqual(desktop._print_window_probe(10, 20, 1),
                                 {"flags": 1, "succeeded": bool(returned),
                                  "last_error_advisory": last_error})
                self.assertEqual(calls.mock_calls, [mock.call.reset(0),
                                                   mock.call.print_window(10, 20, 1),
                                                   mock.call.read_error()])

    def test_successful_diagnostic_cannot_replace_failed_acceptance_and_cleanup_runs(self):
        desktop = smoke.WindowsDesktop.__new__(smoke.WindowsDesktop)
        desktop.size = lambda hwnd: (256, 48)
        desktop.user = mock.Mock()
        desktop.user.GetDC.return_value = 11
        desktop.gdi = mock.Mock()
        desktop.gdi.CreateCompatibleDC.return_value = 22
        desktop.gdi.CreateCompatibleBitmap.return_value = 33
        desktop.gdi.SelectObject.return_value = 44
        desktop._print_window_probe = mock.Mock(return_value={
            "flags": 1, "succeeded": False, "last_error_advisory": 0})
        desktop._diagnose_print_failure = mock.Mock(return_value=True)
        with redirect_stderr(io.StringIO()) as stderr, \
             self.assertRaisesRegex(smoke.SmokeError, "PrintWindow failed; diagnostic probes cannot"):
            desktop._paint_in_capture_process(123)
        desktop._print_window_probe.assert_called_once_with(123, 22, 1)
        desktop._diagnose_print_failure.assert_called_once_with(123, 22)
        self.assertIn("PRINTWINDOW_ACCEPTANCE_FAILURE", stderr.getvalue())
        desktop.gdi.GetDIBits.assert_not_called()
        desktop.gdi.SelectObject.assert_has_calls([mock.call(22, 33), mock.call(22, 44)])
        desktop.gdi.DeleteObject.assert_called_once_with(33)
        desktop.gdi.DeleteDC.assert_called_once_with(22)
        desktop.user.ReleaseDC.assert_called_once_with(None, 11)

    def test_diagnostic_scope_is_own_button_and_verified_target_only(self):
        desktop = smoke.WindowsDesktop.__new__(smoke.WindowsDesktop)
        desktop.user = mock.Mock()
        desktop.user.GetParent.return_value = 456
        desktop.user.CreateWindowExW.return_value = 701
        desktop.kernel = mock.Mock()
        desktop.pid = mock.Mock(return_value=321)
        desktop.class_name = mock.Mock(side_effect=["Button", smoke.PANEL_CLASS])
        desktop.size = mock.Mock(return_value=(256, 48))
        desktop.owner = mock.Mock(return_value={"window": 700, "process_id": 999})
        desktop.pump = mock.Mock()
        desktop._print_window_probe = mock.Mock(side_effect=lambda hwnd, memory, flags: {
            "flags": flags, "succeeded": True, "last_error_advisory": 0})
        with redirect_stderr(io.StringIO()) as stderr:
            desktop._diagnose_print_failure(123, 22)
        self.assertEqual(desktop._print_window_probe.call_args_list,
                         [mock.call(701, 22, 1), mock.call(701, 22, 0), mock.call(123, 22, 0)])
        self.assertEqual(desktop.user.CreateWindowExW.call_args.args[8], 700)
        self.assertIn('"probe": "target"', stderr.getvalue())
        self.assertIn('"probe": "same-process-button"', stderr.getvalue())
        self.assertIn('"probe": "target-whole-window"', stderr.getvalue())
        self.assertNotIn("PASS", stderr.getvalue())

    def test_timeout_retains_bounded_failure_diagnostics_from_capture_child(self):
        desktop = smoke.WindowsDesktop.__new__(smoke.WindowsDesktop)
        desktop.pid = lambda hwnd: 123
        diagnostic = b"PRINTWINDOW_ACCEPTANCE_FAILURE false\n"
        timeout = subprocess.TimeoutExpired("capture", 5, stderr=b"x" * 5000 + diagnostic)
        with mock.patch.object(smoke.subprocess, "run", side_effect=timeout), \
             self.assertRaisesRegex(smoke.SmokeError, "(?s)capture timed out.*PRINTWINDOW_ACCEPTANCE_FAILURE") as caught:
            desktop.painted_pixels(456)
        self.assertLess(len(str(caught.exception)), 4100)


class StateRoundTripTests(unittest.TestCase):
    """Check orchestration/failure gates only, without claiming native execution."""

    def setUp(self):
        self.session = mock.Mock(spec=smoke.Session)
        self.session.deadline = 123.0
        self.session.native_revision.side_effect = [2, 2]
        self.session.save_state.side_effect = ["c3RhdGU=", "c3RhdGU="]
        self.session.parameter.side_effect = [0.0, 1.0, 0.25, 0.5]
        self.session.request.side_effect = [
            {"ParameterChanges": {"changes": [[0, smoke.bits(0.25)]]}},
            {"Success": {"message": "unloaded"}},
            {"Success": {"message": "restored"}},
        ]
        self.desktop = mock.Mock(spec=smoke.WindowsDesktop)

    def run_roundtrip(self):
        smoke.assert_state_roundtrip(self.session, self.desktop, Path("owned-fixture"), [123, 456])

    def test_capture_precedes_feedback_drain_and_fresh_instance_restore(self):
        self.run_roundtrip()
        self.assertEqual(self.session.method_calls, [
            mock.call.native_revision(),
            mock.call.save_state(),
            mock.call.editor("Query", supported=True, has_editor=True, open=False),
            mock.call.parameter(1000),
            mock.call.request("TakeParameterChanges"),
            mock.call.native_revision(),
            mock.call.request("UnloadPlugin"),
            mock.call.load(Path("owned-fixture")),
            mock.call.parameter(0),
            mock.call.request({"LoadState": {"data": "c3RhdGU=", "context": "Project"}}),
            mock.call.parameter(0),
            mock.call.parameter(1004),
            mock.call.save_state(),
        ])
        self.desktop.gone.assert_called_once_with([123, 456], 123.0)

    def test_capture_requires_native_detach(self):
        self.session.parameter.side_effect = [1.0]
        with self.assertRaisesRegex(smoke.SmokeError, "did not detach"):
            self.run_roundtrip()
        self.session.load.assert_not_called()

    def test_missing_dsp_feedback_is_not_a_pass(self):
        self.session.request.side_effect = [{"ParameterChanges": {"changes": []}}]
        with self.assertRaisesRegex(smoke.SmokeError, "feedback mismatch"):
            self.run_roundtrip()

    def test_changed_native_revision_is_not_a_pass(self):
        self.session.native_revision.side_effect = [2, 3]
        with self.assertRaisesRegex(smoke.SmokeError, "revision changed"):
            self.run_roundtrip()

    def test_restore_requires_fresh_default_instance(self):
        self.session.parameter.side_effect = [0.0, 0.25]
        with self.assertRaisesRegex(smoke.SmokeError, "did not reset"):
            self.run_roundtrip()

    def test_stale_component_state_is_not_a_pass(self):
        self.session.parameter.side_effect = [0.0, 1.0, 1.0]
        with self.assertRaisesRegex(smoke.SmokeError, "did not survive"):
            self.run_roundtrip()

    def test_wrong_restore_context_is_not_a_pass(self):
        self.session.parameter.side_effect = [0.0, 1.0, 0.25, 0.0]
        with self.assertRaisesRegex(smoke.SmokeError, "project state context"):
            self.run_roundtrip()

    def test_changed_state_bytes_are_not_a_pass(self):
        self.session.save_state.side_effect = ["c3RhdGU=", "Y2hhbmdlZA=="]
        with self.assertRaisesRegex(smoke.SmokeError, "state changed"):
            self.run_roundtrip()


if __name__ == "__main__":
    unittest.main()
