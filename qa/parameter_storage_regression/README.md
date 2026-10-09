# Parameter storage: bounded genuine-plugin correctness regression

Combined source:fb7b91a82d31226485a22e9e5e0b73d1f9a1fe3f, integrating reviewed
parameter-storage bf573a45826466db0952eb1f4bd598937c68d1c5. All124 production files
are checked against git objects. The preallocated harness is unchanged from prior
QA (SHA256c38f30c4b50fe7eb1c5d69acec6c59dd941ba394797b21e3960acb6b5d37b072).
A new source-linked test and helper must be built; no prior vendor artifact is
silently substituted. No native result exists until its run receipts complete.

Only default B2048 at48kHz is tested: routed Stochas→Surge→Effects and ordinary
own timeline notes→Surge→Effects, nominal fixed and changing callbacks. Every case
must observe native Effects latency0→32, safe stop, actual stopped Retry with a
new timing revision/fresh epoch, then explicit playback with the existing exact
worker/adapter/MIDI/PDC/finite-PCM assertions. A separate fresh-Surge test restores
the authoritative state before its first Process and immediately sends one note,
checking the controller and saved component state afterward.

Expected new debug helper SHA256:
a29e4942b5fe027e9891a28e5d7f7611d2c5b2f12ef5f283599d15c4278aee64.

This is a bounded unoptimized correctness regression. It is not a full new
performance matrix. Retain failures and raw callback interval overruns. Previous
optimized evidence stays bound to4fdfbc2/244f622; no native GUI, physical CPAL,
hardware or GUI-stall qualification follows from this run.

After explicit shared-target handoff, run build_and_copy.py with the recorded
Rust/linker environment, explicit DAW_SOURCE and DAW_SOURCE_COMMIT,
NATIVE_HARNESS_PROFILE=debug and CARGO_BUILD_JOBS=2. After a quiet-window handoff,
run a unique directory with NATIVE_TIMING_BUDGETS=2048 and filter
native_timing_all_exposed_profiles; then a second directory using filter
native_timing_fresh_restored_surge_first_note_and_state. The runner records exact
source/harness/profile/test/helper bindings and never overwrites a previous run.

Publication is source-only: no plugin bundles, factory content, input states,
compiled executables or WAV payloads are attached. Official GPL-family plugin
versions and module hashes are preserved in receipts/plugin-provenance.json.
