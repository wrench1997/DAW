#!/usr/bin/env python3
"""Build only this checked-in no-GUI Effect; never load an installed plugin."""

import argparse
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
TARGETS = {
    "x86_64-pc-windows-msvc": ("win32", "x86_64-win", "citrus_vst3_effect_fixture.dll", "CitrusStereoDelayFixture.vst3"),
    "x86_64-unknown-linux-gnu": ("linux", "x86_64-linux", "libcitrus_vst3_effect_fixture.so", "CitrusStereoDelayFixture.so"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, default="x86_64-pc-windows-msvc" if os.name == "nt" else "x86_64-unknown-linux-gnu")
    parser.add_argument("--timeout", type=float, default=180.0, help="Cargo deadline in seconds")
    args = parser.parse_args()
    platform, architecture, binary_name, module_name = TARGETS[args.target]
    if not sys.platform.startswith(platform):
        parser.error("build on the requested platform with its installed Rust toolchain")
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("timeout must be finite and positive")
    command = ["cargo", "build", "--locked", "--jobs", "1", "--release", "--manifest-path", str(ROOT / "Cargo.toml"),
               "--target", args.target, "--target-dir", str(ROOT / "target/build")]
    # No native window, audio device, plugin loading, installer or binary download.
    subprocess.run(command, check=True, timeout=args.timeout)
    binary = ROOT / "target/build" / args.target / "release" / binary_name
    bundle = ROOT / "target/bundles/CitrusStereoDelayFixture.vst3"
    destination = bundle / "Contents" / architecture / module_name
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, destination)
    receipt = {"schema": 1, "source_only": True, "target": args.target, "build_command": command,
               "binary": str(destination.relative_to(bundle)).replace("\\", "/"), "binary_bytes": destination.stat().st_size,
               "class_id": "43545253454658440000000000000001", "category": "Fx|Delay",
               "vendor": "Citrus Studio Test Fixtures", "version": "1.0.0",
               "name": "Citrus Stereo Delay Fixture", "latency_samples": 32, "last_tap_samples": 160}
    (bundle / "source-build.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(f"Built no-GUI source Effect: {bundle}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
