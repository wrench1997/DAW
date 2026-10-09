# Observed VST3 state-restore limits

## Current guard and remaining limits

Reviewed guard `c056dc5ce72a78bf1c4b32b5f831fe39303b6866`, integrated as
`60134a5cd668e90eb0801f6da4afc9aaca5abe1c`, now rejects unsafe state restore for
actual factory UID `ABCDEF019182FAEB566D624153675854` and exact version `1.3.4`.
Any attempted positive Process or explicit native editor open/attachment makes the
history ineligible, including failed attempts. Stop, close, reconfigure and mode
changes do not reset it. Zero-sample parameter flushes alone remain eligible.
Metadata chooses a compatibility policy; it is not binary authentication or
verification of every Surge build/platform.

The helper checks before editor closure, and the in-process implementation checks
before component/controller calls, lifecycle changes or queue/state mutation. It
returns an actionable fresh-instance error. The normal App paths use fresh plugin
instances. A safe replacement transaction must restore before playback/native
interaction and retain the old instance until candidate success; a stronger
transactional-retention handshake is separate pending work, not established by
this guard or the published App checkpoint. No hidden processing, sleep,
settlement retry, generic completion fence or helper ownership swap is implemented.
The legacy public `PluginChainControl::load_state` closes its native editor before
backend LoadState and faults its slot when the backend rejects that call. The current App does not call it;
direct helper preservation must not be claimed through that legacy API.

The exact helper `9dd18d74783e2ed13e947008d4dc4d5b2bdc0032948680c1cac678daed9aae60`
passed 12 genuine Surge cases at 17/47/128/256 frames: fresh restore plays its first
note immediately with matching component/getter values; used active/stopped loads
are rejected without changing saved bytes or lifecycle, and subsequent PCM works.
Independent X11 native QA restored −6.28 dB / normalized0.8691863417625427 on a fresh
instance. A different-state rejection kept the same XID, generation1, 1178×735
geometry, menu and value. Closed-but-previously-opened rejection and normal reopen
also passed. No new content/container mismatch was observed in this rejected-load
scenario; this does not erase the historical mismatch below. Stochas reused native
restore/re-export/repaint and the full trusted native fixture passed unchanged.
This native rejection run did not perform continuous note/audio processing.

A proposed 32-frame settlement experiment was rejected before commit: the native
preset worker can delay actual patch application even when Process succeeds.
It is not safe to treat a fixed block count as completion. The original positive
and negative traces remain historical evidence. No general reused-state recovery,
latency improvement, physical-device, Windows/macOS native or broad vendor
compatibility acceptance is claimed. Combined source gates for this integration
are pending; source-slice results are recorded in the work log.

## Historical ownership-preparation scope and identity

The following observations predate the guard and remain measured failures, not a
completed compatibility fix. The ownership-preparation
source is `81fe8bf7267e1affc54fafa45d7b345770837407`, integrated as `3fb549a`.
Its copied helper SHA-256 is
`cb1899fd7b768e61ab3b5de747e4236e250a0387762058b1db12f4fe2bd5ca08`.
The previous Linux editor helper is
`9cc00c81f07dc88c7514c8fb2b016cd1f080803fefbb281e76cc43b0a16a3ffd`.
The same reused-instance defect was reproduced on both helpers with official Surge XT
1.3.4, class `ABCDEF019182FAEB566D624153675854`. It predates this ownership change.

The native-edited input state is 51,929 bytes, SHA-256
`104bc8ab0945eda2b2873ac43da12dfff67f1e4cbb4863b6827a220740d35a42`.
Its component XML stores Global Volume −6.27905654907227 dB; the expected normalized
parameter `1336600346` value is `0.8691863417625427`. The state envelope contains
51,901 component bytes and no separate controller blob. State blobs and vendor
binaries are not distributed in the source package.

## Results that must remain separate

- **Fresh instance, restored before its first Process:** the immediate note sounds;
  native display, controller getter and component state agree. The independent GUI
  check retained the edited value through 2,829 subsequent processing blocks,
  native repaint and another save. This bounded fresh-instance path passed.
- **An instance that already processed, active restore or stopped/restore/restart:**
  the first immediate note produces no PCM over the diagnostic's 4.096 seconds of
  rendered audio plus 200 ms wall wait. A subsequent note sounds. Both historical
  helpers reproduce this; a successful LoadState response does not prove that the
  first following musical event is safe.
- **A positive silent block before that note:** the note loss is avoided, but the
  host getter remains at the old `1.0`. This is not complete restoration acceptance
  and is not prescribed as a general-purpose VST3 warmup workaround.
- **Component state versus controller state:** immediately after reused-instance
  LoadState, the getter and saved component XML are old. After one positive Process
  block, the XML correctly stores −6.27905654907227 dB, but the getter remains `1.0`.
  The component state after positive processing matches across the active, stopped
  and fresh headless cases. The DSP volume is not permanently lost.
- **Independent genuine native observation:** after reused-instance restore and
  3,828 zero-input blocks, the vendor's native menu displays “Edit Value: −6.28 dB”
  while a simultaneous host query remains `1.0`. The second state contains the
  correct volume. No notes were sent in this visual diagnostic; its zero PCM is
  expected and must not be labeled as another audio failure.

The initial interpretation of a permanently lost processor volume was corrected by
component XML and actual native-menu evidence. **Reused-instance first-note and
host/native parameter consistency acceptance remain failed.** No silent settle
block, sleep or replacement transaction was implemented in that
checkpoint. The later guard above refuses the characterized unsafe path rather
than reporting it as a successful restore.

## Historical accepted-load ordering and observed mechanism

The helper acknowledges LoadState after synchronous component restore, immediate
controller synchronization, metadata/bus rebuild and restoration of the previous
lifecycle. Surge defers a used-instance patch until a positive processing block.
Its first sample can consume an offset-zero note before applying that patch, whose
voice reset removes the note. Immediate controller synchronization sees the old
processor model. The existing zero-sample native flush does not enter that positive
sample path and cannot establish completion of this particular deferred operation.

A future successful used-instance restore still needs an explicitly bounded
compatibility policy or
replacement transaction, coherent generation/state fences, no leaked musical input
or feedback, correct controller synchronization, preserved transport/lifecycle and
fail-closed error paths. It must test the first note and immediate GetParameter and
SaveState without caller-provided warmup. The rejection guard does not claim that
such a successful restore transaction exists or that every VST3 plugin requires
positive processing during restore.

## Native editor and performance boundaries

The ownership-preparation source-built fixture passed actual paint/input, focus/key press and
release, factory/frame callbacks, callback retirement, fresh-instance save/restore,
native close/reopen and process cleanup. Genuine Stochas native cell editing,
state re-export, fresh-instance restore and repaint also pass. Separate unpaced
headless checks cover Stochas MIDI/state and Surge PCM/reconfiguration, with the
Surge settle blocks explicitly disclosed; these are functional checks, not an
audio-device deadline measurement or a new routed-graph acceptance matrix.

A reused-state operation also left **913×569 plugin content inside a 1178×735 host
area until detach/reopen**. Full container/content resize consistency is not accepted.
The historical **346.9 ms processing-request stall during native resize** remains a
limit. [Ownership preparation](PLUGIN_PROCESSOR_DOMAINS.md) keeps processing and GUI
on one thread; it does not improve latency or enable a DSP worker. Hardware,
mixed Wayland/XWayland, fractional DPI, sanitizer coverage and broad vendor state
compatibility remain separate acceptance work.
