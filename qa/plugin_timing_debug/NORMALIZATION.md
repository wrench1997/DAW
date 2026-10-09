# Publication normalization and custody

Only the task-local absolute path prefixes listed below were replaced in publication copies of existing files. Suffixes, JSON numeric/text values, event bytes, timestamps, state/audio measurements, errors, outcome labels, source commits, and recorded SHA256 values are unchanged. No JSON reformatting was used for copied evidence. Every copied file was reverse-normalized privately and checked byte-for-byte against its raw original.

- `__QA_ROOT__`: callback-timing QA directory
- `__VST3_VALIDATION_ROOT__`: external genuine-plugin validation/input directory
- `__DAW_SOURCE__`: original production checkout
- `__CARGO_TARGET_DIR__`: shared build target
- `__CARGO_HOME__`: dependency/toolchain Cargo home
- `__BUILD_TMPDIR__`: build temporary directory
- `__ALSA_PKG_CONFIG_PATH__`: task-local ALSA development-library probe directory
- `__LINKER_LIBRARY_PATH__`: task-local linker support directory

These tokens are neutral path defaults in source scripts and semantic placeholders in historical logs/JSON, including compiler `file://` identifiers. They do not expand automatically. Set the documented environment variables for a fresh run. Standard OS metadata paths, such as `/proc/loadavg` and `/sys/fs/cgroup/cpu.stat`, are preserved.

`INVENTORY.json` records every file in the original QA tree at packaging time, including excluded artifacts, with original byte size and SHA256. Included entries also give publication path, byte size, SHA256, and replacement counts. The count for a broader source prefix can include a more specific prefix; actual replacement is longest-prefix-first. Original hashes inside receipts, including source/build bindings and `raw_log_sha256`, identify original bytes and were never rewritten to publication hashes. Use the inventory or `PUBLICATION_SHA256SUMS` to verify publication bytes.

Raw runs, their WAV/state payloads, compiled executables and production snapshots were only read. The private reverse map and packaging work files are outside the archive. This package has no serialized states, plugin bundles/modules, test/helper executables, factory presets, audio payloads, caches, or symlinks. Existing historical e54a6e4 routing receipts were not touched or repackaged. The reference harness text is clearly separated under `historical-reference/`.

No zero-byte files/logs existed in the inventoried QA tree. If present, they would be omitted and individually recorded with the standard empty-file SHA256 and an explicit reason. All six nonempty run logs and nonempty current build logs are retained.

Generated documents, the inventory and integrity-verification files are new publication material. They interpret the retained receipts; they are not additional experimental results. `PUBLICATION_SHA256SUMS` covers every archive file other than itself.
