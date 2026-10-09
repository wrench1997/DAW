"""Synthetic, plugin/device-free package regressions; no Windows binary executes."""

import copy
import hashlib
import json
from pathlib import Path
import stat
import struct
import tempfile
import tomllib
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

    def test_windows_combase_system_import(self):
        # Windows Runtime/COM OS component, not the MSVC redistributable CRT.
        for delayed in (False, True):
            with self.subTest(delayed=delayed):
                image = pe_image(delayed=("ComBase.dll",)) if delayed else pe_image(("ComBase.dll",))
                result = pkg.inspect_pe(image)
                self.assertIn("combase.dll", result["delay_imports" if delayed else "imports"])

    def test_windows_ui_automation_system_import(self):
        # Microsoft's UI Automation OS provider; do not package a private DLL copy.
        result = pkg.inspect_pe(pe_image(("UIAutomationCore.dll",), ("UIAutomationCore.dll",)))
        self.assertEqual(result["imports"], ["uiautomationcore.dll"])
        self.assertEqual(result["delay_imports"], ["uiautomationcore.dll"])

    def test_windows_random_number_generator_system_import(self):
        # ProcessPrng is supplied by the Windows OS, independently of the VC CRT.
        result = pkg.inspect_pe(pe_image(("BCryptPrimitives.dll",), ("BCryptPrimitives.dll",)))
        self.assertEqual(result["imports"], ["bcryptprimitives.dll"])
        self.assertEqual(result["delay_imports"], ["bcryptprimitives.dll"])

    def test_all_unapproved_direct_and_delay_imports_are_reported(self):
        with self.assertRaisesRegex(
                pkg.PackageError, "Unapproved runtime imports: libunwind.dll, plugin.dll, vcruntime140.dll"):
            pkg.inspect_pe(pe_image(("plugin.dll", "KERNEL32.dll"), ("vcruntime140.dll", "libunwind.dll")))

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
        self.root = Path(self.temp.name).resolve()
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
            {"id": "root", "name": "citrus-studio", "version": "0.4.0", "source": None, "license": "MIT",
             "manifest_path": str(self.repo / "Cargo.toml")},
            {"id": "dep", "name": "example", "version": "1.0.0", "source": self.source,
             "license": "MIT OR Apache-2.0", "manifest_path": "C:/private/path/Cargo.toml"},
        ], "workspace_root": str(self.repo), "resolve": {"root": "root", "nodes": [{"id": "root", "features": ["vst3", "vst2"]}, {"id": "dep", "features": []}]}}
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
        self.assertEqual(paths, pkg.PACKAGE_FILES | (pkg.VENDOR_FILES if isinstance(self, VendorTests) else set()))
        self.assertNotIn("private", (extracted / "DEPENDENCIES.json").read_text())
        self.assertIn("PREVIEW", archive.name)
        checksum = archive.with_suffix(".zip.sha256").read_text().split()[0]
        self.assertEqual(checksum, hashlib.sha256(archive.read_bytes()).hexdigest())

    def test_audit_reports_both_binaries_before_rejecting_package(self):
        (self.binaries / pkg.BINARIES[0]).write_bytes(pe_image(("plugin.dll",)))
        (self.binaries / pkg.BINARIES[1]).write_bytes(pe_image(delayed=("libunwind.dll",)))
        with self.assertRaises(pkg.PackageError) as error:
            self.create()
        self.assertIn("citrus-studio.exe: Unapproved runtime imports: plugin.dll", str(error.exception))
        self.assertIn("vst3-host-helper.exe: Unapproved runtime imports: libunwind.dll", str(error.exception))
        self.assertFalse((self.root / "out").exists())

    def test_deterministic_archive_for_identical_inputs(self):
        self.assertEqual(self.create("one").read_bytes(), self.create("two").read_bytes())

    def test_optional_media_document_is_included_when_present(self):
        (self.repo / "docs/PROJECT_MEDIA.md").write_text("Media recovery guide\n")
        (self.repo / "README.md").write_text("[Media](docs/PROJECT_MEDIA.md)\n")
        archive = self.create()
        info = pkg.verify_package(archive)
        self.assertIn("docs/PROJECT_MEDIA.md", info["source_documents"])

    def test_optional_feature_guides_are_packaged_together(self):
        for path in ("docs/PROJECT_MEDIA.md", "docs/OFFLINE_EXPORT_WORKFLOW.md", "docs/AUDIO_SPLIT_FIDELITY.md", "docs/WAV_EXPORT_OPTIONS.md", "docs/MIXER_METERING.md", "docs/LOCAL_SAMPLE_BROWSER.md", "docs/HEADLESS_UI_QA.md", "docs/FL_INSPIRED_NATIVE_THEME.md", "docs/MULTIWINDOW_WORKSPACE.md", "docs/COMPACT_WORKSPACE.md"):
            (self.repo / path).write_text("[README](../README.md)\n")
        (self.repo / "README.md").write_text(
            "[Media](docs/PROJECT_MEDIA.md) [Export](docs/OFFLINE_EXPORT_WORKFLOW.md) "
            "[Split](docs/AUDIO_SPLIT_FIDELITY.md) [Options](docs/WAV_EXPORT_OPTIONS.md) "
            "[Meters](docs/MIXER_METERING.md) [Samples](docs/LOCAL_SAMPLE_BROWSER.md) "
            "[UI QA](docs/HEADLESS_UI_QA.md) [Native theme](docs/FL_INSPIRED_NATIVE_THEME.md) "
            "[Workspace](docs/MULTIWINDOW_WORKSPACE.md) [Compact](docs/COMPACT_WORKSPACE.md)\n"
        )
        archive = self.create()
        info = pkg.verify_package(archive)
        self.assertIn("docs/PROJECT_MEDIA.md", info["source_documents"])
        self.assertIn("docs/OFFLINE_EXPORT_WORKFLOW.md", info["source_documents"])
        self.assertIn("docs/AUDIO_SPLIT_FIDELITY.md", info["source_documents"])
        self.assertIn("docs/WAV_EXPORT_OPTIONS.md", info["source_documents"])
        self.assertIn("docs/MIXER_METERING.md", info["source_documents"])
        self.assertIn("docs/LOCAL_SAMPLE_BROWSER.md", info["source_documents"])
        self.assertIn("docs/HEADLESS_UI_QA.md", info["source_documents"])
        self.assertIn("docs/FL_INSPIRED_NATIVE_THEME.md", info["source_documents"])
        self.assertIn("docs/MULTIWINDOW_WORKSPACE.md", info["source_documents"])
        self.assertIn("docs/COMPACT_WORKSPACE.md", info["source_documents"])

    def test_prerelease_package_version_is_preserved(self):
        for name in ("Cargo.toml", "Cargo.lock"):
            path = self.repo / name
            path.write_text(path.read_text().replace('version="0.4.0"', 'version="0.5.0-alpha.1"'))
        self.metadata["packages"][0]["version"] = "0.5.0-alpha.1"
        self.info["cargo_lock_sha256"] = pkg.digest((self.repo / "Cargo.lock").read_bytes())
        archive = self.create()
        info = pkg.verify_package(archive)
        self.assertEqual(info["version"], "0.5.0-alpha.1")
        self.assertIn("0.5.0-alpha.1", archive.name)

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

    def test_legacy_registry_license_and_build_metadata_version_remain_supported(self):
        self.metadata["packages"][1]["license"] = "MIT/Apache-2.0"
        self.metadata["packages"][1]["version"] = "1.0.0+wasi.1"
        path = self.repo / "Cargo.lock"
        path.write_text(path.read_text().replace('version="1.0.0"', 'version="1.0.0+wasi.1"'))
        self.info["cargo_lock_sha256"] = pkg.digest(path.read_bytes())
        pkg.verify_package(self.create())

    def test_license_url_and_private_path_forms_are_rejected(self):
        for license_expression in ("https://private.example.com/secret-token", "C:/private/path",
                                   "C:\\private\\path", "/private/path", "MIT\n/private/path"):
            self.metadata["packages"][1]["license"] = license_expression
            with self.subTest(license=license_expression), self.assertRaises(pkg.PackageError):
                self.create()

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

