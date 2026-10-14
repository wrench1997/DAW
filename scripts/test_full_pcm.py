"""Streaming file-audit regressions. Synthetic files do not certify SDK/NOIRE output."""
from dataclasses import replace
import hashlib
import math
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock

import validate_full_pcm as audit


def scalar32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def fixture_plan(**changes):
    values = dict(sample_rate=100, tempo=60, musical_frames=43, coda_start_frame=27,
                  tail_frames=11, latency_frames=3, quantum=8,
                  master_flat_until_beat=.08, master_final_beat=.27,
                  scope="synthetic fixture only")
    values.update(changes)
    return audit.Plan(**values)


def reference_value(value, frame, plan, *, double_base=False, no_tanh=False):
    """Test-side declarative math; does not call the implementation's oracle."""
    beat = scalar32(min(frame, plan.musical_frames - 1) / plan.sample_rate * (plan.tempo / 60))
    a, b = scalar32(plan.master_flat_until_beat), scalar32(plan.master_final_beat)
    first, final = scalar32(.7), scalar32(.56)
    if beat <= a:
        level = first
    elif beat >= b:
        level = final
    else:
        position = scalar32(scalar32(beat - a) / scalar32(b - a))
        level = scalar32(first + scalar32(scalar32(final - first) * position))
    channel = scalar32(min(8., max(-8., value)) * scalar32(.55))
    mixed = scalar32(channel + 0.0)
    driven = scalar32(mixed * level)
    if double_base:
        driven = scalar32(driven * scalar32(.7))
    return scalar32(driven if no_tanh else math.tanh(driven))


def raw_pairs(plan):
    result = []
    for frame in range(plan.generated_frames):
        if frame < plan.latency_frames:
            pair = (1.25 + frame * .1, -.9 - frame * .1)
        elif frame >= plan.latency_frames + plan.retained_frames:
            pair = (.123, -.456)  # Finite nonzero padding is discarded, not normalized.
        else:
            index = frame - plan.latency_frames
            pair = (.03 + index * .013, -.11 + index * .007)
        result.append(tuple(scalar32(value) for value in pair))
    return result


def expected_pairs(raw, plan, *, discard_multiplier=1, double_base=False, no_tanh=False):
    return [tuple(reference_value(value, frame, plan, double_base=double_base, no_tanh=no_tanh)
                  for value in raw[frame + plan.latency_frames * discard_multiplier])
            for frame in range(plan.retained_frames)]


def pcm_bytes(pairs):
    return b"".join(struct.pack("<ff", *pair) for pair in pairs)


def chunk(kind, data):
    return kind + struct.pack("<I", len(data)) + data + (b"\0" if len(data) % 2 else b"")


def wave_bytes(plan, pairs, *, tag=3, channels=2, fact=None, duplicate_data=False,
               extra_chunk=b"", fmt_extension=b"\0\0"):
    fmt = struct.pack("<HHIIHH", tag, channels, plan.sample_rate, plan.sample_rate * 8, 8, 32) + fmt_extension
    body = b"WAVE" + chunk(b"fmt ", fmt) + chunk(b"fact", struct.pack("<I", len(pairs) if fact is None else fact))
    body += chunk(b"JUNK", b"odd") + chunk(b"data", pcm_bytes(pairs)) + extra_chunk
    if duplicate_data:
        body += chunk(b"data", b"")
    return b"RIFF" + struct.pack("<I", len(body)) + body


class TrackedFileReader:
    """Actual FileIO seam; growth writes only to a test-owned temporary file."""
    def __init__(self, delegate, path, *, grow=False, first_short=None):
        self.delegate, self.path, self.grow, self.first_short = delegate, path, grow, first_short
        self.initial_bytes = os.fstat(delegate.fileno()).st_size
        self.requests = []
        self.returned = 0
        self.appended = 0

    def read(self, size=-1):
        if not 0 <= size <= audit.MAX_BLOCK_BYTES:
            raise AssertionError("unbounded/over64KiB actual read request")
        if len(self.requests) > self.initial_bytes * 2 + 8:
            # Deterministic fail-fast if the old read-until-EOF loop is restored.
            raise AssertionError("growing reader exceeded frozen finite call budget")
        self.requests.append(size)
        request = self.first_short if len(self.requests) == 1 and self.first_short is not None else size
        block = self.delegate.read(request)
        self.returned += len(block)
        if self.grow:
            with self.path.open("ab", buffering=0) as appender:
                written = appender.write(b"\0" * 16)
            if written != 16:
                raise AssertionError("test's actual append did not complete")
            self.appended += written
        return block

    def __getattr__(self, name):
        return getattr(self.delegate, name)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.delegate.close()


def tracked_open(path_to_wrap, output, *, grow=False, first_short=None):
    real_open = Path.open
    def open_path(path, *args, **kwargs):
        delegate = real_open(path, *args, **kwargs)
        mode = args[0] if args else kwargs.get("mode", "r")
        if path == path_to_wrap and mode == "rb":
            wrapped = TrackedFileReader(delegate, path, grow=grow, first_short=first_short)
            output.append(wrapped)
            return wrapped
        return delegate
    return open_path


class StatView:
    """Keep real common identity while controlling one exact timestamp API."""
    def __init__(self, actual, **changes):
        self.actual, self.changes = actual, changes

    def __getattr__(self, name):
        return self.changes[name] if name in self.changes else getattr(self.actual, name)


