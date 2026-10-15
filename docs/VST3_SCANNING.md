# VST3 discovery and loaded identity

## Ordinary scans collect a module catalog

Citrus probes VST3 modules through its packaged `vst3-host-helper`, located beside the
application. The background scan does not load third-party code in the application or
on its audio callback. Ordinary UI `start_scan` and synchronous
`scan_catalog_checked` query factory declarations without instantiating an audio
component, controller or editor, or requesting plugin state, MIDI or DSP. Loading
the module and calling its entry points/getters still executes third-party code;
process separation is not a security sandbox.

Each module keeps its established path-based descriptor ID and one index entry.
Its factory/classes are stored separately in `vst3_catalog`. A catalog-only entry
has `verified=false`, no loaded-default metadata, category `Unknown` and unknown
runtime MIDI/audio/editor capabilities. Even an `Instrument` declaration cannot
identify which class the runtime default LOAD will select. No declaration,
including the first audio class, is promoted into verified runtime metadata.

Catalog-only modules can be chosen in both Channel and Mixer pickers. Browser
selection opens an explicit target choice instead of interpreting Unknown as an
effect. A chosen module loads through the original default `class_uid=None` path;
its persisted Project role remains Unknown. A successful user LOAD does not
rewrite that role or upgrade the scan cache.

## Loaded-default verification remains explicit

Existing `scan_checked` and `scan` retain their separate default-audio-class
verification contract for strict consumers. They instantiate that class and read
its actual factory/class and event-bus metadata. They are not a fallback from a
failed catalog scan. Classification of this actual metadata retains these rules:

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

A successful default-class verification can still have unknown classification.
An explicit metadata failure remains unverified with its actual error; Unknown
alone is not evidence of failure or successful runtime validation.

For the new catalog-to-export path, Source77 retains session-only identity from
the user's actual default LOAD, full prepare, initial timing and active event-bus
query. Unknown can enter Whole Review only with a current, callback-installed
Instrument and MIDI-input identity bound to its original control owner. Query
failure leaves ordinary loading usable with an export-ineligibility reason.
Stopped state capture and strict clone validation still determine the actual
rendered class/state. This bridge does not change selected-conversion gates or
the existing classified-Instrument export contract. Current execution scope is
recorded in [Windows melody workflow](WINDOWS_MELODY_WORKFLOW.md).

VST2 filename/export behavior is unchanged by this slice. VST2 MIDI capability discovery
is not added.

## Class identity, bundles and old caches

A `.vst3` bundle is indexed once; nested module binaries are not additional plugins.
The catalog preserves all returned declarations and their order, but exposes no
new runtime class selector. Explicit loaded-default verification represents only
the actually loaded audio class; other classes do not inherit its capabilities.
Actual class UID remains metadata rather than replacing the path-based descriptor
ID or changing saved project identities.

Old caches used `verified` to mean only a bundle/export was detected. That boolean
cannot establish current VST3 metadata. Cache loading accepts valid catalogs,
actual default-class metadata and explicit scan failures. Other legacy VST3
entries request a rescan; old VST2 entries stay unchanged. A startup notice and a
persistent manager/settings message request a rescan. Completing a scan clears
that notice. Current explicit failures
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

Each probe gets one five-second write/read/decode exchange window after spawn.
Neither catalog nor explicit default-class probes replay a failed request or
fall back to another class. Cancellation wakes the owned exchange and then
retires that same helper. Cleanup retains the original 250ms graceful-exit and
2250ms cleanup deadlines; Pending keeps ownership of the actual process/pipes.
A later scan advances that same abandoned cleanup before another spawn.

Publication requires natural successful exit, actual reap, completed pipes and
no cleanup error. A single metadata-query failure with fully healthy retirement
is retained as Unknown/unverified with `scan_error`, and the walk continues.
Cancellation, Busy, Pending, kill, abnormal exit or unclean retirement stops the
scan without replacing the old index, rescan flag or disk cache. The manager
keeps the actionable stopped reason. Filesystem/network mounts and OS process
creation have their own latency; the protocol window is not a total scan-time promise.

## Current Windows evidence

Source76 passed 32 factory regressions and 2344 root tests with 12 ignored in
cloud Windows checks, plus strict Clippy, both binaries, protocol/no-feature
checks and the complete preview ZIP/extracted-helper smoke path. The independent
native paint check still fails PrintWindow on the cloud runner; that result
does not determine local Kontakt visibility.

Source74's actual factory-only Kontakt 8.4.1 query returned two declarations and
retired its helper naturally. Source78 later completed a real ordinary GUI scan
with nine published entries. Kontakt retained Unknown/unverified/no runtime
metadata and the two real declarations. The separate ignored production-catalog
QA selector was not executed; the older component-scanning ExitDll failure
remains separate from this UI catalog result.
Source77's initial Windows build passed 17 new App/Whole/restoration/eligibility
cases. All three migrated Windows/Unix backend cases subsequently passed in both
cloud jobs. The Source77 preview completed 2364 root passes with 12 ignored, then
failed strict Clippy on the large `GeneratorReadiness` variant. Its quality job
failed first on the fresh-source guard's exact retained-worker assertion and
finished with 2162 passed, 137 failed and 12 ignored, including later sticky
fixture refusals. These failures remain open pending the repair candidate's
actual checks; there is no Source77 release/ZIP or new NI qualification.

Source78 subsequently passed 2364 root tests/12 ignored and strict Clippy in both
cloud lanes; quality also passed 32 factory and all backend/build gates, failing
only independent native PrintWindow capture. Its preview release/ZIP and
extracted-helper smoke passed. A real UI trial then found the new catalog's
explicit experimental fresh-source LOAD still demanded scanned runtime-class
metadata. Its rescan hint cannot resolve that requirement. Actual None-class
loading must establish the class before full fresh preparation; a catalog entry
cannot supply it. The source-only Source79 candidate implements this handoff
but still needs compilation, regression and native checkbox acceptance.
An ordinary Source78 None-class session separately loaded NOIRE Pure, edited it,
wrote eight notes, normally saved and naturally retired App/helper with exit0.
That does not qualify the fresh checkbox, cold reopening, automation or audio
export. See [workflow evidence](WINDOWS_MELODY_WORKFLOW.md).

## Historical loaded-default validation

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
