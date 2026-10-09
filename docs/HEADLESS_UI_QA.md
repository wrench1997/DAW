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
footer controls at the minimum size. It also identified pre-existing issues at checkpoint `cb49961`, corrected in the
follow-up described below. This is **not** a blanket visual acceptance pass:

- At minimum width, the main toolbar's Plugins button overlaps Mixer navigation,
  and Playlist group/snap controls crowd each other behind the export modal.
  This was recorded before the responsive-toolbar correction.
- The About page hardcodes “CPAL / Windows WASAPI” even on this Linux run.
  This was recorded before the runtime/platform-label correction; the harness
  opens no audio device.

The default demo arrangement is shown in overview renders. Import evidence uses
a deliberately tiny, known two-frame PCM16 fixture. Neither is evidence of
real-world playback, recorded audio, or hardware meter acceptance.


## Responsive-layout correction and recapture

The follow-up keeps all transport/navigation actions visible. Below 1240 logical
points the top toolbar uses two rows. Workspace tools and the Playlist navigation
row wrap their bounded trailing control groups instead of allowing right-aligned
children to paint over earlier siblings. A wide workspace still keeps tools and
Snap together on one row. Custom navigation and plugin-manager buttons now expose
real accessibility labels, also used by the pointer-event regression tests.

The About page displays the build platform and formats its backend from the
observed running profile's host. Without an engine it explicitly says
`CPAL / offline (no active stream)`; it no longer assumes Windows WASAPI on Linux.
The presentation helper performs no device enumeration.

Verified against the final follow-up source:

- Linux no-default-features / all-target Rust: **910 passed, 0 failed, 0 ignored**
- `cargo fmt --all -- --check`: passed
- Linux no-default-features / all-target Clippy with `-D warnings`: passed
- Windows MSVC no-default-features / all-target **cross-check**: passed; not a
  Windows execution, binary, native UI, or hardware result
- Explicit Vulkan capture run: **8 app UI tests passed**, 17 genuine offscreen
  render checkpoints; no mock pixels or display surface
- At 1080, 1240, 1280, and 1440 logical-point widths: actual navigation/Plugins
  bounds are inside the viewport and pairwise non-overlapping; Group/Snap bounds
  are separate; clicks open Mixer/Plugins/Group and select a different Snap value
- Linux About UI explicitly shows `linux`, the offline state, and no WASAPI claim

Before/after pixel inspection confirms the minimum-width Plugins/Mixer overlap
and Group/Snap crowding are gone; normal-width layout remains compact. The
minimum-size export review footer remains fully visible. Minimum-size Playlist
and Mixer captures are 1123 × 707 physical pixels at the existing 1.04 zoom.
All earlier desktop/dialog/device acceptance boundaries still apply.

## Integrated checkpoint, 2026-10-09 04:58 UTC

Reviewed commits `cb49961` and `493cc89` integrated cleanly as `1663f8f` and `dda0ccadbb5e7351889f9d085727c9e344ff6119`. The complete combined Linux no-default/all-target suite was rerun: **910 passed, 0 failed, 0 ignored**, default test stack; formatting, strict Clippy and app build also pass. No source conflict or extra executable change was introduced. The 17-image Vulkan capture evidence above belongs to the reviewed feature source; integration preserves it but does not assert a new native desktop capture. Fresh Windows execution for the integrated candidate remains pending.

## FL-inspired visual refresh

The native workstation design and its additional paint/pointer regressions are
summarized in [FL_INSPIRED_NATIVE_THEME.md](FL_INSPIRED_NATIVE_THEME.md). The capture
suite now also includes `decoded-waveform` and `decoded-waveform-minimum-window`:
these import and decode a deterministic four-second PCM fixture through the normal
worker, rather than drawing an invented wave. The small two-frame fixture remains
in the import-recovery tests for its original purpose. Fader/pan pointer tests call
the production controls and compare actual painted geometry with interaction state.
All existing native-desktop and hardware acceptance boundaries still apply.

## Native editor inspector layout regression

The production-app harness reproduces clipped generator actions and Mixer action/status
crowding in the FL-inspired checkpoint `76232d9`. The generator action strip now wraps;
the Mixer puts status on its own row and wraps its actions. Runtime commands, native
owner validation, automation exclusion, snapshot/save barriers and plug-in loading
are unchanged.

The regression exercises first-frame layout at the inspector's configured 240 and
340 logical-point widths, then settled layout, for both Channel Rack and Mixer.
It checks action bounds, nonoverlap with siblings/status, supported/open/closed,
pending, unsupported and no-editor presentations, and real Replace/LOAD pointer
clicks followed by Escape and reopening. Project data stays unchanged and no
runtime chain is created. Existing ordinary inspector controls impose a settled
content minimum of about 258 points for Rack and 270 for Mixer when 240 is requested;
the test checks the first requested-width frame as well as that settled layout.
This correction does not claim that unrelated inspector controls now resize to 240.

A `cfg(test)` presentation-only snapshot exposes the Windows-specific Open/Close
controls on Linux. Only the two inspector render sites consume it. Native commands
and persistence/topology predicates still use the real runtime snapshot accessor;
the regression explicitly confirms that it returns no native runtime state. The
fixture is labeled `UI test only` and is never loaded from disk. These controls are
not evidence of native Windows editor execution or Linux editor support.