def actual_saved_project_path():
    supplied = os.environ.get("CITRUS_NOIRE_FULL_QA_PROJECT")
    if supplied:
        return Path(supplied).resolve(strict=True)
    workspace = next((parent for parent in tuple(Path(__file__).resolve().parents)[:10]
                      if (parent / "Cargo.toml").is_file() and (parent / "src/app.rs").is_file()),
                     None)
    if workspace is None:
        raise unittest.SkipTest("the optional local Saved13 qualification input is unavailable")
    project = workspace / "target/melody-stability-20261011/desktop-song-13/After-Rain-NOIRE-Pure-native13.citrus"
    if not project.is_file():
        raise unittest.SkipTest("the optional local Saved13 qualification input is not distributed in Git")
    return project


class CallbackReader:
    """Trigger one actual test-owned mutation after the first real read."""
    def __init__(self, delegate, callback):
        self.delegate, self.callback = delegate, callback
        self.calls = 0

    def read(self, size=-1):
        self.calls += 1
        result = self.delegate.read(size)
        if self.calls == 1:
            self.callback()
        return result

    def __getattr__(self, name):
        return getattr(self.delegate, name)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.delegate.close()


def callback_open(path_to_wrap, callback, output):
    real_open = Path.open

    def open_path(path, *args, **kwargs):
        delegate = real_open(path, *args, **kwargs)
        mode = args[0] if args else kwargs.get("mode", "r")
        if path == path_to_wrap and mode == "rb":
            wrapped = CallbackReader(delegate, callback)
            output.append(wrapped)
            return wrapped
        return delegate
    return open_path