class VendorTests(PackageTests):
    """Synthetic provenance bytes keep this independent of the feature checkout.

    Override only reviewed file digests; upstream identities and all schema/path
    checks are production code. The integrated vendor bytes get a separate exact
    source-input check; tests never copy vendor sources into the package/repo.
    """
    def setUp(self):
        super().setUp()
        from unittest.mock import patch
        self.vendor = self.repo / pkg.VENDOR_PATH
        self.vendor.mkdir(parents=True)
        (self.vendor / "Cargo.toml").write_text(
            '[package]\nname="vst3-host"\nversion="0.9.0"\nlicense="MIT"\n')
        data = {
            "LICENSE": b"Synthetic MIT license fixture\n",
            "CITRUS_PATCHES.md": b"Synthetic reviewed patch notes\n",
            "CITRUS.patch": b"--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n",
            ".cargo_vcs_info.json": pkg.json_bytes(
                {"git": {"sha1": pkg.VENDOR_COMMIT}, "path_in_vcs": "vst3-host"}),
        }
        hashes = {}
        for name, value in data.items():
            (self.vendor / name).write_bytes(value)
            hashes[f"{pkg.VENDOR_PATH}/{name}"] = pkg.digest(value)
        override = patch.object(pkg, "VENDOR_FILE_HASHES", hashes)
        override.start()
        self.addCleanup(override.stop)
        with (self.repo / "Cargo.toml").open("a") as handle:
            handle.write(f'\n[patch.crates-io]\nvst3-host = {{ path = "{pkg.VENDOR_PATH}" }}\n')
        with (self.repo / "Cargo.lock").open("a") as handle:
            handle.write('\n[[package]]\nname="vst3-host"\nversion="0.9.0"\n')
        self.refresh_lock()
        self.metadata["packages"].append({
            "id": "path+file:///private/CargoIdentity#vst3-host@0.9.0", "name": "vst3-host",
            "version": "0.9.0", "source": None, "license": "MIT",
            "manifest_path": str(self.vendor / "Cargo.toml"),
            "metadata": {"secret": "private Cargo metadata"},
        })
        self.metadata["resolve"]["nodes"].append(
            {"id": self.metadata["packages"][-1]["id"], "features": ["process-isolation"]})

    def refresh_lock(self):
        self.info["cargo_lock_sha256"] = pkg.digest((self.repo / "Cargo.lock").read_bytes())

    def change_payload(self, archive, change):
        """Rehash everything so these tests reach semantic checks, not just SHA checks."""
        def rewrite(entries):
            payload = {i.filename.split("/", 1)[1]: data for i, data in entries}
            change(payload)
            payload["SHA256SUMS.txt"] = "".join(
                f"{pkg.digest(payload[p])}  {p}\n"
                for p in sorted(set(payload) - {"SHA256SUMS.txt"})).encode("ascii")
            return [(f"{archive.stem}/{p}", value) for p, value in sorted(payload.items())]
        self.rewrite(archive, rewrite)

    def invalid_archive(self, archive):
        destination = self.root / "rejected-extract"
        with self.assertRaises(pkg.PackageError):
            pkg.verify_package(archive, destination)
        self.assertFalse(destination.exists())

    def test_sanitized_vendor_inventory_and_provenance(self):
        archive = self.create()
        with zipfile.ZipFile(archive) as handle:
            raw = handle.read(f"{archive.stem}/DEPENDENCIES.json")
            inventory = json.loads(raw)
            for path in pkg.VENDOR_FILES:
                self.assertEqual(handle.read(f"{archive.stem}/{path}"), (self.repo / path).read_bytes())
        for private in (b"private", b"manifest_path", b"workspace_root", b"CargoIdentity", b"metadata"):
            self.assertNotIn(private, raw)
        vendor = next(p for p in inventory["packages"] if p["name"] == "vst3-host")
        self.assertIsNone(vendor["source"])
        self.assertIsNone(vendor["checksum"])
        self.assertEqual(vendor["vendor_provenance"], pkg.vendor_provenance())

    def test_each_missing_or_tampered_bundle_input_fails_before_write(self):
        for path in pkg.VENDOR_FILES:
            file = self.repo / path
            original = file.read_bytes()
            for changed in (None, original + b"changed"):
                with self.subTest(path=path, changed=changed is not None):
                    file.unlink()
                    if changed is not None:
                        file.write_bytes(changed)
                    with self.assertRaises((pkg.PackageError, OSError)):
                        self.create()
                    self.assertFalse((self.root / "out").exists())
                    file.write_bytes(original)

    def test_each_missing_or_tampered_bundle_entry_fails_even_rehashed(self):
        index = 0
        for path in pkg.VENDOR_FILES:
            for missing in (False, True):
                with self.subTest(path=path, missing=missing):
                    archive = self.create(f"bundle-{index}")
                    index += 1
                    def change(payload):
                        if missing:
                            del payload[path]
                            info = json.loads(payload["BUILD_PROVENANCE.json"])
                            info["source_documents"].remove(path)
                            payload["BUILD_PROVENANCE.json"] = pkg.json_bytes(info)
                        else:
                            payload[path] += b"tampered"
                    self.change_payload(archive, change)
                    self.invalid_archive(archive)

    def test_rehashed_bundle_and_matching_provenance_tampering_fails(self):
        archive = self.create()
        def change(payload):
            path = f"{pkg.VENDOR_PATH}/CITRUS.patch"
            payload[path] += b"tampered"
            inventory = json.loads(payload["DEPENDENCIES.json"])
            inventory["packages"][-1]["vendor_provenance"]["files"][path] = pkg.digest(payload[path])
            payload["DEPENDENCIES.json"] = pkg.json_bytes(inventory)
        self.change_payload(archive, change)
        self.invalid_archive(archive)

    def test_rehashed_vendor_provenance_fields_fail(self):
        edits = {
            "path": "../private/vendor", "upstream_archive_url": "https://unreviewed.invalid/source",
            "upstream_archive_sha256": "0" * 64, "upstream_commit": "0" * 40,
            "manifest_path": "/private/Cargo.toml", "files": {},
        }
        for index, (key, value) in enumerate(edits.items()):
            with self.subTest(key=key):
                archive = self.create(f"provenance-{index}")
                def change(payload):
                    inventory = json.loads(payload["DEPENDENCIES.json"])
                    inventory["packages"][-1]["vendor_provenance"][key] = value
                    payload["DEPENDENCIES.json"] = pkg.json_bytes(inventory)
                self.change_payload(archive, change)
                self.invalid_archive(archive)

    def test_vendor_record_and_bundle_must_be_paired(self):
        for drop_record in (True, False):
            archive = self.create(f"pair-{drop_record}")
            def change(payload):
                if drop_record:
                    inventory = json.loads(payload["DEPENDENCIES.json"])
                    inventory["packages"].pop()
                    payload["DEPENDENCIES.json"] = pkg.json_bytes(inventory)
                else:
                    for path in pkg.VENDOR_FILES:
                        del payload[path]
                    info = json.loads(payload["BUILD_PROVENANCE.json"])
                    info["source_documents"] = sorted(set(info["source_documents"]) - pkg.VENDOR_FILES)
                    payload["BUILD_PROVENANCE.json"] = pkg.json_bytes(info)
            self.change_payload(archive, change)
            self.invalid_archive(archive)

    def test_vendor_metadata_wrong_name_version_license_path_source_fails(self):
        original = self.metadata["packages"][-1].copy()
        for key, value in (("name", "other"), ("version", "0.9.1"), ("license", "GPL-3.0"),
                           ("manifest_path", str(self.repo / "other/Cargo.toml")),
                           ("manifest_path", str(self.vendor / "../vst3-host-0.9.0/Cargo.toml")),
                           ("manifest_path", f"{pkg.VENDOR_PATH}/Cargo.toml"),
                           ("source", pkg.REGISTRY_SOURCE)):
            with self.subTest(key=key, value=value):
                self.metadata["packages"][-1] = {**original, key: value}
                with self.assertRaises(pkg.PackageError):
                    self.create()
        self.metadata["packages"][-1] = original

    def test_vendor_manifest_wrong_name_version_license_fails(self):
        path = self.vendor / "Cargo.toml"
        original = path.read_text()
        for before, after in (("vst3-host", "other"), ("0.9.0", "0.9.1"), ("MIT", "GPL-3.0")):
            with self.subTest(before=before):
                path.write_text(original.replace(before, after))
                with self.assertRaises(pkg.PackageError):
                    self.create()
        path.write_text(original)

    def test_patch_missing_absolute_traversal_wrong_or_extra_override_fails(self):
        path = self.repo / "Cargo.toml"
        original = path.read_text()
        invalid = [original.split("[patch.crates-io]")[0]]
        # Quote the entire TOML string so Windows backslashes reach the policy
        # validator as path data instead of becoming invalid TOML escape sequences.
        invalid += [original.replace(json.dumps(pkg.VENDOR_PATH), json.dumps(replacement, ensure_ascii=False))
                    for replacement in (
                        str(self.vendor), "../outside", "vendor/../vendor/vst3-host-0.9.0", "other/vendor",
                        "C:/private/vendor", r"C:\Users\runner\vendor", r"vendor\vst3-host-0.9.0")]
        invalid += [original + '\nother = { path = "vendor/other" }\n',
                    original.replace('{ path =', '{ package = "other", path =')]
        for index, value in enumerate(invalid):
            with self.subTest(index=index):
                tomllib.loads(value)  # Every case must exercise policy, not parser failure.
                path.write_text(value)
                with self.assertRaises(pkg.PackageError):
                    self.create()
        path.write_text(original)

    def test_local_lock_source_checksum_wrong_version_missing_or_duplicate_fails(self):
        path = self.repo / "Cargo.lock"
        original = path.read_text()
        vendor_entry = '\n[[package]]\nname="vst3-host"\nversion="0.9.0"\n'
        invalid = [original + 'checksum="' + 'a' * 64 + '"\n',
                   original + f'source="{pkg.REGISTRY_SOURCE}"\n',
                   original.replace('version="0.9.0"', 'version="0.9.1"'),
                   original.replace(vendor_entry, ''), original + vendor_entry]
        for index, value in enumerate(invalid):
            with self.subTest(index=index):
                path.write_text(value)
                self.refresh_lock()
                with self.assertRaises(pkg.PackageError):
                    self.create()
        path.write_text(original)
        self.refresh_lock()

    def test_unapproved_local_dependency_fails(self):
        self.metadata["packages"][1]["source"] = None
        self.metadata["packages"][1]["manifest_path"] = str(self.repo / "other/Cargo.toml")
        path = self.repo / "Cargo.lock"
        path.write_text(path.read_text().replace(f'source="{self.source}"\nchecksum="' + "a" * 64 + '"\n', ''))
        self.refresh_lock()
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_root_name_spoof_and_graph_root_mismatch_fail(self):
        original = copy.deepcopy(self.metadata)
        for key, value in (("manifest_path", str(self.vendor / "Cargo.toml")),
                           ("manifest_path", str(self.repo / "../repo/Cargo.toml")),
                           ("version", "0.9.0")):
            self.metadata = copy.deepcopy(original)
            self.metadata["packages"][0][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(pkg.PackageError):
                self.create()
        self.metadata = copy.deepcopy(original)
        self.metadata["resolve"]["root"] = "dep"
        with self.assertRaises(pkg.PackageError):
            self.create()
        self.metadata = copy.deepcopy(original)
        self.metadata["workspace_root"] = str(self.vendor)
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_duplicate_metadata_ids_and_nodes_fail(self):
        for field in ("packages", "nodes"):
            values = self.metadata[field] if field == "packages" else self.metadata["resolve"][field]
            values.append(copy.deepcopy(values[0]))
            with self.subTest(field=field), self.assertRaises(pkg.PackageError):
                self.create()
            values.pop()

    def test_symlink_vendor_parent_manifest_and_bundle_fail(self):
        for index, path in enumerate((self.repo / "vendor", self.vendor,
                                     self.vendor / "Cargo.toml", self.vendor / "LICENSE")):
            with self.subTest(path=path):
                outside = self.root / f"outside-{index}"
                path.rename(outside)
                try:
                    path.symlink_to(outside, target_is_directory=outside.is_dir())
                except OSError:
                    outside.rename(path)
                    self.skipTest("Symlink creation unavailable on this host")
                try:
                    with self.assertRaises(pkg.PackageError):
                        self.create()
                finally:
                    path.unlink()
                    outside.rename(path)

    def test_windows_reparse_point_flag_is_rejected(self):
        from types import SimpleNamespace
        from unittest.mock import patch
        original = Path.lstat
        def status(path):
            result = original(path)
            return SimpleNamespace(st_mode=result.st_mode, st_file_attributes=0x400) if path == self.vendor else result
        with patch.object(Path, "lstat", status), self.assertRaises(pkg.PackageError):
            self.create()

    def test_changed_vcs_identity_fails_even_with_matching_synthetic_hash(self):
        path = f"{pkg.VENDOR_PATH}/.cargo_vcs_info.json"
        (self.repo / path).write_bytes(pkg.json_bytes({"git": {"sha1": "0" * 40}, "path_in_vcs": "vst3-host"}))
        pkg.VENDOR_FILE_HASHES[path] = pkg.digest((self.repo / path).read_bytes())
        with self.assertRaises(pkg.PackageError):
            self.create()

    def test_native_guide_closes_only_with_bundle_and_external_fixture_link(self):
        guide = self.repo / "docs/NATIVE_VST3_EDITORS.md"
        guide.write_text('[Vendor](../vendor/vst3-host-0.9.0/CITRUS_PATCHES.md)\n'
                         '[Fixture](../tests/fixtures/vst3-editor/README.md)\n')
        with self.assertRaisesRegex(pkg.PackageError, "Broken package document link"):
            self.create()
        guide.write_text(guide.read_text().replace('../tests/fixtures/vst3-editor/README.md',
                         'https://example.org/verified-source/fixture/README.md'))
        info = pkg.verify_package(self.create())
        self.assertIn("docs/NATIVE_VST3_EDITORS.md", info["source_documents"])
        self.assertTrue(pkg.VENDOR_FILES <= set(info["source_documents"]))

    def test_vendor_source_and_fixture_never_copied_and_extra_entries_rejected(self):
        for name in (f"{pkg.VENDOR_PATH}/src/lib.rs", "tests/fixtures/vst3-editor/README.md"):
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("not shipped")
        archive = self.create()
        with zipfile.ZipFile(archive) as handle:
            self.assertFalse(any("/src/" in name or "/tests/" in name for name in handle.namelist()))
        for index, name in enumerate((f"{pkg.VENDOR_PATH}/src/lib.rs", "tests/fixtures/vst3-editor/README.md")):
            extra = self.create(f"extra-{index}")
            self.change_payload(extra, lambda payload: payload.update({name: b"not shipped"}))
            self.invalid_archive(extra)

    def test_rehashed_unbounded_or_malformed_inventory_fails(self):
        def changes(inventory):
            return [
                {**inventory, "metadata": {"private": "secret"}},
                {**inventory, "packages": [{**p, "manifest_path": "/private/path"} for p in inventory["packages"]]},
                {**inventory, "packages": inventory["packages"] * 2},
                {**inventory, "packages": [{**inventory["packages"][0], "features": ["/private/path"]}]},
                {**inventory, "packages": [{**inventory["packages"][0], "license": "MIT\\n/private/path"}]},
                {**inventory, "packages": [{**inventory["packages"][0], "license": "https://private.example.com/token"}]},
                {**inventory, "packages": [{**inventory["packages"][0], "license": "C:/private/path"}]},
                {**inventory, "packages": [{**inventory["packages"][0], "checksum": "a" * 64}]},
                {**inventory, "packages": [{**inventory["packages"][0], "version": "0.0.1"}]},
            ]
        inventory = pkg.dependency_inventory(self.metadata, pkg.tomllib.loads((self.repo / "Cargo.lock").read_text()), self.repo)
        for index, malformed in enumerate(changes(inventory)):
            archive = self.create(f"invalid-inventory-{index}")
            self.change_payload(archive, lambda payload: payload.update({"DEPENDENCIES.json": pkg.json_bytes(malformed)}))
            self.invalid_archive(archive)


class ProvenanceConstantsTests(unittest.TestCase):
    def test_reviewed_provenance_constants_are_pinned(self):
        self.assertEqual(pkg.VENDOR_ARCHIVE_SHA256, "6ec579d54bd13b83c60c1fd8bb756cf234e36ccbfb4833ff756b417e64db7fea")
        self.assertEqual(pkg.VENDOR_COMMIT, "ed054908cfe057694d8cf037d0c39dfb5eb4c2ca")
        self.assertEqual(pkg.VENDOR_FILE_HASHES[f"{pkg.VENDOR_PATH}/LICENSE"], "a65a537295910b776a8b2edb2e7410c3b0e975ca6388994e032c4d1842b4952d")
        self.assertEqual(pkg.VENDOR_FILE_HASHES[f"{pkg.VENDOR_PATH}/CITRUS.patch"], "dd74099ae07fcb4d0f2668bebf3b85714f992247c65714b72d12d1fc245b4c5d")



if __name__ == "__main__":
    unittest.main()
