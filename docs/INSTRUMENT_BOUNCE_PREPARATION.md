# Selected-instrument bounce: dormant preparation checkpoint

Status: **UNVALIDATED SOURCE CHECKPOINT**. This branch starts from published
`dcdb2c14809b5e6f34af7db071db36f763d757f5`. No new App action is enabled. There is
no WAV renderer, project replacement, freeze, or complete-song plug-in export.
Existing whole-song WAV export refuses active VSTs exactly as before.

## First checkpoint

- `instrument_bounce.rs` prepares an immutable selection and owns one bounded,
  private capture receipt. Admission and completion require the same project
  fingerprint/session, selected channel/instance, endpoint, Timeline revision and
  stopped epoch, with no editor, edits, recording, MIDI or session transition.
- The current conservative eligibility rejects enabled nonempty automation,
  including Tempo; configured MIDI routing; missing/ambiguous placements;
  bypass/partial wet/muted source; Timeline diagnostics; unsupported rate/length.
  It compiles the original Project, retaining channel ordering. It does not
  strip unsupported content and present the result as a full mix.
- `PluginChainControl::request_instrument_bounce_state` uses one dedicated
  bounded request/reply. It does not use the regular SaveState publication,
  `RuntimeEvent::State`, or `NativeEditorSnapshot` capture serials.
- The worker requires the existing epoch to be quiescent. It does not perform
  the normal tagged-save catch-up reset. Open/pending editor, pending parameter
  receipt, existing slot fault, queue-full and stale epoch reject explicitly.
- VST3 capture queries the closed editor and durable native generation/revision,
  uses the production zero-sample SaveState flush, checks a freshly read catalog
  and reads current values for exactly the stored generic base IDs. Generation,
  revision, metadata and epoch are checked again. Nonempty state is mandatory;
  there is no default-preset fallback. Snapshot bytes/bases are never copied into
  the source Project by this path.
- Capture rejection is separate from a newly observed backend/IPC failure. The
  latter remains a normal visible source-worker fault. This is not a guarantee
  that arbitrary vendor `getState` implementations are side-effect free.
- `offline_instrument.rs` constructs a thread-affine fresh isolated VST3 owner
  through the existing backend/helper load route. It pins the actually captured
  factory class, verifies class/version, selects `kOffline` before restore/start,
  replays captured parameter bases, and validates current metadata/buses again.
  No editor is opened, no source LoadState occurs, and no positive warmup/reset
  or fresh re-save is used as a state-settlement claim.
- Actual instrument/MIDI capability metadata is required. No audio input and
  exactly one active mono/stereo output bus are accepted; even inactive extra
  buses are rejected in this first slice. Missing metadata never becomes zero.
  Prepared maximum is 2048. Future musical rendering is Q128, preserving the
  existing exact Surge 1.3.4 reset256 rule for clone cleanup when it is added.

## Deliberate pending work

App capture-fence integration, production note-ID/MIDI mapping, synchronous Q128
rendering, post-Process drift rejection, latency trimming, explicit fixed-tail
review, cancellation during helper setup/render/teardown, existing atomic WAV
writer reuse, and the user-facing review/action remain separate reviewed stages.
The existing source remains installed; the eventual operation is a pre-Channel/
Mixer instrument WAV, not a full mix or automatic project edit.

The preparation bound is 8000–192000 Hz, a requested tail of 0–30 seconds, no more
than ten minutes of song and 24 million total reserved frames including a maximum
one-second plug-in-latency allowance. Tail truncation consent and output-format
review are not implemented yet. Metadata is not binary authentication.

## Verification at checkpoint

Only source inspection/formatting and Git diff hygiene may have been performed
before this checkpoint archive. New tests have **not been executed**. The new
plugin-free test source covers immutable capture, exact current bases, source
identity drift, pending work/editor, queue-full and cancellation admission,
receipt timeout/disconnect, error/fault distinction and unsupported metadata.

No genuine plug-in, helper/native editor, OS device or WAV output has been run.
No previous source's receipts are attributed to this branch. Source plus Git
bundle must be durably saved and independently reviewed before long tests; native
plug-in qualification requires its own explicit gate.

## Pre-execution review corrections

The first frozen preparation checkpoint `d8c7bc0` was saved before execution.
Review found that a capture-time backend fault needed immediate reliable metadata
publication and that cached `PluginInfo` MIDI bits could not prove post-state bus
eligibility. Both are corrected in the next still-unvalidated source checkpoint:

- Capture failure republishes active-mask/latency/MIDI capability before its
  private reply, even if the best-effort SlotFault event ring is full.
- An additive, fallible `EventBusLayout` command queries actual component bus
  count/info on the owner thread. Unknown/invalid/unsupported/dead/wrong replies
  reject; there is no cached capability fallback or automatic recovery. Existing
  commands are unchanged. Activation is attested only by a matching successful
  host activation record, never by `kDefaultActive`; otherwise it is unknown.
- Bounce requires exactly one currently attested active main event-input bus and
  zero event-output buses, including inactive outputs. Enumeration is repeated
  after mode/state/base application and at both source capture fences.
- Added plugin-free component and line-protocol stand-ins exercise the actual
  new capture/factory sequence, post-state topology changes, unknown activation,
  missing parameter keys, empty state, and generation/revision changes. Those
  tests load no vendor plug-in or production helper. They are unexecuted here.

The dormant caller-supplied source stamp still requires an App adapter that binds
the control to the exact project-session/channel/endpoint probe and revalidates
that probe on completion; same-instance replacement must not pass by ID alone.
App integration is intentionally not enabled by this checkpoint.

## Strict audio-bus review correction (still unvalidated)

Review of `e0ea8aa` found that ordinary `AudioBusLayout` describes prepared buffers,
not a fresh component enumeration. The additive `CheckedAudioBusLayout` query now
checks actual audio count, index, channel count and main/aux descriptor against
those prepared buffers. The prepared main/aux descriptor is recorded from the
same SDK read that established each buffer's channel count; unknown fallback
metadata cannot attest consistency. The ordinary prepared-layout API is unchanged.

A non-draining restart flag observation rejects pending I/O/reload both before and
after enumeration. Neither source capture nor candidate admission services that
work, reconfigures buses, stops/resets the source, or infers active status from
bus default flags. The synchronous factory uses this strict query after state,
Start and parameter-base application. Later per-block rendering guards must use
the same query; no rendering method exists in this checkpoint.

A bounded `MetadataQueryRejection` enum distinguishes explicit Unsupported,
PendingTopology and InconsistentTopology refusal from faults. Only the new typed
response may produce harmless rejection. Generic remote Error (including an old
helper), wrong/malformed reply, SDK error, crash, timeout and poisoned state still
fail and fault the capture source. No failure can produce a valid layout, clear a
previous fault, or trigger automatic recovery. Fresh candidate failure disposes
only the candidate. Plugin-free tests cover Start/SetParameter/type changes,
unknown activation, pending/reentrant restart without flag drain, SDK errors and
protocol classification. All new tests remain unexecuted at source freeze.

The follow-on source correction enforces the offline owner's documented !Send/!Sync
boundary with a private Rc marker and compile-time negative-trait test. Failed fresh
candidate cleanup may issue the existing backend Drop's Editor Close; no Open/Focus
or source editor mutation is permitted. The protocol tests distinguish that teardown
from setup. Malformed audio layout/unknown rejection enums remain protocol faults.