class AuditFixture(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="citrus-independent-full-pcm-")
        self.addCleanup(self.directory.cleanup)
        self.raw_path = Path(self.directory.name).resolve() / "raw.f32"
        self.wav_path = Path(self.directory.name).resolve() / "master.wav"

    def write(self, plan=None, raw=None, wav=None, **wave_options):
        plan = fixture_plan() if plan is None else plan
        raw = raw_pairs(plan) if raw is None else raw
        raw = [struct.unpack("<ff", struct.pack("<ff", *pair)) for pair in raw]
        wav = expected_pairs(raw, plan) if wav is None else wav
        self.raw_path.write_bytes(pcm_bytes(raw))
        self.wav_path.write_bytes(wave_bytes(plan, wav, **wave_options))
        return plan, raw, wav

    def validate(self, plan, **options):
        return audit.validate_files(self.raw_path, self.wav_path, plan, **options)

    def test_all_raw_and_retained_samples_hashes_and_regions_are_checked_across_blocks(self):
        plan, raw, wav = self.write()
        first, second = self.validate(plan, block_bytes=16), self.validate(plan, block_bytes=40)
        for report in (first, second):
            self.assertTrue(report["file_consistency_matches"], report["errors"])
            self.assertEqual(report["raw_audio"]["audited_samples"], plan.generated_frames * 2)
            self.assertEqual(report["wav_audio"]["audited_samples"], plan.retained_frames * 2)
            self.assertEqual(report["comparison"]["compared_samples"], plan.retained_frames * 2)
            self.assertEqual(report["raw_audio"]["sha256"], hashlib.sha256(pcm_bytes(raw)).hexdigest())
            self.assertEqual(report["wav_audio"]["sha256"], hashlib.sha256(self.wav_path.read_bytes()).hexdigest())
            self.assertEqual(report["comparison"]["derived_expected_pcm_sha256"], hashlib.sha256(pcm_bytes(wav)).hexdigest())
            self.assertFalse(report["frozen_input_verified"])
            self.assertFalse(report["sdk_verified"])
        self.assertEqual({key: value for key, value in first["raw_audio"].items() if key != "read_contract"},
                         {key: value for key, value in second["raw_audio"].items() if key != "read_contract"})
        for report in (first, second):
            self.assertEqual(report["raw_audio"]["read_contract"]["total_requested_bytes"], plan.generated_frames * 8 + 1)
            self.assertEqual(report["raw_audio"]["read_contract"]["total_returned_bytes"], plan.generated_frames * 8)
            self.assertTrue(report["raw_audio"]["read_contract"]["eof_confirmed"])
        self.assertEqual({key: value for key, value in first["wav_audio"].items() if key != "sample_read_contract"},
                         {key: value for key, value in second["wav_audio"].items() if key != "sample_read_contract"})
        for report, size in ((first, 16), (second, 40)):
            contract = report["wav_audio"]["sample_read_contract"]
            self.assertEqual(contract["total_returned_bytes"], plan.retained_frames * 8)
            self.assertEqual(contract["total_requested_bytes"], plan.retained_frames * 8)
            self.assertEqual(contract["initial_pcm_bytes_remaining"], 0)
            self.assertEqual(contract["sample_frames_scanned"], plan.retained_frames)
            self.assertLessEqual(contract["maximum_request_bytes"], size)
            self.assertFalse(contract["failed"])
        self.assertEqual(first["comparison"], second["comparison"])
        regions = first["raw_audio"]["metrics"]
        self.assertEqual(regions["latency_prefix"]["observed_frames"], plan.latency_frames)
        self.assertEqual(regions["padding_suffix"]["observed_frames"], plan.padding_frames)
        self.assertGreater(regions["padding_suffix"]["nonzero_frames"], 0)

    def test_shifted_one_frame_is_rejected_without_alignment_search(self):
        plan, raw, wav = self.write()
        shifted = wav[1:] + wav[:1]
        self.write(plan, raw, shifted)
        report = self.validate(plan, first_mismatches=3)
        self.assertFalse(report["file_consistency_matches"])
        self.assertGreater(report["comparison"]["samples_outside_tolerance"], 3)
        self.assertEqual(len(report["comparison"]["first_mismatches"]), 3)
        self.assertTrue(report["comparison"]["mismatch_rows_truncated"])
        self.assertFalse(report["alignment_or_tolerance_fitted"])

    def test_double_latency_discard_is_rejected(self):
        plan = fixture_plan(latency_frames=3, quantum=16)
        raw = raw_pairs(plan)
        self.assertGreaterEqual(len(raw), plan.latency_frames * 2 + plan.retained_frames)
        self.write(plan, raw, expected_pairs(raw, plan, discard_multiplier=2))
        self.assertFalse(self.validate(plan)["file_consistency_matches"])

    def test_wrong_declared_latency_is_not_fitted_from_the_files(self):
        plan, _, _ = self.write()
        wrong = replace(plan, latency_frames=plan.latency_frames + 1)
        report = self.validate(wrong)
        self.assertFalse(report["file_consistency_matches"])
        self.assertEqual(report["geometry"]["declared_latency_frames"], wrong.latency_frames)
        self.assertFalse(report["actual_capture_binding_verified"])

    def test_extra_static_master_base_gain_is_rejected(self):
        plan = fixture_plan()
        raw = raw_pairs(plan)
        self.write(plan, raw, expected_pairs(raw, plan, double_base=True))
        report = self.validate(plan)
        self.assertFalse(report["file_consistency_matches"])
        self.assertGreater(report["comparison"]["maximum_absolute_sample_error"], .01)

    def test_missing_once_tanh_is_rejected_with_raw_clamp_active(self):
        plan = fixture_plan()
        raw = [(scalar32(30.), scalar32(-20.)) for _ in range(plan.generated_frames)]
        self.write(plan, raw, expected_pairs(raw, plan, no_tanh=True))
        self.assertFalse(self.validate(plan)["file_consistency_matches"])
        self.write(plan, raw, expected_pairs(raw, plan))
        self.assertTrue(self.validate(plan)["file_consistency_matches"])

    def test_nan_in_padding_and_latency_is_rejected_even_when_every_wav_sample_matches(self):
        for region in ("padding", "latency"):
            with self.subTest(region=region):
                plan, raw, wav = self.write()
                index = len(raw) - 1 if region == "padding" else 0
                raw[index] = (float("nan"), raw[index][1])
                self.write(plan, raw, wav)
                report = self.validate(plan)
                self.assertFalse(report["file_consistency_matches"])
                self.assertEqual(report["comparison"]["samples_outside_tolerance"], 0)
                self.assertEqual(report["raw_audio"]["audited_samples"], plan.generated_frames * 2)
                self.assertEqual(report["raw_audio"]["metrics"]["all"]["nonfinite_samples"], 1)
                self.assertEqual(report["raw_audio"]["first_nonfinite_samples"][0]["raw_frame"], index)

    def test_nan_in_actual_retained_or_wav_never_becomes_a_partial_success(self):
        plan, raw, wav = self.write()
        raw[plan.latency_frames + 5] = (float("nan"), raw[plan.latency_frames + 5][1])
        self.write(plan, raw, wav)
        report = self.validate(plan)
        self.assertFalse(report["file_consistency_matches"])
        self.assertIsNone(report["comparison"]["derived_expected_pcm_sha256"])
        plan, raw, wav = self.write()
        wav[3] = (float("inf"), wav[3][1])
        self.write(plan, raw, wav)
        report = self.validate(plan)
        self.assertFalse(report["file_consistency_matches"])
        self.assertEqual(report["wav_audio"]["metrics"]["all"]["nonfinite_samples"], 1)
        self.assertEqual(report["comparison"]["float32_bits_compared_samples"], plan.retained_frames * 2)

    def test_truncated_and_extra_whole_raw_frames_are_structural_failures(self):
        plan, raw, wav = self.write()
        for changed in (raw[:-1], raw + [(0., 0.)]):
            with self.subTest(frames=len(changed)):
                self.write(plan, changed, wav)
                report = self.validate(plan)
                self.assertFalse(report["file_consistency_matches"])
                self.assertTrue(any(row["stage"] == "raw_geometry" for row in report["errors"]))
                self.assertEqual(report["raw_audio"]["audited_samples"], len(changed) * 2)

    def test_partial_raw_frame_and_truncated_riff_and_extra_pcm_are_rejected(self):
        plan, _, _ = self.write()
        self.raw_path.write_bytes(self.raw_path.read_bytes()[:-1])
        self.assertFalse(self.validate(plan)["file_consistency_matches"])
        plan, raw, wav = self.write()
        self.wav_path.write_bytes(self.wav_path.read_bytes()[:-1])
        report = self.validate(plan)
        self.assertFalse(report["file_consistency_matches"])
        self.assertEqual(report["raw_audio"]["audited_samples"], plan.generated_frames * 2)
        self.write(plan, raw, wav + [(0., 0.)])
        self.assertFalse(self.validate(plan)["file_consistency_matches"])

    def test_wrong_fmt_fact_duplicate_data_and_extra_trailing_bytes_are_rejected(self):
        plan, raw, wav = self.write()
        cases = ({"tag": 1}, {"channels": 1}, {"fmt_extension": b"\1\0"},
                 {"fact": plan.retained_frames + 1}, {"duplicate_data": True})
        for options in cases:
            with self.subTest(options=options):
                self.write(plan, raw, wav, **options)
                report = self.validate(plan)
                self.assertFalse(report["file_consistency_matches"])
                self.assertEqual(report["raw_audio"]["audited_samples"], plan.generated_frames * 2)
        self.write(plan, raw, wav)
        with self.wav_path.open("ab") as handle:
            handle.write(b"trailing")
        self.assertFalse(self.validate(plan)["file_consistency_matches"])

    def test_signed_zero_bit_difference_is_honest_without_failing_numeric_tolerance(self):
        plan = fixture_plan()
        raw = [(0., 0.)] * plan.generated_frames
        wav = [(0., 0.)] * plan.retained_frames
        wav[0] = (-0., 0.)
        self.write(plan, raw, wav)
        report = self.validate(plan)
        self.assertTrue(report["file_consistency_matches"])
        self.assertEqual(report["comparison"]["samples_outside_tolerance"], 0)
        self.assertEqual(report["comparison"]["samples_with_different_float32_bits"], 1)
        self.assertEqual(report["wav_audio"]["metrics"]["all"]["negative_zero_samples"], 1)

    def test_exact_rms_zero_runs_and_adjacency_survive_chunk_boundaries(self):
        plan = fixture_plan(musical_frames=14, coda_start_frame=12, tail_frames=4,
                            latency_frames=2, quantum=4, master_flat_until_beat=1., master_final_beat=2.)
        sequence = [(0., 0.)] * 2 + [(.25, -.25)] + [(0., 0.)] * 5 + [(.5, .5), (.25, -.25)]
        sequence += [(0., 0.)] * 3 + [(-.25, .25)] + [(0., 0.)] * 4
        raw = [(0., 0.)] * plan.latency_frames + sequence
        _, _, wav = self.write(plan, raw)
        reports = [self.validate(plan, block_bytes=size) for size in (8, 24, 64)]
        expected_rms = math.sqrt(math.fsum(v * v for pair in wav for v in pair) / (len(wav) * 2))
        for report in reports:
            self.assertTrue(report["file_consistency_matches"])
            metric = report["wav_audio"]["metrics"]["all"]
            self.assertEqual(metric["leading_exact_zero_frames"], 2)
            self.assertEqual(metric["trailing_exact_zero_frames"], 4)
            self.assertEqual(metric["maximum_exact_zero_run"], 5)
            self.assertEqual(metric["maximum_internal_exact_zero_run"], 5)
            self.assertAlmostEqual(metric["rms"], expected_rms, places=15)
            self.assertGreater(metric["maximum_adjacent_same_channel_delta"], 0)
        self.assertEqual(reports[0]["wav_audio"]["metrics"], reports[2]["wav_audio"]["metrics"])

    def test_silent_coda_fixed_tail_and_end100ms_do_not_claim_full_natural_release(self):
        plan = fixture_plan(musical_frames=43, coda_start_frame=20, tail_frames=11)
        raw = raw_pairs(plan)
        for frame in range(plan.coda_start_frame, plan.retained_frames):
            raw[plan.latency_frames + frame] = (0., 0.)
        self.write(plan, raw)
        report = self.validate(plan, reported_tail=0xFFFFFFFF, metadata_tail_limit_reached=True,
                               capture_reference_sha256="a" * 64)
        self.assertTrue(report["file_consistency_matches"])
        for name in ("coda", "fixed_tail", "end100ms"):
            self.assertEqual(report["wav_audio"]["metrics"][name]["nonzero_frames"], 0)
        self.assertTrue(report["unknown_metadata_tail_warning_retained"])
        self.assertFalse(report["complete_natural_tail_claim"])
        self.assertFalse(report["actual_capture_binding_verified"])

    def test_fixed_expected_buffer_hash_flushes_across_64k_boundary(self):
        plan = fixture_plan(sample_rate=1000, musical_frames=8201, coda_start_frame=8100,
                            tail_frames=3, latency_frames=1, quantum=8,
                            master_flat_until_beat=1., master_final_beat=8.1)
        raw = [(.025, -.017)] * plan.generated_frames
        _, _, wav = self.write(plan, raw)
        report = self.validate(plan, block_bytes=56)
        self.assertTrue(report["file_consistency_matches"])
        self.assertEqual(report["comparison"]["derived_expected_pcm_bytes"], plan.retained_frames * 8)
        self.assertEqual(report["comparison"]["derived_expected_pcm_sha256"], hashlib.sha256(pcm_bytes(wav)).hexdigest())

    def test_chunk_and_metadata_quotas_are_finite(self):
        plan, raw, wav = self.write()
        too_many = b"".join(chunk(b"junk", b"") for _ in range(audit.MAX_CHUNKS))
        self.write(plan, raw, wav, extra_chunk=too_many)
        self.assertFalse(self.validate(plan)["file_consistency_matches"])
        self.write(plan, raw, wav, extra_chunk=chunk(b"junk", b"x" * audit.MAX_RIFF_METADATA))
        self.assertFalse(self.validate(plan)["file_consistency_matches"])

    def test_checked_scalar_limits_reject_overflow_bool_and_unbounded_diagnostics_rows(self):
        for changes in (dict(latency_frames=-1), dict(latency_frames=True),
                        dict(musical_frames=audit.MAX_FRAMES, tail_frames=1), dict(tempo=float("nan"))):
            with self.subTest(changes=changes):
                with self.assertRaises(ValueError):
                    fixture_plan(**changes)
        plan, _, _ = self.write()
        for options in (dict(block_bytes=7), dict(block_bytes=65544), dict(first_mismatches=33),
                        dict(first_mismatches=True), dict(metadata_tail_limit_reached=1)):
            with self.subTest(options=options):
                with self.assertRaises(ValueError):
                    self.validate(plan, **options)
        for size in (0, -8, 65544):
            with self.wav_path.open("rb") as handle:
                with self.assertRaises(ValueError):
                    audit.parse_wave(handle, plan, size)

    def test_full_original_spec_geometry_is_bound_without_creating_full_audio(self):
        # Works from working, sealed and copied runner subdirectories. This is
        # a bounded ancestor lookup, not a drive-wide search or a generated score.
        original = actual_saved_project_path()
        project = audit.bounded_project(original)
        plan, binding = audit.Plan.saved13(project, 19)
        self.assertEqual(binding["project_sha256"], audit.SAVED_SHA)
        self.assertEqual(binding["note_identities"], 405)
        self.assertEqual((binding["patterns"], binding["placements"]), (5, 5))
        self.assertEqual(plan.musical_frames, 5485714)
        self.assertEqual(plan.tail_frames, 144000)
        self.assertEqual(plan.retained_frames, 5629714)
        self.assertEqual(plan.generated_frames, ((19 + 5629714 + 127) // 128) * 128)
        with self.assertRaises(ValueError):
            audit.Plan.saved13(project + b" ", 19)

    def test_different_paths_cannot_alias_one_actual_raw_and_wav_file(self):
        plan, _, _ = self.write()
        self.raw_path.unlink()
        os.link(self.wav_path, self.raw_path)
        report = self.validate(plan)
        self.assertFalse(report["file_consistency_matches"])
        self.assertTrue(any("alias the same actual file" in row["error"] for row in report["errors"]))

    def test_actual_continuous_raw_append_cannot_extend_the_first_stat_read_budget(self):
        plan, raw, _ = self.write()
        wrappers = []
        with mock.patch.object(Path, "open", new=tracked_open(self.raw_path, wrappers, grow=True)):
            report = self.validate(plan, block_bytes=16)
        reader = wrappers[0]
        self.assertFalse(report["file_consistency_matches"])
        self.assertGreater(reader.appended, 0)
        self.assertEqual(sum(reader.requests), reader.initial_bytes + 1)
        self.assertEqual(reader.returned, reader.initial_bytes + 1)
        self.assertEqual(reader.requests[-1], 1)
        self.assertLessEqual(max(reader.requests), 16)
        self.assertGreater(self.raw_path.stat().st_size, reader.initial_bytes)
        audio = report["raw_audio"]
        self.assertEqual(audio["sha256"], hashlib.sha256(pcm_bytes(raw)).hexdigest())
        self.assertEqual(audio["audited_samples"], plan.generated_frames * 2)
        self.assertTrue(audio["complete_initial_samples_scanned"])
        self.assertFalse(audio["input_stable"])
        self.assertEqual(audio["read_contract"]["total_returned_bytes"], reader.initial_bytes + 1)
        self.assertEqual(audio["read_contract"]["extra_bytes_observed"], 1)
        self.assertTrue(any("grew beyond frozen initial byte budget" in row["error"] for row in report["errors"]))

    def test_short_aligned_partial_pair_and_early_eof_are_rejected_without_retry(self):
        for short in (8, 15, 0):
            with self.subTest(first_actual_return=short):
                plan, _, _ = self.write()
                wrappers = []
                with mock.patch.object(Path, "open", new=tracked_open(self.raw_path, wrappers, first_short=short)):
                    report = self.validate(plan, block_bytes=16)
                reader = wrappers[0]
                self.assertFalse(report["file_consistency_matches"])
                self.assertEqual(reader.requests, [16])
                self.assertEqual(reader.returned, short)
                contract = report["raw_audio"]["read_contract"]
                self.assertEqual(contract["initial_bytes_returned"], short)
                self.assertTrue(contract["failed"])
                self.assertFalse(contract["extra_byte_probe_attempted"])
                self.assertFalse(report["raw_audio"]["complete_initial_samples_scanned"])
                self.assertIsNone(report["raw_audio"]["sha256"])
                with self.raw_path.open("rb", buffering=0) as handle:
                    tracked = TrackedFileReader(handle, self.raw_path, first_short=short)
                    frozen = audit.FrozenFileReader(tracked, tracked.initial_bytes, 16, "fixture raw")
                    with self.assertRaises(ValueError):
                        list(frozen.initial_blocks())
                    with self.assertRaises(ValueError):
                        list(frozen.initial_blocks())
                    self.assertEqual(tracked.requests, [16])

    def test_stable_raw_reads_exact_initial_bytes_and_only_one_empty_extra_probe(self):
        plan, _, _ = self.write()
        wrappers = []
        with mock.patch.object(Path, "open", new=tracked_open(self.raw_path, wrappers)):
            report = self.validate(plan, block_bytes=24)
        reader = wrappers[0]
        self.assertTrue(report["file_consistency_matches"])
        self.assertEqual(sum(reader.requests), reader.initial_bytes + 1)
        self.assertEqual(reader.returned, reader.initial_bytes)
        self.assertEqual(reader.requests[-1], 1)
        contract = report["raw_audio"]["read_contract"]
        self.assertTrue(contract["eof_confirmed"])
        self.assertEqual(contract["extra_bytes_observed"], 0)
        self.assertTrue(report["raw_audio"]["input_stable"])

    def test_actual_score_append_is_also_limited_to_initial_bytes_plus_one(self):
        score = Path(self.directory.name).resolve() / "test-owned-score.json"
        score.write_bytes(b"{}\n")
        wrappers = []
        with mock.patch.object(Path, "open", new=tracked_open(score, wrappers, grow=True)):
            with self.assertRaisesRegex(ValueError, "grew beyond frozen initial byte budget"):
                audit.bounded_project(score)
        reader = wrappers[0]
        self.assertEqual(sum(reader.requests), reader.initial_bytes + 1)
        self.assertEqual(reader.returned, reader.initial_bytes + 1)
        self.assertEqual(reader.requests, [3, 1])

    def test_actual_riff_append_is_rejected_at_frozen_end_after_one_extra_byte(self):
        plan, _, _ = self.write()
        with self.wav_path.open("rb", buffering=0) as handle:
            reader = TrackedFileReader(handle, self.wav_path, grow=True)
            with self.assertRaisesRegex(ValueError, "extra bytes after complete RIFF"):
                audit.parse_wave(reader, plan, 16)
            self.assertGreater(reader.appended, 0)
            self.assertEqual(sum(reader.requests), reader.initial_bytes + 1)
            self.assertEqual(reader.returned, reader.initial_bytes + 1)
            self.assertEqual(reader.requests[-1], 1)

    def identity_inputs(self):
        plan, _, _ = self.write()
        score = Path(self.directory.name).resolve() / "frozen-original-score.citrus"
        dependency = Path(self.directory.name).resolve() / "independent-builder-copy.py"
        score.write_bytes(actual_saved_project_path().read_bytes())
        dependency.write_bytes(Path(audit.frozen_score.__file__).read_bytes())
        return score, dependency, (
            ("score", score, lambda: audit.bounded_project(score)),
            ("dependency", dependency,
             lambda: audit.Plan.saved13(audit.bounded_project(score), 19)),
            ("raw", self.raw_path, lambda: self.validate(plan, block_bytes=16)),
            ("WAV", self.wav_path, lambda: self.validate(plan, block_bytes=16)),
        )

    def assert_identity_result(self, result, accepted):
        if isinstance(result, dict):
            self.assertEqual(result["file_consistency_matches"], accepted, result["errors"])
            if not accepted:
                self.assertTrue(any("changed" in row["error"] or "identity differs" in row["error"]
                                    for row in result["errors"]), result["errors"])
        else:
            self.assertTrue(accepted, "non-report identity failure must raise")

    def test_actual_native_file_apis_accept_unchanged_score_dependency_raw_and_wav(self):
        _, dependency, inputs = self.identity_inputs()
        observations = []
        with mock.patch.object(audit.frozen_score, "__file__", str(dependency)):
            for label, path, action in inputs:
                with self.subTest(input=label):
                    path_stat = path.stat()
                    with path.open("rb", buffering=0) as handle:
                        handle_stat = os.fstat(handle.fileno())
                    self.assertEqual(audit.fingerprint(path_stat)[:4], audit.fingerprint(handle_stat)[:4])
                    observations.append({"input": label, "path_ctime_ns": path_stat.st_ctime_ns,
                                         "handle_ctime_ns": handle_stat.st_ctime_ns})
                    self.assert_identity_result(action(), True)
        # Actual platform observations; this does not assume both APIs differ.
        print("ACTUAL_STAT_API_CTIME " + repr(observations))

    def test_cross_api_ctime_difference_accepts_each_unchanged_full_input(self):
        _, dependency, inputs = self.identity_inputs()
        actual_fstat = os.fstat
        def split_ctime(fd):
            stat = actual_fstat(fd)
            return StatView(stat, st_ctime_ns=stat.st_ctime_ns + 505500)
        with mock.patch.object(audit.frozen_score, "__file__", str(dependency)), \
                mock.patch.object(os, "fstat", side_effect=split_ctime):
            for label, _, action in inputs:
                with self.subTest(input=label):
                    self.assert_identity_result(action(), True)

    def test_one_nanosecond_path_api_change_rejects_each_input_after_real_read(self):
        _, dependency, inputs = self.identity_inputs()
        actual_stat = Path.stat
        with mock.patch.object(audit.frozen_score, "__file__", str(dependency)):
            for label, target, action in inputs:
                with self.subTest(input=label):
                    changed = {"calls": 0}
                    readers = []
                    def mutate():
                        changed["calls"] += 1
                    def changed_stat(path, *args, **kwargs):
                        stat = actual_stat(path, *args, **kwargs)
                        return StatView(stat, st_ctime_ns=stat.st_ctime_ns + 1) if path == target and changed["calls"] else stat
                    with mock.patch.object(Path, "open", new=callback_open(target, mutate, readers)), \
                            mock.patch.object(Path, "stat", new=changed_stat):
                        if label in ("score", "dependency"):
                            with self.assertRaisesRegex(ValueError, "path changed/replaced"):
                                action()
                        else:
                            self.assert_identity_result(action(), False)
                    self.assertEqual(changed["calls"], 1)
                    self.assertEqual(len(readers), 1)
                    self.assertGreater(readers[0].calls, 0)

    def test_one_nanosecond_handle_api_change_rejects_each_input_after_real_read(self):
        _, dependency, inputs = self.identity_inputs()
        actual_fstat = os.fstat
        with mock.patch.object(audit.frozen_score, "__file__", str(dependency)):
            for label, target, action in inputs:
                with self.subTest(input=label):
                    changed = {"calls": 0}
                    readers = []
                    def mutate():
                        changed["calls"] += 1
                    def changed_fstat(fd):
                        stat = actual_fstat(fd)
                        if changed["calls"] and readers and fd == readers[0].fileno():
                            return StatView(stat, st_ctime_ns=stat.st_ctime_ns + 1)
                        return stat
                    with mock.patch.object(Path, "open", new=callback_open(target, mutate, readers)), \
                            mock.patch.object(os, "fstat", side_effect=changed_fstat):
                        if label in ("score", "dependency"):
                            with self.assertRaisesRegex(ValueError, "handle changed"):
                                action()
                        else:
                            self.assert_identity_result(action(), False)
                    self.assertEqual(changed["calls"], 1)
                    self.assertEqual(len(readers), 1)
                    self.assertGreater(readers[0].calls, 0)

    def test_cross_api_common_identity_mismatch_rejects_each_input_before_read(self):
        _, dependency, inputs = self.identity_inputs()
        actual_fstat = os.fstat
        with mock.patch.object(audit.frozen_score, "__file__", str(dependency)):
            for field in ("st_dev", "st_ino", "st_size", "st_mtime_ns"):
                for label, target, action in inputs:
                    with self.subTest(field=field, input=label):
                        readers = []
                        def mismatch(fd):
                            stat = actual_fstat(fd)
                            if readers and fd == readers[0].fileno():
                                return StatView(stat, **{field: getattr(stat, field) + 1})
                            return stat
                        with mock.patch.object(os, "fstat", side_effect=mismatch), \
                                mock.patch.object(Path, "open", new=callback_open(target, lambda: None, readers)):
                            if label in ("score", "dependency"):
                                with self.assertRaisesRegex(ValueError, "identity differs"):
                                    action()
                            else:
                                self.assert_identity_result(action(), False)
                        self.assertEqual(len(readers), 1)
                        self.assertEqual(readers[0].calls, 0)

    def test_actual_same_bytes_path_replacement_between_stat_and_open_rejects_each_input(self):
        _, dependency, inputs = self.identity_inputs()
        actual_open = Path.open
        with mock.patch.object(audit.frozen_score, "__file__", str(dependency)):
            for label, target, action in inputs:
                with self.subTest(input=label):
                    original_stat = target.stat()
                    original = target.read_bytes()
                    replacement = target.with_name(target.name + ".replacement")
                    replacement.write_bytes(original)
                    os.utime(replacement, ns=(original_stat.st_atime_ns, original_stat.st_mtime_ns))
                    self.assertNotEqual((replacement.stat().st_dev, replacement.stat().st_ino),
                                        (original_stat.st_dev, original_stat.st_ino))
                    changed = {"calls": 0}
                    def replace_before_open(path, *args, **kwargs):
                        mode = args[0] if args else kwargs.get("mode", "r")
                        if path == target and mode == "rb" and not changed["calls"]:
                            os.replace(replacement, target)
                            changed["calls"] += 1
                        return actual_open(path, *args, **kwargs)
                    with mock.patch.object(Path, "open", new=replace_before_open):
                        if label in ("score", "dependency"):
                            with self.assertRaisesRegex(ValueError, "identity differs"):
                                action()
                        else:
                            self.assert_identity_result(action(), False)
                    self.assertEqual(changed["calls"], 1)
                    self.assertEqual(target.read_bytes(), original)
                    self.assertFalse(replacement.exists())

    def test_sample_pass_short_read_never_resumes_or_reads_following_riff_chunk(self):
        for short in (0, 8, 15):
            with self.subTest(actual_partial_bytes=short):
                plan, _, _ = self.write(extra_chunk=chunk(b"AFTR", b"\x7f" * 64))
                contents = self.wav_path.read_bytes()
                data_offset = contents.index(b"data") + 8
                wrappers = []
                actual_open = Path.open
                class SampleShortReader:
                    def __init__(self, delegate):
                        self.delegate, self.sample = delegate, False
                        self.sample_requests, self.sample_returned = [], 0
                    def seek(self, offset, *args):
                        result = self.delegate.seek(offset, *args)
                        self.sample = offset == data_offset
                        return result
                    def read(self, size=-1):
                        if not self.sample:
                            return self.delegate.read(size)
                        self.sample_requests.append(size)
                        if len(self.sample_requests) > 1:
                            raise AssertionError("failed sample cursor attempted another real read")
                        result = self.delegate.read(short)
                        self.sample_returned += len(result)
                        return result
                    def __getattr__(self, name):
                        return getattr(self.delegate, name)
                def open_path(path, *args, **kwargs):
                    delegate = actual_open(path, *args, **kwargs)
                    mode = args[0] if args else kwargs.get("mode", "r")
                    if path == self.wav_path and mode == "rb":
                        wrapped = SampleShortReader(delegate)
                        wrappers.append(wrapped)
                        return wrapped
                    return delegate
                with mock.patch.object(Path, "open", new=open_path):
                    report = self.validate(plan, block_bytes=16)
                self.assertFalse(report["file_consistency_matches"])
                self.assertEqual(wrappers[0].sample_requests, [16])
                self.assertEqual(wrappers[0].sample_returned, short)
                contract = report["wav_audio"]["sample_read_contract"]
                self.assertTrue(contract["failed"])
                self.assertEqual(contract["total_returned_bytes"], short)
                self.assertEqual(contract["initial_pcm_bytes_remaining"], plan.retained_frames * 8 - short)
                self.assertEqual(contract["sample_frames_scanned"], 0)
                self.assertNotIn("metrics", report["wav_audio"])
                self.assertTrue(any("cannot be retried/drained" in row["error"] for row in report["errors"]))


class ReaderBudgetTests(unittest.TestCase):
    def test_cursor_short_partial_read_is_sticky_and_keeps_actual_consumed_bytes(self):
        plan = fixture_plan()
        pairs = expected_pairs(raw_pairs(plan), plan)
        for short in (0, 8, 15):
            with self.subTest(actual_partial_bytes=short), tempfile.TemporaryDirectory(prefix="citrus-cursor-short-") as directory:
                path = Path(directory) / "short.wav"
                path.write_bytes(wave_bytes(plan, pairs, extra_chunk=chunk(b"AFTR", b"\x7f" * 64)))
                with path.open("rb", buffering=0) as handle:
                    info = audit.parse_wave(handle, plan, 16)
                    reader = TrackedFileReader(handle, path, first_short=short)
                    cursor = audit.WaveCursor(reader, info, 16)
                    with self.assertRaisesRegex(ValueError, "changed/truncated"):
                        cursor.next_pair()
                    position = handle.tell()
                    self.assertEqual(position, info["data_offset"] + short)
                    for _ in range(3):
                        with self.assertRaisesRegex(ValueError, "cannot be retried/drained"):
                            cursor.next_pair()
                    self.assertEqual(handle.tell(), position)
                    self.assertEqual(reader.requests, [16])
                    self.assertEqual(cursor.remaining, info["data_bytes"] - short)
                    self.assertEqual(cursor.snapshot()["total_returned_bytes"], short)
                    self.assertEqual(cursor.frames, 0)
                    self.assertTrue(cursor.failed)

    def test_cursor_later_partial_read_preserves_scanned_frames_and_remaining_extent(self):
        plan = fixture_plan()
        pairs = expected_pairs(raw_pairs(plan), plan)
        with tempfile.TemporaryDirectory(prefix="citrus-cursor-late-short-") as directory:
            path = Path(directory) / "later-short.wav"
            path.write_bytes(wave_bytes(plan, pairs, extra_chunk=chunk(b"AFTR", b"\x7f" * 64)))
            with path.open("rb", buffering=0) as handle:
                info = audit.parse_wave(handle, plan, 16)
                class LaterShortReader:
                    def __init__(self, delegate):
                        self.delegate, self.requests = delegate, []
                    def read(self, size=-1):
                        self.requests.append(size)
                        return self.delegate.read(8 if len(self.requests) == 2 else size)
                    def __getattr__(self, name):
                        return getattr(self.delegate, name)
                reader = LaterShortReader(handle)
                cursor = audit.WaveCursor(reader, info, 16)
                self.assertIsNotNone(cursor.next_pair())
                self.assertIsNotNone(cursor.next_pair())
                with self.assertRaisesRegex(ValueError, "changed/truncated"):
                    cursor.next_pair()
                with self.assertRaisesRegex(ValueError, "cannot be retried/drained"):
                    cursor.next_pair()
                self.assertEqual(reader.requests, [16, 16])
                self.assertEqual(handle.tell(), info["data_offset"] + 24)
                self.assertEqual(cursor.frames, 2)
                self.assertEqual(cursor.remaining, info["data_bytes"] - 24)
                self.assertEqual(cursor.snapshot()["total_returned_bytes"], 24)
                self.assertEqual(cursor.hash.hexdigest(), hashlib.sha256(pcm_bytes(pairs)[:16]).hexdigest())

    def test_cursor_read_error_is_sticky_and_no_followup_read_is_attempted(self):
        plan = fixture_plan()
        pairs = expected_pairs(raw_pairs(plan), plan)
        with tempfile.TemporaryDirectory(prefix="citrus-cursor-io-error-") as directory:
            path = Path(directory) / "read-error.wav"
            path.write_bytes(wave_bytes(plan, pairs))
            with path.open("rb", buffering=0) as handle:
                info = audit.parse_wave(handle, plan, 16)
                class FailingReader:
                    def __init__(self, delegate):
                        self.delegate, self.calls = delegate, 0
                    def read(self, size=-1):
                        self.calls += 1
                        raise OSError("actual controlled reader failure")
                    def __getattr__(self, name):
                        return getattr(self.delegate, name)
                reader = FailingReader(handle)
                cursor = audit.WaveCursor(reader, info, 16)
                with self.assertRaisesRegex(OSError, "controlled reader failure"):
                    cursor.next_pair()
                with self.assertRaisesRegex(ValueError, "cannot be retried/drained"):
                    cursor.next_pair()
                self.assertEqual(reader.calls, 1)
                self.assertEqual(cursor.remaining, info["data_bytes"])
                self.assertEqual(cursor.snapshot()["total_returned_bytes"], 0)
                self.assertEqual(cursor.snapshot()["total_requested_bytes"], 16)
                self.assertTrue(cursor.failed)

    def test_real_riff_and_sample_passes_never_request_an_unbounded_or_over64k_read(self):
        plan = fixture_plan()
        pairs = expected_pairs(raw_pairs(plan), plan)

        class Reader:
            def __init__(self, delegate):
                self.delegate = delegate
                self.maximum = 0
                self.calls = 0
            def read(self, size=-1):
                if not 0 <= size <= audit.MAX_BLOCK_BYTES:
                    raise AssertionError("unbounded/over64KiB read")
                self.maximum = max(self.maximum, size)
                self.calls += 1
                return self.delegate.read(size)
            def __getattr__(self, name):
                return getattr(self.delegate, name)

        with tempfile.TemporaryDirectory(prefix="citrus-independent-read-budget-") as directory:
            path = Path(directory) / "bounded.wav"
            path.write_bytes(wave_bytes(plan, pairs))
            with path.open("rb") as handle:
                reader = Reader(handle)
                info = audit.parse_wave(reader, plan, 16)
                cursor = audit.WaveCursor(reader, info, 16)
                while cursor.next_pair() is not None:
                    pass
                self.assertEqual(cursor.frames, plan.retained_frames)
                self.assertLessEqual(reader.maximum, audit.MAX_BLOCK_BYTES)
                self.assertGreater(reader.calls, 4)


if __name__ == "__main__":
    unittest.main()
