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
# Explicitly reviewed documentation that may land on an independent branch.
OPTIONAL_SOURCE_FILES = frozenset(("docs/PROJECT_MEDIA.md",))
GENERATED_FILES = ("START_HERE_PREVIEW.txt", "BUILD_PROVENANCE.json", "DEPENDENCIES.json")
PAYLOAD_FILES = frozenset(BINARIES + SOURCE_FILES + GENERATED_FILES)
PACKAGE_FILES = PAYLOAD_FILES | {"SHA256SUMS.txt"}
MAX_FILE_BYTES = 256 * 1024 * 1024
# These are Windows 10/11 OS components, never copied from a developer PATH.
# Unknown imports must be investigated; do not 'fix' a failure by collecting DLLs.
SYSTEM_DLLS = frozenset("""
advapi32.dll avrt.dll bcrypt.dll cfgmgr32.dll comctl32.dll comdlg32.dll
crypt32.dll cryptbase.dll d2d1.dll d3d11.dll d3d12.dll d3dcompiler_47.dll
dcomp.dll dbghelp.dll dnsapi.dll dsound.dll dwmapi.dll dwrite.dll dxgi.dll
gdi32.dll hid.dll imm32.dll iphlpapi.dll kernel32.dll kernelbase.dll
mmdevapi.dll mpr.dll msimg32.dll msvcrt.dll ncrypt.dll netapi32.dll ntdll.dll
ole32.dll oleaut32.dll opengl32.dll powrprof.dll propsys.dll rpcrt4.dll
runtimeobject.dll secur32.dll setupapi.dll shell32.dll shlwapi.dll
synchronization.dll ucrtbase.dll user32.dll userenv.dll usp10.dll uxtheme.dll
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
        require(name in SYSTEM_DLLS or API_SET.fullmatch(name),
                f"Non-Windows runtime import {name}; this PREVIEW must use static CRT")
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
    return {"machine": "AMD64", "subsystem": "Windows GUI", "imports": direct,
            "delay_imports": imports(13, 32, 1)}


def dependency_inventory(metadata, lock):
    nodes = metadata.get("resolve", {}).get("nodes", [])
    wanted = {node["id"] for node in nodes}
    require(wanted, "Cargo metadata has no resolved graph")
    checksums = {(p["name"], p["version"], p.get("source")): p.get("checksum")
                 for p in lock["package"]}
    result = []
    for package in metadata["packages"]:
        if package["id"] not in wanted:
            continue
        source = package.get("source")
        # Reject new unreviewed dependency source types rather than leaking paths,
        # credentials, or accidentally attesting an arbitrary registry or git URL.
        require(source in (None, "registry+https://github.com/rust-lang/crates.io-index"),
                f"Unreviewed Cargo source for {package['name']}")
        require(source is not None or package["name"] == "citrus-studio",
                "Unexpected local/path dependency")
        key = (package["name"], package["version"], source)
        require(key in checksums, f"Dependency absent from Cargo.lock: {key[:2]}")
        checksum = checksums[key]
        require(source is None or re.fullmatch(r"[0-9a-f]{64}", checksum or ""),
                "Missing locked registry checksum")
        result.append({"name": package["name"], "version": package["version"],
                       "source": source, "checksum": checksum, "license": package.get("license"),
                       "features": sorted(next(n["features"] for n in nodes if n["id"] == package["id"]))})
    require(len(result) == len(wanted), "Incomplete Cargo metadata package set")
    return {"target": TARGET, "scope": "Cargo target-resolved graph; may include build/proc-macro dependencies",
            "license_review": "Manifest expressions only; not a legal compliance attestation",
            "packages": sorted(result, key=lambda p: (p["name"], p["version"]))}


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
    sources = set(SOURCE_FILES) | {p for p in OPTIONAL_SOURCE_FILES if (repo / p).exists()}
    payload = {path: read_input(repo / path) for path in sources}
    audits = {}
    for binary in BINARIES:
        payload[binary] = read_input(binaries / binary)
        audits[binary] = inspect_pe(payload[binary])
    payload["DEPENDENCIES.json"] = json_bytes(dependency_inventory(metadata, tomllib.loads(lock_bytes.decode())))
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
    ).encode("utf-8")
    provenance = {**build_info, "schema": 1, "channel": "unsigned-msvc-developer-preview",
                  "version": version, "pe_audit": audits, "source_documents": sorted(sources),
                  "acceptance": "Package integrity only; no commercial or official-release attestation"}
    payload["BUILD_PROVENANCE.json"] = json_bytes(provenance)
    require(set(payload) == PAYLOAD_FILES | (sources & OPTIONAL_SOURCE_FILES), "Internal payload whitelist mismatch")
    check_document_links(payload)
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
        allowed = expected | {f"{root}/{p}" for p in OPTIONAL_SOURCE_FILES}
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
    require(info.get("source_documents") == sorted(set(payload) & (set(SOURCE_FILES) | OPTIONAL_SOURCE_FILES)),
            "Source document manifest mismatch")
    check_document_links(payload)
    for binary in BINARIES:
        require(inspect_pe(payload[binary]) == info["pe_audit"][binary], "PE audit/provenance mismatch")
    inventory = json.loads(payload["DEPENDENCIES.json"])
    require(inventory.get("target") == TARGET and bool(inventory.get("packages")), "Missing dependency inventory")
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
