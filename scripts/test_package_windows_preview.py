"""Synthetic, plugin/device-free package regressions; no Windows binary executes."""

import copy
import hashlib
import json
from pathlib import Path
import stat
import struct
import tempfile
import unittest
import warnings
import zipfile

import package_windows_preview as pkg


def pe_image(imports=("KERNEL32.dll",), delayed=()):
    data = bytearray(4608)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HH", data, 0x84, 0x8664, 1)
    struct.pack_into("<HH", data, 0x94, 240, 2)
    opt = 0x98
    struct.pack_into("<H", data, opt, 0x20B)
    struct.pack_into("<I", data, opt + 60, 512)
    struct.pack_into("<H", data, opt + 68, 2)
    struct.pack_into("<I", data, opt + 108, 16)
    struct.pack_into("<IIII", data, opt + 240 + 8, 4096, 0x1000, 4096, 512)
    name_offset = 0x600
    for directory, values, stride, table in ((1, imports, 20, 0x200), (13, delayed, 32, 0x400)):
        if not values:
            continue
        struct.pack_into("<II", data, opt + 112 + directory * 8, table + 0xE00, (len(values) + 1) * stride)
        for index, name in enumerate(values):
            record = table + stride * index
            if directory == 13:
                struct.pack_into("<II", data, record, 1, name_offset + 0xE00)
            else:
                struct.pack_into("<I", data, record + 12, name_offset + 0xE00)
            encoded = name.encode("ascii") + b"\0"
            data[name_offset:name_offset + len(encoded)] = encoded
            name_offset += len(encoded)
    return bytes(data)


def changed(data, fmt, offset, value):
    result = bytearray(data)
    struct.pack_into(fmt, result, offset, value)
    return bytes(result)


