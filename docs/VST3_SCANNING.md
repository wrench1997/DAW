# VST3 metadata scanning

## Classification and capabilities

Citrus probes VST3 modules through its packaged `vst3-host-helper`, located beside the
application. The background scan does not load third-party code in the application or
on its audio callback. It loads the same **default audio class** that the current
project runtime loads, then reads that class's real factory name, vendor, class UID,
subcategory string and event-bus capabilities. It does not create an editor or start
processing for a scan. This does execute the candidate plugin's load/initialization
code in the helper. Process separation is not an OS security sandbox; use trusted
plugin sources and do not interpret successful metadata loading as a security audit.

- The exact `Instrument` subcategory token identifies an instrument.
- Otherwise, the exact `Fx` token identifies an effect.
- An otherwise unclassified plugin with a reported event output is labeled `MIDI`.
- Other categories remain `Unknown`. An absent or unfamiliar subcategory never falls
  back to filename guesses.
- MIDI input/output capabilities come only from the actual helper response. Category,
  filename, and audio-bus count are not substitutes for event-bus information.

`Instrument|Synth` is therefore an instrument, while official **Surge XT Effects**
reports `Fx` and is correctly shown as an effect despite its filename. Effect plugins
can have event inputs/outputs without becoming instruments.

A successful load can still have unknown classification. A failed load is explicitly
unverified and keeps its error (including a missing helper, timeout, or unsupported
build). The manager's `Unknown` label exposes that error on hover, and scan completion
reports how many metadata probes were unavailable. It does not turn those failures
into successful plugin validations.

VST2 filename/export behavior is unchanged by this slice. VST2 MIDI capability discovery
is not added.

## Class identity, bundles and old caches

A `.vst3` bundle is indexed once; nested module binaries are not additional plugins.
Only the actually probed default audio class is represented. A multi-class module's
other audio/controller classes are **not** inferred to share its category or MIDI
capabilities. Additional class selection is not implemented by this change. The
reported class UID is retained as metadata, rather than replacing the established
path-based descriptor ID or changing saved project identities.

Old caches used `verified` to mean only a bundle/export was detected. That boolean
cannot establish current VST3 metadata. Legacy VST3 entries without either new
metadata or an explicit new scan failure are discarded on cache load, while old VST2
entries stay unchanged. A startup notice and a persistent manager/settings message
request a rescan. Completing a scan clears that notice. Current explicit failures
remain readable and actionable across restarts.

Serialization defaults permit old descriptor JSON to decode, but do not invent MIDI
capabilities. Saved project roles, paths, IDs and opaque states are not migrated.
The cache is still an index, not a file-integrity attestation: rescan after replacing
or upgrading installed plugin modules. This change does not pin a project's default
class against a vendor changing class order in a later plugin version.

## Bounds and cancellation

The UI owns a scan handle, while discovery and helper calls run on a background
thread. Dropping that handle signals cancellation; the worker discards partial results
and does not begin another probe after observing cancellation. No UI-thread join is
performed. This covers owner/application teardown; no separate Cancel button is added.

Each load attempt gets a 5-second response deadline, including the helper client's
slow-command deadline. The existing client may retry a **crashed** load once; timeouts
are not retried. Its existing shutdown has a bounded grace period before killing a
stuck helper, and its stdout reader has a bounded join/detach grace period. Cancellation
waits for an already-running bounded load/teardown rather than interrupting plugin
code in the application. A disconnected worker clears the busy state and retains the
previous index. These are helper bounds, not a promise about latency of arbitrary
filesystem/network mounts or OS process creation.

## Validation and boundaries

On 9 October 2026, the corrected production scanner was run against the genuine
[official Surge XT 1.3.4 Linux release](https://github.com/surge-synthesizer/releases-xt/releases/tag/1.3.4)
using the previously verified official archive. No plugin or factory content is
redistributed in this repository.

- Surge XT: `Instrument|Synth`, MIDI input true, output false.
- Surge XT Effects: `Fx`, MIDI input false, output false.
- Both returned `Surge Synth Team`, distinct actual class UIDs and successful isolated
  loads. Their descriptors survived a JSON/cache round-trip without changes.
- Both helper processes exited cleanly. This metadata rerun did not open a plugin GUI,
  access a physical device, process audio, or prove MIDI routing.

Regression coverage includes misleading effect/instrument filenames, exact token
matching, independent MIDI capabilities, missing categories, explicit failures,
legacy-cache invalidation, current cache round-trips, default-class requests, bundle
pruning, cancellation, and a trusted hung-helper fixture with a short test deadline.
Actual multi-class binary behavior, commercial plugin compatibility, native editors,
physical audio/MIDI and Windows execution require their own validation.

The final Linux source checks passed with the default test stack: the all-feature
application test binary passed **1,107 tests**, with **14 helper + 5 protocol** tests
also passing; the no-default application suite passed **1,103 tests**. Strict all-feature and no-default/all-target Clippy, formatting, the
app/helper build, Windows MSVC no-default/all-target **source cross-check**, and all
**167 Python harness tests** passed. The Windows cross-check is not Windows execution.
Final application suites were also run from exact copied test executables outside the
shared build target; no plugin fixture or backend replacement was used for the actual
Surge metadata result above.


## Integrated checkpoint

Exact merged source `e6bd216863f6180c2745de05c6701c6300d41cf2` independently passes
**1,107 app +14 helper +5 protocol all-feature tests**, **1,103 no-default app
tests**, formatting, both strict Clippy modes, build/helper smoke and Windows
MSVC source cross-check. The full UI suite passes **106 entries** (105 input
flows plus the timing-disabled benchmark entry), with **44 genuine Vulkan frames**.
The new production UI flow checks rescan notice, failure hover and completed-empty
scan behavior without loading a plugin. The final source-only receipt/packaging
regressions pass **177 Python tests** and all **72 explicitly packaged inputs**
have valid relative links and complete QA inventories.

The portable metadata probe was then compiled from this exact merged source using
its matching Cargo-resolved dependencies and freshly built helper. It loaded the
same official Surge bundles, rechecked category/event capabilities and cache
round-trip, then shut both helpers down cleanly. The helper hash still matches
the prior approved helper. This is a fresh metadata-only rerun; prior controlled
DSP and configured Stochas MIDI-generation evidence retain their original source
attribution. See [real-plugin receipts](REAL_VST3_VALIDATION.md). New Windows
execution and broader plugin/native/hardware acceptance remain separate.
