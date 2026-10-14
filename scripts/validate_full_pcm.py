"""Independent bounded audio-file consistency audit; never SDK/plugin authority.

Audio is read in <=64KiB blocks. No DAW renderer, mix, automation or SDK module
is imported. The copied independent score builder pins the complete Saved13 input.
"""
import argparse
from dataclasses import dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import struct

import build_full_spec as frozen_score

MAX_BLOCK_BYTES = 64 * 1024
MAX_FRAMES = 24_000_000
MAX_RIFF_METADATA = 1024 * 1024
MAX_CHUNKS = 64
MAX_MISMATCH_ROWS = 32
MAX_PROJECT_BYTES = 2 * 1024 * 1024
MAX_PATH_BYTES = 32 * 1024
ABS_TOLERANCE = 2e-6
QUIET_THRESHOLD = 1e-7
END_THRESHOLD = 1e-5
SPEC_BUILDER_SHA = "b5399a2a93ea6064e8bb8f9955f4674fb096cc44dc8b6edfa80cddb026a9337d"
SAVED_SHA = "90923c6384ff5bb3df7df2a6d31b0d7eea016e496a2588aaf2ed06e947cc3c23"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, minimum, maximum, label):
    require(isinstance(value, int) and not isinstance(value, bool)
            and minimum <= value <= maximum, label + " outside finite declared bounds")
    return value


def f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def checked_block(value):
    integer(value, 8, MAX_BLOCK_BYTES, "read block")
    require(value % 8 == 0, "audio read block must be whole stereo frames")
    return value


def checked_path(value):
    path = Path(value)
    require(path.is_absolute(), "input/output path must be absolute")
    require(len(os.fsencode(path)) <= MAX_PATH_BYTES, "encoded path exceeds finite bound")
    return path


class FrozenFileReader:
    """One immutable initial byte budget, then at most one extra-byte probe.

    Actual calls/requests/returns are counted even when short reads or growth
    fail. A changed stat never replenishes this reader's initial budget.
    """
    def __init__(self, handle, initial_bytes, block_bytes, label):
        integer(initial_bytes, 0, MAX_FRAMES * 8 + MAX_RIFF_METADATA, "initial byte budget")
        checked_block(block_bytes)
        self.handle, self.initial_bytes, self.block_bytes, self.label = handle, initial_bytes, block_bytes, label
        self.remaining = initial_bytes
        self.initial_returned = self.requested = self.returned = self.calls = self.maximum_request = 0
        self.eof_probe_attempted = self.eof_confirmed = False
        self.failed = False
        self.extra_observed = None

    def _read(self, request):
        self.calls += 1
        self.requested += request
        self.maximum_request = max(self.maximum_request, request)
        try:
            block = self.handle.read(request)
        except BaseException:
            self.failed = True
            raise
        self.returned += len(block)
        if len(block) > request:
            self.failed = True
            raise ValueError(self.label + " reader returned more than requested")
        return block

    def initial_blocks(self):
        require(not self.failed and not self.eof_probe_attempted, self.label + " failed/sealed initial budget cannot be retried")
        while self.remaining:
            request = min(self.block_bytes, self.remaining)
            block = self._read(request)
            self.remaining -= len(block)
            self.initial_returned += len(block)
            if len(block) != request:
                self.failed = True
                raise ValueError(self.label + " short read/early EOF inside frozen initial bytes")
            yield block

    def check_eof_once(self):
        require(not self.failed and self.remaining == 0, self.label + " initial bytes were not completely read")
        require(not self.eof_probe_attempted, self.label + " extra-byte probe already attempted")
        self.eof_probe_attempted = True
        extra = self._read(1)
        self.extra_observed = len(extra)
        if extra:
            self.failed = True
            raise ValueError(self.label + " grew beyond frozen initial byte budget")
        self.eof_confirmed = True

    def snapshot(self):
        return {"initial_file_bytes_frozen": self.initial_bytes, "initial_bytes_returned": self.initial_returned,
                "initial_bytes_remaining": self.remaining, "total_requested_bytes": self.requested,
                "total_returned_bytes": self.returned, "read_calls_attempted": self.calls,
                "maximum_request_bytes": self.maximum_request,
                "initial_byte_budget_consumed": self.remaining == 0,
                "extra_byte_probe_attempted": self.eof_probe_attempted,
                "extra_bytes_observed": self.extra_observed, "eof_confirmed": self.eof_confirmed,
                "failed": self.failed}


