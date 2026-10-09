# Native-edit transport: bounded real-plugin correctness regression

Prepared source:87ceb06ce3dc23d817b1623093c8a51fddc2bea3, integrating reviewed
native-edit transport48d3f9706b49430c8cfd1556325bf6f0d6d3b8b6. Every manifested
production file is verified against the combined git objects. The preallocated
harness is byte-identical to the earlier matched debug/release harness; a new
linked test and helper are required. No native result exists before a run receipt.

Scope is deliberately limited to default B2048 at48kHz: routed Stochas→Surge→FX
and ordinary timeline→Surge→FX, nominal fixed and changing callbacks. Each case
must observe actual native FX latency0→32, stop safely, execute stopped Retry with
a newer timing revision/fresh epoch, then explicitly play with the original exact
worker/adapter/MIDI/PDC/finite-PCM assertions. Also test a fresh Surge restored from
the authoritative captured state before its first Process and immediately given
one NoteOn, with controller and component-state readback.

This is an unoptimized source-linked correctness regression with the changed
helper, not a new performance matrix. Previous optimized results stay historically
bound to4fdfbc2/244f622 and are not reattributed to this helper. No native GUI,
physical CPAL/device, speaker or GUI-stall resolution claim follows.

The helper must match the reviewed combined build; expected debug SHA256:
dca08353e3f23308d535a791c9fa2c89635ee683db29625fa8d2a3d0a988cbe8.

Use build_and_copy.py with explicit DAW_SOURCE/DAW_SOURCE_COMMIT,
NATIVE_HARNESS_PROFILE=debug and CARGO_BUILD_JOBS=2 after shared-target handoff.
Run the matrix with NATIVE_TIMING_BUDGETS=2048 and the test filter
native_timing_all_exposed_profiles. Run the separate test filter
native_timing_fresh_restored_surge_first_note_and_state. Every invocation uses a
new run directory and records source, harness, binary and profile bindings.

Retain failures; no retry-until-pass. Source-only publication excludes plugin
bundles, factory content, state files, compiled executables and WAV payloads.
