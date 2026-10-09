# Prepared callback-budget validation with genuine Linux VST3

This is a new source-bound QA generation. It does not modify or supersede the
historical e54a6e4 routing receipts. Initial harness compilation used18478d2;
qualification must bind the corrected, frozen commit in source-snapshot.json.

## Test path

The external builder copies the chosen DAW checkout and appends a test module.
It reuses the production Timeline/DspState/PluginChain/FixedQuantumAdapter path,
replacing only mock fixture construction with genuine official Stochas and Surge
loads. No manual helper-to-helper MIDI forwarding is used. Raw callback admission
runs before rendering, just as the production CPAL callback does. This does not
exercise a physical audio backend or qualify CPAL/hardware scheduling.

Every callback-budget profile is tested at48kHz/Q128:

| Budget B | Guard Q | Lookahead K | One physical-worker bridge L |
|---:|---:|---:|---:|
|128|2|3|512samples|
|256|2|4|640samples|
|512|2|6|896samples|
|2048|2|18|2432samples|

The16-case matrix covers every B with routed Stochas→Surge→Surge Effects and
ordinary timeline MIDI→Surge→Surge Effects, at fixed B and changing callback
partitions B,31,B/2,127. Each accepted measurement covers48,000frames,375quanta.
Every endpoint must return375−K exact plug-in output blocks and K startup blocks,
with no new deadlines, queue loss, latency drift, bridge gaps or processing faults.
Reported graph PDC must match3L+32 routed or2L+32 ordinary. Audio is sampled from
the already-rendered actual sink mixer track, separately from master.

Stochas uses our source-schema-validated/re-exported test pattern60/64/67/60.
The ordinary path uses our own C/E/G timeline notes beginning at sample6000; this
keeps stopped Retry at frame0 free of active-note chase and avoids confusing a
prior release tail with the measured fresh-note onset. Cold setup initially uses
blank Stochas and an ordinary timeline position beyond these notes, so a previous
held-note release cannot contaminate the new-epoch onset measurement.

Surge Effects lazily changes native latency0→32. The test records the real fence,
uses a newer prepared timing revision at a fresh epoch, verifies Retry remains
stopped, then explicitly starts again. B+1 raw callbacks must be rejected before
any worker submission for every profile. A deliberate unpaced callback burst tests
fault publication, silent fencing, cleanup and stopped-Retry/explicit restart.

## Reproduction

- Reuse the verified official plugin bundles under the parent validation directory.
  Their versions, URLs and actual module hashes are recorded in plugin-provenance.json.
- Generate the test/blank Stochas states with the earlier genuine-state builder.
- Set DAW_SOURCE to the frozen checkout and reserve the shared Cargo target.
- Supply the project's Cargo/toolchain/linker environment, then run
  python3 build_and_copy.py. This copies exact compiler-artifact executables and hashes
  them. No modification-time selection is used. Release the target after copying.
- Coordinate a quiet window before a qualification run. Run
  ./run_profiles.sh run001-quiet
  Each run gets a unique directory; the script refuses to overwrite an old run.
- To test a declared bounded CPU-load condition, use NATIVE_CPU_LOAD_THREADS=4 and
  NATIVE_TIMING_CASE to select a case; retain its exit status, raw log and counters.
  It is a separate condition, never a replacement for a failed quiet run.

The callback runtime includes the harness's output-copy closure. Worker/helper CPU
execution time is not directly instrumented. The exact-output, deadline, overflow,
sequence, epoch, latency and fault-reason telemetry is the delivery evidence.

## Limits

Passing controlled headless tests does not guarantee hard real-time behavior.
Every failed run remains in the QA record. Native editor resizing has separately
shown346.9ms processing wait versus29.8ms baseline; this buffering cannot hide it.
Native-editor real-time qualification remains blocked, and ownership extraction
work is outside this timing source checkpoint. No Windows or Harmony Blueprint
compatibility claim follows from these Linux tests. Plugin binaries and factory
assets will not be included in the source-only publication package.

## Retained observations before the next corrected source

- run001-quiet-f164 used an incomplete external callback wrapper: its missing
  pre-admission PDC refresh did not reproduce CPAL's control order. Its graph
  outcomes are setup failures, not endpoint timing qualification. The independent
  fresh-instance Surge state/first-note proof passed and is retained separately.
- run002-quiet-cpal-order corrected that wrapper without changing production code.
  Ten of sixteen paced cases passed. Three had actual classified DeadlineMiss
  failures (B128 routed fixed, B128 ordinary changing, B256 ordinary fixed).
  Three B2048 cases stalled during cold setup with no submissions and no fault.
  All four B512 route/ordinary/fixed/changing cases passed this controlled run.
- run003-f164-cold-trace added read-only identity/suspension diagnostics. All four
  B2048 configurations reproduced a production classification hole: native FX
  latency becomes32 after activation, the first cold callback clears timeline
  revision/epoch with one timeline execution failure, and processing remains
  suspended/Priming with no plugin fault. Every endpoint submitted zero blocks.
  The source owner and independent review confirmed this requires a source fix.
  This trace was diagnostic, with concurrent GUI work permitted, not quiet pacing
  acceptance. It does not explain away the actual lower-profile deadline failures.
- The original one-second overload cleanup observation had zero source MIDI but
  nonzero isolated post-FX release audio. Since the real plugins report a two-second
  tail, it does not establish a hung voice. The next harness observes four seconds,
  saves WAVs and quarter-second peak/RMS before assertions, and checks the last second.

A generated receipts/run-index.json indexes all retained attempts. Subsequent
successes never replace earlier failures. Raw source manifests, harness text and
executable hashes bind each attempt. Locally archived executables remain outside
any source-only publication bundle. Maximum callback runtime includes the test
wrapper; callback-start lateness measures this host's pacing schedule, not physical
CPAL or device latency and not native DSP execution time in isolation.
