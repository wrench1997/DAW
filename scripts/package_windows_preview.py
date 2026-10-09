#!/usr/bin/env python3
"""Create/check an unsigned MSVC developer PREVIEW; Python 3.11+, stdlib only.

This is deliberately separate from the pinned gnullvm release contract. It never
runs an executable, downloads dependencies, signs, uploads, or publishes files.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path, PurePosixPath
import posixpath
import re
import stat
import struct
import subprocess
import sys
import tomllib
import zipfile
from urllib.parse import unquote, urlsplit

TARGET = "x86_64-pc-windows-msvc"
TOOLCHAIN = "1.99.0-x86_64-pc-windows-msvc"
RUSTFLAGS = "-C target-feature=+crt-static"
BINARIES = ("citrus-studio.exe", "vst3-host-helper.exe")
SOURCE_FILES = (
    "LICENSE", "THIRD_PARTY_NOTICES.md", "README.md", "DEV_STATE.md",
    "docs/FL_STUDIO_PARITY.md", "docs/BUILD_AND_RELEASE.md",
    "docs/DEVELOPMENT_ROADMAP.md", "docs/WORK_LOG.md", "docs/WINDOWS_PREVIEW.md",
    "docs/HISTORICAL_DEV_STATE.md",
)
# Source-only, manually executed real-plugin QA; no plugin or compiled probe ships.
# Exact filenames are reviewed together; any present bundle must be complete.
REAL_VST3_QA_ROOT = "qa/real_vst3/"
REAL_VST3_QA_FILES = frozenset((
    "qa/real_vst3/.gitignore",
    "qa/real_vst3/CONTENTS-SHA256.txt",
    "qa/real_vst3/PROVENANCE.md",
    "qa/real_vst3/REPRODUCE.md",
    "qa/real_vst3/build_runtime_probe.py",
    "qa/real_vst3/probe.py",
    "qa/real_vst3/provenance.json",
    "qa/real_vst3/receipts/README.md",
    "qa/real_vst3/receipts/effects-probe.json",
    "qa/real_vst3/receipts/instrument-probe.json",
    "qa/real_vst3/receipts/production-runtime.log",
    "qa/real_vst3/receipts/production-runtime.stderr.log",
    "qa/real_vst3/receipts/runtime-build-command.json",
    "qa/real_vst3/receipts/stochas-pattern-output.json",
    "qa/real_vst3/receipts/stochas-pattern.log",
    "qa/real_vst3/receipts/stochas-probe.json",
    "qa/real_vst3/receipts/stochas-probe.log",
    "qa/real_vst3/receipts/stochas-qa-pattern.state",
    "qa/real_vst3/receipts/stochas-qa-pattern.xml",
    "qa/real_vst3/receipts/stochas-summary.md",
    "qa/real_vst3/receipts/stochas-tempo-check.json",
    "qa/real_vst3/receipts/stochas-tempo-check.log",
    "qa/real_vst3/receipts/stochas-vendor-md5sum.txt",
    "qa/real_vst3/receipts/validation-summary.log",
    "qa/real_vst3/receipts/validation.json",
    "qa/real_vst3/receipts/vendor-md5sum.txt",
    "qa/real_vst3/render_probe.py",
    "qa/real_vst3/runtime_probe.rs",
    "qa/real_vst3/scanner-integrated/actual-scan.log",
    "qa/real_vst3/scanner-integrated/actual-scan.stderr.log",
    "qa/real_vst3/scanner-integrated/scanner-reproduction-build.json",
    "qa/real_vst3/scanner/actual-scan.log",
    "qa/real_vst3/scanner/actual-scan.stderr.log",
    "qa/real_vst3/scanner/compile-command.json",
    "qa/real_vst3/scanner/source-and-hashes.json",
    "qa/real_vst3/scanner_probe.rs",
    "qa/real_vst3/stochas_pattern.py",
    "qa/real_vst3/stochas_probe.py",
    "qa/real_vst3/stochas_tempo.py",
    "qa/real_vst3/validate.py",
    "qa/real_vst3/verify_receipts.py",
))
MIDI_ROUTE_QA_ROOT = "qa/plugin_midi_route/"
MIDI_ROUTE_QA_FILES = frozenset((
    "qa/plugin_midi_route/.gitignore",
    "qa/plugin_midi_route/CONTENTS-SHA256.txt",
    "qa/plugin_midi_route/INVENTORY.json",
    "qa/plugin_midi_route/PUBLICATION.json",
    "qa/plugin_midi_route/REPRODUCE.md",
    "qa/plugin_midi_route/build_and_copy.py",
    "qa/plugin_midi_route/comparison-quiet-same-binary/receipts/executable-attribution.json",
    "qa/plugin_midi_route/comparison-quiet-same-binary/receipts/native-fx-reactivation.json",
    "qa/plugin_midi_route/comparison-quiet-same-binary/receipts/native-tests-final.log",
    "qa/plugin_midi_route/comparison-quiet-same-binary/receipts/native-tests-quiet-same-binary.log",
    "qa/plugin_midi_route/comparison-quiet-same-binary/receipts/native-tests.log",
    "qa/plugin_midi_route/comparison-quiet-same-binary/receipts/source-snapshot.json",
    "qa/plugin_midi_route/diagnostic-attempt-4-master-confounded/receipts/executable-attribution.json",
    "qa/plugin_midi_route/diagnostic-attempt-4-master-confounded/receipts/native-tests.log",
    "qa/plugin_midi_route/diagnostic-attempt-4-master-confounded/receipts/source-snapshot.json",
    "qa/plugin_midi_route/diagnostic-attempt-6-fx-recovery-before-fix/receipts/executable-attribution.json",
    "qa/plugin_midi_route/diagnostic-attempt-6-fx-recovery-before-fix/receipts/native-fx-reactivation.json",
    "qa/plugin_midi_route/diagnostic-attempt-6-fx-recovery-before-fix/receipts/native-tests-final.log",
    "qa/plugin_midi_route/diagnostic-attempt-6-fx-recovery-before-fix/receipts/native-tests.log",
    "qa/plugin_midi_route/diagnostic-attempt-6-fx-recovery-before-fix/receipts/source-snapshot.json",
    "qa/plugin_midi_route/diagnostic-attempt-7-off-deadlines/receipts/executable-attribution.json",
    "qa/plugin_midi_route/diagnostic-attempt-7-off-deadlines/receipts/native-fx-reactivation.json",
    "qa/plugin_midi_route/diagnostic-attempt-7-off-deadlines/receipts/native-tests-final.log",
    "qa/plugin_midi_route/diagnostic-attempt-7-off-deadlines/receipts/native-tests.log",
    "qa/plugin_midi_route/diagnostic-attempt-7-off-deadlines/receipts/source-snapshot.json",
    "qa/plugin_midi_route/diagnostic-attempt-8-concurrent-load/receipts/executable-attribution.json",
    "qa/plugin_midi_route/diagnostic-attempt-8-concurrent-load/receipts/native-fx-reactivation.json",
    "qa/plugin_midi_route/diagnostic-attempt-8-concurrent-load/receipts/native-tests-final.log",
    "qa/plugin_midi_route/diagnostic-attempt-8-concurrent-load/receipts/native-tests.log",
    "qa/plugin_midi_route/diagnostic-attempt-8-concurrent-load/receipts/source-snapshot.json",
    "qa/plugin_midi_route/native_suffix.rs",
    "qa/plugin_midi_route/prepare_snapshot.py",
    "qa/plugin_midi_route/receipts/concurrent-load-outcome.json",
    "qa/plugin_midi_route/receipts/executable-attribution.json",
    "qa/plugin_midi_route/receipts/final-source-binding.json",
    "qa/plugin_midi_route/receipts/metronome-confound-verification.json",
    "qa/plugin_midi_route/receipts/native-fx-reactivation.json",
    "qa/plugin_midi_route/receipts/native-overload-recovery.json",
    "qa/plugin_midi_route/receipts/native-production-graph.json",
    "qa/plugin_midi_route/receipts/native-release-isolation.json",
    "qa/plugin_midi_route/receipts/native-retrigger-chord-latch.json",
    "qa/plugin_midi_route/receipts/native-safety-latch.json",
    "qa/plugin_midi_route/receipts/native-tests-final.log",
    "qa/plugin_midi_route/receipts/quiet-comparison-condition.json",
    "qa/plugin_midi_route/receipts/source-snapshot.json",
    "qa/plugin_midi_route/receipts/summary.json",
    "qa/plugin_midi_route/run_native.sh",
    "qa/plugin_midi_route/summarize_receipts.py",
    "qa/plugin_midi_route/verify_metronome.py",
    "qa/plugin_midi_route/verify_receipts.py",
))
TIMING_DEBUG_QA_ROOT = "qa/plugin_timing_debug/"
TIMING_DEBUG_QA_FILES = frozenset((
    "qa/plugin_timing_debug/INVENTORY.json",
    "qa/plugin_timing_debug/INVENTORY.md",
    "qa/plugin_timing_debug/LICENSE",
    "qa/plugin_timing_debug/NORMALIZATION.md",
    "qa/plugin_timing_debug/ORIGINAL_README.md",
    "qa/plugin_timing_debug/PROVENANCE_CHECKS.json",
    "qa/plugin_timing_debug/PUBLICATION_SHA256SUMS",
    "qa/plugin_timing_debug/REPRODUCE.md",
    "qa/plugin_timing_debug/RESULT.md",
    "qa/plugin_timing_debug/build_and_copy.py",
    "qa/plugin_timing_debug/historical-reference/previous-reference.txt",
    "qa/plugin_timing_debug/native_timing_tests.rs",
    "qa/plugin_timing_debug/preflight-18478d2/executable-attribution.json",
    "qa/plugin_timing_debug/preflight-18478d2/source-snapshot.json",
    "qa/plugin_timing_debug/prepare_snapshot.py",
    "qa/plugin_timing_debug/receipts/build-binding.json",
    "qa/plugin_timing_debug/receipts/build-environment.json",
    "qa/plugin_timing_debug/receipts/executable-attribution.json",
    "qa/plugin_timing_debug/receipts/git-object-verification.json",
    "qa/plugin_timing_debug/receipts/helper-build.jsonl",
    "qa/plugin_timing_debug/receipts/helper-build.stderr",
    "qa/plugin_timing_debug/receipts/native-build.jsonl",
    "qa/plugin_timing_debug/receipts/native-build.stderr",
    "qa/plugin_timing_debug/receipts/plugin-provenance.json",
    "qa/plugin_timing_debug/receipts/run-index.json",
    "qa/plugin_timing_debug/receipts/source-snapshot.json",
    "qa/plugin_timing_debug/receipts/status.json",
    "qa/plugin_timing_debug/run_profiles.sh",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/admission-b128-cold.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/admission-b512-cold.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b128_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b128_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b128_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b128_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b2048_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b2048_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b2048_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b2048_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b256_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b256_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b256_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b256_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b512_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b512_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b512_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/b512_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/executable-attribution.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/exit-status.txt",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/fresh-restored-surge.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/raw.log",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/run-binding.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/setup-diagnosis.json",
    "qa/plugin_timing_debug/runs/run001-quiet-f164/source-snapshot.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/admission-b128-cold.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/admission-b128.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/admission-b512-cold.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b128_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b128_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b128_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b128_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b2048_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b2048_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b2048_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b2048_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b256_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b256_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b256_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b256_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b512_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b512_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b512_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/b512_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/controlled-overload-before-retry.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/controlled-overload-blank-cleanup.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/executable-attribution.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/exit-status.txt",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/fresh-restored-surge.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/native_timing_tests.rs",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/raw.log",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/run-binding.json",
    "qa/plugin_timing_debug/runs/run002-quiet-cpal-order/source-snapshot.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/b2048_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/b2048_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/b2048_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/b2048_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/b2048_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/b2048_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/b2048_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/b2048_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/diagnosis.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/executable-attribution.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/exit-status.txt",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/native_timing_tests.rs",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/raw.log",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/run-binding.json",
    "qa/plugin_timing_debug/runs/run003-f164-cold-trace/source-snapshot.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/admission-b128-cold.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/admission-b128.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/admission-b2048-cold.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/admission-b256-cold.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/admission-b256.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/admission-b512-cold.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/admission-b512.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b128_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b128_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b128_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b128_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b128_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b128_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b128_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b128_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b2048_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b2048_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b2048_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b2048_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b2048_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b2048_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b2048_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b2048_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b256_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b256_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b256_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b256_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b256_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b256_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b256_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b256_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b512_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b512_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b512_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b512_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b512_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b512_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b512_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/b512_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/build-binding.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/build_and_copy.py",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/cold-assertion-diagnosis.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/controlled-overload-before-retry.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/controlled-overload-blank-cleanup.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/executable-attribution.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/exit-status.txt",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/fresh-restored-surge.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/host-resources-after.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/host-resources-before.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/native_timing_tests.rs",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/prepare_snapshot.py",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/raw.log",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/run-binding.json",
    "qa/plugin_timing_debug/runs/run004-quiet-f68b9a0/source-snapshot.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/admission-b128-cold.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/admission-b128.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/admission-b2048-cold.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/admission-b2048.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/admission-b256-cold.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/admission-b256.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/admission-b512-cold.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/admission-b512.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b128_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b128_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b128_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b128_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b128_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b128_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b128_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b128_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b2048_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b2048_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b2048_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b2048_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b2048_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b2048_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b2048_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b2048_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b256_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b256_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b256_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b256_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b256_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b256_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b256_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b256_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b512_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b512_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b512_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b512_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b512_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b512_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b512_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/b512_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/build-binding.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/build_and_copy.py",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/controlled-overload-before-retry.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/controlled-overload-blank-cleanup.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/controlled-overload-recovery.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/executable-attribution.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/exit-status.txt",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/fresh-restored-surge.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/host-resources-after.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/host-resources-before.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/native_timing_tests.rs",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/prepare_snapshot.py",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/raw.log",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/run-binding.json",
    "qa/plugin_timing_debug/runs/run005-quiet-native-latency/source-snapshot.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b128_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b128_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b128_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b128_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b128_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b128_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b128_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b128_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b2048_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b2048_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b2048_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b2048_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b2048_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b2048_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b2048_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b2048_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b256_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b256_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b256_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b256_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b256_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b256_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b256_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b256_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b512_routedfalse_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b512_routedfalse_changingfalse.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b512_routedfalse_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b512_routedfalse_changingtrue.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b512_routedtrue_changingfalse-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b512_routedtrue_changingfalse.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b512_routedtrue_changingtrue-cold-trace.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/b512_routedtrue_changingtrue.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/build-binding.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/build_and_copy.py",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/executable-attribution.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/exit-status.txt",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/host-resources-after.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/host-resources-before.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/native_timing_tests.rs",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/prepare_snapshot.py",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/raw.log",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/run-binding.json",
    "qa/plugin_timing_debug/runs/run006-cpu4-native-latency/source-snapshot.json",
    "qa/plugin_timing_debug/summarize_runs.py",
    "qa/plugin_timing_debug/verify_publication.py",
))
TIMING_DEBUG_INVENTORY_SHA256 = "4dc1d40b8e98fa092913eabb06e313077322d4465a322c7780daaacb24f65caa"
TIMING_COMBINED_QA_ROOT = "qa/plugin_timing_combined/"
TIMING_COMBINED_QA_FILES = frozenset((
    "qa/plugin_timing_combined/CONTENTS-SHA256.txt",
    "qa/plugin_timing_combined/README.md",
    "qa/plugin_timing_combined/summary.json",
))
TIMING_COMBINED_INVENTORY_SHA256 = "942e22d6d6e9d75c163d88023823011adec12ed529d401de0c84820dd34bf96d"
NATIVE_EDIT_QA_ROOT = "qa/native_edit_regression/"
NATIVE_EDIT_QA_FILES = frozenset((
    "qa/native_edit_regression/INVENTORY.json",
    "qa/native_edit_regression/README.md",
    "qa/native_edit_regression/REPRODUCE.md",
    "qa/native_edit_regression/RESULT.md",
    "qa/native_edit_regression/SHA256SUMS.json",
    "qa/native_edit_regression/SUMMARY.json",
    "qa/native_edit_regression/build_and_copy.py",
    "qa/native_edit_regression/native_timing_tests.rs",
    "qa/native_edit_regression/prepare_snapshot.py",
    "qa/native_edit_regression/receipts/build-binding.json",
    "qa/native_edit_regression/receipts/build-environment.json",
    "qa/native_edit_regression/receipts/debug-resource-watch.jsonl",
    "qa/native_edit_regression/receipts/executable-attribution.json",
    "qa/native_edit_regression/receipts/git-object-verification.json",
    "qa/native_edit_regression/receipts/helper-build.jsonl",
    "qa/native_edit_regression/receipts/helper-build.stderr",
    "qa/native_edit_regression/receipts/native-build.jsonl",
    "qa/native_edit_regression/receipts/native-build.stderr",
    "qa/native_edit_regression/receipts/plugin-provenance.json",
    "qa/native_edit_regression/receipts/source-snapshot.json",
    "qa/native_edit_regression/run_profiles.sh",
    "qa/native_edit_regression/runs/run001-default2048/b2048_routedfalse_changingfalse-cold-trace.json",
    "qa/native_edit_regression/runs/run001-default2048/b2048_routedfalse_changingfalse.json",
    "qa/native_edit_regression/runs/run001-default2048/b2048_routedfalse_changingtrue-cold-trace.json",
    "qa/native_edit_regression/runs/run001-default2048/b2048_routedfalse_changingtrue.json",
    "qa/native_edit_regression/runs/run001-default2048/b2048_routedtrue_changingfalse-cold-trace.json",
    "qa/native_edit_regression/runs/run001-default2048/b2048_routedtrue_changingfalse.json",
    "qa/native_edit_regression/runs/run001-default2048/b2048_routedtrue_changingtrue-cold-trace.json",
    "qa/native_edit_regression/runs/run001-default2048/b2048_routedtrue_changingtrue.json",
    "qa/native_edit_regression/runs/run001-default2048/build-binding.json",
    "qa/native_edit_regression/runs/run001-default2048/build_and_copy.py",
    "qa/native_edit_regression/runs/run001-default2048/executable-attribution.json",
    "qa/native_edit_regression/runs/run001-default2048/exit-status.txt",
    "qa/native_edit_regression/runs/run001-default2048/host-resources-after.json",
    "qa/native_edit_regression/runs/run001-default2048/host-resources-before.json",
    "qa/native_edit_regression/runs/run001-default2048/native_timing_tests.rs",
    "qa/native_edit_regression/runs/run001-default2048/prepare_snapshot.py",
    "qa/native_edit_regression/runs/run001-default2048/raw.log",
    "qa/native_edit_regression/runs/run001-default2048/run-binding.json",
    "qa/native_edit_regression/runs/run001-default2048/source-snapshot.json",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/build-binding.json",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/build_and_copy.py",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/executable-attribution.json",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/exit-status.txt",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/fresh-restored-surge.json",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/host-resources-after.json",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/host-resources-before.json",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/native_timing_tests.rs",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/prepare_snapshot.py",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/raw.log",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/run-binding.json",
    "qa/native_edit_regression/runs/run002-fresh-restored-state/source-snapshot.json",
))
NATIVE_EDIT_INVENTORY_SHA256 = "2d23f1e0a049feabd04925287a6924bf65d2350baaa518a3c18125c7f0b29f6a"
PARAMETER_QA_ROOT = "qa/parameter_storage_regression/"
PARAMETER_QA_FILES = frozenset((
    "qa/parameter_storage_regression/INVENTORY.json",
    "qa/parameter_storage_regression/README.md",
    "qa/parameter_storage_regression/REPRODUCE.md",
    "qa/parameter_storage_regression/RESULT.md",
    "qa/parameter_storage_regression/SHA256SUMS.json",
    "qa/parameter_storage_regression/SUMMARY.json",
    "qa/parameter_storage_regression/build_and_copy.py",
    "qa/parameter_storage_regression/native_timing_tests.rs",
    "qa/parameter_storage_regression/prepare_snapshot.py",
    "qa/parameter_storage_regression/receipts/build-binding.json",
    "qa/parameter_storage_regression/receipts/build-environment.json",
    "qa/parameter_storage_regression/receipts/debug-resource-watch.jsonl",
    "qa/parameter_storage_regression/receipts/executable-attribution.json",
    "qa/parameter_storage_regression/receipts/git-object-verification.json",
    "qa/parameter_storage_regression/receipts/helper-build.jsonl",
    "qa/parameter_storage_regression/receipts/helper-build.stderr",
    "qa/parameter_storage_regression/receipts/native-build.jsonl",
    "qa/parameter_storage_regression/receipts/native-build.stderr",
    "qa/parameter_storage_regression/receipts/plugin-provenance.json",
    "qa/parameter_storage_regression/receipts/source-snapshot.json",
    "qa/parameter_storage_regression/run_profiles.sh",
    "qa/parameter_storage_regression/runs/run001-default2048/b2048_routedfalse_changingfalse-cold-trace.json",
    "qa/parameter_storage_regression/runs/run001-default2048/b2048_routedfalse_changingfalse.json",
    "qa/parameter_storage_regression/runs/run001-default2048/b2048_routedfalse_changingtrue-cold-trace.json",
    "qa/parameter_storage_regression/runs/run001-default2048/b2048_routedfalse_changingtrue.json",
    "qa/parameter_storage_regression/runs/run001-default2048/b2048_routedtrue_changingfalse-cold-trace.json",
    "qa/parameter_storage_regression/runs/run001-default2048/b2048_routedtrue_changingfalse.json",
    "qa/parameter_storage_regression/runs/run001-default2048/b2048_routedtrue_changingtrue-cold-trace.json",
    "qa/parameter_storage_regression/runs/run001-default2048/b2048_routedtrue_changingtrue.json",
    "qa/parameter_storage_regression/runs/run001-default2048/build-binding.json",
    "qa/parameter_storage_regression/runs/run001-default2048/build_and_copy.py",
    "qa/parameter_storage_regression/runs/run001-default2048/executable-attribution.json",
    "qa/parameter_storage_regression/runs/run001-default2048/exit-status.txt",
    "qa/parameter_storage_regression/runs/run001-default2048/host-resources-after.json",
    "qa/parameter_storage_regression/runs/run001-default2048/host-resources-before.json",
    "qa/parameter_storage_regression/runs/run001-default2048/native_timing_tests.rs",
    "qa/parameter_storage_regression/runs/run001-default2048/prepare_snapshot.py",
    "qa/parameter_storage_regression/runs/run001-default2048/raw.log",
    "qa/parameter_storage_regression/runs/run001-default2048/run-binding.json",
    "qa/parameter_storage_regression/runs/run001-default2048/source-snapshot.json",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/build-binding.json",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/build_and_copy.py",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/executable-attribution.json",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/exit-status.txt",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/fresh-restored-surge.json",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/host-resources-after.json",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/host-resources-before.json",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/native_timing_tests.rs",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/prepare_snapshot.py",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/raw.log",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/run-binding.json",
    "qa/parameter_storage_regression/runs/run002-fresh-restored-state/source-snapshot.json",
))
PARAMETER_INVENTORY_SHA256 = "ccdf6dad6dffa54a6f986a9ab607d6f8f27a7b29626eaac50232accffa8d9a14"
PARAMETER_COST_QA_ROOT = "qa/parameter_storage_cost/"
PARAMETER_COST_QA_FILES = frozenset((
    "qa/parameter_storage_cost/README.md",
    "qa/parameter_storage_cost/SHA256SUMS.txt",
    "qa/parameter_storage_cost/archive-receipt.json",
    "qa/parameter_storage_cost/final-empty-summary.csv",
    "qa/parameter_storage_cost/final-review-summary.md",
    "qa/parameter_storage_cost/final-summary.csv",
    "qa/parameter_storage_cost/native-parameter-storage-receipt.json",
    "qa/parameter_storage_cost/raw-cost-inventory.json",
    "qa/parameter_storage_cost/validation-receipt.json",
))
PARAMETER_COST_INVENTORY_SHA256 = "da353fbbc050cd18f9855934c73b9969f411a71cfe3249168b827663af285436"
# Explicitly reviewed documentation that may land on an independent branch.
OPTIONAL_SOURCE_FILES = frozenset((
    "docs/PROJECT_MEDIA.md", "docs/OFFLINE_EXPORT_WORKFLOW.md", "docs/AUDIO_SPLIT_FIDELITY.md",
    "docs/WAV_EXPORT_OPTIONS.md", "docs/MIXER_METERING.md", "docs/LOCAL_SAMPLE_BROWSER.md",
    "docs/HEADLESS_UI_QA.md", "docs/NATIVE_VST3_EDITORS.md", "docs/FL_INSPIRED_NATIVE_THEME.md", "docs/MULTIWINDOW_WORKSPACE.md",
    "docs/COMPACT_WORKSPACE.md",
    "docs/PIANO_KEYBOARD_EDITING.md", "docs/PIANO_MOUSE_WORKFLOW.md",
    "docs/PIANO_NOTE_EXPRESSION.md", "docs/PIANO_RANGES_AND_SNAP.md",
    "docs/VST3_SCANNING.md", "docs/REAL_VST3_VALIDATION.md",
    "docs/PLUGIN_MIDI_ROUTING.md", "docs/PLUGIN_MIDI_ROUTE_VALIDATION.md",
    "docs/PLUGIN_PROCESSOR_DOMAINS.md", "docs/PLUGIN_STATE_RESTORE_LIMITS.md", "docs/PLUGIN_TIMING.md",
    "docs/LINUX_VST3_EDITORS.md", "docs/LINUX_VST3_EDITOR_VALIDATION.md", "docs/METRONOME.md",
)) | REAL_VST3_QA_FILES | MIDI_ROUTE_QA_FILES | TIMING_DEBUG_QA_FILES | TIMING_COMBINED_QA_FILES | NATIVE_EDIT_QA_FILES | PARAMETER_QA_FILES | PARAMETER_COST_QA_FILES
GENERATED_FILES = ("START_HERE_PREVIEW.txt", "BUILD_PROVENANCE.json", "DEPENDENCIES.json")
PAYLOAD_FILES = frozenset(BINARIES + SOURCE_FILES + GENERATED_FILES)
PACKAGE_FILES = PAYLOAD_FILES | {"SHA256SUMS.txt"}
REGISTRY_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
VENDOR_PATH = "vendor/vst3-host-0.9.0"
VENDOR_ARCHIVE_URL = "https://static.crates.io/crates/vst3-host/vst3-host-0.9.0.crate"
VENDOR_ARCHIVE_SHA256 = "6ec579d54bd13b83c60c1fd8bb756cf234e36ccbfb4833ff756b417e64db7fea"
VENDOR_COMMIT = "ed054908cfe057694d8cf037d0c39dfb5eb4c2ca"
# These reviewed provenance bytes are deliberately pinned, not a hash of the
# modified dependency's entire source tree. A changed bundle needs fresh review.
VENDOR_FILE_HASHES = {
    f"{VENDOR_PATH}/LICENSE": "a65a537295910b776a8b2edb2e7410c3b0e975ca6388994e032c4d1842b4952d",
    f"{VENDOR_PATH}/CITRUS_PATCHES.md": "b36248cb17dc94332de5ac4ac8d3f0c3202dee5fd4399df5c1410a3ac0c37174",
    f"{VENDOR_PATH}/CITRUS.patch": "e98eaca9106473971a45ae89b6a443a1624f675f014ee463f1dd0bbc03df5086",
    f"{VENDOR_PATH}/.cargo_vcs_info.json": "737b52ce29e201e3cc14bab20bc449c6e4a3238c78320391a14d8bc1cf8a9657",
}
VENDOR_FILES = frozenset(VENDOR_FILE_HASHES)
INVENTORY_SCOPE = "Cargo target-resolved graph; may include build/proc-macro dependencies"
LICENSE_REVIEW = "Manifest expressions only; not a legal compliance attestation"
MAX_FILE_BYTES = 256 * 1024 * 1024
# These are Windows 10/11 OS components, never copied from a developer PATH.
# Unknown imports must be investigated; do not 'fix' a failure by collecting DLLs.
SYSTEM_DLLS = frozenset("""
advapi32.dll avrt.dll bcrypt.dll bcryptprimitives.dll cfgmgr32.dll combase.dll comctl32.dll comdlg32.dll
crypt32.dll cryptbase.dll d2d1.dll d3d11.dll d3d12.dll d3dcompiler_47.dll
dcomp.dll dbghelp.dll dnsapi.dll dsound.dll dwmapi.dll dwrite.dll dxgi.dll
gdi32.dll hid.dll imm32.dll iphlpapi.dll kernel32.dll kernelbase.dll
mmdevapi.dll mpr.dll msimg32.dll msvcrt.dll ncrypt.dll netapi32.dll ntdll.dll
ole32.dll oleaut32.dll opengl32.dll powrprof.dll propsys.dll rpcrt4.dll
runtimeobject.dll secur32.dll setupapi.dll shell32.dll shlwapi.dll
synchronization.dll ucrtbase.dll uiautomationcore.dll user32.dll userenv.dll usp10.dll uxtheme.dll
version.dll windowscodecs.dll winhttp.dll wininet.dll winmm.dll winspool.drv
wintrust.dll ws2_32.dll wtsapi32.dll
""".split())
API_SET = re.compile(r"(?:api|ext)-ms-win-[a-z0-9-]+-l\d+-\d+-\d+\.dll\Z")


class PackageError(ValueError):
    """An input or package violates the preview contract."""


def require(condition, message):
    if not condition:
        raise PackageError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode()


def read_input(path):
    require(path.is_file() and not path.is_symlink(), f"Missing/non-regular input: {path}")
    require(0 < path.stat().st_size <= MAX_FILE_BYTES, f"Empty or oversized input: {path}")
    return path.read_bytes()


def inspect_pe(data):
    """Bounds-checked PE32+ headers plus normal and RVA-based delay imports.

    Specification: https://learn.microsoft.com/en-us/windows/win32/debug/pe-format
    This is a packaging audit, not an execution sandbox or malware scanner.
    """
    def unpack(fmt, offset):
        size = struct.calcsize(fmt)
        require(0 <= offset <= len(data) - size, "Truncated PE structure")
        return struct.unpack_from(fmt, data, offset)

    require(data[:2] == b"MZ", "Not a DOS/PE image")
    pe, = unpack("<I", 0x3C)
    require(data[pe:pe + 4] == b"PE\0\0", "Missing PE signature")
    machine, sections = unpack("<HH", pe + 4)
    optional_size, characteristics = unpack("<HH", pe + 20)
    require(machine == 0x8664, "Expected AMD64 PE machine")
    require(characteristics & 0x0002 and not characteristics & 0x2000,
            "Expected executable PE, not a DLL")
    require(0 < sections <= 96, "Invalid PE section count")
    opt = pe + 24
    require(optional_size >= 224 and opt + optional_size <= len(data), "Short PE optional header")
    require(unpack("<H", opt)[0] == 0x20B, "Expected PE32+ optional header")
    require(unpack("<H", opt + 68)[0] == 2, "Expected Windows GUI subsystem")
    headers_size, = unpack("<I", opt + 60)
    require(0 < headers_size <= len(data), "Invalid PE headers size")
    directories, = unpack("<I", opt + 108)
    require(14 <= directories <= (optional_size - 112) // 8, "Missing/invalid PE data directories")
    ranges = []
    for index in range(sections):
        entry = opt + optional_size + index * 40
        virtual_size, rva, raw_size, raw = unpack("<IIII", entry + 8)
        require(raw <= len(data) and raw_size <= len(data) - raw, "Truncated PE section")
        ranges.append((rva, max(virtual_size, raw_size), raw, raw_size))

    def offset(rva, size=1):
        if rva < headers_size:
            require(rva + size <= headers_size, "PE RVA crosses headers")
            return rva
        matches = [(start, raw, raw_size) for start, span, raw, raw_size in ranges
                   if start <= rva < start + span]
        require(len(matches) == 1, "Unmapped/ambiguous PE RVA")
        start, raw, raw_size = matches[0]
        require(rva - start + size <= raw_size, "PE RVA outside raw section")
        return raw + rva - start

    def dll_name(rva):
        start = offset(rva)
        end = data.find(b"\0", start, min(len(data), start + 260))
        require(end > start, "Invalid/unterminated DLL name")
        offset(rva, end - start + 1)
        try:
            name = data[start:end].decode("ascii").lower()
        except UnicodeDecodeError as error:
            raise PackageError("Non-ASCII DLL name") from error
        require(re.fullmatch(r"[a-z0-9_.-]+", name), "Invalid DLL name/path")
        return name

    def imports(directory, stride, name_index):
        rva, size = unpack("<II", opt + 112 + directory * 8)
        if rva == 0 and size == 0:
            return []
        require(rva and stride <= size <= 1024 * 1024, "Invalid PE import directory")
        result = set()
        for index in range(size // stride):
            record = unpack("<" + "I" * (stride // 4), offset(rva + index * stride, stride))
            if not any(record):
                return sorted(result)
            if directory == 13:
                require(record[0] == 1, "Unsupported non-RVA delay import")
            require(record[name_index] != 0, "Missing imported DLL name")
            result.add(dll_name(record[name_index]))
        raise PackageError("Unterminated PE import directory")

    direct = imports(1, 20, 3)
    require(direct, "Expected nonempty Windows OS import table")
    delayed = imports(13, 32, 1)
    unapproved = sorted(name for name in set(direct + delayed)
                        if name not in SYSTEM_DLLS and not API_SET.fullmatch(name))
    require(not unapproved,
            f"Unapproved runtime imports: {', '.join(unapproved)}; "
            "this PREVIEW requires reviewed Windows OS imports and static CRT")
    return {"machine": "AMD64", "subsystem": "Windows GUI", "imports": direct,
            "delay_imports": delayed}


def repository_file(repo, relative):
    """Anchor approved inputs before resolving; reject symlink/reparse components.

    FILE_ATTRIBUTE_REPARSE_POINT also catches Windows junctions on Python 3.11,
    where pathlib has no is_junction(). Resolving the expected path first would
    accidentally bless a vendor directory redirected outside the checkout.
    """
    root = repo.resolve(strict=True)
    path = root
    for part in PurePosixPath(relative).parts:
        require(part not in ("", ".", "..") and "/" not in part and "\\" not in part,
                "Invalid repository-relative input")
        path = path / part
        try:
            status = path.lstat()
        except OSError as error:
            raise PackageError("Missing/unreadable repository input: " + relative) from error
        require(not stat.S_ISLNK(status.st_mode) and
                not getattr(status, "st_file_attributes", 0) & 0x400,
                "Symlink/junction repository input")
    require(path.resolve(strict=True) == path and path.is_relative_to(root),
            "Repository input escapes approved root")
    return path


def vendor_provenance():
    return {"path": VENDOR_PATH, "upstream_archive_url": VENDOR_ARCHIVE_URL,
            "upstream_archive_sha256": VENDOR_ARCHIVE_SHA256,
            "upstream_commit": VENDOR_COMMIT, "files": dict(VENDOR_FILE_HASHES)}


def validate_vendor_bundle(payload):
    require(VENDOR_FILES <= set(payload), "Incomplete vendor provenance bundle")
    for path, checksum in VENDOR_FILE_HASHES.items():
        require(digest(payload[path]) == checksum, "Unreviewed vendor provenance bytes: " + path)
    require(json.loads(payload[f"{VENDOR_PATH}/.cargo_vcs_info.json"]) ==
            {"git": {"sha1": VENDOR_COMMIT}, "path_in_vcs": "vst3-host"},
            "Wrong upstream VCS identity")


def validate_inventory(inventory, root_version, payload=None):
    """Check the same bounded public schema at creation and before extraction."""
    require(isinstance(inventory, dict) and set(inventory) ==
            {"target", "scope", "license_review", "packages"}, "Unreviewed inventory fields")
    require(inventory["target"] == TARGET and inventory["scope"] == INVENTORY_SCOPE and
            inventory["license_review"] == LICENSE_REVIEW, "Wrong inventory identity")
    packages = inventory["packages"]
    require(isinstance(packages, list) and 0 < len(packages) <= 10000, "Missing/oversized dependency inventory")
    seen = set()
    roots = 0
    vendors = 0
    fields = {"name", "version", "source", "checksum", "license", "features"}
    for package in packages:
        require(isinstance(package, dict) and fields <= set(package) <= fields | {"vendor_provenance"},
                "Unreviewed dependency fields")
        name, version = package["name"], package["version"]
        require(isinstance(name, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,128}", name), "Invalid dependency name")
        require(isinstance(version, str) and len(version) <= 128 and re.fullmatch(
                r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?(?:\+[A-Za-z0-9.-]+)?", version),
                "Invalid dependency version")
        source, checksum = package["source"], package["checksum"]
        require(source is None or source == REGISTRY_SOURCE, "Unreviewed dependency source")
        identity = (name, version, source)
        require(identity not in seen, "Duplicate dependency identity")
        seen.add(identity)
        license_expression = package["license"]
        require(license_expression is None or (isinstance(license_expression, str) and
                len(license_expression) <= 512 and ":/" not in license_expression and
                re.fullmatch(r"[A-Za-z0-9(][A-Za-z0-9_.+(): /-]*", license_expression)), "Invalid manifest license expression")
        features = package["features"]
        require(isinstance(features, list) and len(features) <= 10000 and
                all(isinstance(f, str) and re.fullmatch(r"[A-Za-z0-9_+.-]{1,128}", f) for f in features),
                "Invalid dependency features")
        require(features == sorted(set(features)), "Noncanonical dependency features")
        if source is not None:
            require(isinstance(checksum, str) and re.fullmatch(r"[0-9a-f]{64}", checksum),
                    "Missing locked registry checksum")
            require("vendor_provenance" not in package, "Registry package has vendor provenance")
        else:
            require(checksum is None, "Local package must not have a registry checksum")
            if name == "citrus-studio" and version == root_version:
                roots += 1
                require("vendor_provenance" not in package, "Root has vendor provenance")
            else:
                require((name, version, license_expression) == ("vst3-host", "0.9.0", "MIT"),
                        "Unexpected local/path dependency")
                require(package.get("vendor_provenance") == vendor_provenance(),
                        "Unreviewed vendor provenance")
                vendors += 1
    require(roots == 1 and vendors <= 1, "Missing/duplicate root or vendor dependency")
    require(packages == sorted(packages, key=lambda p: (p["name"], p["version"])),
            "Noncanonical dependency order")
    if payload is not None:
        present = set(payload) & VENDOR_FILES
        require(present == (VENDOR_FILES if vendors else set()), "Vendor dependency/bundle mismatch")
        if vendors:
            validate_vendor_bundle(payload)
    return bool(vendors)


def dependency_inventory(metadata, lock, repo):
    root_manifest = repository_file(repo, "Cargo.toml")
    manifest = tomllib.loads(read_input(root_manifest).decode())
    require(metadata.get("workspace_root") == str(root_manifest.parent), "Cargo workspace root mismatch")
    root_identity = (manifest["package"]["name"], manifest["package"]["version"])
    require(root_identity[0] == "citrus-studio", "Unexpected root manifest identity")
    patch = manifest.get("patch", {})
    require(not patch or patch == {"crates-io": {"vst3-host": {"path": VENDOR_PATH}}},
            "Unreviewed root patch override")
    require(not manifest.get("replace"), "Unreviewed root replacement override")
    nodes = metadata.get("resolve", {}).get("nodes", [])
    wanted = {node["id"]: node["features"] for node in nodes}
    require(wanted and len(wanted) == len(nodes), "Missing/duplicate Cargo graph nodes")
    checksums = {}
    for entry in lock["package"]:
        key = (entry["name"], entry["version"], entry.get("source"))
        require(key not in checksums, "Duplicate Cargo.lock package")
        checksums[key] = entry
    result = []
    found_ids = set()
    root_id = None
    vendors = 0
    for package in metadata["packages"]:
        if package["id"] not in wanted:
            continue
        require(package["id"] not in found_ids, "Duplicate Cargo metadata package")
        found_ids.add(package["id"])
        source = package.get("source")
        require(source is None or source == REGISTRY_SOURCE, "Unreviewed Cargo source")
        key = (package["name"], package["version"], source)
        require(key in checksums, "Dependency absent from Cargo.lock")
        entry = checksums[key]
        record = {"name": package["name"], "version": package["version"],
                  "source": source, "checksum": entry.get("checksum"), "license": package.get("license"),
                  "features": sorted(wanted[package["id"]])}
        if source is None:
            require("source" not in entry and "checksum" not in entry,
                    "Local lock entry contains source/checksum")
            manifest_path = package.get("manifest_path")
            require(isinstance(manifest_path, str) and Path(manifest_path).is_absolute(),
                    "Missing local manifest location")
            if key[:2] == root_identity:
                require(Path(manifest_path) == root_manifest, "Root manifest location mismatch")
                require(root_id is None, "Duplicate root package")
                root_id = package["id"]
            else:
                require(key[:2] == ("vst3-host", "0.9.0") and package.get("license") == "MIT",
                        "Unexpected local/path dependency")
                require(bool(patch), "Missing reviewed root patch override")
                approved_manifest = repository_file(repo, f"{VENDOR_PATH}/Cargo.toml")
                require(Path(manifest_path) == approved_manifest, "Vendor manifest location mismatch")
                vendored = tomllib.loads(read_input(approved_manifest).decode())["package"]
                require((vendored.get("name"), vendored.get("version"), vendored.get("license")) ==
                        ("vst3-host", "0.9.0", "MIT"), "Wrong vendor manifest identity/license")
                bundle = {path: read_input(repository_file(repo, path)) for path in VENDOR_FILES}
                validate_vendor_bundle(bundle)
                record["vendor_provenance"] = vendor_provenance()
                vendors += 1
        result.append(record)
    require(found_ids == set(wanted), "Incomplete Cargo metadata package set")
    require(root_id is not None and metadata.get("resolve", {}).get("root") == root_id,
            "Cargo graph root does not match root manifest")
    require(bool(patch) == bool(vendors), "Root patch/resolved dependency mismatch")
    inventory = {"target": TARGET, "scope": INVENTORY_SCOPE, "license_review": LICENSE_REVIEW,
                 "packages": sorted(result, key=lambda p: (p["name"], p["version"]))}
    validate_inventory(inventory, root_identity[1])
    return inventory


def validate_build_info(info):
    fields = {"target", "toolchain", "rustflags", "profile", "features", "rustc", "cargo",
              "source_commit", "source_epoch", "cargo_lock_sha256", "msvc_linker_version",
              "msvc_linker_sha256", "msvc_tools_version", "windows_sdk_version", "runner_image",
              "runner_image_version", "python", "schema", "channel", "version", "pe_audit",
              "source_documents", "acceptance"}
    require(isinstance(info, dict) and set(info) <= fields, "Unreviewed build provenance fields")
    require(info.get("target") == TARGET and info.get("toolchain") == TOOLCHAIN,
            "Wrong preview toolchain/target")
    require(info.get("rustflags") == RUSTFLAGS, "Expected static CRT RUSTFLAGS")
    require(info.get("profile") == "release" and info.get("features") == ["vst2", "vst3"],
            "Expected optimized all-features build")
    require("release: 1.99.0\n" in info.get("rustc", "") + "\n" and
            f"host: {TARGET}\n" in info.get("rustc", "") + "\n", "Wrong rustc version/host")
    require(info.get("cargo", "").startswith("cargo 1.99.0 "), "Wrong Cargo version")
    require(re.fullmatch(r"[0-9a-f]{40}", info.get("source_commit", "")), "Invalid source commit")
    require(isinstance(info.get("source_epoch"), int) and info["source_epoch"] > 0, "Invalid source time")
    require(re.fullmatch(r"[0-9a-f]{64}", info.get("cargo_lock_sha256", "")), "Invalid lock hash")
    require(bool(info.get("msvc_linker_version")), "Missing MSVC linker version")


def package_name(version, commit):
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", version), "Invalid package version")
    require(re.fullmatch(r"[0-9a-f]{40}", commit), "Invalid source commit")
    return f"Citrus-Studio-{version}-Windows-x64-MSVC-PREVIEW-{commit[:12]}"


def check_document_links(payload):
    for path, data in payload.items():
        if not path.endswith(".md"):
            continue
        for link in re.findall(r"\]\(([^\s)]+)", data.decode("utf-8")):
            url = urlsplit(link.strip("<>"))
            if url.scheme or url.netloc or not url.path:
                continue
            destination = posixpath.normpath(posixpath.join(posixpath.dirname(path), unquote(url.path)))
            require(destination in payload, f"Broken package document link: {path} -> {link}")


def validate_real_vst3_qa(payload):
    """Verify the complete explicitly allowed source-only measurement bundle."""
    present = set(payload) & REAL_VST3_QA_FILES
    if not present:
        return
    require(present == REAL_VST3_QA_FILES, "Incomplete real VST3 QA source bundle")
    inventory = REAL_VST3_QA_ROOT + "CONTENTS-SHA256.txt"
    expected = "".join(
        f"{digest(payload[path])}  {path.removeprefix(REAL_VST3_QA_ROOT)}\n"
        for path in sorted(REAL_VST3_QA_FILES - {inventory})
    ).encode("utf-8")
    require(payload[inventory] == expected, "Real VST3 QA inventory/hash mismatch")


def validate_midi_route_qa(payload):
    """Keep the separately attributed production-routing receipt set complete."""
    present = set(payload) & MIDI_ROUTE_QA_FILES
    if not present:
        return
    require(present == MIDI_ROUTE_QA_FILES, "Incomplete MIDI route QA source bundle")
    inventory = MIDI_ROUTE_QA_ROOT + "CONTENTS-SHA256.txt"
    expected = "".join(
        f"{digest(payload[path])}  {path.removeprefix(MIDI_ROUTE_QA_ROOT)}\n"
        for path in sorted(MIDI_ROUTE_QA_FILES - {inventory})
    ).encode("utf-8")
    require(payload[inventory] == expected, "MIDI route QA inventory/hash mismatch")


def validate_timing_debug_qa(payload):
    """Keep historical debug failures and source attribution byte-exact, never execute plugins."""
    present = set(payload) & TIMING_DEBUG_QA_FILES
    unexpected = {name for name in payload if name.startswith(TIMING_DEBUG_QA_ROOT)} - TIMING_DEBUG_QA_FILES
    require(not unexpected, "Unexpected timing debug QA path")
    if not present:
        return
    require(present == TIMING_DEBUG_QA_FILES, "Incomplete timing debug QA source bundle")
    inventory = TIMING_DEBUG_QA_ROOT + "PUBLICATION_SHA256SUMS"
    require(digest(payload[inventory]) == TIMING_DEBUG_INVENTORY_SHA256,
            "Timing debug QA pinned inventory/hash mismatch")
    expected = "".join(
        f"{digest(payload[path])}  {path.removeprefix(TIMING_DEBUG_QA_ROOT)}\n"
        for path in sorted(TIMING_DEBUG_QA_FILES - {inventory})
    ).encode("utf-8")
    require(payload[inventory] == expected, "Timing debug QA inventory/hash mismatch")


def validate_timing_combined_qa(payload):
    """Validate the distinct exact combined-helper summary without relabeling historical runs."""
    present = set(payload) & TIMING_COMBINED_QA_FILES
    unexpected = {name for name in payload if name.startswith(TIMING_COMBINED_QA_ROOT)} - TIMING_COMBINED_QA_FILES
    require(not unexpected, "Unexpected timing combined QA path")
    if not present:
        return
    require(present == TIMING_COMBINED_QA_FILES, "Incomplete timing combined QA source bundle")
    inventory = TIMING_COMBINED_QA_ROOT + "CONTENTS-SHA256.txt"
    require(digest(payload[inventory]) == TIMING_COMBINED_INVENTORY_SHA256,
            "Timing combined QA pinned inventory/hash mismatch")
    expected = "".join(
        f"{digest(payload[path])}  {path.removeprefix(TIMING_COMBINED_QA_ROOT)}\n"
        for path in sorted(TIMING_COMBINED_QA_FILES - {inventory})
    ).encode("utf-8")
    require(payload[inventory] == expected, "Timing combined QA inventory/hash mismatch")


def validate_native_edit_qa(payload):
    """Pin the changed-helper correctness receipt separately from historical performance."""
    present = set(payload) & NATIVE_EDIT_QA_FILES
    unexpected = {name for name in payload if name.startswith(NATIVE_EDIT_QA_ROOT)} - NATIVE_EDIT_QA_FILES
    require(not unexpected, "Unexpected native edit QA path")
    if not present:
        return
    require(present == NATIVE_EDIT_QA_FILES, "Incomplete native edit QA source bundle")
    inventory = NATIVE_EDIT_QA_ROOT + "SHA256SUMS.json"
    require(digest(payload[inventory]) == NATIVE_EDIT_INVENTORY_SHA256,
            "Native edit QA pinned inventory/hash mismatch")
    expected = {
        path.removeprefix(NATIVE_EDIT_QA_ROOT): digest(payload[path])
        for path in NATIVE_EDIT_QA_FILES - {inventory}
    }
    require(json.loads(payload[inventory]) == expected, "Native edit QA inventory/hash mismatch")


def validate_parameter_qa(payload):
    """Pin exact parameter-helper correctness evidence, preserving historical runs."""
    present = set(payload) & PARAMETER_QA_FILES
    unexpected = {name for name in payload if name.startswith(PARAMETER_QA_ROOT)} - PARAMETER_QA_FILES
    require(not unexpected, "Unexpected parameter QA path")
    if not present:
        return
    require(present == PARAMETER_QA_FILES, "Incomplete parameter QA source bundle")
    inventory = PARAMETER_QA_ROOT + "SHA256SUMS.json"
    require(digest(payload[inventory]) == PARAMETER_INVENTORY_SHA256,
            "Parameter QA pinned inventory/hash mismatch")
    expected = {path.removeprefix(PARAMETER_QA_ROOT): digest(payload[path])
                for path in PARAMETER_QA_FILES - {inventory}}
    require(json.loads(payload[inventory]) == expected, "Parameter QA inventory/hash mismatch")


def validate_parameter_cost_qa(payload):
    """Preserve finite-storage cost tradeoffs and the immutable full-appendix identity."""
    present = set(payload) & PARAMETER_COST_QA_FILES
    unexpected = {name for name in payload if name.startswith(PARAMETER_COST_QA_ROOT)} - PARAMETER_COST_QA_FILES
    require(not unexpected, "Unexpected parameter cost QA path")
    if not present:
        return
    require(present == PARAMETER_COST_QA_FILES, "Incomplete parameter cost QA source bundle")
    inventory = PARAMETER_COST_QA_ROOT + "SHA256SUMS.txt"
    require(digest(payload[inventory]) == PARAMETER_COST_INVENTORY_SHA256,
            "Parameter cost QA pinned inventory/hash mismatch")
    expected = "".join(
        f"{digest(payload[path])}  {path.removeprefix(PARAMETER_COST_QA_ROOT)}\n"
        for path in sorted(PARAMETER_COST_QA_FILES - {inventory})
    ).encode("utf-8")
    require(payload[inventory] == expected, "Parameter cost QA inventory/hash mismatch")


def create_package(repo, binaries, metadata, build_info, output):
    validate_build_info(build_info)
    manifest = tomllib.loads(read_input(repo / "Cargo.toml").decode())
    version = manifest["package"]["version"]
    lock_bytes = read_input(repo / "Cargo.lock")
    require(digest(lock_bytes) == build_info["cargo_lock_sha256"], "Cargo.lock changed since build")
    name = package_name(version, build_info["source_commit"])
    archive_path = output / (name + ".zip")
    checksum_path = output / (name + ".zip.sha256")
    require(not archive_path.exists() and not checksum_path.exists(), "Output already exists")
    inventory = dependency_inventory(metadata, tomllib.loads(lock_bytes.decode()), repo)
    sources = set(SOURCE_FILES) | {p for p in OPTIONAL_SOURCE_FILES if (repo / p).exists()}
    if any("vendor_provenance" in p for p in inventory["packages"]):
        sources |= VENDOR_FILES
    payload = {path: read_input(repository_file(repo, path)) for path in sources}
    validate_inventory(inventory, version, payload)
    audits = {}
    audit_errors = []
    for binary in BINARIES:
        try:
            payload[binary] = read_input(binaries / binary)
            audits[binary] = inspect_pe(payload[binary])
        except PackageError as error:
            audit_errors.append(f"{binary}: {error}")
    require(not audit_errors, "Binary audit failed: " + "; ".join(audit_errors))
    payload["DEPENDENCIES.json"] = json_bytes(inventory)
    payload["START_HERE_PREVIEW.txt"] = (
        f"Citrus Studio {version} — UNSIGNED MSVC DEVELOPER PREVIEW\n\n"
        "Extract the entire ZIP, then launch citrus-studio.exe. Keep vst3-host-helper.exe beside it.\n"
        "Windows 10/11 x64 is the intended target; clean-machine compatibility is not yet certified.\n"
        "No Python, Rust, Visual Studio, or separately installed VC runtime is intended at runtime.\n"
        "This optimized MSVC/static-CRT build is NOT the pinned gnullvm official release.\n"
        "No libunwind.dll is included or required by the audited import tables.\n"
        "Unsigned: Windows may show a reputation warning. Verify the source and checksum first;\n"
        "do not disable security controls. A checksum is integrity data, not publisher authentication.\n\n"
        "Use copies of projects. GUI, audio/MIDI hardware, plugins, clean Windows installation,\n"
        "signing, malware scanning and legal/license release review remain separate acceptance gates.\n"
        "The import audit cannot establish dynamically loaded plugin/graphics dependencies.\n"
        "THIRD_PARTY_NOTICES.md preserves the historical gnullvm notices; its libunwind/MinGW\n"
        "distribution claims do not describe this MSVC preview. See docs/WINDOWS_PREVIEW.md.\n"
        "When DEPENDENCIES.json records the reviewed local vst3-host 0.9.0 extension, its\n"
        "original MIT license and four-file provenance bundle ship under vendor/vst3-host-0.9.0/.\n"
        "The upstream archive hash identifies the original crate, not the modified code.\n"
    ).encode("utf-8")
    provenance = {**build_info, "schema": 1, "channel": "unsigned-msvc-developer-preview",
                  "version": version, "pe_audit": audits, "source_documents": sorted(sources),
                  "acceptance": "Package integrity only; no commercial or official-release attestation"}
    payload["BUILD_PROVENANCE.json"] = json_bytes(provenance)
    require(set(payload) == PAYLOAD_FILES | (sources & (OPTIONAL_SOURCE_FILES | VENDOR_FILES)), "Internal payload whitelist mismatch")
    check_document_links(payload)
    validate_real_vst3_qa(payload)
    validate_midi_route_qa(payload)
    validate_timing_debug_qa(payload)
    validate_timing_combined_qa(payload)
    validate_native_edit_qa(payload)
    validate_parameter_qa(payload)
    validate_parameter_cost_qa(payload)
    payload["SHA256SUMS.txt"] = "".join(f"{digest(payload[p])}  {p}\n" for p in sorted(payload)).encode("ascii")
    output.mkdir(parents=True, exist_ok=True)
    epoch = max(315532800, min(build_info["source_epoch"], 4354819198))
    timestamp = datetime.fromtimestamp(epoch, timezone.utc).timetuple()[:6]
    archive_created = False
    try:
        with zipfile.ZipFile(archive_path, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            archive_created = True
            for path in sorted(payload):
                entry = zipfile.ZipInfo(f"{name}/{path}", timestamp)
                entry.create_system = 3
                entry.external_attr = (stat.S_IFREG | 0o644) << 16
                entry.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(entry, payload[path], compresslevel=9)
        verify_package(archive_path)
        with checksum_path.open("x", encoding="ascii", newline="\n") as handle:
            handle.write(f"{digest(archive_path.read_bytes())}  {archive_path.name}\n")
    except Exception:
        # Do not leave an apparently successful package after a validation failure.
        if archive_created:
            archive_path.unlink(missing_ok=True)
        raise
    return archive_path


def verify_package(path, extract_to=None):
    require(path.suffix == ".zip", "Expected .zip archive")
    root = path.stem
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        expected = {f"{root}/{p}" for p in PACKAGE_FILES}
        allowed = expected | {f"{root}/{p}" for p in OPTIONAL_SOURCE_FILES | VENDOR_FILES}
        actual = {e.filename for e in entries}
        require(len(entries) == len(actual) and expected <= actual <= allowed,
                "ZIP entries do not match exact whitelist (extra/missing/duplicate/path traversal)")
        payload = {}
        for entry in entries:
            require(not entry.is_dir() and not entry.flag_bits & 1, "Directory/encrypted ZIP entry")
            mode = entry.external_attr >> 16
            require(stat.S_IFMT(mode) in (0, stat.S_IFREG), "Non-regular ZIP entry")
            require(0 < entry.file_size <= MAX_FILE_BYTES, "Empty/oversized ZIP entry")
            payload[entry.filename[len(root) + 1:]] = archive.read(entry)
    info = json.loads(payload["BUILD_PROVENANCE.json"])
    validate_build_info(info)
    require(info.get("schema") == 1 and info.get("channel") == "unsigned-msvc-developer-preview",
            "Incorrect preview provenance")
    require(package_name(info["version"], info["source_commit"]) == root, "ZIP identity does not match provenance")
    expected_hashes = "".join(f"{digest(payload[p])}  {p}\n" for p in sorted(set(payload) - {"SHA256SUMS.txt"})).encode("ascii")
    require(payload["SHA256SUMS.txt"] == expected_hashes, "Package checksum mismatch")
    require(info.get("source_documents") == sorted(set(payload) & (set(SOURCE_FILES) | OPTIONAL_SOURCE_FILES | VENDOR_FILES)),
            "Source document manifest mismatch")
    check_document_links(payload)
    validate_real_vst3_qa(payload)
    validate_midi_route_qa(payload)
    validate_timing_debug_qa(payload)
    validate_timing_combined_qa(payload)
    validate_native_edit_qa(payload)
    validate_parameter_qa(payload)
    validate_parameter_cost_qa(payload)
    for binary in BINARIES:
        require(inspect_pe(payload[binary]) == info["pe_audit"][binary], "PE audit/provenance mismatch")
    inventory = json.loads(payload["DEPENDENCIES.json"])
    require(payload["DEPENDENCIES.json"] == json_bytes(inventory), "Noncanonical dependency inventory")
    validate_inventory(inventory, info["version"], payload)
    if extract_to is not None:
        require(not extract_to.exists(), "Extraction destination already exists")
        extract_to.mkdir(parents=True)
        # All names, regular types, sizes and checksums are checked before writing.
        for relative, data in payload.items():
            destination = extract_to.joinpath(*PurePosixPath(relative).parts)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
    return info


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True, encoding="utf-8").strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("create")
    create.add_argument("--repo", type=Path, default=Path.cwd())
    create.add_argument("--binaries", type=Path, required=True)
    create.add_argument("--metadata", type=Path, required=True)
    create.add_argument("--build-info", type=Path, required=True)
    create.add_argument("--output", type=Path, required=True)
    verify = commands.add_parser("verify")
    verify.add_argument("archive", type=Path)
    verify.add_argument("--extract-to", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "create":
            info = json.loads(read_input(args.build_info))
            require(git(args.repo, "status", "--porcelain", "--untracked-files=normal") == "", "Source checkout must be clean")
            require(git(args.repo, "rev-parse", "HEAD") == info["source_commit"], "Source changed since build")
            require(int(git(args.repo, "show", "-s", "--format=%ct", "HEAD")) == info["source_epoch"], "Source timestamp mismatch")
            output = create_package(args.repo, args.binaries, json.loads(read_input(args.metadata)), info, args.output)
            print(f"Created and validated PREVIEW: {output}")
        else:
            verify_package(args.archive, args.extract_to)
            print(f"PREVIEW integrity PASS: {args.archive}")
    except (PackageError, OSError, KeyError, ValueError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        print(f"PREVIEW packaging FAILED: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
