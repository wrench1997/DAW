# Display-independent app UI regression tests

The tests in `src/app/headless_ui_tests.rs` run the production `CitrusApp::ui` through
`egui::Context::run_ui` and eframe's provided test context/frame. Controls are found
by their actual accessibility labels and layout bounds, then operated using real
pointer press/release or key press/release events. They do not reimplement dialogs
or shortcut policy in a separate test model.

The startup seam shares the normal app constructor while suppressing audio/MIDI
device discovery, stream creation, and user-profile paths. Tests retain production
paint generation, application state, background directory/WAV workers, project
history, and modal/shortcut handling. The harness prevents periodic MIDI discovery
and never invokes native file pickers or device Apply/Refresh controls.

Run the focused suite with the normal Rust/ALSA build prerequisites:

```sh
cargo test --locked --no-default-features --bin citrus-studio app::headless_ui_tests
```

Coverage:

- Playlist, Channel Rack, Piano Roll and Mixer keyboard navigation
- All six Settings pages via actual buttons; F10/Escape dismissal, reopening,
  and preventing workspace shortcuts behind Settings
- File menu → WAV export → level choice → Review → Back → Close/Escape;
  canceled settings reset, no worker starts, and project data stays intact
- File menu → Project media → Close/Escape and reopen; underlying shortcuts blocked
- Browser text-input focus blocks workspace shortcuts; filtering disables hidden
  selection import; Refresh and Up clear selection and navigate normally
- Export review footer stays within the 1080 × 680 logical app viewport
- Sounds browser disabled controls without a folder/selection; actual bounded
  directory scan; failed WAV import; successful import after failure; one-step
  Undo/Redo; dirty New-project Cancel/Escape preserving the imported project

The temporary WAV fixture is supplied at the native folder-picker return boundary.
That boundary injection does **not** test the platform file picker. The listing,
selection, import button, decoder worker, app commit and history operations after
that boundary are exercised normally. Existing export UI tests additionally cover
format dropdown interactions and the modeled native Save-cancel return boundary.

## Acceptance boundary

These are display-independent input/layout integration regressions. They do not
validate native desktop presentation, OS window integration, native file dialogs,
physical audio/MIDI hardware, plug-in editors, or perceived sound. A Linux pass
covers shared app code running on Linux; it is not a Windows execution result.
No X11 connection, compositor, virtual display, system installation, or permission
change is required.

## Optional Vulkan-only offscreen images

`CITRUS_UI_CAPTURE_DIR` opts the same tests into the existing egui-WGPU renderer.
It creates a Vulkan instance with no display handle and requests an adapter with
no compatible surface. The real app's tessellated shapes/font textures are drawn
into an RGBA8 texture, copied back from WGPU, and written as lossless RGB PPM files.
The renderer's predictable single-sample/no-dither mode is used. These images are
**offscreen app renders**, not native desktop screenshots.

```sh
CITRUS_UI_CAPTURE_DIR=/absolute/path/to/ui-captures \
  cargo test --locked --no-default-features --bin citrus-studio \
  app::headless_ui_tests -- --test-threads=1 --nocapture
```

An already-installed Vulkan implementation is required only for the explicit
capture mode. If necessary, select an installed ICD using the normal process-local
`VK_ICD_FILENAMES` setting. The tests neither install a driver nor alter graphics
or security settings. A requested capture fails loudly if adapter/device/readback
fails; it does not silently substitute mock pixels. Adapter details are saved
alongside the images. PPM output needs no image-encoding dependency and can be
losslessly converted to PNG with an existing image tool.

Capture checkpoints include the Playlist, Mixer, six Settings pages, WAV review
at regular and minimum app sizes, Project media, failed/successful WAV import,
and the unsaved-project Cancel dialog. Pointer/key and state assertions still run
in capture mode. Text layout and clipping can be inspected in these renders;
window presentation, desktop focus, native dialogs and audio hardware still need
separate manual acceptance.

## First Linux offscreen inspection (2026-10-09)

The six app-flow tests passed with and without explicit Vulkan capture. Fourteen
actual UI renders were captured using the already-installed SwiftShader CPU
Vulkan adapter. Normal frames are 1498 × 936 pixels from a 1440 × 900 logical
viewport at the app's 1.04 zoom; the minimum-size export review is 1123 × 707
pixels from 1080 × 680 logical points. The PPM-to-PNG conversion was checked for
identical RGB pixels.

Visual inspection confirmed legible normal-size Settings, browser import
success/error, media and unsaved-project dialogs, and fully visible export review
footer controls at the minimum size. It also identified unresolved pre-existing
issues; these are **not** a blanket visual acceptance pass:

- At minimum width, the main toolbar's Plugins button overlaps Mixer navigation,
  and Playlist group/snap controls crowd each other behind the export modal.
  Responsive toolbar layout needs a separate correction and recapture.
- The About page hardcodes “CPAL / Windows WASAPI” even on this Linux run.
  Its platform label needs correction; the harness opens no audio device.

The default demo arrangement is shown in overview renders. Import evidence uses
a deliberately tiny, known two-frame PCM16 fixture. Neither is evidence of
real-world playback, recorded audio, or hardware meter acceptance.
