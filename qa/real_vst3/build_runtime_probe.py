#!/usr/bin/env python3
"""Build a portable runtime/scanner probe from captured production source.

This is a reproduction utility, not the measured build script. It never invokes
Cargo or a plugin. Build matching dependencies first in a separate idle target.
Run in a disposable copy of this directory, as described in REPRODUCE.md.
"""

import argparse
import hashlib
import json
import pathlib
import shutil
import subprocess


DEFAULT_REFS = {
    "runtime": "b57076ad990869f4a421cc816d67409b5c69b694",
    "scanner": "1c673eb47ace331b4b05d147dd8d29747670d99e",
}


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, type=pathlib.Path)
    parser.add_argument("--target", required=True, type=pathlib.Path)
    parser.add_argument("--rustc", default=shutil.which("rustc"))
    parser.add_argument("--probe", choices=DEFAULT_REFS, default="runtime")
    parser.add_argument("--ref", help="Exact source revision; default depends on --probe")
    parser.add_argument("--native-library-path", action="append", default=[])
    parser.add_argument(
        "--extern", action="append", default=[], metavar="NAME=RLIB",
        help="Select an exact matching rlib when the target has multiple candidates",
    )
    args = parser.parse_args()
    if not args.rustc:
        parser.error("Set --rustc to the compiler used to build the target cache")
    repo = args.repo.resolve()
    target = args.target.resolve()
    revision = args.ref or DEFAULT_REFS[args.probe]
    commit = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", revision + "^{commit}"], text=True
    ).strip()
    dependencies = ["object", "serde", "walkdir", "rtrb", "vst3_host"]
    if args.probe == "scanner":
        dependencies.append("serde_json")
    selected = {}
    for value in args.extern:
        name, sep, value_path = value.partition("=")
        if not sep or name not in dependencies or name in selected:
            parser.error("--extern requires a unique expected dependency NAME=RLIB")
        path = pathlib.Path(value_path).resolve()
        if not path.is_file() or path.suffix != ".rlib":
            parser.error(f"Missing rlib: {path}")
        selected[name] = path
    deps = target / "debug/deps"
    for name in dependencies:
        if name not in selected:
            candidates = sorted(deps.glob("lib" + name + "-*.rlib"))
            if len(candidates) != 1:
                parser.error(
                    f"Expected one {name} rlib, found {len(candidates)}. Use a clean "
                    f"matching target or --extern {name}=/path/to/exact.rlib"
                )
            selected[name] = candidates[0]
    helper = target / "debug/vst3-host-helper"
    if not helper.is_file():
        parser.error(f"Build the matching production helper first: {helper}")
    root = pathlib.Path(__file__).resolve().parent
    source_dir = root / "production-src"
    source_dir.mkdir(exist_ok=True)
    (root / "bin").mkdir(exist_ok=True)
    (root / "receipts").mkdir(exist_ok=True)
    captured = {}
    for name in ["plugins.rs", "plugin_runtime.rs"]:
        path = source_dir / name
        path.write_bytes(subprocess.check_output(
            ["git", "-C", str(repo), "show", commit + ":src/" + name]
        ))
        captured["src/" + name] = sha256(path)
    probe = root / (args.probe + "_probe.rs")
    executable = root / "bin" / (args.probe + "_probe")
    command = [
        args.rustc, "--edition=2024", "--cfg", 'feature="vst3"', "-A", "dead_code",
        str(probe), "-L", "dependency=" + str(deps), "-o", str(executable),
    ]
    for name in dependencies:
        command += ["--extern", name + "=" + str(selected[name])]
    for path in args.native_library_path:
        command += ["-L", "native=" + str(pathlib.Path(path).resolve())]
    receipt = {
        "commit": commit,
        "probe": args.probe,
        "command": command,
        "source_sha256": captured,
        "probe_source_sha256": sha256(probe),
        "helper_sha256": sha256(helper),
        "rlib_sha256": {name: sha256(path) for name, path in selected.items()},
        "compiler": subprocess.check_output([args.rustc, "--version", "--verbose"], text=True),
        "compile_succeeded": False,
    }
    receipt_path = root / "receipts" / (args.probe + "-reproduction-build.json")
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    subprocess.run(command, cwd=root, check=True)
    shutil.copy2(helper, root / "bin/vst3-host-helper")
    receipt["compile_succeeded"] = True
    receipt["executable_sha256"] = sha256(executable)
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"Built {args.probe} probe from {commit}; no plugin has been executed")
    print(f"Helper SHA256: {receipt['helper_sha256']}")


if __name__ == "__main__":
    main()