def bounded_project(path):
    """The frozen JSON score is bounded separately; this never stages audio."""
    path = checked_path(path)
    path_stat = path.stat()
    with path.open("rb", buffering=0) as handle:
        stat = os.fstat(handle.fileno())
        identity = FileIdentity(path, handle, path_stat, stat, "score")
        integer(stat.st_size, 0, MAX_PROJECT_BYTES, "score file length")
        reader = FrozenFileReader(handle, stat.st_size, MAX_BLOCK_BYTES, "score")
        content = bytearray()
        for block in reader.initial_blocks():
            content.extend(block)
        reader.check_eof_once()
        identity.verify("during frozen read")
        return bytes(content)


@dataclass(frozen=True)
class Plan:
    sample_rate: int
    tempo: float
    musical_frames: int
    coda_start_frame: int
    tail_frames: int
    latency_frames: int
    quantum: int
    master_flat_until_beat: float
    master_final_beat: float
    scope: str

    def __post_init__(self):
        integer(self.sample_rate, 1, 192_000, "sample rate")
        integer(self.musical_frames, 1, MAX_FRAMES, "musical frames")
        integer(self.coda_start_frame, 0, self.musical_frames, "coda start")
        integer(self.tail_frames, 0, MAX_FRAMES, "fixed tail")
        integer(self.latency_frames, 0, MAX_FRAMES, "declared latency")
        integer(self.quantum, 1, 4096, "quantum")
        require(isinstance(self.tempo, (int, float)) and not isinstance(self.tempo, bool)
                and math.isfinite(self.tempo) and 1 <= self.tempo <= 1000, "tempo outside finite bound")
        require(all(isinstance(point, (int, float)) and not isinstance(point, bool)
                    for point in (self.master_flat_until_beat, self.master_final_beat))
                and math.isfinite(self.master_flat_until_beat) and math.isfinite(self.master_final_beat)
                and 0 <= self.master_flat_until_beat < self.master_final_beat <= 1_000_000,
                "invalid declared master curve positions")
        require(self.scope in ("frozen Saved13", "synthetic fixture only"), "undeclared plan scope")
        integer(self.retained_frames, 1, MAX_FRAMES, "retained frames")
        integer(self.generated_frames, 1, MAX_FRAMES, "generated frames")

    @property
    def retained_frames(self):
        return self.musical_frames + self.tail_frames

    @property
    def generated_frames(self):
        total = max(self.latency_frames + self.retained_frames, self.musical_frames + 1)
        return ((total + self.quantum - 1) // self.quantum) * self.quantum

    @property
    def padding_frames(self):
        return self.generated_frames - self.latency_frames - self.retained_frames

    @classmethod
    def saved13(cls, project_bytes, latency):
        # The copied builder validates original SHA, every note/placement count,
        # direct Master channel and the enabled .7/.7/.56 replacement curve.
        dependency = Path(frozen_score.__file__)
        dependency_path_stat = dependency.stat()
        with dependency.open("rb", buffering=0) as handle:
            stat = os.fstat(handle.fileno())
            identity = FileIdentity(dependency, handle, dependency_path_stat, stat, "independent score dependency")
            integer(stat.st_size, 0, MAX_BLOCK_BYTES, "independent score dependency length")
            reader = FrozenFileReader(handle, stat.st_size, MAX_BLOCK_BYTES, "score dependency")
            source = bytearray()
            for block in reader.initial_blocks():
                source.extend(block)
            reader.check_eof_once()
            identity.verify("during frozen read")
        require(len(source) <= MAX_BLOCK_BYTES
                and hashlib.sha256(source).hexdigest() == SPEC_BUILDER_SHA,
                "independent frozen score dependency changed")
        spec = frozen_score.build(project_bytes, latency)
        geometry = spec["geometry"]
        require(spec["project_sha256"] == SAVED_SHA and spec["note_identities"] == 405
                and spec["identity_boundaries"] == 810 and spec["wire_messages"] == 782
                and spec["source_mixer_track_id"] == 0xFFFFFFFF
                and spec["static_channel_f32"] == f32(.55)
                and spec["master_points_f32"] == [[0., f32(.7)], [120., f32(.7)], [128., f32(.56)]]
                and spec["master_automation_replaces_base"] and spec["output_gain"] == 1
                and spec["absolute_tolerance"] == ABS_TOLERANCE, "frozen full score policy differs")
        plan = cls(48000, 84, geometry["musical_frames"], frozen_score.sample_frame(128),
                   geometry["tail_frames"], latency, 128, 120., 128., "frozen Saved13")
        require(plan.retained_frames == geometry["retained_frames"]
                and plan.generated_frames == geometry["generated_frames"]
                and plan.padding_frames == geometry["discarded_suffix_frames"],
                "independent full geometry disagrees")
        return plan, {
            "project_sha256": SAVED_SHA, "project_bytes": len(project_bytes),
            "independent_builder_sha256": SPEC_BUILDER_SHA,
            "note_identities": 405, "patterns": 5, "placements": 5,
            "identity_boundaries": 810, "wire_messages": 782,
            "tempo": 84, "song_length_beats": 160, "source_route": "0xffffffff direct MASTER",
            "channel_volume_f32": f32(.55), "master_automation_replaces_base": True,
            "master_points_f32": spec["master_points_f32"], "output_gain": 1,
        }


def master_control(plan, frame):
    beat = f32(min(frame, plan.musical_frames - 1) * plan.tempo / (60 * plan.sample_rate))
    low, high = f32(.7), f32(.56)
    start, end = f32(plan.master_flat_until_beat), f32(plan.master_final_beat)
    if beat <= start:
        return low
    if beat >= end:
        return high
    fraction = f32(f32(beat - start) / f32(end - start))
    return f32(low + f32(f32(high - low) * fraction))


def expected_sample(raw, plan, frame):
    require(math.isfinite(raw), "nonfinite actual raw has no valid expected sample")
    channel = f32(max(-8., min(8., raw)) * f32(.55))
    direct_master = f32(0.0 + channel)
    driven_master = f32(direct_master * master_control(plan, frame))
    return f32(math.tanh(driven_master))  # Once only; output gain is exactly 1.


def fingerprint(stat):
    return (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns)


class FileIdentity:
    """Each stat API keeps its exact baseline; only common identity crosses APIs.

    Windows Path.stat and fstat may expose different ctime_ns values for the
    same unchanged file. This is not a timestamp tolerance: both APIs' complete
    five-field signatures must remain exactly unchanged independently.
    """
    def __init__(self, path, handle, path_stat, handle_stat, label):
        self.path, self.handle, self.label = path, handle, label
        self.path_signature = fingerprint(path_stat)
        self.handle_signature = fingerprint(handle_stat)
        require(self.path_signature[:4] == self.handle_signature[:4],
                label + " path/open handle identity differs")
        self.verify("while binding open handle")

    def verify(self, context):
        current_handle = fingerprint(os.fstat(self.handle.fileno()))
        current_path = fingerprint(self.path.stat())
        require(current_handle == self.handle_signature,
                self.label + " handle changed " + context)
        require(current_path == self.path_signature,
                self.label + " path changed/replaced " + context)
        require(current_path[:4] == current_handle[:4],
                self.label + " path/open handle identity differs " + context)


class Errors:
    def __init__(self):
        self.rows = []
        self.count = 0

    def add(self, stage, message):
        self.count += 1
        if len(self.rows) < 32:
            prefix = str(message).encode("utf-8", errors="replace")[:4096].decode("utf-8", errors="ignore")
            self.rows.append({"stage": stage, "error": prefix})


class Metrics:
    """Constant state: compensated squares and frame runs cross read boundaries."""
    def __init__(self, start, end):
        self.start, self.end = start, end
        self.frames = self.finite = self.nonfinite = self.clipped = self.nonzero = self.quiet = 0
        self.invalid_frames = self.negative_zeros = 0
        self.peak = self.squares = self.compensation = self.max_delta = 0.
        self.first = self.last = None
        self.leading = self.zero_run = self.quiet_run = self.max_zero = self.max_quiet = self.max_internal = 0
        self.leading_open = True
        self.internal_anchor = False
        self.previous = (None, None)

    def feed(self, frame, pair):
        self.frames += 1
        valid = all(math.isfinite(value) for value in pair)
        next_previous = []  # Exactly two entries, never an audio collection.
        for channel, value in enumerate(pair):
            if math.isfinite(value):
                self.finite += 1
                self.peak = max(self.peak, abs(value))
                term = value * value - self.compensation
                updated = self.squares + term
                self.compensation = (updated - self.squares) - term
                self.squares = updated
                self.clipped += abs(value) >= 1.
                self.negative_zeros += value == 0 and math.copysign(1., value) < 0
                if self.previous[channel] is not None:
                    self.max_delta = max(self.max_delta, abs(value - self.previous[channel]))
                next_previous.append(value)
            else:
                self.nonfinite += 1
                next_previous.append(None)
        self.previous = tuple(next_previous)
        if not valid:
            self.invalid_frames += 1
            self.zero_run = self.quiet_run = 0
            self.leading_open = self.internal_anchor = False
            return
        active = any(value != 0 for value in pair)
        if active:
            self.nonzero += 1
            if self.first is None:
                self.first = frame
            elif self.internal_anchor and self.zero_run:
                self.max_internal = max(self.max_internal, self.zero_run)
            self.last, self.zero_run = frame, 0
            self.leading_open = False
            self.internal_anchor = True
        else:
            self.zero_run += 1
            self.max_zero = max(self.max_zero, self.zero_run)
            if self.leading_open:
                self.leading += 1
        quiet = all(abs(value) <= QUIET_THRESHOLD for value in pair)
        self.quiet += quiet
        self.quiet_run = self.quiet_run + 1 if quiet else 0
        self.max_quiet = max(self.max_quiet, self.quiet_run)

    def result(self, rate):
        return {
            "declared_start_frame": self.start, "declared_end_frame_exclusive": self.end,
            "observed_frames": self.frames, "observed_samples": self.frames * 2,
            "finite_samples": self.finite, "nonfinite_samples": self.nonfinite,
            "invalid_frames": self.invalid_frames, "peak_of_finite_samples": self.peak,
            "rms": math.sqrt(self.squares / (self.frames * 2)) if self.frames and not self.nonfinite else None,
            "finite_only_rms_if_invalid": math.sqrt(self.squares / self.finite) if self.finite else None,
            "clipping_samples_at_or_above_one": self.clipped, "negative_zero_samples": self.negative_zeros,
            "nonzero_frames": self.nonzero, "quiet_frames": self.quiet,
            "first_nonzero_frame": self.first, "last_nonzero_frame": self.last,
            "leading_exact_zero_frames": self.leading, "trailing_exact_zero_frames": self.zero_run,
            "maximum_exact_zero_run": self.max_zero, "maximum_internal_exact_zero_run": self.max_internal,
            "maximum_quiet_run": self.max_quiet, "maximum_quiet_run_seconds": self.max_quiet / rate,
            "maximum_adjacent_same_channel_delta": self.max_delta,
        }


class MetricBank:
    def __init__(self, plan, raw, actual_frames):
        shift = plan.latency_frames if raw else 0
        n, r, c = plan.musical_frames, plan.retained_frames, plan.coda_start_frame
        spans = {"all": (0, actual_frames), "musical": (shift, shift + n),
                 "coda": (shift + c, shift + n), "fixed_tail": (shift + n, shift + r),
                 "end100ms": (shift + max(0, r - plan.sample_rate // 10), shift + r)}
        if raw:
            spans.update({"latency_prefix": (0, shift), "retained": (shift, shift + r),
                          "padding_suffix": (shift + r, actual_frames)})
        self.regions = {name: Metrics(start, end) for name, (start, end) in spans.items()}
        self.rate = plan.sample_rate

    def feed(self, frame, pair):
        for metric in self.regions.values():
            if metric.start <= frame < metric.end:
                metric.feed(frame, pair)

    def result(self):
        return {name: metric.result(self.rate) for name, metric in self.regions.items()}


def small_read(handle, length, hasher):
    integer(length, 0, MAX_BLOCK_BYTES, "individual read length")
    data = handle.read(length)
    require(len(data) == length, "file ended inside a declared RIFF field/chunk")
    hasher.update(data)
    return data


def parse_wave(handle, plan, block_bytes):
    """One full bounded pass hashes every RIFF byte; no PCM copy is retained."""
    checked_block(block_bytes)
    stat = os.fstat(handle.fileno())
    require(12 <= stat.st_size <= MAX_FRAMES * 8 + MAX_RIFF_METADATA, "WAV length outside finite bound")
    whole_hash, pcm_hash = hashlib.sha256(), hashlib.sha256()
    prefix = small_read(handle, 12, whole_hash)
    require(prefix[:4] == b"RIFF" and prefix[8:] == b"WAVE", "not little-endian RIFF WAVE")
    require(struct.unpack_from("<I", prefix, 4)[0] + 8 == stat.st_size, "RIFF declared/file length mismatch")
    chunks, fmt, data_range, fact = [], None, None, None
    metadata_bytes = 12
    while handle.tell() < stat.st_size:
        require(len(chunks) < MAX_CHUNKS, "RIFF chunk count exceeds finite bound")
        require(handle.tell() + 8 <= stat.st_size, "truncated RIFF chunk header")
        chunk_header = small_read(handle, 8, whole_hash)
        identifier, size = chunk_header[:4], struct.unpack_from("<I", chunk_header, 4)[0]
        start, end = handle.tell(), handle.tell() + size
        require(end + size % 2 <= stat.st_size, "RIFF chunk/padding exceeds actual file")
        chunks.append({"id_hex": identifier.hex(), "payload_offset": start, "bytes": size})
        metadata_bytes += 8 + size % 2 + (0 if identifier == b"data" else size)
        require(metadata_bytes <= MAX_RIFF_METADATA, "RIFF metadata exceeds finite bound")
        if identifier == b"fmt ":
            require(fmt is None and size in (16, 18), "duplicate or unsupported fmt chunk size")
            payload = small_read(handle, size, whole_hash)
            fmt = struct.unpack_from("<HHIIHH", payload)
            require(size == 16 or payload[16:] == b"\0\0", "nonzero IEEE Float32 fmt extension")
        elif identifier == b"fact":
            require(fact is None and size == 4, "duplicate or malformed fact chunk")
            fact = struct.unpack("<I", small_read(handle, 4, whole_hash))[0]
        else:
            if identifier == b"data":
                require(data_range is None, "duplicate PCM data chunk")
                require(size == plan.retained_frames * 8, "PCM data length differs from exact retained frames")
                data_range = (start, size)
            remaining = size
            while remaining:
                block = small_read(handle, min(block_bytes, remaining), whole_hash)
                if identifier == b"data":
                    pcm_hash.update(block)
                remaining -= len(block)
        if size % 2:
            small_read(handle, 1, whole_hash)
    require(handle.tell() == stat.st_size and not handle.read(1), "extra bytes after complete RIFF")
    require(fmt == (3, 2, plan.sample_rate, plan.sample_rate * 8, 8, 32), "wrong Float32 stereo format/rate/alignment")
    require(data_range is not None, "WAV has no PCM data")
    require(fact is None or fact == plan.retained_frames, "fact sample length differs from retained frames")
    require(fingerprint(os.fstat(handle.fileno())) == fingerprint(stat), "WAV changed during complete RIFF pass")
    return {"file_bytes": stat.st_size, "sha256": whole_hash.hexdigest(),
            "pcm_sha256": pcm_hash.hexdigest(), "data_offset": data_range[0],
            "data_bytes": data_range[1], "frames": plan.retained_frames,
            "fmt": list(fmt), "fact_frames": fact, "chunks": chunks,
            "fingerprint": fingerprint(stat)}


class WaveCursor:
    def __init__(self, handle, info, block_bytes):
        checked_block(block_bytes)
        integer(info["data_bytes"], 8, MAX_FRAMES * 8, "frozen WAV PCM byte count")
        integer(info["frames"], 1, MAX_FRAMES, "frozen WAV PCM frame count")
        integer(info["data_offset"], 12, MAX_FRAMES * 8 + MAX_RIFF_METADATA, "frozen WAV PCM offset")
        require(info["data_bytes"] == info["frames"] * 8, "frozen WAV PCM byte/frame count differs")
        self.handle, self.info, self.block_bytes = handle, info, block_bytes
        handle.seek(info["data_offset"])
        self.remaining = info["data_bytes"]
        self.block = b""
        self.offset = self.frames = 0
        self.hash = hashlib.sha256()
        self.failed = False
        self.requested = self.returned = self.calls = self.maximum_request = 0

    def next_pair(self):
        require(not self.failed, "WAV failed sample pass cannot be retried/drained")
        if self.offset == len(self.block):
            if not self.remaining:
                return None
            size = min(self.block_bytes, self.remaining)
            self.calls += 1
            self.requested += size
            self.maximum_request = max(self.maximum_request, size)
            try:
                block = self.handle.read(size)
            except BaseException:
                self.failed = True
                raise
            self.returned += len(block)
            # Track actual consumed bytes before rejecting a short/partial read.
            # The failed cursor cannot resume into remaining PCM or later chunks.
            self.remaining -= min(len(block), size)
            self.block = block
            self.offset = 0
            if len(block) != size:
                self.failed = True
                raise ValueError("WAV changed/truncated during sample pass")
            self.hash.update(block)
        bits = self.block[self.offset:self.offset + 8]
        self.offset += 8
        self.frames += 1
        return struct.unpack("<ff", bits), bits

    def snapshot(self):
        return {"initial_pcm_bytes_frozen": self.info["data_bytes"],
                "initial_pcm_bytes_remaining": self.remaining,
                "total_requested_bytes": self.requested,
                "total_returned_bytes": self.returned,
                "read_calls_attempted": self.calls,
                "maximum_request_bytes": self.maximum_request,
                "sample_frames_scanned": self.frames, "failed": self.failed}


def raw_summary(stat, reader, hasher, hashed_bytes, bank, nonfinite_rows, rate, stable):
    frames = bank.regions["all"].frames
    return {"file_bytes": stat.st_size, "sha256": hasher.hexdigest() if hashed_bytes == stat.st_size else None,
            "initial_bytes_hashed": hashed_bytes, "initial_bytes_sample_scanned": frames * 8,
            "complete_initial_samples_scanned": frames * 8 == stat.st_size,
            "input_stable": stable, "read_contract": reader.snapshot(),
            "frames": frames, "audited_samples": frames * 2, "duration_seconds": frames / rate,
            "first_nonfinite_samples": nonfinite_rows, "metrics": bank.result()}


def validate_files(raw_path, wav_path, plan, *, block_bytes=MAX_BLOCK_BYTES,
                   first_mismatches=16, capture_reference_sha256=None,
                   reported_tail=None, metadata_tail_limit_reached=None):
    """Complete file consistency only; supplied latency/metadata is not authenticated."""
    require(isinstance(plan, Plan), "explicit finite Plan required")
    checked_block(block_bytes)
    integer(first_mismatches, 0, MAX_MISMATCH_ROWS, "first mismatch count")
    if capture_reference_sha256 is not None:
        require(isinstance(capture_reference_sha256, str) and len(capture_reference_sha256) == 64
                and all(char in "0123456789abcdef" for char in capture_reference_sha256), "invalid external capture hash")
    if reported_tail is not None:
        integer(reported_tail, 0, 0xFFFFFFFF, "reported tail")
    require(metadata_tail_limit_reached is None or isinstance(metadata_tail_limit_reached, bool), "invalid tail-limit flag")
    raw_path, wav_path = checked_path(raw_path), checked_path(wav_path)
    require(raw_path != wav_path, "raw and WAV paths must differ")
    errors, mismatches, invalid_raw = Errors(), [], []
    warnings = ["File consistency does not authenticate capture latency or SDK/source/clone lifecycle."]
    if reported_tail == 0xFFFFFFFF:
        warnings.append("Unknown plug-in tail metadata retained; declared fixed tail does not prove complete natural release.")
    elif reported_tail is None:
        warnings.append("Plug-in tail metadata unavailable; no complete natural-tail claim.")
    if metadata_tail_limit_reached:
        warnings.append("Producer reports metadata tail limit reached; warning remains authoritative only after external binding.")
    comparison = {"expected_samples": plan.retained_frames * 2, "compared_samples": 0,
                  "finite_expected_samples": 0, "samples_outside_tolerance": 0,
                  "float32_bits_compared_samples": 0, "finite_difference_samples": 0,
                  "samples_with_different_float32_bits": 0, "maximum_absolute_sample_error": 0.,
                  "absolute_tolerance_frozen": ABS_TOLERANCE, "first_mismatches": mismatches}
    raw_report, wav_report = None, None
    raw_handle = wav_handle = None
    wave_info = wave_cursor = raw_bank = wav_bank = None
    raw_reader = raw_stat = raw_hash = None
    raw_identity = wav_identity = None
    raw_hashed_bytes = 0
    expected_hash, expected_buffer = hashlib.sha256(), bytearray(MAX_BLOCK_BYTES)
    expected_bytes = expected_used = 0
    try:
        try:
            wav_path_stat = wav_path.stat()
            wav_handle = wav_path.open("rb", buffering=0)
            wav_identity = FileIdentity(wav_path, wav_handle, wav_path_stat,
                                        os.fstat(wav_handle.fileno()), "WAV")
            wave_info = parse_wave(wav_handle, plan, block_bytes)
            wav_identity.verify("after complete RIFF pass")
            wave_cursor = WaveCursor(wav_handle, wave_info, block_bytes)
            wav_bank = MetricBank(plan, False, plan.retained_frames)
            wav_report = {key: value for key, value in wave_info.items() if key != "fingerprint"}
        except (OSError, ValueError, struct.error) as reason:
            errors.add("wav_structure", reason)
        try:
            raw_path_stat = raw_path.stat()
            raw_handle = raw_path.open("rb", buffering=0)
            raw_stat = os.fstat(raw_handle.fileno())
            raw_identity = FileIdentity(raw_path, raw_handle, raw_path_stat, raw_stat, "raw")
            if wav_handle is not None:
                wav_stat = os.fstat(wav_handle.fileno())
                require((raw_stat.st_dev, raw_stat.st_ino) != (wav_stat.st_dev, wav_stat.st_ino),
                        "raw and WAV claims alias the same actual file")
            require(0 <= raw_stat.st_size <= MAX_FRAMES * 8, "raw file exceeds finite frame bound")
            require(raw_stat.st_size % 8 == 0, "raw file has partial stereo Float32 frame")
            actual_frames = raw_stat.st_size // 8
            if actual_frames != plan.generated_frames:
                errors.add("raw_geometry", "raw frames %d differ from exact generated %d" % (actual_frames, plan.generated_frames))
            raw_bank = MetricBank(plan, True, actual_frames)
            raw_hash = hashlib.sha256()
            raw_reader = FrozenFileReader(raw_handle, raw_stat.st_size, block_bytes, "raw")
            frame = 0
            for block in raw_reader.initial_blocks():
                raw_hash.update(block)
                raw_hashed_bytes += len(block)
                require(len(block) % 8 == 0, "raw changed/truncated within stereo frame")
                for offset in range(0, len(block), 8):
                    bits = block[offset:offset + 8]
                    pair = struct.unpack("<ff", bits)
                    raw_bank.feed(frame, pair)
                    for channel, value in enumerate(pair):
                        if not math.isfinite(value) and len(invalid_raw) < first_mismatches:
                            invalid_raw.append({"raw_frame": frame, "channel": channel,
                                                "float32_bits_hex": bits[channel * 4:channel * 4 + 4].hex()})
                    output_frame = frame - plan.latency_frames
                    if 0 <= output_frame < plan.retained_frames and wave_cursor is not None:
                        actual = wave_cursor.next_pair()
                        require(actual is not None, "WAV ended before declared retained sample")
                        wav_pair, wav_bits = actual
                        wav_bank.feed(output_frame, wav_pair)
                        for channel in range(2):
                            source, observed = pair[channel], wav_pair[channel]
                            comparison["compared_samples"] += 1
                            expected = None
                            if math.isfinite(source):
                                expected = expected_sample(source, plan, output_frame)
                                packed = struct.pack("<f", expected)
                                expected_buffer[expected_used:expected_used + 4] = packed
                                expected_used += 4
                                expected_bytes += 4
                                comparison["finite_expected_samples"] += 1
                                comparison["float32_bits_compared_samples"] += 1
                                comparison["samples_with_different_float32_bits"] += packed != wav_bits[channel * 4:channel * 4 + 4]
                                if expected_used == MAX_BLOCK_BYTES:
                                    expected_hash.update(expected_buffer)
                                    expected_used = 0
                            invalid = expected is None or not math.isfinite(observed)
                            difference = None if invalid else abs(observed - expected)
                            if difference is not None:
                                comparison["finite_difference_samples"] += 1
                                comparison["maximum_absolute_sample_error"] = max(comparison["maximum_absolute_sample_error"], difference)
                            if invalid or difference > ABS_TOLERANCE:
                                comparison["samples_outside_tolerance"] += 1
                                if len(mismatches) < first_mismatches:
                                    row = {"frame": output_frame, "channel": channel,
                                           "reason": "nonfinite raw/WAV" if invalid else "fixed tolerance exceeded",
                                           "raw_bits_hex": bits[channel * 4:channel * 4 + 4].hex(),
                                           "wav_bits_hex": wav_bits[channel * 4:channel * 4 + 4].hex()}
                                    if not invalid:
                                        row.update({"raw": source, "master_control": master_control(plan, output_frame),
                                                    "expected": expected, "actual": observed, "absolute_error": difference})
                                    mismatches.append(row)
                    frame += 1
            raw_reader.check_eof_once()
            raw_identity.verify("during audit")
            require(frame == actual_frames, "not every actual raw frame was scanned")
            raw_report = raw_summary(raw_stat, raw_reader, raw_hash, raw_hashed_bytes,
                                     raw_bank, invalid_raw, plan.sample_rate, True)
            if raw_bank.regions["all"].nonfinite:
                errors.add("raw_finite", "full raw including latency/padding contains nonfinite samples")
        except (OSError, ValueError, struct.error) as reason:
            if raw_reader is not None and raw_bank is not None:
                raw_report = raw_summary(raw_stat, raw_reader, raw_hash, raw_hashed_bytes,
                                         raw_bank, invalid_raw, plan.sample_rate, False)
            errors.add("raw_audio", reason)
        if wave_cursor is not None:
            try:
                # Even an invalid/short raw does not prevent auditing every WAV sample.
                while wave_cursor.frames < plan.retained_frames:
                    actual = wave_cursor.next_pair()
                    require(actual is not None, "WAV cursor ended before frozen retained count")
                    wav_bank.feed(wave_cursor.frames - 1, actual[0])
                require(wave_cursor.next_pair() is None, "WAV cursor exceeds frozen retained count")
                require(wave_cursor.frames == plan.retained_frames, "not every WAV frame was scanned")
                require(wave_cursor.hash.hexdigest() == wave_info["pcm_sha256"], "WAV PCM changed between complete RIFF and sample passes")
                # parse_wave's fstat-to-fstat guard remains separate and exact.
                require(fingerprint(os.fstat(wav_handle.fileno())) == wave_info["fingerprint"],
                        "WAV handle changed between complete RIFF and sample passes")
                wav_identity.verify("during sample pass")
                wav_report["sample_pass_pcm_sha256"] = wave_cursor.hash.hexdigest()
                wav_report["audited_samples"] = wave_cursor.frames * 2
                wav_report["duration_seconds"] = wave_cursor.frames / plan.sample_rate
                wav_report["metrics"] = wav_bank.result()
                if wav_bank.regions["all"].nonfinite:
                    errors.add("wav_finite", "WAV contains nonfinite samples")
            except (OSError, ValueError, struct.error) as reason:
                errors.add("wav_audio", reason)
            if wav_report is not None:
                wav_report["sample_read_contract"] = wave_cursor.snapshot()
    finally:
        if raw_handle is not None:
            raw_handle.close()
        if wav_handle is not None:
            wav_handle.close()
    expected_hash.update(memoryview(expected_buffer)[:expected_used])
    comparison["derived_expected_pcm_bytes"] = expected_bytes
    comparison["derived_expected_pcm_sha256"] = expected_hash.hexdigest() if expected_bytes == plan.retained_frames * 8 else None
    comparison["mismatch_rows_truncated"] = comparison["samples_outside_tolerance"] > len(mismatches)
    if not comparison["finite_difference_samples"]:
        comparison["maximum_absolute_sample_error"] = None
    if comparison["compared_samples"] != comparison["expected_samples"]:
        errors.add("comparison", "not every declared retained stereo sample could be compared")
    if wav_report is not None and "metrics" in wav_report:
        music = wav_report["metrics"]["musical"]
        if music["observed_frames"] and not music["nonfinite_samples"] and music["nonzero_frames"] == 0:
            warnings.append("Musical window is entirely silent; matching files do not prove instrument sound.")
    matches = errors.count == 0 and comparison["samples_outside_tolerance"] == 0
    return {
        "schema": 1, "plan_scope_supplied": plan.scope, "frozen_input_verified": False,
        "raw_path": str(raw_path), "wav_path": str(wav_path),
        "geometry": {"sample_rate": plan.sample_rate, "tempo": plan.tempo, "quantum": plan.quantum,
                     "musical_frames": plan.musical_frames, "coda_start_frame": plan.coda_start_frame,
                     "tail_frames": plan.tail_frames, "declared_latency_frames": plan.latency_frames,
                     "retained_frames": plan.retained_frames, "generated_frames": plan.generated_frames,
                     "padding_suffix_frames": plan.padding_frames},
        "policy": {"read_block_bytes": block_bytes, "maximum_frames": MAX_FRAMES,
                   "first_mismatch_limit": first_mismatches, "channel_gain_f32": f32(.55),
                   "master_low_f32": f32(.7), "master_high_f32": f32(.56),
                   "master_replaces_static_base": True, "raw_clamp": [-8., 8.],
                   "tanh_applications": 1, "output_gain": 1, "absolute_tolerance": ABS_TOLERANCE},
        "raw_audio": raw_report, "wav_audio": wav_report, "comparison": comparison,
        "errors": errors.rows, "error_count": errors.count, "error_rows_truncated": errors.count > len(errors.rows),
        "file_consistency_matches": matches, "warnings": warnings,
        "capture_reference_sha256_supplied": capture_reference_sha256,
        "actual_capture_binding_verified": False, "reported_tail_supplied": reported_tail,
        "metadata_tail_limit_reached_supplied": metadata_tail_limit_reached,
        "unknown_metadata_tail_warning_retained": reported_tail == 0xFFFFFFFF,
        "production_oracle_imported": False, "alignment_or_tolerance_fitted": False,
        "sdk_verified": False, "physical_listening": False,
        "complete_natural_tail_claim": False, "full_noire_qualification_claim": False,
    }


def validate_saved13(project_path, raw_path, wav_path, declared_latency, **kwargs):
    plan, input_binding = Plan.saved13(bounded_project(project_path), declared_latency)
    report = validate_files(raw_path, wav_path, plan, **kwargs)
    report["frozen_input"] = input_binding
    report["frozen_input_verified"] = True
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--raw", type=Path, required=True)
    parser.add_argument("--wav", type=Path, required=True)
    parser.add_argument("--actual-latency", type=int, required=True)
    parser.add_argument("--capture-reference-sha256")
    parser.add_argument("--reported-tail", type=int)
    parser.add_argument("--metadata-tail-limit-reached", action="store_true")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    out = checked_path(args.out)
    require(out not in (checked_path(args.project), checked_path(args.raw), checked_path(args.wav)), "report cannot replace an input")
    report = validate_saved13(args.project, args.raw, args.wav, args.actual_latency,
                             capture_reference_sha256=args.capture_reference_sha256,
                             reported_tail=args.reported_tail,
                             metadata_tail_limit_reached=args.metadata_tail_limit_reached)
    data = (json.dumps(report, indent=2, allow_nan=False) + "\n").encode("utf-8")
    require(len(data) <= 1024 * 1024, "bounded report exceeds finite limit")
    with out.open("xb") as handle:
        handle.write(data)
    print(json.dumps({"report": str(out), "sha256": hashlib.sha256(data).hexdigest(),
                      "file_consistency_matches": report["file_consistency_matches"],
                      "sdk_verified": False, "error_count": report["error_count"]}))
    return 0 if report["file_consistency_matches"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