class PeTests(unittest.TestCase):
    def test_valid_headers_and_imports(self):
        result = pkg.inspect_pe(pe_image(("KERNEL32.dll", "bcrypt.dll"), ("d3dcompiler_47.dll",)))
        self.assertEqual(result["machine"], "AMD64")
        self.assertEqual(result["imports"], ["bcrypt.dll", "kernel32.dll"])
        self.assertEqual(result["delay_imports"], ["d3dcompiler_47.dll"])

    def test_api_set_import(self):
        pkg.inspect_pe(pe_image(("api-ms-win-core-synch-l1-2-0.dll",)))

    def test_reject_dynamic_crt_and_unknown_libraries(self):
        for name in ("vcruntime140.dll", "msvcp140.dll", "libunwind.dll", "plugin.dll"):
            with self.subTest(name=name), self.assertRaises(pkg.PackageError):
                pkg.inspect_pe(pe_image((name,)))

    def test_reject_delayed_non_system_library(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(pe_image(delayed=("vcruntime140.dll",)))

    def test_reject_dll_path(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(pe_image(("../kernel32.dll",)))

    def test_reject_truncated_or_non_pe(self):
        for data in (b"", b"MZ", b"not a pe", pe_image()[:1024]):
            with self.subTest(size=len(data)), self.assertRaises(pkg.PackageError):
                pkg.inspect_pe(data)

    def test_reject_wrong_machine(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(changed(pe_image(), "<H", 0x84, 0x14C))

    def test_reject_console_subsystem(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(changed(pe_image(), "<H", 0x98 + 68, 3))

    def test_reject_dll_characteristic(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(changed(pe_image(), "<H", 0x96, 0x2002))

    def test_reject_pe32(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(changed(pe_image(), "<H", 0x98, 0x10B))

    def test_reject_unmapped_import_rva(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(changed(pe_image(), "<I", 0x98 + 120, 0xFFFFFFF0))

    def test_reject_unterminated_import_table(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(changed(pe_image(), "<I", 0x98 + 124, 20))

    def test_reject_non_rva_delay_table(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(changed(pe_image(delayed=("user32.dll",)), "<I", 0x400, 0))

    def test_reject_empty_import_table(self):
        with self.assertRaises(pkg.PackageError):
            pkg.inspect_pe(pe_image(imports=()))


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        for name in pkg.SOURCE_FILES:
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"Source fixture: {name}\n", encoding="utf-8")
        (self.repo / "Cargo.toml").write_text('[package]\nname="citrus-studio"\nversion="0.4.0"\n')
        self.source = "registry+https://github.com/rust-lang/crates.io-index"
        lock = ('version=4\n[[package]]\nname="citrus-studio"\nversion="0.4.0"\n'
                '[[package]]\nname="example"\nversion="1.0.0"\n'
                f'source="{self.source}"\nchecksum="' + "a" * 64 + '"\n')
        (self.repo / "Cargo.lock").write_text(lock)
        self.metadata = {"packages": [
            {"id": "root", "name": "citrus-studio", "version": "0.4.0", "source": None, "license": "MIT"},
            {"id": "dep", "name": "example", "version": "1.0.0", "source": self.source,
             "license": "MIT OR Apache-2.0", "manifest_path": "C:/private/path/Cargo.toml"},
        ], "resolve": {"nodes": [{"id": "root", "features": ["vst3", "vst2"]}, {"id": "dep", "features": []}]}}
        self.binaries = self.root / "bin"
        self.binaries.mkdir()
        for binary in pkg.BINARIES:
            (self.binaries / binary).write_bytes(pe_image())
        # Neither broad binary-directory copies nor user-data files may enter ZIP.
        (self.binaries / "Autosave.citrus").write_text("private user data")
        (self.binaries / "libunwind.dll").write_bytes(b"must not be included")
        (self.binaries / "debug.pdb").write_bytes(b"must not be included")
        self.info = {"target": pkg.TARGET, "toolchain": pkg.TOOLCHAIN, "rustflags": pkg.RUSTFLAGS,
                     "profile": "release", "features": ["vst2", "vst3"],
                     "rustc": f"rustc 1.99.0 (fixture)\nhost: {pkg.TARGET}\nrelease: 1.99.0",
                     "cargo": "cargo 1.99.0 (fixture)", "source_commit": "b" * 40,
                     "source_epoch": 1791514800,
                     "cargo_lock_sha256": pkg.digest((self.repo / "Cargo.lock").read_bytes()),
                     "msvc_linker_version": "fixture"}

    def create(self, directory="out"):
        return pkg.create_package(self.repo, self.binaries, self.metadata, self.info, self.root / directory)

    def rewrite(self, archive, change):
        with zipfile.ZipFile(archive) as handle:
            entries = [(info, handle.read(info)) for info in handle.infolist()]
        entries = change(entries)
        with warnings.catch_warnings(), zipfile.ZipFile(archive, "w") as handle:
            warnings.simplefilter("ignore", UserWarning)
            for info, data in entries:
                handle.writestr(info, data)

    def test_create_verify_extract_and_exact_whitelist(self):
        archive = self.create()
        extracted = self.root / "extracted"
        result = pkg.verify_package(archive, extracted)
        self.assertEqual(result["channel"], "unsigned-msvc-developer-preview")
        paths = {p.relative_to(extracted).as_posix() for p in extracted.rglob("*") if p.is_file()}
        self.assertEqual(paths, pkg.PACKAGE_FILES)
        self.assertNotIn("private", (extracted / "DEPENDENCIES.json").read_text())
        self.assertIn("PREVIEW", archive.name)
        checksum = archive.with_suffix(".zip.sha256").read_text().split()[0]
        self.assertEqual(checksum, hashlib.sha256(archive.read_bytes()).hexdigest())

    def test_deterministic_archive_for_identical_inputs(self):
        self.assertEqual(self.create("one").read_bytes(), self.create("two").read_bytes())

    def test_optional_media_document_is_included_when_present(self):
        (self.repo / "docs/PROJECT_MEDIA.md").write_text("Media recovery guide\n")
        (self.repo / "README.md").write_text("[Media](docs/PROJECT_MEDIA.md)\n")
        archive = self.create()
        info = pkg.verify_package(archive)
        self.assertIn("docs/PROJECT_MEDIA.md", info["source_documents"])

    def test_optional_export_and_media_guides_are_packaged_together(self):
        for path in ("docs/PROJECT_MEDIA.md", "docs/OFFLINE_EXPORT_WORKFLOW.md"):
            (self.repo / path).write_text("[README](../README.md)\n")
        (self.repo / "README.md").write_text(
            "[Media](docs/PROJECT_MEDIA.md) [Export](docs/OFFLINE_EXPORT_WORKFLOW.md)\n"
        )
        archive = self.create()
        info = pkg.verify_package(archive)
        self.assertIn("docs/PROJECT_MEDIA.md", info["source_documents"])
        self.assertIn("docs/OFFLINE_EXPORT_WORKFLOW.md", info["source_documents"])

    def test_missing_link_target_fails_before_package_write(self):
        (self.repo / "README.md").write_text("[Missing](docs/MISSING.md)\n")
        with self.assertRaises(pkg.PackageError):
            self.create()
        self.assertFalse((self.root / "out").exists())

    def test_relative_document_links_and_external_links(self):
        (self.repo / "docs/WORK_LOG.md").write_text("[README](../README.md) [upstream](https://example.org/page)\n")
        pkg.verify_package(self.create())

    def test_missing_helper_fails_before_archive_write(self):
        (self.binaries / pkg.BINARIES[1]).unlink()
        with self.assertRaises(pkg.PackageError):
            self.create()
        self.assertFalse((self.root / "out").exists())

    def test_missing_license_fails(self):
        (self.repo / "LICENSE").unlink()
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_crlf_lock_hashes_actual_file_bytes(self):
        lock_path = self.repo / "Cargo.lock"
        lock_path.write_bytes(lock_path.read_text().replace("\n", "\r\n").encode("utf-8"))
        self.info["cargo_lock_sha256"] = pkg.digest(lock_path.read_bytes())
        pkg.verify_package(self.create())

    def test_modified_lock_fails(self):
        with (self.repo / "Cargo.lock").open("a") as handle:
            handle.write("\n# changed after build\n")
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_wrong_toolchain_or_build_configuration_fails(self):
        changes = {"target": "x86_64-pc-windows-gnullvm", "toolchain": "stable", "profile": "debug",
                   "features": ["vst3"], "rustflags": "", "rustc": "rustc 1.98.0", "cargo": "cargo 1.98.0"}
        original = copy.deepcopy(self.info)
        for key, value in changes.items():
            self.info = {**original, key: value}
            with self.subTest(key=key), self.assertRaises(pkg.PackageError):
                self.create()

    def test_unreviewed_provenance_cannot_leak_environment(self):
        self.info["environment"] = {"PRIVATE_DATA": "must not enter package"}
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_output_collision_fails(self):
        self.create()
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_existing_extraction_destination_fails(self):
        with self.assertRaises(pkg.PackageError):
            pkg.verify_package(self.create(), self.repo)

    def test_payload_tampering_fails_without_extracting(self):
        archive = self.create()
        self.rewrite(archive, lambda entries: [(i, b"changed" if i.filename.endswith("/LICENSE") else d) for i, d in entries])
        destination = self.root / "extract"
        with self.assertRaises(pkg.PackageError):
            pkg.verify_package(archive, destination)
        self.assertFalse(destination.exists())

    def test_extra_private_file_fails(self):
        archive = self.create()
        self.rewrite(archive, lambda entries: entries + [(f"{archive.stem}/Autosave.citrus", b"private")])
        with self.assertRaises(pkg.PackageError):
            pkg.verify_package(archive)

    def test_path_traversal_absolute_and_second_root_fail(self):
        for index, extra in enumerate(("../escape", "/absolute", "other/root")):
            with self.subTest(extra=extra):
                archive = self.create("out" + str(index))
                self.rewrite(archive, lambda entries: entries[:-1] + [(extra, b"bad")])
                with self.assertRaises(pkg.PackageError):
                    pkg.verify_package(archive)

    def test_duplicate_zip_entry_fails(self):
        archive = self.create()
        self.rewrite(archive, lambda entries: entries[:-1] + [entries[0]])
        with self.assertRaises(pkg.PackageError):
            pkg.verify_package(archive)

    def test_symlink_zip_entry_fails(self):
        archive = self.create()
        def change(entries):
            entries[0][0].external_attr = (stat.S_IFLNK | 0o777) << 16
            return entries
        self.rewrite(archive, change)
        with self.assertRaises(pkg.PackageError):
            pkg.verify_package(archive)

    def test_dependency_absent_from_lock_fails(self):
        self.metadata["packages"][1]["version"] = "2.0.0"
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_unreviewed_dependency_source_fails(self):
        self.metadata["packages"][1]["source"] = "git+https://unreviewed.invalid/source"
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_missing_dependency_node_fails(self):
        self.metadata["packages"].pop()
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_package_identity_tampering_fails(self):
        archive = self.create()
        def change(entries):
            result = []
            for info, data in entries:
                if info.filename.endswith("/BUILD_PROVENANCE.json"):
                    record = json.loads(data)
                    record["source_commit"] = "c" * 40
                    data = pkg.json_bytes(record)
                result.append((info, data))
            return result
        self.rewrite(archive, change)
        with self.assertRaises(pkg.PackageError):
            pkg.verify_package(archive)


if __name__ == "__main__":
    unittest.main()