Capture mode adds four genuine offscreen frames named `native-generator-inspector-*`
and `native-effect-inspector-*`; their 240/340 suffix is the requested panel width.
The complete capture suite passes 12 tests and emits 23 real app renders, including
all existing navigation, modal, Browser and decoded-waveform checkpoints. All native
desktop, file-dialog, hardware and real vendor plug-in acceptance limits still apply.

## Combined visual/editor checkpoint, 2026-10-09 05:58 UTC

The reviewed visual refresh and inspector correction integrate as `76232d9` and
`37bfabab8c9a6f6c065702aa6ebfa5a7f3eb04a4`. The full combined tree was rerun:
943 application  + 14 helper  + 5 protocol all-feature tests, 941 no-default application tests,
fmt, both strict Clippy configurations, app/helper build and ordinary helper smoke pass.
The final combined capture rerun passes 12 actual-app tests and generates 23 genuine Vulkan
frames; every PPM-to-PNG conversion is checked for identical RGB pixels. Independent
integration review confirms native command/ownership and shared import-transition guards
are unchanged. This still does not establish native desktop/plugin or hardware acceptance.

## Simultaneous native editor workspace

The app now defaults to four simultaneous internal editor windows. The earlier
single-editor flow tests explicitly use the supported maximized layout; new tests
exercise the production floating default. [MULTIWINDOW_WORKSPACE.md](MULTIWINDOW_WORKSPACE.md)
describes the real pointer/keyboard, interrupted-gesture, modal stacking, shared
history, persistence and small-window coverage. Optional Vulkan capture includes
`multiwindow-workspace`, `multiwindow-moved-resized`, `multiwindow-shared-pattern-edit`
and `multiwindow-minimum-window`. These are genuine offscreen application renders,
not native desktop screenshots or detached OS-window evidence.


## Bounded Piano clipboard regression

The session-local [Piano clipboard](MULTIWINDOW_WORKSPACE.md#piano-note-clipboard)
adds ten production-app flows: semantic and raw shortcut deduplication; exact
platform CopyText output; actual Copy/Cut/Paste buttons; target-channel select-all;
source channel/group/time/velocity/mute preservation; PAT snap-down and SONG-zero
anchors; deterministic repeated paste; empty/malformed clipboard; shared one-step
Undo/Redo and preceding-edit boundaries; floating focus, hidden windows, modal,
Browser text and actual Tempo numeric-text isolation; real note drag interruption;
project reset, missing channels and snapshot barriers; minimum-size button bounds.
Four data regressions also cover payload/schema/size bounds, invalid numeric/channel
values, global note identity collisions, isolated group allocation and linear
selection expansion at the full 65,536-note limit.

The final isolated source passed 966 no-default application tests and 968 application
+ 14 helper + 5 protocol all-feature tests, both strict all-target Clippy modes,
formatting, all-feature app/helper build and Windows MSVC no-default/all-target
cross-check. Python discovery passed 167 tests. A fresh final Vulkan run passed all
30 production-app UI flows and generated 30 genuine offscreen frames, converted to
RGB-identical PNGs. New checkpoints are `piano-clipboard-pattern-paste`,
`piano-clipboard-song-pattern-start` (1997×1123) and
`piano-clipboard-minimum-floating` (1123×707). Inspection confirms the clipboard
controls and visible anchor stay within their Piano window at both sizes.

These tests inspect actual egui platform clipboard commands and inject semantic
clipboard events; they do not perform an OS clipboard round trip. Windows runtime,
native desktop focus/window presentation, external MIDI clipboard exchange and
physical audio/MIDI acceptance are not established by these checks.


## Combined compact workspace and clipboard validation

Exact integrated source `8d6b54c11fe07386b105de8d41e209bb0f45af5e` passed the complete
production-input capture suite: **36 harness entries**, comprising 35 flow/input
checks plus the included opt-in CPU benchmark entry. Timing mode was not enabled
in this capture run. All 33 Vulkan PPM frames were converted to RGB-identical PNGs.
The ten clipboard flows and five compact interaction flows run with the twenty
preceding flows; the full aggregate counts are executed results, not summed claims.
The merged wide/minimum layouts retain accessible wrapped clipboard controls.

The independent controlled motion benchmark and its mixed debug-CPU result are
recorded in [COMPACT_WORKSPACE.md](COMPACT_WORKSPACE.md). Neither that measurement,
the app footer nor offscreen captures establishes physical desktop smoothness or
native OS clipboard behavior. Existing native paint acceptance remains failed.


## Combined Piano keyboard and mouse composition validation

Exact source `43fc5a388a292cd15503cc3759c24ba420fcb7f4` passes **64 harness entries**
(63 production-input/flow checks plus the opt-in benchmark entry, timing disabled)
and produces **37 genuine Vulkan frames** with lossless RGB-identical PNG conversion.
The new six keyboard and 22 pointer flows execute with all preceding workspace,
clipboard, inspector, modal and import flows. In particular, production Undo/Redo
waits for active/interrupted held gestures but ordinary released menu actions remain
usable. First-event modifier order, frontmost clipped ownership, hidden/host focus,
global Stamp/Paint/clone identities and legacy group bounds are checked.

The merged Tools menu, simultaneous cloned phrase and minimum-size drawn length
were visually inspected. These are real application input/render tests, not native
OS clipboard/keyboard/plugin focus, Windows desktop or hardware acceptance. See
[Piano keyboard editing](PIANO_KEYBOARD_EDITING.md) and
[Piano mouse composition](PIANO_MOUSE_WORKFLOW.md) for exact implemented semantics.
