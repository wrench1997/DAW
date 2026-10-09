# Bounded parameter storage: cost and validation evidence

Feature commit: `bf573a45826466db0952eb1f4bd598937c68d1c5`.
Source-only archive: `parameter-storage-bf573a4-source-evidence.zip`.
Archive SHA256: `1bd33df7c1ec9afa58cd5b6a508b7af12c4a84f68ce4ad12cda5dca3f388773c`.
Archive size: 5,092,126 bytes.

This concise set contains exact portable copies of the final reader summary, complete final full/empty-queue cost CSV matrices, source-owner validation receipt, native receipt, and an exhaustive raw-cost/source-manifest locator/hash inventory. The separately retained archive contains all 1,677 logical source/evidence files as 435 deduplicated text objects. No repository changes, builds, plugins, push or merge were performed for packaging.

Final accepted source binding: COM `cf82a47eb29745f2c8c8ed93f2fe06092c1d333109b4fcf9eb55159b1d0b13c7`; helper `a29e4942b5fe027e9891a28e5d7f7611d2c5b2f12ef5f283599d15c4278aee64`; source diff `b77e86e385579a919961e1a9600dd4dd77f25d0b96fe08446070633ef27fedcd`; final comparison `parameter-comparison/populated/`; final native `parameter-storage-sparse/`. All earlier cursor/indexed/append/maintained/dense candidates, including the old folder named `final` and `extended/final-parameter-tests.log`, are historical. Their original labels and numbers were preserved.

Constructor accounting: input 8,198 allocations / 1,048,696 requested bytes; output 4,102 / 524,408; combined 1,573,104 bytes. The sparse index adds 98,352 bytes. Outer runtime COM wrappers, allocator overhead and RSS are excluded. Post-constructor sparse parameter regions measured zero allocations, reallocations and frees. This does not certify the whole callback, plugin/controller/event work, or final release of externally retained queues after owner teardown.

The five-sample matrix retains debug and optimized profiles, cold and reused phases, 32/128/512/4096/8192 sizes, allocation/free/byte data, correctness checksums, and maximum-empty-queue diagnostics. Some random reads, small/distinct workloads and large populated suffixes remain slower; aggregate worst cases can remain quadratic. Timings are observations on the recorded environment, not deadline guarantees. The historical 346.9 ms native resize stall remains unresolved. Native hardware, mixed-Wayland, sanitizer, Windows/macOS runtime and full-app integration coverage remain outside this parameter-only acceptance. See the complete original limits in `validation-receipt.json` and `native-parameter-storage-receipt.json`.

Integrity: every original raw hash, 41 frozen source entries, 966 comparative source entries, six native fixture sources, every archive member hash, actual reconstruction and path safety were checked. Portable text changes only named private task-root prefixes. Original/published hash strings, values and statuses remain intact. The private reverse map stays local outside publication. Original MIT notices and exact upstream/CITRUS provenance are associated with every source snapshot in the full archive. No binaries, targets, caches, plugin assets, state, audio, screenshots or native/headless raw wire data are delivered.
