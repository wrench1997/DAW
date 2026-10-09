# Measured mixer meters

The mixer strips display measured stereo sample peaks from the realtime mixer graph. The previous Project `peak` demo value and transport-driven sine animation are no longer used by the UI. Meter activity does not change the Project, undo history or saved music data.

## What is measured

- Each strip reads its actual stereo bus after insert effects, volume, balance pan and mute/solo gating.
- MASTER reads the same post-fader bus before the existing `tanh` output protection, device mono fold-down and output-format clamp. A red CLIP latch means the measured bus reached or exceeded 0 dBFS; it does not assert that protected device output clipped.
- Internal buses remain stereo even with a mono output device. A mono source is expanded by the existing audio renderer before measurement.
- Pre-effects/pre-fader sends can remain audible while their source's post-fader meter reads zero.
- These are sample-peak meters, not RMS, integrated loudness/LUFS, true-peak or hardware loopback measurements.

The bars use a −60 to 0 dBFS scale. The number is the larger L/R sample peak in dBFS, including positive overload values; exact silence displays `−inf`. A white marker holds each channel's peak for one second. CLIP is latched until the strip is clicked or its identity/device becomes unavailable. Clicking resets the hold/CLIP display without modifying audio controls. An unavailable measurement displays a dash with no active bars; non-finite observed data displays FAULT rather than an invented level.

## Realtime and identity contract

`audio_meter.rs` reduces complete post-fader graph buffers into fixed-size stereo peaks. The callback does no meter-related allocation, locking, waiting, I/O, or per-sample atomic operations. A four-entry bounded SPSC carries snapshots to the UI. If full, a callback-local accumulator keeps the maximum of each channel until the next successful publication, retaining short peaks without blocking audio. A per-strip reset generation is acknowledged at the next render-segment boundary; one atomic revision read per segment prevents pre-reset queued/coalesced overloads from relatching after a click.

Every snapshot carries the exact graph revision, transport epoch, graph fingerprint, stable Mixer track IDs and runtime slots. UI display order is never used as audio identity. A replaced graph/epoch or recycled slot cannot inherit another track's values, hold or clip latch. Generation changes discard pending old-generation measurements. Compatibility playback without an exact stable graph binding reports unavailable rather than guessing per-track identity.

The visible Mixer requests a bounded 33 ms UI refresh even when transport is stopped, MIDI is disconnected and no other transient UI work remains. Other views retain their existing idle cadence. This keeps silent-device measurements current and checks stale displays within one UI interval of the 250 ms timeout.

Measurement is independent of the transport playing flag: the normal active graph and the existing paused live-MIDI downstream graph are measured. Paused/unrendered segments publish silence without reading stale track buffers. Device faults that invalidate the stream clear the display. If no fresh callback measurement arrives for 250 ms, the display and latches clear and callback-local historical peaks are invalidated; after a long UI/device gap, a new callback must be observed before queued history can appear as current activity. Restarted engines own fresh meter channels.

Peak values are maxima over blocks received since the prior UI observation. During queue saturation, that interval grows; this preserves a real transient but is not a continuous-time envelope. A stale/disconnected display intentionally discards old evidence instead of replaying historical signal as live activity.

## Verification scope

Focused automated tests cover:

- Stereo amplitude and dBFS conversion, exact silence, clipping, non-finite input and bounded display fractions
- Full-queue transient retention and generation replacement while the queue is full
- Stable-ID/runtime-slot remapping, graph revision/epoch replacement, peak-hold expiry, exact reset targets and reset-versus-full-queue/mid-block races
- Stale/no-device/backlogged snapshots and a new engine's empty meter state
- Actual `render_transport_chunk` PCM fixtures with mono and stereo sources, gain/pan, mute and paused silence
- Bit-identical rendered PCM with observation enabled/disabled across generated sinusoidal/impulse signals and irregular callback partitions
- Actual paused live-MIDI graph output and silent unrelated branches

These deterministic tests do not replace real-device unplug/replug, native GUI interaction, Windows CI or sustained hardware/audio-load acceptance. The implementation deliberately preserves the existing DSP and output protection behavior.

### Local evidence, 2026-10-09

The real Rust 1.99 Linux toolchain passed the no-default-features all-target typecheck. The final focused run passed 7 production meter tests, 9 meter-channel/display-state tests and the existing paused live-MIDI graph test extended with measurement assertions, all on the default test stack. No-default-features all-target Clippy completed with only the pre-existing Linux MIDI warnings and two inherited application fixture lints; it is not recorded as a denied-warning/full-platform pass. Formatting and whitespace checks passed. Windows all-feature and native GUI/device acceptance remain pending integration.

The idle-refresh follow-up also passed two headless UI/state regressions: stopped/no-MIDI/no-transient-work Mixer repaint scheduling (other views remain idle), sustained silent-device availability, and stale-bar clearing within one refresh of expiry. The follow-up's complete filtered run passed 74 meter-related tests plus the paused graph test on the default stack; diagnostic Clippy still adds no meter-related warnings.

### Integrated Linux evidence, 2026-10-09 04:18 UTC

Source `4045cb24da846057461dd6e464b1c90058a0835f` includes both metering commits, the idle-repaint follow-up and platform-correct MIDI compilation boundaries. The complete locked/offline no-default all-target suite passes **878 tests, zero failures/ignored** on the default test stack. Formatting, no-default all-target Clippy with `-D warnings`, and the application debug build pass. The current Linux configuration excludes VST2/VST3. Exact combined Windows and real GUI/device acceptance remain pending; the earlier 862-test Windows checkpoint predates metering. The UI regression includes repeated idle frames receiving genuine meter snapshots without a playing transport.

### Windows integration and actual GUI probe, 2026-10-09 04:36 UTC

[Windows quality run 37883317515](https://github.com/wrench1997/DAW/actions/runs/37883317515) at `0b9cfa1a5d7e8c3384bc3ac435bcedc834c005af` passed **880 all-feature/all-target Rust tests**, fmt, strict Clippy, app/helper builds, 14 Python harness tests, actual helper protocol smoke and no-default all-target check. The independent static-CRT lane also passed these Rust gates and its optimized build, then failed package auditing on a Windows OS import; it does not establish a distributable archive.

An actual Linux `4045cb24` launch created an X11 native window but failed to present the UI: Mesa reported `EGL_BAD_SURFACE` at `xcb_shm_attach_checked`. Repeated supported renderer probes did not obtain a visible interface. Meter GUI interaction and create/edit/save/reopen/export flows are **BLOCKED / NOT RUN**; no physical audio device was present. Deterministic callback/UI-unit evidence remains distinct from these pending checks.
