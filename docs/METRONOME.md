# Playback metronome

The compact `CLICK OFF` / `CLICK ON` button beside Record toggles the playback
metronome. New installations and older preferences without the field start Off.
The selected button and its text show the current setting; the tooltip identifies
it as the metronome. No keyboard shortcut is assigned.

The choice is an application preference stored with the existing versioned audio
preferences. It survives an app restart and audio-device replacement, and does
not edit a Project, add Undo history, or mark the song dirty. It can be changed
while stopped, paused, or playing. Enabling it while stopped does not start
playback. Enabling during playback retains the existing beat phase and waits for
the next beat.

## Audio and export boundary

`AudioEngine::set_metronome_enabled` publishes an `AudioStatus` atomic flag. It
does not use the bounded audio-command queue, request a Timeline activation, or
change the transport clock. The realtime callback checks the flag without locks,
allocation, filesystem work, or plugin calls. Off suppresses new clicks and clears
the active click envelope/oscillator phase when observed by the callback, including
paused or unavailable-Timeline segments that skip the normal source renderer.
The beat phase continues during ordinary playing source renders so a toggle does
not restart the beat clock.

The existing click enters the direct-Master source before its existing PDC and
Master effects. Switching it off does not flush shared audio buffers or effect
state: a click already submitted to PDC, plugins or the output device can still
drain through their latency or effect tails. Clearing those shared buffers would
also disrupt the music and is deliberately outside this control.

Realtime Master Capture records the live Master, including an enabled metronome.
Turn the click off before capturing a click-free performance and allow existing
latency/tails to drain. Offline arrangement WAV export takes only Project music
and never adds this application metronome, regardless of the toggle.

This change preserves the existing constant-tempo click implementation. Count-in,
record-only mode, configurable click sound/level, time signatures, downbeat accents,
and sample-accurate tempo-map click scheduling are separate work. It does not
change MIDI routing, worker timing, PDC, Timeline activation or project format.

## Regression coverage

Source tests cover:

- Actual production UI pointer toggles, including repeated changes during playback
- On and Off storage/reopen, migration from preferences without the field, and
  preference preservation across device-profile commits
- Unchanged Project fingerprint, dirty flag and Undo/Redo stacks
- Empty-source silence across a complete beat in both compatibility and activated
  Mixer-DAG renderers, audible enabled click, immediate source-envelope clearing,
  and no old click resurrecting on reenable between beats
- Clearing during a paused, activated-Timeline callback that skips source rendering
- Engine flag publication without transport or command-queue mutation
- Identical callback output/transport for one-frame, 137-frame and 512-frame
  partitions across on/off transitions
- Byte-identical silent offline WAVs while the app preference is Off and On
- Button/navigation layout bounds at the supported minimum and wider app sizes

## Executed checks, 2026-10-09 UTC

On the metronome source candidate based on routing checkpoint `e54a6e4`:

- Six focused metronome regressions passed.
- Linux no-default-features/all-target tests: **1,133 application tests passed**.
- Linux all-features/all-target tests: **1,137 application +15 helper +5 editor
  protocol +2 transport protocol tests passed**. No failures or ignored tests.
- Formatting, both all-target strict Clippy configurations, all-feature app/helper
  build, and actual helper protocol smoke passed. The existing upstream dependency
  deprecation warning was retained; project warnings were not relaxed.
- Windows MSVC no-default-features/all-target **source cross-check** passed. This
  is not a Windows executable, native Windows UI or device result.
- **167 Python tests passed**.
- Both the new pointer/persistence/export flow and responsive toolbar flow passed
  again with genuine Vulkan offscreen capture from the copied final test binary.
  Three renders were inspected: enabled at 1498 × 936 pixels and Off in minimum
  Playlist/Mixer layouts at 1123 × 707. Button text/state were visible and the
  transport/navigation controls remained separated. PNG RGB matched each readback.
- Independent read-only source review found no remaining blocker. Startup and
  replacement engine propagation were source-reviewed; the headless UI creates no
  audio device, so real-device preference restoration remains unverified.

These are deterministic source and display-independent app-input tests, not native
OS presentation, physical-device latency or listening acceptance. Callback changes
were inspected to contain only bounded scalar/atomic operations; no new allocator
or lock instrumentation was introduced. The application remains a prerelease DAW
foundation, not a commercially complete product.


## Combined integration

At `fcf57b0`, the metronome is combined with reviewed Linux native editors. Fresh
all-feature tests pass 1,137 app +21 helper +5 editor protocol +2 transport protocol;
no-default passes 1,133 app tests, with fmt, both strict Clippy modes, build, helper
smoke, Windows source cross-check and 195 Python checks. These are recomputed
aggregate results, not a sum of independently tested slices. Real native editor
processing-stall observations remain unchanged; a metronome toggle does not fix them.
