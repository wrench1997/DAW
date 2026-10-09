#!/usr/bin/env python3
"""Build only the checked-in MIT fixture source, never download a plugin binary."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
SOURCE_FILES = ("Cargo.toml", "Cargo.lock", "LICENSE", "src/lib.rs", "src/native_windows.rs")
UPSTREAM_COMMIT = "ed054908cfe057694d8cf037d0c39dfb5eb4c2ca"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", default="x86_64-pc-windows-msvc", choices=(
        "x86_64-pc-windows-msvc", "x86_64-pc-windows-gnullvm", "x86_64-pc-windows-gnu"))
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("build the acceptance bundles on Windows with an already installed Rust target")
    target = ROOT / "target"
    # The lockfile is checked in. Cargo may fetch its normal official registry dependencies;
    # this script does not fetch or install a VST3 binary or any toolchain.
    for variant, name in (("editor", "CitrusEditorFixture"), ("no-editor", "CitrusNoEditorFixture")):
        command = ["cargo", "build", "--locked", "--manifest-path", str(ROOT / "Cargo.toml"),
                   "--release", "--target", args.target, "--target-dir", str(target / "build")]
        if variant == "no-editor":
            command.extend(("--features", "no-editor"))
        subprocess.run(command, check=True)
        binary = target / "build" / args.target / "release" / "citrus_vst3_editor_fixture.dll"
        bundle = target / "bundles" / f"{name}.vst3"
        destination = bundle / "Contents" / "x86_64-win" / f"{name}.vst3"
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(binary, destination)
        receipt = {
            "schema": 1, "variant": variant, "upstream_commit": UPSTREAM_COMMIT,
            "target": args.target, "sources": {name: sha256(ROOT / name) for name in SOURCE_FILES},
            "binary": str(destination.relative_to(bundle)).replace("\\", "/"),
            "binary_sha256": sha256(destination), "build_command": command,
        }
        (bundle / "source-build.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
        print(f"Built source-only fixture: {bundle}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
