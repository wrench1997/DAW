# Piano melody keyboard editing

The active, visible Piano editor owns these commands. Playlist retains Ctrl+D for
Clip duplication. Text/numeric editors, menus, modal windows, held pointers and
unfinished drags own input instead. The same project-transition and lifecycle barriers used by the note clipboard
guard each immediate edit. Explicit
menu commands may act through their own popup; raw keyboard events may not.

| Command | Piano behavior |
| --- | --- |
| Ctrl/Cmd+D | Deselect notes, without modifying the project or history |
| Ctrl/Cmd+B | Duplicate the selected phrase to its right and select the new copies |
| Shift+Left / Right | Move one local Piano snap step |
| Shift+Up / Down | Move one chromatic semitone |
| Ctrl/Cmd+Up / Down | Move one chromatic octave |
| Shift+D | Replace lengths with the current Piano snap, at least the minimum note length |
| Ctrl+Q (Windows/Linux), Option+Cmd+Q (macOS) | Quantize starts and durations to the Piano snap |
| Shift+Q | Quantize starts only, preserving durations |
| Alt/Option+V | Toggle ghost-note visibility without project history |

These edits affect selected notes in the active TARGET Channel, expanded through
that Channel's groups when grouping is enabled. With no selected notes in the
active Channel, all its notes are affected. Ghost and unassigned notes remain
unchanged. Clipboard Copy/Cut/Paste keeps its existing separate channel-preserving
behavior. The chromatic transpose commands do not apply the draw-tool scale lock.

Repeated physical presses are independent edits. Only arrow movement accepts
native key autorepeat; one frame's accepted movement is one complete Undo step.
There is no open held-key transaction or timer. Release, focus loss and modal
interruption cannot leave history waiting for a key-up. Duplicate, quantize,
discard, deselect and ghost toggle do not autorepeat. No-op steps create no history.

## Explicit Citrus policies and limits

- Phrase-repeat spacing is the selected extent (latest end minus earliest start),
  with a minimum of one Piano snap step. Repeating selects the generated phrase,
  so the next press repeats it again. An independent Piano time-range/repeat region
  and exact FL default bar/gap rounding are not implemented.
- Quick quantize uses the local Piano snap. FL's manual describes global snap;
  Citrus deliberately retains independent Piano and Playlist snap settings.
  Its basic quick quantize uses the existing starts-and-durations quantizer;
  Shift+Q leaves duration unchanged. The full Alt+Q dialog is unchanged.
- Time moves clamp a common offset to beat 0 and the 4096-beat endpoint, and pitch
  moves clamp a common interval to MIDI 0–127. This preserves group spacing.
  Repeat and length changes that would exceed bounds are rejected as a whole.
  These are Citrus safety policies, not claims about undocumented FL edge behavior.
- Note candidates must be finite and valid, with at most 65,536 notes in the active
  Pattern. New copies receive project-wide unique note IDs and fresh copy-group
  IDs. Invalid targets, exhausted IDs, and invalid snaps are rejected atomically.
- macOS Cmd+Q is deliberately unbound by the Piano shortcut policy, leaving the
  native Quit combination alone. Alt+arrow pixel nudging is not implemented.

## Sources and verification boundary

Image-Line's official [Piano Roll menu](https://www.image-line.com/fl-studio-learning-content/fl-studio-online-manual/html/pianoroll_menu.htm)
and [keyboard shortcuts](https://www.image-line.com/fl-studio-learning-content/fl-studio-online-manual/html/basics_shortcuts.htm)
were checked for command mappings and selected-or-all scope. This is a bounded
clean-room compatibility slice, not complete FL Studio Piano Roll parity.

Pure-operation tests cover channel/group scope, finite/pitch/time/count/ID bounds,
common clamping, repeat spacing and quantize differences. Production egui app tests
exercise actual key events, native-repeat flags, Undo/Redo, focus, pointer-drag,
modal, numeric text and snapshot barriers. Optional Vulkan captures render that
same UI offscreen. These do not prove native OS keyboard ownership, physical MIDI,
audio quality, or real native-plugin-editor interaction.

Verified feature-source gates: Linux all-feature/all-target 1006 application,
14 helper and 5 protocol tests; no-default/all-target 1004 application tests; both
strict Clippy configurations and formatting checks passed. The six new production
app-flow tests also passed with two genuine Vulkan readbacks (Tools menu and
repeated phrase). The inspected PNG conversions are RGB-identical to the readbacks.
Windows MSVC no-default/all-target cross-check passed; this is source type-checking,
not Windows execution or native desktop acceptance.


## Verified combined keyboard and mouse source

The reviewed keyboard and mouse slices integrate at
`43fc5a388a292cd15503cc3759c24ba420fcb7f4`. Fresh full locked/offline Linux gates pass
**1,028 application +14 helper +5 protocol tests** with all features and **1,026
application tests** without default features, with zero failed/ignored on default
stack. Formatting, both strict Clippy profiles, app/helper builds, actual ordinary
helper smoke, Windows MSVC no-default/all-target source cross-check and **167 Python
regressions** pass. Independent merge review found no blocker.

The fresh complete production-input Vulkan run passes **64 entries**: 63 flow/input
checks plus the included opt-in benchmark entry, whose timing mode was disabled.
It produces **37 genuine app frames**, all PNGs RGB-identical to readback. Both
keyboard and pointer paths execute together, including semantic/text/modal/snapshot
ownership, held-pointer Undo/Redo protection, ordinary menu Undo after release,
first-event modifiers, global IDs and legacy geometry bounds. Merged Tools-menu,
cloned-phrase and minimum-size drawn-length frames were inspected.

These are combined source/input/offscreen results. This candidate needs its own
Windows runtime run; native OS/plugin keyboard focus, physical input/audio, full
FL parity and native paint acceptance remain separate. Velocity-wheel and numeric
note-properties work belongs to a later slice and is not included here.
