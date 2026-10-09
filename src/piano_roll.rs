//! Deterministic, project-safe editing primitives for the Piano Roll.
//!
//! Pointer handling stays in `app`, while transforms live here so every drag
//! can preview a candidate and commit exactly once at gesture end.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
};

use serde::{Deserialize, Serialize};

use crate::model::{PianoNote, normalize_piano_note_groups};

pub const MIN_NOTE_LENGTH_BEATS: f32 = 1.0 / 64.0;
pub const MAX_TRANSFORM_NOTES: usize = 65_536;
const NOTE_TIME_EPSILON: f32 = 1.0e-6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PianoRollTool {
    #[default]
    Draw,
    Paint,
    Delete,
    Mute,
    Slice,
    Select,
    Stamp,
}

impl PianoRollTool {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Draw => "DRAW",
            Self::Paint => "PAINT",
            Self::Delete => "DELETE",
            Self::Mute => "MUTE",
            Self::Slice => "SLICE",
            Self::Select => "SELECT",
            Self::Stamp => "STAMP",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PianoScale {
    #[default]
    Major,
    NaturalMinor,
    HarmonicMinor,
    MelodicMinor,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Locrian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
    Chromatic,
}

impl PianoScale {
    pub const ALL: [Self; 13] = [
        Self::Major,
        Self::NaturalMinor,
        Self::HarmonicMinor,
        Self::MelodicMinor,
        Self::Dorian,
        Self::Phrygian,
        Self::Lydian,
        Self::Mixolydian,
        Self::Locrian,
        Self::MajorPentatonic,
        Self::MinorPentatonic,
        Self::Blues,
        Self::Chromatic,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::NaturalMinor => "Natural minor",
            Self::HarmonicMinor => "Harmonic minor",
            Self::MelodicMinor => "Melodic minor",
            Self::Dorian => "Dorian",
            Self::Phrygian => "Phrygian",
            Self::Lydian => "Lydian",
            Self::Mixolydian => "Mixolydian",
            Self::Locrian => "Locrian",
            Self::MajorPentatonic => "Major pentatonic",
            Self::MinorPentatonic => "Minor pentatonic",
            Self::Blues => "Blues",
            Self::Chromatic => "Chromatic",
        }
    }

    pub const fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Self::MajorPentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
            Self::Blues => &[0, 3, 5, 6, 7, 10],
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PianoChordStamp {
    #[default]
    Major,
    Minor,
    Diminished,
    Augmented,
    Sus2,
    Sus4,
    Power,
    Major6,
    Minor6,
    Dominant7,
    Major7,
    Minor7,
    HalfDiminished7,
    Diminished7,
    Add9,
    MinorAdd9,
    Octave,
}

impl PianoChordStamp {
    pub const ALL: [Self; 17] = [
        Self::Major,
        Self::Minor,
        Self::Diminished,
        Self::Augmented,
        Self::Sus2,
        Self::Sus4,
        Self::Power,
        Self::Major6,
        Self::Minor6,
        Self::Dominant7,
        Self::Major7,
        Self::Minor7,
        Self::HalfDiminished7,
        Self::Diminished7,
        Self::Add9,
        Self::MinorAdd9,
        Self::Octave,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::Minor => "Minor",
            Self::Diminished => "Diminished",
            Self::Augmented => "Augmented",
            Self::Sus2 => "Suspended 2",
            Self::Sus4 => "Suspended 4",
            Self::Power => "Power chord",
            Self::Major6 => "Major 6",
            Self::Minor6 => "Minor 6",
            Self::Dominant7 => "Dominant 7",
            Self::Major7 => "Major 7",
            Self::Minor7 => "Minor 7",
            Self::HalfDiminished7 => "Half-diminished 7",
            Self::Diminished7 => "Diminished 7",
            Self::Add9 => "Add 9",
            Self::MinorAdd9 => "Minor add 9",
            Self::Octave => "Octave",
        }
    }

    pub const fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Sus2 => &[0, 2, 7],
            Self::Sus4 => &[0, 5, 7],
            Self::Power => &[0, 7],
            Self::Major6 => &[0, 4, 7, 9],
            Self::Minor6 => &[0, 3, 7, 9],
            Self::Dominant7 => &[0, 4, 7, 10],
            Self::Major7 => &[0, 4, 7, 11],
            Self::Minor7 => &[0, 3, 7, 10],
            Self::HalfDiminished7 => &[0, 3, 6, 10],
            Self::Diminished7 => &[0, 3, 6, 9],
            Self::Add9 => &[0, 4, 7, 14],
            Self::MinorAdd9 => &[0, 3, 7, 14],
            Self::Octave => &[0, 12],
        }
    }
}

use crate::piano_snap::{self, PianoSnap};

pub const PIANO_ROLL_PREFERENCES_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PianoRollPreferences {
    pub version: u32,
    pub local_snap: PianoSnap,
    pub scale_root: u8,
    pub scale: PianoScale,
    pub scale_highlighting: bool,
    pub snap_to_scale: bool,
    pub chord_stamp: PianoChordStamp,
    pub stamp_only_one: bool,
}

impl Default for PianoRollPreferences {
    fn default() -> Self {
        Self {
            version: PIANO_ROLL_PREFERENCES_VERSION,
            local_snap: PianoSnap::default(),
            scale_root: 0,
            scale: PianoScale::Major,
            scale_highlighting: true,
            snap_to_scale: false,
            chord_stamp: PianoChordStamp::Major,
            stamp_only_one: false,
        }
    }
}

impl PianoRollPreferences {
    pub const fn is_valid(&self) -> bool {
        self.version == PIANO_ROLL_PREFERENCES_VERSION && self.scale_root < 12
    }
}

/// Half-open edit range, independent of selected notes and transport playback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PianoTimeRange {
    pub start: f64,
    pub end: f64,
}
impl PianoTimeRange {
    pub fn new(start: f64, end: f64) -> Option<Self> {
        if !start.is_finite() || !end.is_finite() {
            return None;
        }
        let start = start.clamp(0.0, MAX_PIANO_BEAT);
        let end = end.clamp(0.0, MAX_PIANO_BEAT);
        (end > start).then_some(Self { start, end })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PianoRollState {
    pub tool: PianoRollTool,
    pub selection_ids: HashSet<u64>,
    pub last_note_length: f32,
    pub local_snap: PianoSnap,
    pub repeat_range: Option<PianoTimeRange>,
    pub range_owner: Option<(u64, usize, u32, Option<u32>)>,
    pub ghosts_visible: bool,
    pub grouping_enabled: bool,
    pub scale_root: u8,
    pub scale: PianoScale,
    pub scale_highlighting: bool,
    pub snap_to_scale: bool,
    pub chord_stamp: PianoChordStamp,
    pub stamp_only_one: bool,
}

impl Default for PianoRollState {
    fn default() -> Self {
        Self::from_preferences(PianoRollPreferences::default())
    }
}

impl PianoRollState {
    pub fn from_preferences(preferences: PianoRollPreferences) -> Self {
        let preferences = if preferences.is_valid() {
            preferences
        } else {
            PianoRollPreferences::default()
        };
        Self {
            tool: PianoRollTool::Draw,
            selection_ids: HashSet::new(),
            last_note_length: 1.0,
            local_snap: preferences.local_snap,
            repeat_range: None,
            range_owner: None,
            ghosts_visible: true,
            grouping_enabled: true,
            scale_root: preferences.scale_root,
            scale: preferences.scale,
            scale_highlighting: preferences.scale_highlighting,
            snap_to_scale: preferences.snap_to_scale,
            chord_stamp: preferences.chord_stamp,
            stamp_only_one: preferences.stamp_only_one,
        }
    }

    pub fn preferences(&self) -> PianoRollPreferences {
        PianoRollPreferences {
            version: PIANO_ROLL_PREFERENCES_VERSION,
            local_snap: self.local_snap,
            scale_root: self.scale_root,
            scale: self.scale,
            scale_highlighting: self.scale_highlighting,
            snap_to_scale: self.snap_to_scale,
            chord_stamp: self.chord_stamp,
            stamp_only_one: self.stamp_only_one,
        }
    }
}

pub const fn pitch_class_label(pitch_class: u8) -> &'static str {
    match pitch_class % 12 {
        0 => "C",
        1 => "C#",
        2 => "D",
        3 => "D#",
        4 => "E",
        5 => "F",
        6 => "F#",
        7 => "G",
        8 => "G#",
        9 => "A",
        10 => "A#",
        _ => "B",
    }
}

pub fn pitch_class_in_scale(note: u8, root: u8, scale: PianoScale) -> bool {
    let interval = (note % 12 + 12 - root % 12) % 12;
    scale.intervals().contains(&interval)
}

/// Finds the nearest MIDI pitch in the selected scale. Equidistant choices
/// resolve downward, matching the predictable feel expected during a drag.
pub fn snap_pitch_to_scale(note: u8, root: u8, scale: PianoScale) -> u8 {
    let note = note.min(127);
    if pitch_class_in_scale(note, root, scale) {
        return note;
    }
    for distance in 1..=12_u8 {
        if let Some(lower) = note.checked_sub(distance)
            && pitch_class_in_scale(lower, root, scale)
        {
            return lower;
        }
        if let Some(upper) = note
            .checked_add(distance)
            .filter(|candidate| *candidate <= 127)
            && pitch_class_in_scale(upper, root, scale)
        {
            return upper;
        }
    }
    note
}

pub fn chord_stamp_pitches(
    root_note: u8,
    chord: PianoChordStamp,
    scale_root: u8,
    scale: PianoScale,
    snap_to_scale: bool,
) -> Vec<u8> {
    chord
        .intervals()
        .iter()
        .map(|interval| root_note.saturating_add(*interval).min(127))
        .map(|note| {
            if snap_to_scale {
                snap_pitch_to_scale(note, scale_root, scale)
            } else {
                note
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn note_group_members(
    notes: &[PianoNote],
    note_id: u64,
    grouping_enabled: bool,
) -> HashSet<u64> {
    let Some(anchor) = notes.iter().find(|note| note.id == note_id) else {
        return HashSet::new();
    };
    let Some(group_id) = anchor.group_id.filter(|_| grouping_enabled) else {
        return HashSet::from([note_id]);
    };
    notes
        .iter()
        .filter(|note| note.group_id == Some(group_id) && note.channel_id == anchor.channel_id)
        .map(|note| note.id)
        .collect()
}

pub fn expand_note_group_selection(
    notes: &[PianoNote],
    selection_ids: &HashSet<u64>,
    grouping_enabled: bool,
) -> HashSet<u64> {
    selection_ids
        .iter()
        .flat_map(|note_id| note_group_members(notes, *note_id, grouping_enabled))
        .collect()
}

pub fn toggle_note_group_selection(
    notes: &[PianoNote],
    selection_ids: &mut HashSet<u64>,
    note_id: u64,
    grouping_enabled: bool,
) {
    let members = note_group_members(notes, note_id, grouping_enabled);
    let remove = !members.is_empty() && members.iter().all(|id| selection_ids.contains(id));
    if remove {
        selection_ids.retain(|id| !members.contains(id));
    } else {
        selection_ids.extend(members);
    }
}

pub fn select_note_group_members(
    notes: &[PianoNote],
    selection_ids: &mut HashSet<u64>,
    note_id: u64,
    grouping_enabled: bool,
    additive: bool,
) {
    if !additive {
        selection_ids.clear();
    }
    selection_ids.extend(note_group_members(notes, note_id, grouping_enabled));
}

pub fn ensure_note_group_selected(
    notes: &[PianoNote],
    selection_ids: &mut HashSet<u64>,
    note_id: u64,
    grouping_enabled: bool,
) {
    let expanded = expand_note_group_selection(notes, selection_ids, grouping_enabled);
    if expanded.contains(&note_id) {
        *selection_ids = expanded;
    } else {
        select_note_group_members(notes, selection_ids, note_id, grouping_enabled, false);
    }
}

impl PianoRollState {
    pub fn retain_existing(&mut self, notes: &[PianoNote]) {
        self.selection_ids
            .retain(|id| notes.iter().any(|note| note.id == *id));
    }

    pub fn select_only(&mut self, id: u64) {
        self.selection_ids.clear();
        if id != 0 {
            self.selection_ids.insert(id);
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection_ids.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PianoRollTransformKind {
    Quantize,
    Strum,
    Chop,
    Flam,
    Articulate,
    Arpeggiate,
}

impl PianoRollTransformKind {
    pub const ALL: [Self; 6] = [
        Self::Quantize,
        Self::Strum,
        Self::Chop,
        Self::Flam,
        Self::Articulate,
        Self::Arpeggiate,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Quantize => "Quantize",
            Self::Strum => "Strum",
            Self::Chop => "Chop",
            Self::Flam => "Flam",
            Self::Articulate => "Articulate",
            Self::Arpeggiate => "Arpeggiate",
        }
    }

    pub const fn shortcut(self) -> &'static str {
        match self {
            Self::Quantize => "Alt+Q",
            Self::Strum => "Alt+S",
            Self::Chop => "Alt+U",
            Self::Flam => "Alt+F",
            Self::Articulate => "Alt+L",
            Self::Arpeggiate => "Alt+A",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuantizeDurationMode {
    #[default]
    QuantizeDuration,
    QuantizeEndTime,
    LeaveDuration,
    LeaveEndTime,
}

impl QuantizeDurationMode {
    pub const ALL: [Self; 4] = [
        Self::QuantizeDuration,
        Self::QuantizeEndTime,
        Self::LeaveDuration,
        Self::LeaveEndTime,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::QuantizeDuration => "Quantize duration",
            Self::QuantizeEndTime => "Quantize end time",
            Self::LeaveDuration => "Leave duration",
            Self::LeaveEndTime => "Leave end time",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuantizeSettings {
    pub snap_beats: f32,
    pub start_strength: f32,
    pub sensitivity: f32,
    pub duration_strength: f32,
    pub duration_mode: QuantizeDurationMode,
}

impl QuantizeSettings {
    pub fn new(snap_beats: f32) -> Self {
        Self {
            snap_beats: snap_beats.max(MIN_NOTE_LENGTH_BEATS),
            start_strength: 1.0,
            sensitivity: 1.0,
            duration_strength: 1.0,
            duration_mode: QuantizeDurationMode::QuantizeDuration,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrumSettings {
    pub start_enabled: bool,
    pub start_time_beats: f32,
    pub start_tension: f32,
    pub velocity_change: f32,
    pub velocity_tension: f32,
    pub preserve_end: bool,
    pub trigger_ahead: bool,
    pub end_enabled: bool,
    pub end_time_beats: f32,
    pub end_tension: f32,
    pub chop_chords: bool,
    pub alternate_direction: bool,
}

impl Default for StrumSettings {
    fn default() -> Self {
        Self {
            start_enabled: true,
            start_time_beats: 1.0 / 16.0,
            start_tension: 0.0,
            velocity_change: 0.0,
            velocity_tension: 0.0,
            preserve_end: true,
            trigger_ahead: false,
            end_enabled: false,
            end_time_beats: 0.0,
            end_tension: 0.0,
            chop_chords: false,
            alternate_direction: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChopSettings {
    pub step_beats: f32,
    pub time_multiplier: f32,
    pub absolute_pattern: bool,
}

impl ChopSettings {
    pub fn new(step_beats: f32) -> Self {
        Self {
            step_beats: step_beats.max(MIN_NOTE_LENGTH_BEATS),
            time_multiplier: 1.0,
            absolute_pattern: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlamSettings {
    pub absolute_time: bool,
    pub time_beats: f32,
    pub time_ms: f32,
    pub before: bool,
    pub velocity: f32,
}

impl Default for FlamSettings {
    fn default() -> Self {
        Self {
            absolute_time: false,
            time_beats: 1.0 / 16.0,
            time_ms: 30.0,
            before: true,
            velocity: 0.55,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArticulationPreset {
    #[default]
    Legato,
    Portato,
    Staccato,
    SmallGap,
    ChopChords,
    Custom,
}

impl ArticulationPreset {
    pub const ALL: [Self; 5] = [
        Self::Legato,
        Self::Portato,
        Self::Staccato,
        Self::SmallGap,
        Self::ChopChords,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Legato => "Legato",
            Self::Portato => "Portato",
            Self::Staccato => "Staccato",
            Self::SmallGap => "Small gap",
            Self::ChopChords => "Chop chords",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArticulateSettings {
    pub preset: ArticulationPreset,
    pub multiply: f32,
    pub variation: f32,
    pub seed: u64,
    pub chop_chords: bool,
    pub use_lengths: bool,
    pub only_with_selection: bool,
    pub gap_beats: f32,
}

impl ArticulateSettings {
    pub const fn for_preset(preset: ArticulationPreset) -> Self {
        match preset {
            ArticulationPreset::Legato => Self {
                preset,
                multiply: 1.0,
                variation: 0.0,
                seed: 0,
                chop_chords: true,
                use_lengths: false,
                only_with_selection: false,
                gap_beats: 0.0,
            },
            ArticulationPreset::Portato => Self {
                preset,
                multiply: 0.9,
                variation: 0.0,
                seed: 0,
                chop_chords: true,
                use_lengths: false,
                only_with_selection: false,
                gap_beats: 0.0,
            },
            ArticulationPreset::Staccato => Self {
                preset,
                multiply: 0.5,
                variation: 0.0,
                seed: 0,
                chop_chords: false,
                use_lengths: true,
                only_with_selection: false,
                gap_beats: 0.0,
            },
            ArticulationPreset::SmallGap => Self {
                preset,
                multiply: 1.0,
                variation: 0.0,
                seed: 0,
                chop_chords: true,
                use_lengths: false,
                only_with_selection: false,
                gap_beats: MIN_NOTE_LENGTH_BEATS,
            },
            ArticulationPreset::ChopChords => Self {
                preset,
                multiply: 1.0,
                variation: 0.0,
                seed: 0,
                chop_chords: true,
                use_lengths: true,
                only_with_selection: false,
                gap_beats: 0.0,
            },
            ArticulationPreset::Custom => Self {
                preset,
                multiply: 1.0,
                variation: 0.0,
                seed: 0,
                chop_chords: false,
                use_lengths: true,
                only_with_selection: false,
                gap_beats: 0.0,
            },
        }
    }

    pub fn apply_preset(&mut self, preset: ArticulationPreset) {
        let seed = self.seed;
        let only_with_selection = self.only_with_selection;
        *self = Self::for_preset(preset);
        self.seed = seed;
        self.only_with_selection = only_with_selection;
    }
}

impl Default for ArticulateSettings {
    fn default() -> Self {
        Self::for_preset(ArticulationPreset::Legato)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArpeggioDirection {
    #[default]
    Up,
    Down,
    UpDown,
    DownUp,
}

impl ArpeggioDirection {
    pub const ALL: [Self; 4] = [Self::Up, Self::Down, Self::UpDown, Self::DownUp];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::UpDown => "Up / down",
            Self::DownUp => "Down / up",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArpeggioSync {
    Time,
    #[default]
    Block,
    Chord,
}

impl ArpeggioSync {
    pub const ALL: [Self; 3] = [Self::Time, Self::Block, Self::Chord];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Time => "Time",
            Self::Block => "Block",
            Self::Chord => "Chord",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArpeggiateSettings {
    pub step_beats: f32,
    pub time_multiplier: f32,
    pub range_octaves: u8,
    pub direction: ArpeggioDirection,
    pub sync: ArpeggioSync,
    pub gate: f32,
    pub group_notes: bool,
}

impl ArpeggiateSettings {
    pub fn new(step_beats: f32) -> Self {
        Self {
            step_beats: step_beats.max(MIN_NOTE_LENGTH_BEATS),
            time_multiplier: 1.0,
            range_octaves: 1,
            direction: ArpeggioDirection::Up,
            sync: ArpeggioSync::Block,
            gate: 0.8,
            group_notes: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PianoRollTransformSettings {
    Quantize(QuantizeSettings),
    Strum(StrumSettings),
    Chop(ChopSettings),
    Flam(FlamSettings),
    Articulate(ArticulateSettings),
    Arpeggiate(ArpeggiateSettings),
}

impl PianoRollTransformSettings {
    pub fn for_kind(kind: PianoRollTransformKind, snap_beats: f32) -> Self {
        match kind {
            PianoRollTransformKind::Quantize => Self::Quantize(QuantizeSettings::new(snap_beats)),
            PianoRollTransformKind::Strum => Self::Strum(StrumSettings::default()),
            PianoRollTransformKind::Chop => Self::Chop(ChopSettings::new(snap_beats)),
            PianoRollTransformKind::Flam => Self::Flam(FlamSettings::default()),
            PianoRollTransformKind::Articulate => Self::Articulate(ArticulateSettings::default()),
            PianoRollTransformKind::Arpeggiate => {
                Self::Arpeggiate(ArpeggiateSettings::new(snap_beats))
            }
        }
    }

    pub const fn kind(&self) -> PianoRollTransformKind {
        match self {
            Self::Quantize(_) => PianoRollTransformKind::Quantize,
            Self::Strum(_) => PianoRollTransformKind::Strum,
            Self::Chop(_) => PianoRollTransformKind::Chop,
            Self::Flam(_) => PianoRollTransformKind::Flam,
            Self::Articulate(_) => PianoRollTransformKind::Articulate,
            Self::Arpeggiate(_) => PianoRollTransformKind::Arpeggiate,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PianoRollEditError {
    InvalidNumber,
    InvalidSnap,
    InvalidTarget,
    NoteIdExhausted,
    GroupIdExhausted,
    TransformTooLarge,
    MismatchedTransformSettings,
}

impl fmt::Display for PianoRollEditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidNumber => "a note or transform parameter is not finite or is out of range",
            Self::InvalidSnap => "the transform grid must be a positive finite value",
            Self::InvalidTarget => "the transform target no longer matches the active pattern",
            Self::NoteIdExhausted => "no stable Piano Roll note ID remains",
            Self::GroupIdExhausted => "no stable Piano Roll note-group ID remains",
            Self::TransformTooLarge => "the transform would exceed the 65,536-note safety limit",
            Self::MismatchedTransformSettings => {
                "the transform kind does not match its parameter set"
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct DuplicateNotesResult {
    pub notes: Vec<PianoNote>,
    pub selection_ids: HashSet<u64>,
}

#[derive(Clone, Debug)]
pub struct StampNotesResult {
    pub notes: Vec<PianoNote>,
    pub selection_ids: HashSet<u64>,
    pub inserted_notes: Vec<PianoNote>,
}

#[derive(Clone, Debug)]
pub struct PianoRollTransformResult {
    pub notes: Vec<PianoNote>,
    pub selection_ids: HashSet<u64>,
    pub transformed_notes: usize,
    pub generated_notes: usize,
}

#[derive(Clone, Debug)]
pub struct PianoNoteGroupEditResult {
    pub notes: Vec<PianoNote>,
    pub selection_ids: HashSet<u64>,
    pub affected_notes: usize,
    pub group_id: Option<u64>,
}

struct NoteIdAllocator {
    used: HashSet<u64>,
    next: u64,
}

struct NoteGroupIdAllocator {
    used: HashSet<u64>,
    next: u64,
}

impl NoteGroupIdAllocator {
    fn new(notes: &[PianoNote]) -> Result<Self, PianoRollEditError> {
        let used = notes
            .iter()
            .filter_map(|note| note.group_id)
            .collect::<HashSet<_>>();
        if used.contains(&0) {
            return Err(PianoRollEditError::InvalidTarget);
        }
        let next = used.iter().copied().max().unwrap_or(0);
        Ok(Self { used, next })
    }

    fn allocate(&mut self) -> Result<u64, PianoRollEditError> {
        loop {
            self.next = self
                .next
                .checked_add(1)
                .ok_or(PianoRollEditError::GroupIdExhausted)?;
            if self.next != 0 && self.used.insert(self.next) {
                return Ok(self.next);
            }
        }
    }
}

pub fn group_selected_notes(
    notes: &[PianoNote],
    selection_ids: &HashSet<u64>,
) -> Result<PianoNoteGroupEditResult, PianoRollEditError> {
    validate_group_edit_target(notes, selection_ids)?;
    if selection_ids.len() < 2 {
        return Ok(PianoNoteGroupEditResult {
            notes: notes.to_vec(),
            selection_ids: selection_ids.clone(),
            affected_notes: 0,
            group_id: None,
        });
    }
    let mut selected_channels = notes
        .iter()
        .filter(|note| selection_ids.contains(&note.id))
        .map(|note| note.channel_id);
    let channel = selected_channels
        .next()
        .ok_or(PianoRollEditError::InvalidTarget)?;
    if selected_channels.any(|candidate| candidate != channel) {
        return Err(PianoRollEditError::InvalidTarget);
    }
    let group_id = NoteGroupIdAllocator::new(notes)?.allocate()?;
    let mut result = notes.to_vec();
    for note in &mut result {
        if selection_ids.contains(&note.id) {
            note.group_id = Some(group_id);
        }
    }
    normalize_piano_note_groups(&mut result);
    Ok(PianoNoteGroupEditResult {
        notes: result,
        selection_ids: selection_ids.clone(),
        affected_notes: selection_ids.len(),
        group_id: Some(group_id),
    })
}

pub fn ungroup_selected_notes(
    notes: &[PianoNote],
    selection_ids: &HashSet<u64>,
) -> Result<PianoNoteGroupEditResult, PianoRollEditError> {
    validate_group_edit_target(notes, selection_ids)?;
    let mut result = notes.to_vec();
    let mut affected_notes = 0;
    for note in &mut result {
        if selection_ids.contains(&note.id) && note.group_id.take().is_some() {
            affected_notes += 1;
        }
    }
    normalize_piano_note_groups(&mut result);
    Ok(PianoNoteGroupEditResult {
        notes: result,
        selection_ids: selection_ids.clone(),
        affected_notes,
        group_id: None,
    })
}

fn validate_group_edit_target(
    notes: &[PianoNote],
    selection_ids: &HashSet<u64>,
) -> Result<(), PianoRollEditError> {
    let mut seen = HashSet::with_capacity(notes.len());
    let mut matched = 0;
    for note in notes {
        if note.id == 0 || !seen.insert(note.id) {
            return Err(PianoRollEditError::InvalidTarget);
        }
        matched += usize::from(selection_ids.contains(&note.id));
    }
    if matched != selection_ids.len() {
        return Err(PianoRollEditError::InvalidTarget);
    }
    Ok(())
}

impl NoteIdAllocator {
    fn new(notes: &[PianoNote]) -> Result<Self, PianoRollEditError> {
        let mut used = HashSet::with_capacity(notes.len());
        for note in notes {
            if note.id == 0 || !used.insert(note.id) {
                return Err(PianoRollEditError::InvalidTarget);
            }
        }
        let next = used.iter().copied().max().unwrap_or(0);
        Ok(Self { used, next })
    }

    fn allocate(&mut self) -> Result<u64, PianoRollEditError> {
        loop {
            self.next = self
                .next
                .checked_add(1)
                .ok_or(PianoRollEditError::NoteIdExhausted)?;
            if self.next != 0 && self.used.insert(self.next) {
                return Ok(self.next);
            }
        }
    }
}

/// Builds one all-or-none chord-stamp candidate with stable note IDs.
/// Existing notes at the same Channel, pitch and snapped start are preserved
/// and not duplicated; the returned selection contains only inserted notes.
#[allow(clippy::too_many_arguments)]
pub fn stamp_chord_notes(
    notes: &[PianoNote],
    channel_id: Option<u32>,
    root_note: u8,
    start: f32,
    length: f32,
    velocity: f32,
    chord: PianoChordStamp,
    scale_root: u8,
    scale: PianoScale,
    snap_to_scale: bool,
) -> Result<StampNotesResult, PianoRollEditError> {
    if !start.is_finite()
        || start < 0.0
        || !length.is_finite()
        || length < MIN_NOTE_LENGTH_BEATS
        || !velocity.is_finite()
        || !(0.0..=1.0).contains(&velocity)
        || scale_root >= 12
    {
        return Err(PianoRollEditError::InvalidNumber);
    }

    let pitches = chord_stamp_pitches(root_note, chord, scale_root, scale, snap_to_scale)
        .into_iter()
        .filter(|pitch| {
            !notes.iter().any(|existing| {
                existing.channel_id == channel_id
                    && existing.note == *pitch
                    && (existing.start - start).abs() < NOTE_TIME_EPSILON
            })
        })
        .collect::<Vec<_>>();
    if notes.len().saturating_add(pitches.len()) > MAX_TRANSFORM_NOTES {
        return Err(PianoRollEditError::TransformTooLarge);
    }

    let mut allocator = NoteIdAllocator::new(notes)?;
    let mut inserted_notes = Vec::with_capacity(pitches.len());
    let mut selection_ids = HashSet::with_capacity(pitches.len());
    for pitch in pitches {
        let id = allocator.allocate()?;
        selection_ids.insert(id);
        inserted_notes.push(PianoNote {
            id,
            channel_id,
            group_id: None,
            note: pitch,
            start,
            length,
            velocity,
            selected: false,
            muted: false,
        });
    }

    let mut result = notes.to_vec();
    for note in &mut result {
        note.selected = false;
    }
    result.extend(inserted_notes.iter().cloned());
    Ok(StampNotesResult {
        notes: result,
        selection_ids,
        inserted_notes,
    })
}

pub fn transform_piano_notes(
    notes: &[PianoNote],
    target_ids: &HashSet<u64>,
    kind: PianoRollTransformKind,
    settings: &PianoRollTransformSettings,
    tempo_bpm: f32,
) -> Result<PianoRollTransformResult, PianoRollEditError> {
    if settings.kind() != kind {
        return Err(PianoRollEditError::MismatchedTransformSettings);
    }
    validate_transform_input(notes, target_ids)?;
    if target_ids.is_empty() {
        return Ok(PianoRollTransformResult {
            notes: notes.to_vec(),
            selection_ids: HashSet::new(),
            transformed_notes: 0,
            generated_notes: 0,
        });
    }
    match settings {
        PianoRollTransformSettings::Quantize(settings) => {
            quantize_notes(notes, target_ids, *settings)
        }
        PianoRollTransformSettings::Strum(settings) => strum_notes(notes, target_ids, *settings),
        PianoRollTransformSettings::Chop(settings) => chop_notes(notes, target_ids, *settings),
        PianoRollTransformSettings::Flam(settings) => {
            flam_notes(notes, target_ids, *settings, tempo_bpm)
        }
        PianoRollTransformSettings::Articulate(settings) => {
            articulate_notes(notes, target_ids, *settings)
        }
        PianoRollTransformSettings::Arpeggiate(settings) => {
            arpeggiate_notes(notes, target_ids, *settings)
        }
    }
}

fn validate_transform_input(
    notes: &[PianoNote],
    target_ids: &HashSet<u64>,
) -> Result<(), PianoRollEditError> {
    if notes.len() > MAX_TRANSFORM_NOTES {
        return Err(PianoRollEditError::TransformTooLarge);
    }
    let mut seen = HashSet::with_capacity(notes.len());
    let mut matched = 0;
    for note in notes {
        if note.id == 0 || !seen.insert(note.id) {
            return Err(PianoRollEditError::InvalidTarget);
        }
        if target_ids.contains(&note.id) {
            matched += 1;
            if !note.start.is_finite()
                || note.start < 0.0
                || !note.length.is_finite()
                || note.length < MIN_NOTE_LENGTH_BEATS
                || !note.velocity.is_finite()
                || !(0.0..=1.0).contains(&note.velocity)
                || note.note > 127
            {
                return Err(PianoRollEditError::InvalidNumber);
            }
        }
    }
    if matched != target_ids.len() {
        return Err(PianoRollEditError::InvalidTarget);
    }
    Ok(())
}

fn quantize_notes(
    notes: &[PianoNote],
    target_ids: &HashSet<u64>,
    settings: QuantizeSettings,
) -> Result<PianoRollTransformResult, PianoRollEditError> {
    if !settings.snap_beats.is_finite() || settings.snap_beats <= 0.0 {
        return Err(PianoRollEditError::InvalidSnap);
    }
    if !settings.start_strength.is_finite()
        || !settings.sensitivity.is_finite()
        || !settings.duration_strength.is_finite()
        || !(0.0..=1.0).contains(&settings.start_strength)
        || !(0.0..=1.0).contains(&settings.sensitivity)
        || !(0.0..=1.0).contains(&settings.duration_strength)
    {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let mut result = notes.to_vec();
    for note in &mut result {
        if !target_ids.contains(&note.id) {
            continue;
        }
        let original_start = note.start;
        let original_length = note.length;
        let original_end = original_start + original_length;
        let target_start =
            sensitive_grid_target(original_start, settings.snap_beats, settings.sensitivity);
        let mut start = mix(original_start, target_start, settings.start_strength).max(0.0);
        let length = match settings.duration_mode {
            QuantizeDurationMode::QuantizeDuration => {
                let target_length = sensitive_grid_target(
                    original_length,
                    settings.snap_beats,
                    settings.sensitivity,
                )
                .max(MIN_NOTE_LENGTH_BEATS);
                mix(original_length, target_length, settings.duration_strength)
                    .max(MIN_NOTE_LENGTH_BEATS)
            }
            QuantizeDurationMode::QuantizeEndTime => {
                let target_end =
                    sensitive_grid_target(original_end, settings.snap_beats, settings.sensitivity);
                let end = mix(original_end, target_end, settings.duration_strength);
                (end - start).max(MIN_NOTE_LENGTH_BEATS)
            }
            QuantizeDurationMode::LeaveDuration => original_length,
            QuantizeDurationMode::LeaveEndTime => {
                start = start.min((original_end - MIN_NOTE_LENGTH_BEATS).max(0.0));
                (original_end - start).max(MIN_NOTE_LENGTH_BEATS)
            }
        };
        if !start.is_finite() || !length.is_finite() {
            return Err(PianoRollEditError::InvalidNumber);
        }
        note.start = start;
        note.length = length;
    }
    Ok(PianoRollTransformResult {
        notes: result,
        selection_ids: target_ids.clone(),
        transformed_notes: target_ids.len(),
        generated_notes: 0,
    })
}

fn sensitive_grid_target(value: f32, snap: f32, sensitivity: f32) -> f32 {
    let target = quantize_beat(value, snap, false).unwrap_or(value);
    let threshold = snap * 0.5 * sensitivity;
    if (target - value).abs() <= threshold + NOTE_TIME_EPSILON {
        target
    } else {
        value
    }
}

fn mix(original: f32, target: f32, amount: f32) -> f32 {
    original + (target - original) * amount
}

fn strum_notes(
    notes: &[PianoNote],
    target_ids: &HashSet<u64>,
    settings: StrumSettings,
) -> Result<PianoRollTransformResult, PianoRollEditError> {
    if [
        settings.start_time_beats,
        settings.start_tension,
        settings.velocity_change,
        settings.velocity_tension,
        settings.end_time_beats,
        settings.end_tension,
    ]
    .iter()
    .any(|value| !value.is_finite())
    {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let mut result = notes.to_vec();
    let mut indices = result
        .iter()
        .enumerate()
        .filter_map(|(index, note)| target_ids.contains(&note.id).then_some(index))
        .collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        result[*left]
            .start
            .total_cmp(&result[*right].start)
            .then_with(|| result[*left].note.cmp(&result[*right].note))
            .then_with(|| result[*left].id.cmp(&result[*right].id))
    });
    let groups = same_start_groups(&result, &indices);
    for (group_number, group) in groups.iter().enumerate() {
        if group.len() < 2 {
            continue;
        }
        let mut ordered = group.clone();
        ordered.sort_by_key(|index| (result[*index].note, result[*index].id));
        let mut low_to_high = settings.start_time_beats >= 0.0;
        if settings.alternate_direction && group_number % 2 == 1 {
            low_to_high = !low_to_high;
        }
        if !low_to_high {
            ordered.reverse();
        }
        let divisor = (ordered.len() - 1) as f32;
        let original = ordered
            .iter()
            .map(|index| (result[*index].start, result[*index].length))
            .collect::<Vec<_>>();
        let mut starts = Vec::with_capacity(ordered.len());
        for (rank, (original_start, _)) in original.iter().copied().enumerate() {
            let progress = rank as f32 / divisor;
            let start_curve = tension_curve(progress, settings.start_tension);
            let mut offset = if settings.start_enabled {
                settings.start_time_beats.abs() * start_curve
            } else {
                0.0
            };
            if settings.start_enabled && settings.trigger_ahead {
                offset -= settings.start_time_beats.abs() * 0.5;
            }
            starts.push(original_start + offset);
        }
        let minimum_start = starts.iter().copied().reduce(f32::min).unwrap_or(0.0);
        if minimum_start < 0.0 {
            for start in &mut starts {
                *start -= minimum_start;
            }
        }
        for (rank, index) in ordered.iter().copied().enumerate() {
            let progress = rank as f32 / divisor;
            let velocity_curve = tension_curve(progress, settings.velocity_tension);
            let start = starts[rank];
            let old_end = original[rank].0 + original[rank].1;
            let mut end = if settings.preserve_end {
                old_end
            } else {
                start + original[rank].1
            };
            if settings.end_enabled {
                end += settings.end_time_beats * tension_curve(progress, settings.end_tension);
            }
            result[index].start = start;
            result[index].length = (end - start).max(MIN_NOTE_LENGTH_BEATS);
            result[index].velocity = (result[index].velocity
                - settings.velocity_change * velocity_curve)
                .clamp(0.0, 1.0);
        }
    }
    if settings.chop_chords {
        for pair in groups.windows(2) {
            let next_start = pair[1]
                .iter()
                .map(|index| result[*index].start)
                .reduce(f32::min)
                .unwrap_or(f32::INFINITY);
            for index in &pair[0] {
                if next_start > result[*index].start + MIN_NOTE_LENGTH_BEATS {
                    result[*index].length = result[*index]
                        .length
                        .min(next_start - result[*index].start)
                        .max(MIN_NOTE_LENGTH_BEATS);
                }
            }
        }
    }
    if result.iter().any(|note| {
        !note.start.is_finite()
            || !note.length.is_finite()
            || note.start < 0.0
            || note.length < MIN_NOTE_LENGTH_BEATS
    }) {
        return Err(PianoRollEditError::InvalidNumber);
    }
    Ok(PianoRollTransformResult {
        notes: result,
        selection_ids: target_ids.clone(),
        transformed_notes: target_ids.len(),
        generated_notes: 0,
    })
}

fn same_start_groups(notes: &[PianoNote], indices: &[usize]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for index in indices {
        let belongs_to_last = groups.last().is_some_and(|group| {
            (notes[group[0]].start - notes[*index].start).abs() <= NOTE_TIME_EPSILON
        });
        if belongs_to_last {
            groups.last_mut().expect("group exists").push(*index);
        } else {
            groups.push(vec![*index]);
        }
    }
    groups
}

fn tension_curve(progress: f32, tension: f32) -> f32 {
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }
    progress.powf(2.0_f32.powf(tension.clamp(-1.0, 1.0) * 2.0))
}

fn exact_grid_step(snap: f32) -> f64 {
    PianoSnap::from_step(snap)
        .and_then(PianoSnap::step)
        .unwrap_or(f64::from(snap))
}

fn bounded_length_before_end(
    start: f32,
    end: f64,
    requested: f64,
) -> Result<f32, PianoRollEditError> {
    let remaining = end - f64::from(start);
    let raw = requested
        .max(f64::from(MIN_NOTE_LENGTH_BEATS))
        .min(remaining);
    let mut length = raw as f32;
    if f64::from(start) + f64::from(length) > end {
        length = length.next_down();
    }
    if !length.is_finite() || length < MIN_NOTE_LENGTH_BEATS {
        return Err(PianoRollEditError::InvalidNumber);
    }
    Ok(length)
}

fn chop_notes(
    notes: &[PianoNote],
    target_ids: &HashSet<u64>,
    settings: ChopSettings,
) -> Result<PianoRollTransformResult, PianoRollEditError> {
    if !settings.step_beats.is_finite()
        || settings.step_beats <= 0.0
        || !settings.time_multiplier.is_finite()
        || settings.time_multiplier <= 0.0
    {
        return Err(PianoRollEditError::InvalidSnap);
    }
    let step = exact_grid_step(settings.step_beats) * f64::from(settings.time_multiplier);
    if !step.is_finite() || step < f64::from(MIN_NOTE_LENGTH_BEATS) {
        return Err(PianoRollEditError::InvalidSnap);
    }
    let mut ids = NoteIdAllocator::new(notes)?;
    let mut result = Vec::with_capacity(notes.len());
    let mut selection_ids = HashSet::new();
    let mut generated_notes = 0;
    for note in notes {
        if !target_ids.contains(&note.id) {
            result.push(note.clone());
            continue;
        }
        let end = f64::from(note.start) + f64::from(note.length);
        let mut starts = vec![note.start];
        let phase = if settings.absolute_pattern {
            0.0
        } else {
            f64::from(note.start)
        };
        let first_index = if settings.absolute_pattern {
            (f64::from(note.start) / step).floor() + 1.0
        } else {
            1.0
        };
        // Each boundary is derived from the fixed phase and index. A rational
        // triplet never accumulates f32 error across a long/late phrase.
        for offset in 0..=MAX_TRANSFORM_NOTES {
            let boundary = phase + (first_index + offset as f64) * step;
            if !boundary.is_finite() {
                return Err(PianoRollEditError::InvalidNumber);
            }
            let boundary = boundary as f32;
            if f64::from(boundary) > end - f64::from(MIN_NOTE_LENGTH_BEATS) {
                break;
            }
            if offset == MAX_TRANSFORM_NOTES {
                return Err(PianoRollEditError::TransformTooLarge);
            }
            let previous = *starts.last().expect("the note start is present");
            if boundary - previous >= MIN_NOTE_LENGTH_BEATS {
                starts.push(boundary);
            }
        }
        for (slice_index, start) in starts.iter().copied().enumerate() {
            if result.len() >= MAX_TRANSFORM_NOTES {
                return Err(PianoRollEditError::TransformTooLarge);
            }
            let slice_end = starts
                .get(slice_index + 1)
                .map_or(end, |value| f64::from(*value));
            let mut slice = note.clone();
            slice.start = start;
            slice.length =
                bounded_length_before_end(start, slice_end, slice_end - f64::from(start))?;
            if slice_index != 0 {
                slice.id = ids.allocate()?;
                generated_notes += 1;
            }
            selection_ids.insert(slice.id);
            result.push(slice);
        }
    }
    Ok(PianoRollTransformResult {
        notes: result,
        selection_ids,
        transformed_notes: target_ids.len(),
        generated_notes,
    })
}

fn flam_notes(
    notes: &[PianoNote],
    target_ids: &HashSet<u64>,
    settings: FlamSettings,
    tempo_bpm: f32,
) -> Result<PianoRollTransformResult, PianoRollEditError> {
    if !settings.time_beats.is_finite()
        || !settings.time_ms.is_finite()
        || !settings.velocity.is_finite()
        || settings.time_beats <= 0.0
        || settings.time_ms <= 0.0
        || !(0.0..=1.0).contains(&settings.velocity)
        || !tempo_bpm.is_finite()
        || tempo_bpm <= 0.0
    {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let separation = if settings.absolute_time {
        settings.time_ms * tempo_bpm / 60_000.0
    } else {
        settings.time_beats
    }
    .max(MIN_NOTE_LENGTH_BEATS);
    if !separation.is_finite() {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let mut ids = NoteIdAllocator::new(notes)?;
    let mut result = Vec::with_capacity(notes.len().saturating_add(target_ids.len()));
    let mut selection_ids = HashSet::with_capacity(target_ids.len().saturating_mul(2));
    let mut generated_notes = 0;
    for note in notes {
        result.push(note.clone());
        if !target_ids.contains(&note.id) {
            continue;
        }
        selection_ids.insert(note.id);
        let (start, length) = if settings.before {
            let available = separation.min(note.start);
            if available < MIN_NOTE_LENGTH_BEATS {
                continue;
            }
            (note.start - available, available)
        } else {
            (
                note.start + separation,
                separation.min(note.length).max(MIN_NOTE_LENGTH_BEATS),
            )
        };
        if result.len() >= MAX_TRANSFORM_NOTES {
            return Err(PianoRollEditError::TransformTooLarge);
        }
        let mut grace = note.clone();
        grace.id = ids.allocate()?;
        grace.start = start;
        grace.length = length;
        grace.velocity = settings.velocity;
        grace.selected = false;
        selection_ids.insert(grace.id);
        result.push(grace);
        generated_notes += 1;
    }
    Ok(PianoRollTransformResult {
        notes: result,
        selection_ids,
        transformed_notes: target_ids.len(),
        generated_notes,
    })
}

fn articulate_notes(
    notes: &[PianoNote],
    target_ids: &HashSet<u64>,
    settings: ArticulateSettings,
) -> Result<PianoRollTransformResult, PianoRollEditError> {
    if !settings.multiply.is_finite()
        || !(0.1..=1.0).contains(&settings.multiply)
        || !settings.variation.is_finite()
        || !(0.0..=1.0).contains(&settings.variation)
        || !settings.gap_beats.is_finite()
        || settings.gap_beats < 0.0
    {
        return Err(PianoRollEditError::InvalidNumber);
    }

    let mut result = notes.to_vec();
    for note in &mut result {
        if !target_ids.contains(&note.id) {
            continue;
        }
        let next_start = notes
            .iter()
            .filter(|candidate| {
                candidate.channel_id == note.channel_id
                    && candidate.start.is_finite()
                    && candidate.start > note.start + NOTE_TIME_EPSILON
                    && (!settings.only_with_selection || target_ids.contains(&candidate.id))
            })
            .map(|candidate| candidate.start)
            .reduce(f32::min);

        let boundary_length = next_start.map(|start| start - note.start);
        let mut length = if settings.use_lengths {
            note.length
        } else {
            boundary_length.unwrap_or(note.length)
        };
        let mut boundary_limited = !settings.use_lengths && boundary_length.is_some();
        if settings.chop_chords
            && let Some(boundary) = boundary_length
            && length > boundary
        {
            length = boundary;
            boundary_limited = true;
        }
        if boundary_limited {
            length = (length - settings.gap_beats).max(MIN_NOTE_LENGTH_BEATS);
        }

        let variation = deterministic_signed_unit(settings.seed, note.id) * settings.variation;
        length = (length * settings.multiply * (1.0 + variation)).max(MIN_NOTE_LENGTH_BEATS);
        if settings.chop_chords
            && let Some(boundary) = boundary_length
        {
            length = length.min((boundary - settings.gap_beats).max(MIN_NOTE_LENGTH_BEATS));
        }
        if !length.is_finite() {
            return Err(PianoRollEditError::InvalidNumber);
        }
        note.length = length;
    }

    Ok(PianoRollTransformResult {
        notes: result,
        selection_ids: target_ids.clone(),
        transformed_notes: target_ids.len(),
        generated_notes: 0,
    })
}

fn deterministic_signed_unit(seed: u64, id: u64) -> f32 {
    let mut value = seed ^ id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^= value >> 31;
    let unit = ((value >> 40) as u32) as f32 / ((1_u32 << 24) - 1) as f32;
    unit * 2.0 - 1.0
}

#[derive(Clone, Copy)]
struct ArpeggioPitch {
    note: u8,
    velocity: f32,
    muted: bool,
}

fn arpeggiate_notes(
    notes: &[PianoNote],
    target_ids: &HashSet<u64>,
    settings: ArpeggiateSettings,
) -> Result<PianoRollTransformResult, PianoRollEditError> {
    if !settings.step_beats.is_finite()
        || settings.step_beats <= 0.0
        || !settings.time_multiplier.is_finite()
        || settings.time_multiplier <= 0.0
        || !(1..=4).contains(&settings.range_octaves)
        || !settings.gate.is_finite()
        || !(0.05..=1.0).contains(&settings.gate)
    {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let step = exact_grid_step(settings.step_beats) * f64::from(settings.time_multiplier);
    if !step.is_finite() || step < f64::from(MIN_NOTE_LENGTH_BEATS) {
        return Err(PianoRollEditError::InvalidSnap);
    }

    let source_groups = arpeggio_source_groups(notes, target_ids);
    let mut note_ids = NoteIdAllocator::new(notes)?;
    let mut group_ids = NoteGroupIdAllocator::new(notes)?;
    let mut replacements = BTreeMap::<usize, Vec<PianoNote>>::new();
    let mut selection_ids = HashSet::new();
    let mut output_count = 0_usize;

    for source_indices in source_groups {
        let pattern = arpeggio_pitch_pattern(notes, &source_indices, settings);
        if pattern.is_empty() {
            return Err(PianoRollEditError::InvalidTarget);
        }
        let source_start = source_indices
            .iter()
            .map(|index| notes[*index].start)
            .reduce(f32::min)
            .ok_or(PianoRollEditError::InvalidTarget)?;
        let chord_end = source_indices
            .iter()
            .map(|index| f64::from(notes[*index].start) + f64::from(notes[*index].length))
            .reduce(f64::min)
            .ok_or(PianoRollEditError::InvalidTarget)?;
        let block_end = source_indices
            .iter()
            .map(|index| f64::from(notes[*index].start) + f64::from(notes[*index].length))
            .reduce(f64::max)
            .ok_or(PianoRollEditError::InvalidTarget)?;
        let one_pass_end = f64::from(source_start) + step * pattern.len() as f64;
        if !one_pass_end.is_finite() {
            return Err(PianoRollEditError::InvalidNumber);
        }
        let end = match settings.sync {
            ArpeggioSync::Time => one_pass_end.min(block_end),
            ArpeggioSync::Block => block_end,
            ArpeggioSync::Chord => chord_end,
        };

        let mut reusable_ids = source_indices
            .iter()
            .map(|index| notes[*index].id)
            .collect::<Vec<_>>();
        reusable_ids.sort_unstable();
        let mut generated = Vec::new();
        let mut event_index = 0_usize;
        loop {
            let start = (f64::from(source_start) + step * event_index as f64) as f32;
            let remaining = end - f64::from(start);
            if remaining < f64::from(MIN_NOTE_LENGTH_BEATS) {
                break;
            }
            if notes.len() - target_ids.len() + output_count + generated.len()
                >= MAX_TRANSFORM_NOTES
            {
                return Err(PianoRollEditError::TransformTooLarge);
            }
            let pitch = pattern[event_index % pattern.len()];
            let id = if let Some(id) = reusable_ids.get(event_index).copied() {
                id
            } else {
                note_ids.allocate()?
            };
            generated.push(PianoNote {
                id,
                channel_id: notes[source_indices[0]].channel_id,
                group_id: None,
                note: pitch.note,
                start,
                length: bounded_length_before_end(start, end, step * f64::from(settings.gate))?,
                velocity: pitch.velocity,
                selected: false,
                muted: pitch.muted,
            });
            selection_ids.insert(id);
            event_index += 1;
            if !start.is_finite() || event_index > MAX_TRANSFORM_NOTES {
                return Err(PianoRollEditError::TransformTooLarge);
            }
        }
        if generated.is_empty() {
            return Err(PianoRollEditError::InvalidNumber);
        }
        if settings.group_notes && generated.len() >= 2 {
            let group_id = group_ids.allocate()?;
            for note in &mut generated {
                note.group_id = Some(group_id);
            }
        }
        output_count += generated.len();
        let anchor = source_indices
            .iter()
            .copied()
            .min()
            .ok_or(PianoRollEditError::InvalidTarget)?;
        replacements.insert(anchor, generated);
    }

    let mut result = Vec::with_capacity(notes.len() - target_ids.len() + output_count);
    for (index, note) in notes.iter().enumerate() {
        if let Some(generated) = replacements.remove(&index) {
            result.extend(generated);
        }
        if !target_ids.contains(&note.id) {
            result.push(note.clone());
        }
    }
    normalize_piano_note_groups(&mut result);
    Ok(PianoRollTransformResult {
        notes: result,
        selection_ids,
        transformed_notes: target_ids.len(),
        generated_notes: output_count.saturating_sub(target_ids.len()),
    })
}

fn arpeggio_source_groups(notes: &[PianoNote], target_ids: &HashSet<u64>) -> Vec<Vec<usize>> {
    let mut indices = notes
        .iter()
        .enumerate()
        .filter_map(|(index, note)| target_ids.contains(&note.id).then_some(index))
        .collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        notes[*left]
            .channel_id
            .cmp(&notes[*right].channel_id)
            .then_with(|| notes[*left].start.total_cmp(&notes[*right].start))
            .then_with(|| notes[*left].note.cmp(&notes[*right].note))
            .then_with(|| notes[*left].id.cmp(&notes[*right].id))
    });
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for index in indices {
        let belongs_to_last = groups.last().is_some_and(|group| {
            notes[group[0]].channel_id == notes[index].channel_id
                && (notes[group[0]].start - notes[index].start).abs() <= NOTE_TIME_EPSILON
        });
        if belongs_to_last {
            groups.last_mut().expect("group exists").push(index);
        } else {
            groups.push(vec![index]);
        }
    }
    groups
}

fn arpeggio_pitch_pattern(
    notes: &[PianoNote],
    source_indices: &[usize],
    settings: ArpeggiateSettings,
) -> Vec<ArpeggioPitch> {
    let mut sources = source_indices.to_vec();
    sources.sort_by_key(|index| (notes[*index].note, notes[*index].id));
    let mut pitches = Vec::new();
    let mut seen = BTreeSet::new();
    for octave in 0..settings.range_octaves {
        for index in &sources {
            let shifted = u16::from(notes[*index].note) + u16::from(octave) * 12;
            if shifted <= 127 && seen.insert(shifted as u8) {
                pitches.push(ArpeggioPitch {
                    note: shifted as u8,
                    velocity: notes[*index].velocity,
                    muted: notes[*index].muted,
                });
            }
        }
    }
    pitches.sort_by_key(|pitch| pitch.note);
    match settings.direction {
        ArpeggioDirection::Up => pitches,
        ArpeggioDirection::Down => {
            pitches.reverse();
            pitches
        }
        ArpeggioDirection::UpDown => ping_pong_pattern(&pitches),
        ArpeggioDirection::DownUp => {
            pitches.reverse();
            ping_pong_pattern(&pitches)
        }
    }
}

fn ping_pong_pattern(pitches: &[ArpeggioPitch]) -> Vec<ArpeggioPitch> {
    if pitches.len() < 2 {
        return pitches.to_vec();
    }
    let mut pattern = pitches.to_vec();
    pattern.extend(pitches[1..pitches.len() - 1].iter().rev().copied());
    pattern
}

pub fn quantize_beat(value: f32, snap: f32, bypass_snap: bool) -> Result<f32, PianoRollEditError> {
    if !value.is_finite() {
        return Err(PianoRollEditError::InvalidNumber);
    }
    if bypass_snap {
        return Ok(value);
    }
    if !snap.is_finite() || snap <= 0.0 {
        return Err(PianoRollEditError::InvalidSnap);
    }
    Ok(if let Some(mode) = PianoSnap::from_step(snap) {
        piano_snap::quantize_nearest(f64::from(value), mode) as f32
    } else {
        (f64::from(value) / f64::from(snap)).round() as f32 * snap
    })
}

pub fn moved_note_start(
    origin: f32,
    delta: f32,
    snap: f32,
    bypass_snap: bool,
) -> Result<f32, PianoRollEditError> {
    let raw = origin + delta;
    if !raw.is_finite() {
        return Err(PianoRollEditError::InvalidNumber);
    }
    Ok(quantize_beat(raw, snap, bypass_snap)?.max(0.0))
}

pub fn resized_note_length(
    origin: f32,
    delta: f32,
    snap: f32,
    bypass_snap: bool,
) -> Result<f32, PianoRollEditError> {
    if delta == 0.0 {
        return origin
            .is_finite()
            .then_some(origin.max(MIN_NOTE_LENGTH_BEATS))
            .ok_or(PianoRollEditError::InvalidNumber);
    }
    let raw = origin + delta;
    if !raw.is_finite() {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let quantized = quantize_beat(raw, snap, bypass_snap)?;
    Ok(quantized.max(MIN_NOTE_LENGTH_BEATS))
}

/// Clamps a requested common move so a selected group keeps its spacing while
/// remaining inside the editor's legal time and MIDI-pitch range.
pub fn clamp_group_move_delta(
    notes: &[PianoNote],
    requested_time_delta: f32,
    requested_pitch_delta: i16,
) -> Result<(f32, i16), PianoRollEditError> {
    if !requested_time_delta.is_finite()
        || notes
            .iter()
            .any(|note| !note.start.is_finite() || note.note > 127 || note.id == 0)
    {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let Some(minimum_start) = notes.iter().map(|note| note.start).reduce(f32::min) else {
        return Ok((requested_time_delta, requested_pitch_delta));
    };
    let minimum_pitch = notes
        .iter()
        .map(|note| i16::from(note.note))
        .min()
        .unwrap_or(0);
    let maximum_pitch = notes
        .iter()
        .map(|note| i16::from(note.note))
        .max()
        .unwrap_or(127);
    Ok((
        requested_time_delta.max(-minimum_start),
        requested_pitch_delta.clamp(-minimum_pitch, 127 - maximum_pitch),
    ))
}

/// Clamps a common right-edge resize delta without changing relative note
/// lengths or ever producing a zero/negative note.
pub fn clamp_group_resize_delta(
    notes: &[PianoNote],
    requested_delta: f32,
) -> Result<f32, PianoRollEditError> {
    if !requested_delta.is_finite()
        || notes
            .iter()
            .any(|note| !note.length.is_finite() || note.length <= 0.0 || note.id == 0)
    {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let Some(minimum_length) = notes.iter().map(|note| note.length).reduce(f32::min) else {
        return Ok(requested_delta);
    };
    Ok(requested_delta.max(MIN_NOTE_LENGTH_BEATS - minimum_length))
}

/// Clamps a common velocity delta so selected notes preserve their expression
/// differences rather than collapsing to one absolute value.
pub fn clamp_group_velocity_delta(
    notes: &[PianoNote],
    requested_delta: f32,
) -> Result<f32, PianoRollEditError> {
    if !requested_delta.is_finite()
        || notes
            .iter()
            .any(|note| !note.velocity.is_finite() || !(0.0..=1.0).contains(&note.velocity))
    {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let Some(minimum) = notes.iter().map(|note| note.velocity).reduce(f32::min) else {
        return Ok(requested_delta);
    };
    let maximum = notes
        .iter()
        .map(|note| note.velocity)
        .reduce(f32::max)
        .unwrap_or(minimum);
    Ok(requested_delta.clamp(-minimum, 1.0 - maximum))
}

/// Immediate, channel-scoped melody edits. Time and pitch moves use one common
/// bounded delta, preserving intervals even at the edges of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PianoKeyboardEdit {
    MoveSteps(i8),
    Transpose(i16),
    RepeatRight,
    DiscardLengths,
    QuickQuantize { starts_only: bool },
}

pub const MAX_PIANO_BEAT: f64 = 4096.0;

/// Group expansion is channel-qualified and linear, including large selections.
/// No selection in the active channel means all notes in that channel.
pub fn piano_keyboard_targets(
    notes: &[PianoNote],
    selection: &HashSet<u64>,
    channel_id: u32,
    grouping: bool,
) -> HashSet<u64> {
    let selected = notes
        .iter()
        .filter(|n| n.channel_id == Some(channel_id) && selection.contains(&n.id));
    let groups: HashSet<_> = if grouping {
        selected.clone().filter_map(|n| n.group_id).collect()
    } else {
        HashSet::new()
    };
    let has_selection = selected.count() != 0;
    notes
        .iter()
        .filter(|n| {
            n.channel_id == Some(channel_id)
                && (!has_selection
                    || selection.contains(&n.id)
                    || n.group_id.is_some_and(|g| groups.contains(&g)))
        })
        .map(|n| n.id)
        .collect()
}

fn validate_keyboard_notes(
    notes: &[PianoNote],
    targets: &HashSet<u64>,
) -> Result<(), PianoRollEditError> {
    if notes.len() > MAX_TRANSFORM_NOTES {
        return Err(PianoRollEditError::TransformTooLarge);
    }
    validate_group_edit_target(notes, targets)?;
    for n in notes.iter().filter(|n| targets.contains(&n.id)) {
        let end = f64::from(n.start) + f64::from(n.length);
        if !n.start.is_finite()
            || n.start < 0.0
            || !n.length.is_finite()
            || n.length < 0.01
            || end > MAX_PIANO_BEAT
            || n.note > 127
            || !n.velocity.is_finite()
            || !(0.0..=1.0).contains(&n.velocity)
            || n.group_id == Some(0)
        {
            return Err(PianoRollEditError::InvalidNumber);
        }
    }
    Ok(())
}

/// The caller supplies the maximum note ID across the entire project, so phrase
/// copies cannot collide with another pattern. Inputs remain untouched on error.
/// Repeat spacing is Citrus's explicit selected extent, not an undocumented FL
/// bar-rounding approximation. The range-aware entry point overrides that spacing.
pub fn edit_piano_notes_from_keyboard(
    notes: &[PianoNote],
    targets: &HashSet<u64>,
    edit: PianoKeyboardEdit,
    snap: f32,
    project_note_id_floor: u64,
) -> Result<DuplicateNotesResult, PianoRollEditError> {
    edit_piano_notes_from_keyboard_with_range(
        notes,
        targets,
        edit,
        snap,
        project_note_id_floor,
        None,
    )
}

pub fn edit_piano_notes_from_keyboard_with_range(
    notes: &[PianoNote],
    targets: &HashSet<u64>,
    edit: PianoKeyboardEdit,
    snap: f32,
    project_note_id_floor: u64,
    repeat_range: Option<PianoTimeRange>,
) -> Result<DuplicateNotesResult, PianoRollEditError> {
    if repeat_range.is_some_and(|range| PianoTimeRange::new(range.start, range.end) != Some(range))
    {
        return Err(PianoRollEditError::InvalidNumber);
    }
    validate_keyboard_notes(notes, targets)?;
    if targets.is_empty() {
        return Ok(DuplicateNotesResult {
            notes: notes.to_vec(),
            selection_ids: targets.clone(),
        });
    }
    if !matches!(edit, PianoKeyboardEdit::Transpose(_))
        && (!snap.is_finite() || snap <= 0.0 || f64::from(snap) > MAX_PIANO_BEAT)
    {
        return Err(PianoRollEditError::InvalidSnap);
    }
    let exact_snap = PianoSnap::from_step(snap)
        .and_then(PianoSnap::step)
        .unwrap_or(f64::from(snap));
    let origins: Vec<_> = notes
        .iter()
        .filter(|n| targets.contains(&n.id))
        .cloned()
        .collect();
    let minimum_start = origins
        .iter()
        .map(|n| f64::from(n.start))
        .reduce(f64::min)
        .unwrap();
    let maximum_end = origins
        .iter()
        .map(|n| f64::from(n.start) + f64::from(n.length))
        .reduce(f64::max)
        .unwrap();
    let mut result = DuplicateNotesResult {
        notes: notes.to_vec(),
        selection_ids: targets.clone(),
    };
    match edit {
        PianoKeyboardEdit::MoveSteps(steps) => {
            // Round each endpoint constraint inward before choosing the shared
            // delta. A direct f64 clamp followed by f32 rounding could otherwise
            // turn e.g. a 0.15-beat note into a tiny 4096-beat overshoot.
            let maximum_delta = origins
                .iter()
                .map(|n| {
                    let limit = MAX_PIANO_BEAT - f64::from(n.length);
                    let rounded = limit as f32;
                    let safe_start = if f64::from(rounded) > limit {
                        rounded.next_down()
                    } else {
                        rounded
                    };
                    f64::from(safe_start) - f64::from(n.start)
                })
                .reduce(f64::min)
                .unwrap();
            // Canonical on-grid f32 anchors retain their rational grid index
            // through repeated nudges instead of accumulating model rounding.
            // Off-grid phrases keep their common relative offset and are not quantized.
            let requested_delta = PianoSnap::from_step(snap)
                .filter(|mode| *mode != PianoSnap::Off)
                .and_then(|mode| {
                    let canonical = piano_snap::quantize_nearest(minimum_start, mode);
                    ((canonical as f32) == minimum_start as f32)
                        .then_some(canonical + exact_snap * f64::from(steps) - minimum_start)
                })
                .unwrap_or(exact_snap * f64::from(steps));
            let delta = requested_delta.clamp(-minimum_start, maximum_delta);
            for n in result.notes.iter_mut().filter(|n| targets.contains(&n.id)) {
                n.start = (f64::from(n.start) + delta) as f32;
            }
        }
        PianoKeyboardEdit::Transpose(semitones) => {
            let (_, delta) = clamp_group_move_delta(&origins, 0.0, semitones)?;
            for n in result.notes.iter_mut().filter(|n| targets.contains(&n.id)) {
                n.note = (i16::from(n.note) + delta) as u8;
            }
        }
        PianoKeyboardEdit::RepeatRight => {
            let delta = repeat_range.map_or_else(
                || (maximum_end - minimum_start).max(exact_snap),
                |range| range.end - range.start,
            );
            if maximum_end + delta > MAX_PIANO_BEAT {
                return Err(PianoRollEditError::InvalidNumber);
            }
            if notes.len().saturating_add(targets.len()) > MAX_TRANSFORM_NOTES {
                return Err(PianoRollEditError::TransformTooLarge);
            }
            result = if project_note_id_floor == 0 {
                duplicate_selected_notes(notes, targets, delta as f32)?
            } else {
                duplicate_selected_notes_with_floor(
                    notes,
                    targets,
                    delta as f32,
                    project_note_id_floor,
                )?
            };
            for (original, candidate) in notes.iter().zip(&mut result.notes) {
                candidate.selected = original.selected;
            }
        }
        PianoKeyboardEdit::DiscardLengths => {
            for n in result.notes.iter_mut().filter(|n| targets.contains(&n.id)) {
                n.length = snap.max(MIN_NOTE_LENGTH_BEATS);
            }
        }
        PianoKeyboardEdit::QuickQuantize { starts_only } => {
            let mut settings = QuantizeSettings::new(snap);
            if starts_only {
                settings.duration_mode = QuantizeDurationMode::LeaveDuration;
            }
            result.notes = quantize_notes(notes, targets, settings)?.notes;
        }
    }
    validate_keyboard_notes(&result.notes, &result.selection_ids)?;
    Ok(result)
}

/// Returns an all-or-none duplicated note set.
///
/// Every selected note moves by the same positive delta. There is deliberately
/// no legacy sixteen-beat ceiling: the editor horizon follows note content.
pub fn duplicate_selected_notes(
    notes: &[PianoNote],
    selection_ids: &HashSet<u64>,
    delta: f32,
) -> Result<DuplicateNotesResult, PianoRollEditError> {
    duplicate_selected_notes_with_floor(notes, selection_ids, delta, 0)
}

fn duplicate_selected_notes_with_floor(
    notes: &[PianoNote],
    selection_ids: &HashSet<u64>,
    delta: f32,
    id_floor: u64,
) -> Result<DuplicateNotesResult, PianoRollEditError> {
    if !delta.is_finite() || delta <= 0.0 {
        return Err(PianoRollEditError::InvalidNumber);
    }
    let selected = notes
        .iter()
        .filter(|note| selection_ids.contains(&note.id))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Ok(DuplicateNotesResult {
            notes: notes.to_vec(),
            selection_ids: selection_ids.clone(),
        });
    }

    let mut used = notes.iter().map(|note| note.id).collect::<HashSet<_>>();
    let mut next_id = used.iter().copied().max().unwrap_or(0).max(id_floor);
    let selected_group_counts = selected.iter().fold(BTreeMap::new(), |mut counts, note| {
        if let Some(group_id) = note.group_id {
            *counts.entry((note.channel_id, group_id)).or_insert(0_usize) += 1;
        }
        counts
    });
    let mut group_ids = NoteGroupIdAllocator::new(notes)?;
    let mut duplicated_groups = BTreeMap::new();
    for (group, count) in selected_group_counts {
        if count >= 2 {
            duplicated_groups.insert(group, group_ids.allocate()?);
        }
    }
    let mut copies = Vec::with_capacity(selected.len());
    let mut duplicated_selection = HashSet::with_capacity(selected.len());
    for note in selected {
        let start = note.start + delta;
        if !start.is_finite() || start < 0.0 {
            return Err(PianoRollEditError::InvalidNumber);
        }
        let id = loop {
            next_id = next_id
                .checked_add(1)
                .ok_or(PianoRollEditError::NoteIdExhausted)?;
            if next_id != 0 && used.insert(next_id) {
                break next_id;
            }
        };
        let mut copy = note.clone();
        copy.id = id;
        copy.group_id = note
            .group_id
            .and_then(|group_id| duplicated_groups.get(&(note.channel_id, group_id)).copied());
        copy.start = start;
        copy.selected = false;
        duplicated_selection.insert(id);
        copies.push(copy);
    }

    let mut result = notes.to_vec();
    for note in &mut result {
        note.selected = false;
    }
    result.extend(copies);
    Ok(DuplicateNotesResult {
        notes: result,
        selection_ids: duplicated_selection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u64, start: f32, length: f32, selected: bool) -> PianoNote {
        PianoNote {
            id,
            channel_id: Some(1),
            group_id: None,
            note: 60,
            start,
            length,
            velocity: 0.75,
            selected,
            muted: false,
        }
    }

    #[test]
    fn coarse_snap_never_panics_or_grows_a_short_tail_note() {
        assert_eq!(resized_note_length(0.25, 0.0, 1.0, false), Ok(0.25));
        assert_eq!(resized_note_length(0.25, 0.0, 1.0, true), Ok(0.25));
    }

    #[test]
    fn alt_bypass_preserves_free_timing() {
        assert_eq!(moved_note_start(1.13, 0.11, 0.25, true), Ok(1.24));
        assert_eq!(moved_note_start(1.13, 0.11, 0.25, false), Ok(1.25));
    }

    #[test]
    fn duplicate_keeps_remote_group_spacing_and_long_lengths() {
        let original = vec![
            note(1, 20.0, 18.0, true),
            note(2, 27.5, 0.25, true),
            note(3, 1.0, 1.0, false),
        ];
        let result = duplicate_selected_notes(&original, &HashSet::from([1, 2]), 0.25).unwrap();
        assert_eq!(result.notes.len(), 5);
        assert_eq!(result.notes[3].start, 20.25);
        assert_eq!(result.notes[3].length, 18.0);
        assert_eq!(result.notes[4].start, 27.75);
        assert_eq!(result.notes[4].start - result.notes[3].start, 7.5);
        assert!(result.notes.iter().all(|note| !note.selected));
        assert_eq!(result.selection_ids, HashSet::from([4, 5]));
    }

    #[test]
    fn duplicate_failure_is_all_or_none() {
        let original = vec![note(u64::MAX, 1.0, 1.0, true)];
        assert!(matches!(
            duplicate_selected_notes(&original, &HashSet::from([u64::MAX]), 0.25),
            Err(PianoRollEditError::NoteIdExhausted)
        ));
        assert!(original[0].selected);
    }

    #[test]
    fn selection_state_is_separate_from_project_notes() {
        let notes = [note(1, 0.0, 1.0, false), note(2, 1.0, 1.0, false)];
        let mut state = PianoRollState::default();
        state.select_only(1);
        toggle_note_group_selection(&notes, &mut state.selection_ids, 2, false);
        assert_eq!(state.selection_ids.len(), 2);
        state.retain_existing(&notes[1..]);
        assert_eq!(state.selection_ids, HashSet::from([2]));
        assert!(notes.iter().all(|note| !note.selected));
    }

    #[test]
    fn enabled_grouping_expands_and_toggles_only_the_anchor_channel() {
        let mut notes = vec![
            note(1, 0.0, 1.0, false),
            note(2, 1.0, 1.0, false),
            note(3, 2.0, 1.0, false),
            note(4, 3.0, 1.0, false),
        ];
        notes[0].group_id = Some(7);
        notes[1].group_id = Some(7);
        notes[3].group_id = Some(7);
        notes[3].channel_id = Some(2);

        assert_eq!(note_group_members(&notes, 1, true), HashSet::from([1, 2]));
        assert_eq!(note_group_members(&notes, 1, false), HashSet::from([1]));

        let mut selection = HashSet::new();
        toggle_note_group_selection(&notes, &mut selection, 1, true);
        assert_eq!(selection, HashSet::from([1, 2]));
        toggle_note_group_selection(&notes, &mut selection, 2, true);
        assert!(selection.is_empty());
    }

    #[test]
    fn grouping_and_ungrouping_are_deterministic_all_or_none_edits() {
        let notes = vec![
            note(1, 0.0, 1.0, false),
            note(2, 1.0, 1.0, false),
            note(3, 2.0, 1.0, false),
        ];
        let grouped = group_selected_notes(&notes, &HashSet::from([1, 2])).unwrap();
        assert_eq!(grouped.affected_notes, 2);
        assert_eq!(grouped.group_id, Some(1));
        assert_eq!(grouped.notes[0].group_id, Some(1));
        assert_eq!(grouped.notes[1].group_id, Some(1));
        assert_eq!(grouped.notes[2].group_id, None);

        let ungrouped = ungroup_selected_notes(&grouped.notes, &HashSet::from([1])).unwrap();
        assert_eq!(ungrouped.affected_notes, 1);
        assert!(ungrouped.notes.iter().all(|note| note.group_id.is_none()));

        let mut cross_channel = notes.clone();
        cross_channel[1].channel_id = Some(2);
        assert_eq!(
            group_selected_notes(&cross_channel, &HashSet::from([1, 2])).unwrap_err(),
            PianoRollEditError::InvalidTarget
        );
    }

    #[test]
    fn duplicate_remaps_complete_groups_without_joining_the_source_group() {
        let mut notes = vec![
            note(1, 0.0, 1.0, false),
            note(2, 1.0, 1.0, false),
            note(3, 2.0, 1.0, false),
        ];
        notes[0].group_id = Some(7);
        notes[1].group_id = Some(7);
        let duplicated = duplicate_selected_notes(&notes, &HashSet::from([1, 2]), 0.25).unwrap();
        assert_eq!(duplicated.notes[3].group_id, Some(8));
        assert_eq!(duplicated.notes[4].group_id, Some(8));
        assert_ne!(duplicated.notes[3].group_id, notes[0].group_id);

        let singleton = duplicate_selected_notes(&notes, &HashSet::from([1]), 0.25).unwrap();
        assert_eq!(singleton.notes[3].group_id, None);
    }

    #[test]
    fn group_move_keeps_spacing_at_time_and_pitch_boundaries() {
        let mut low = note(1, 0.25, 1.0, false);
        low.note = 2;
        let mut high = note(2, 4.25, 1.0, false);
        high.note = 126;
        assert_eq!(
            clamp_group_move_delta(&[low, high], -2.0, 8),
            Ok((-0.25, 1))
        );
    }

    #[test]
    fn group_resize_and_velocity_preserve_relative_values() {
        let mut quiet = note(1, 0.0, 0.25, false);
        quiet.velocity = 0.1;
        let mut loud = note(2, 1.0, 1.5, false);
        loud.velocity = 0.9;
        assert_eq!(
            clamp_group_resize_delta(&[quiet.clone(), loud.clone()], -1.0),
            Ok(MIN_NOTE_LENGTH_BEATS - 0.25)
        );
        let velocity_delta = clamp_group_velocity_delta(&[quiet, loud], 0.5).unwrap();
        assert!((velocity_delta - 0.1).abs() < 0.000_001);
    }

    #[test]
    fn quantize_respects_scope_strength_sensitivity_and_duration_mode() {
        let original = vec![note(1, 0.24, 0.48, false), note(2, 0.19, 0.31, false)];
        let targets = HashSet::from([1]);
        let settings = PianoRollTransformSettings::Quantize(QuantizeSettings {
            snap_beats: 0.25,
            start_strength: 1.0,
            sensitivity: 1.0,
            duration_strength: 1.0,
            duration_mode: QuantizeDurationMode::QuantizeDuration,
        });
        let result = transform_piano_notes(
            &original,
            &targets,
            PianoRollTransformKind::Quantize,
            &settings,
            120.0,
        )
        .unwrap();
        assert!((result.notes[0].start - 0.25).abs() < NOTE_TIME_EPSILON);
        assert!((result.notes[0].length - 0.5).abs() < NOTE_TIME_EPSILON);
        assert_eq!(result.notes[1].start, original[1].start);
        assert_eq!(result.notes[1].length, original[1].length);

        let insensitive = PianoRollTransformSettings::Quantize(QuantizeSettings {
            sensitivity: 0.0,
            ..QuantizeSettings::new(0.25)
        });
        let result = transform_piano_notes(
            &original,
            &targets,
            PianoRollTransformKind::Quantize,
            &insensitive,
            120.0,
        )
        .unwrap();
        assert_eq!(result.notes[0].start, original[0].start);
        assert_eq!(result.notes[0].length, original[0].length);
    }

    #[test]
    fn quantize_leave_end_never_crosses_the_original_end() {
        let original = vec![note(1, 0.38, 0.12, false)];
        let settings = PianoRollTransformSettings::Quantize(QuantizeSettings {
            duration_mode: QuantizeDurationMode::LeaveEndTime,
            ..QuantizeSettings::new(0.25)
        });
        let result = transform_piano_notes(
            &original,
            &HashSet::from([1]),
            PianoRollTransformKind::Quantize,
            &settings,
            120.0,
        )
        .unwrap();
        assert!(result.notes[0].length >= MIN_NOTE_LENGTH_BEATS);
        assert!((result.notes[0].start + result.notes[0].length - 0.5).abs() < NOTE_TIME_EPSILON);
    }

    #[test]
    fn strum_orders_a_chord_and_preserves_its_end_times() {
        let mut low = note(1, 1.0, 1.0, false);
        low.note = 48;
        low.velocity = 0.8;
        let mut middle = note(2, 1.0, 1.0, false);
        middle.note = 60;
        middle.velocity = 0.8;
        let mut high = note(3, 1.0, 1.0, false);
        high.note = 72;
        high.velocity = 0.8;
        let settings = PianoRollTransformSettings::Strum(StrumSettings {
            start_time_beats: 0.2,
            velocity_change: 0.2,
            ..StrumSettings::default()
        });
        let result = transform_piano_notes(
            &[low, middle, high],
            &HashSet::from([1, 2, 3]),
            PianoRollTransformKind::Strum,
            &settings,
            120.0,
        )
        .unwrap();
        assert!((result.notes[0].start - 1.0).abs() < NOTE_TIME_EPSILON);
        assert!((result.notes[1].start - 1.1).abs() < NOTE_TIME_EPSILON);
        assert!((result.notes[2].start - 1.2).abs() < NOTE_TIME_EPSILON);
        for note in &result.notes {
            assert!((note.start + note.length - 2.0).abs() < NOTE_TIME_EPSILON);
        }
        assert!((result.notes[0].velocity - 0.8).abs() < NOTE_TIME_EPSILON);
        assert!((result.notes[2].velocity - 0.6).abs() < NOTE_TIME_EPSILON);
    }

    #[test]
    fn negative_strum_reverses_pitch_order_and_trigger_ahead_clamps_as_a_group() {
        let mut low = note(1, 0.02, 0.5, false);
        low.note = 48;
        let mut high = note(2, 0.02, 0.5, false);
        high.note = 72;
        let settings = PianoRollTransformSettings::Strum(StrumSettings {
            start_time_beats: -0.2,
            trigger_ahead: true,
            ..StrumSettings::default()
        });
        let result = transform_piano_notes(
            &[low, high],
            &HashSet::from([1, 2]),
            PianoRollTransformKind::Strum,
            &settings,
            120.0,
        )
        .unwrap();
        assert!(result.notes[1].start < result.notes[0].start);
        assert!(result.notes.iter().all(|note| note.start >= 0.0));
        assert!((result.notes[0].start - result.notes[1].start - 0.2).abs() < NOTE_TIME_EPSILON);
    }

    #[test]
    fn chop_is_relative_or_grid_absolute_and_allocates_stable_ids() {
        let original = vec![note(1, 0.1, 1.0, false), note(2, 4.0, 0.5, false)];
        let relative = PianoRollTransformSettings::Chop(ChopSettings::new(0.25));
        let result = transform_piano_notes(
            &original,
            &HashSet::from([1]),
            PianoRollTransformKind::Chop,
            &relative,
            120.0,
        )
        .unwrap();
        assert_eq!(result.notes.len(), 5);
        assert_eq!(result.generated_notes, 3);
        assert_eq!(
            result.notes.iter().map(|note| note.id).collect::<Vec<_>>(),
            vec![1, 3, 4, 5, 2]
        );
        assert_eq!(
            result.notes[..4]
                .iter()
                .map(|note| note.start)
                .collect::<Vec<_>>(),
            vec![0.1, 0.35, 0.6, 0.85]
        );
        assert_eq!(result.notes[4].start, 4.0);

        let absolute = PianoRollTransformSettings::Chop(ChopSettings {
            absolute_pattern: true,
            ..ChopSettings::new(0.25)
        });
        let result = transform_piano_notes(
            &[note(1, 0.1, 0.6, false)],
            &HashSet::from([1]),
            PianoRollTransformKind::Chop,
            &absolute,
            120.0,
        )
        .unwrap();
        assert_eq!(
            result
                .notes
                .iter()
                .map(|note| note.start)
                .collect::<Vec<_>>(),
            vec![0.1, 0.25, 0.5]
        );
        assert!(
            result
                .notes
                .iter()
                .all(|note| note.length >= MIN_NOTE_LENGTH_BEATS)
        );
    }

    #[test]
    fn flam_adds_grace_notes_without_crossing_time_zero() {
        let original = vec![note(1, 0.1, 0.5, false), note(2, 0.0, 0.5, false)];
        let settings = PianoRollTransformSettings::Flam(FlamSettings::default());
        let result = transform_piano_notes(
            &original,
            &HashSet::from([1, 2]),
            PianoRollTransformKind::Flam,
            &settings,
            120.0,
        )
        .unwrap();
        assert_eq!(result.generated_notes, 1);
        assert_eq!(result.notes.len(), 3);
        let grace = &result.notes[1];
        assert_eq!(grace.id, 3);
        assert!((grace.start - 0.0375).abs() < NOTE_TIME_EPSILON);
        assert!((grace.length - 0.0625).abs() < NOTE_TIME_EPSILON);
        assert_eq!(grace.velocity, 0.55);
        assert!(result.notes.iter().all(|note| note.start >= 0.0));
        assert_eq!(result.selection_ids, HashSet::from([1, 2, 3]));
    }

    #[test]
    fn articulate_legato_reaches_the_next_channel_onset_without_touching_other_notes() {
        let mut first = note(1, 0.0, 0.25, false);
        first.note = 60;
        let mut chord_tone = note(2, 0.0, 0.5, false);
        chord_tone.note = 64;
        let mut boundary = note(3, 1.0, 0.75, false);
        boundary.note = 67;
        let mut other_channel = note(4, 0.5, 0.5, false);
        other_channel.channel_id = Some(2);
        let original = vec![first, chord_tone, boundary, other_channel];
        let settings = PianoRollTransformSettings::Articulate(ArticulateSettings::default());
        let result = transform_piano_notes(
            &original,
            &HashSet::from([1, 2]),
            PianoRollTransformKind::Articulate,
            &settings,
            120.0,
        )
        .unwrap();

        assert!((result.notes[0].length - 1.0).abs() < NOTE_TIME_EPSILON);
        assert!((result.notes[1].length - 1.0).abs() < NOTE_TIME_EPSILON);
        assert_eq!(result.notes[2].length, original[2].length);
        assert_eq!(result.notes[3].length, original[3].length);
        assert_eq!(result.selection_ids, HashSet::from([1, 2]));
        assert_eq!(result.generated_notes, 0);
    }

    #[test]
    fn articulate_presets_and_selection_boundaries_are_deterministic() {
        let original = vec![note(1, 0.0, 0.8, false), note(2, 1.0, 1.0, false)];
        let targets = HashSet::from([1]);
        for (preset, expected) in [
            (ArticulationPreset::Portato, 0.9),
            (ArticulationPreset::Staccato, 0.4),
            (ArticulationPreset::SmallGap, 1.0 - MIN_NOTE_LENGTH_BEATS),
            (ArticulationPreset::ChopChords, 0.8),
        ] {
            let settings =
                PianoRollTransformSettings::Articulate(ArticulateSettings::for_preset(preset));
            let result = transform_piano_notes(
                &original,
                &targets,
                PianoRollTransformKind::Articulate,
                &settings,
                120.0,
            )
            .unwrap();
            assert!((result.notes[0].length - expected).abs() < NOTE_TIME_EPSILON);
        }

        let selection_only = PianoRollTransformSettings::Articulate(ArticulateSettings {
            only_with_selection: true,
            ..ArticulateSettings::default()
        });
        let result = transform_piano_notes(
            &original,
            &targets,
            PianoRollTransformKind::Articulate,
            &selection_only,
            120.0,
        )
        .unwrap();
        assert_eq!(result.notes[0].length, original[0].length);

        let varied = PianoRollTransformSettings::Articulate(ArticulateSettings {
            variation: 0.3,
            seed: 42,
            use_lengths: true,
            chop_chords: false,
            ..ArticulateSettings::default()
        });
        let first = transform_piano_notes(
            &original,
            &targets,
            PianoRollTransformKind::Articulate,
            &varied,
            120.0,
        )
        .unwrap();
        let second = transform_piano_notes(
            &original,
            &targets,
            PianoRollTransformKind::Articulate,
            &varied,
            120.0,
        )
        .unwrap();
        assert_eq!(first.notes[0].length, second.notes[0].length);
        assert_ne!(first.notes[0].length, original[0].length);
    }

    #[test]
    fn arpeggiate_uses_direction_gate_stable_ids_and_one_new_group() {
        let mut root = note(1, 0.0, 1.0, false);
        root.note = 60;
        let mut third = note(2, 0.0, 1.0, false);
        third.note = 64;
        let mut fifth = note(3, 0.0, 1.0, false);
        fifth.note = 67;
        let original = vec![root, third, fifth];
        let settings = PianoRollTransformSettings::Arpeggiate(ArpeggiateSettings {
            step_beats: 0.25,
            gate: 0.5,
            ..ArpeggiateSettings::new(0.25)
        });
        let result = transform_piano_notes(
            &original,
            &HashSet::from([1, 2, 3]),
            PianoRollTransformKind::Arpeggiate,
            &settings,
            120.0,
        )
        .unwrap();

        assert_eq!(
            result
                .notes
                .iter()
                .map(|note| note.note)
                .collect::<Vec<_>>(),
            vec![60, 64, 67, 60]
        );
        assert_eq!(
            result.notes.iter().map(|note| note.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(
            result
                .notes
                .iter()
                .all(|note| (note.length - 0.125).abs() < NOTE_TIME_EPSILON)
        );
        assert!(result.notes.iter().all(|note| note.group_id == Some(1)));
        assert_eq!(result.generated_notes, 1);
        assert_eq!(result.selection_ids, HashSet::from([1, 2, 3, 4]));

        let down_up = PianoRollTransformSettings::Arpeggiate(ArpeggiateSettings {
            direction: ArpeggioDirection::DownUp,
            sync: ArpeggioSync::Time,
            ..ArpeggiateSettings::new(0.25)
        });
        let result = transform_piano_notes(
            &original,
            &HashSet::from([1, 2, 3]),
            PianoRollTransformKind::Arpeggiate,
            &down_up,
            120.0,
        )
        .unwrap();
        assert_eq!(
            result
                .notes
                .iter()
                .map(|note| note.note)
                .collect::<Vec<_>>(),
            vec![67, 64, 60, 64]
        );
    }

    #[test]
    fn arpeggiate_sync_range_and_grouping_respect_source_boundaries() {
        let mut short = note(1, 0.0, 1.0, false);
        short.note = 60;
        short.group_id = Some(7);
        let mut long = note(2, 0.0, 2.0, false);
        long.note = 64;
        long.group_id = Some(7);
        let mut untouched = note(9, 3.0, 0.5, false);
        untouched.note = 72;
        untouched.group_id = Some(7);
        untouched.channel_id = Some(2);
        let original = vec![short, untouched.clone(), long];
        let targets = HashSet::from([1, 2]);

        for (sync, expected_count) in [
            (ArpeggioSync::Time, 2),
            (ArpeggioSync::Chord, 4),
            (ArpeggioSync::Block, 8),
        ] {
            let settings = PianoRollTransformSettings::Arpeggiate(ArpeggiateSettings {
                step_beats: 0.25,
                sync,
                group_notes: false,
                ..ArpeggiateSettings::new(0.25)
            });
            let result = transform_piano_notes(
                &original,
                &targets,
                PianoRollTransformKind::Arpeggiate,
                &settings,
                120.0,
            )
            .unwrap();
            assert_eq!(result.notes.len(), expected_count + 1);
            assert_eq!(result.notes.last().unwrap().id, untouched.id);
            assert!(result.notes.iter().all(|note| note.group_id.is_none()));
        }

        let mut high = note(1, 0.0, 1.0, false);
        high.note = 120;
        let mut ceiling = note(2, 0.0, 1.0, false);
        ceiling.note = 127;
        let ranged = PianoRollTransformSettings::Arpeggiate(ArpeggiateSettings {
            range_octaves: 4,
            sync: ArpeggioSync::Time,
            ..ArpeggiateSettings::new(0.25)
        });
        let result = transform_piano_notes(
            &[high, ceiling],
            &targets,
            PianoRollTransformKind::Arpeggiate,
            &ranged,
            120.0,
        )
        .unwrap();
        assert_eq!(
            result
                .notes
                .iter()
                .map(|note| note.note)
                .collect::<Vec<_>>(),
            vec![120, 127]
        );
    }

    #[test]
    fn articulate_and_arpeggiate_reject_invalid_settings_all_or_none() {
        let original = vec![note(1, 0.0, 1.0, false), note(2, 1.0, 1.0, false)];
        let targets = HashSet::from([1]);
        let articulate = PianoRollTransformSettings::Articulate(ArticulateSettings {
            multiply: f32::NAN,
            ..ArticulateSettings::default()
        });
        assert_eq!(
            transform_piano_notes(
                &original,
                &targets,
                PianoRollTransformKind::Articulate,
                &articulate,
                120.0,
            )
            .unwrap_err(),
            PianoRollEditError::InvalidNumber
        );
        let arpeggiate = PianoRollTransformSettings::Arpeggiate(ArpeggiateSettings {
            range_octaves: 0,
            ..ArpeggiateSettings::new(0.25)
        });
        assert_eq!(
            transform_piano_notes(
                &original,
                &targets,
                PianoRollTransformKind::Arpeggiate,
                &arpeggiate,
                120.0,
            )
            .unwrap_err(),
            PianoRollEditError::InvalidNumber
        );
        assert_eq!(original[0].length, 1.0);
        assert_eq!(original.len(), 2);
    }

    #[test]
    fn transform_rejects_stale_targets_and_mismatched_settings() {
        let notes = [note(1, 0.0, 1.0, false)];
        let settings = PianoRollTransformSettings::Quantize(QuantizeSettings::new(0.25));
        assert!(matches!(
            transform_piano_notes(
                &notes,
                &HashSet::from([2]),
                PianoRollTransformKind::Quantize,
                &settings,
                120.0,
            ),
            Err(PianoRollEditError::InvalidTarget)
        ));
        assert!(matches!(
            transform_piano_notes(
                &notes,
                &HashSet::from([1]),
                PianoRollTransformKind::Flam,
                &settings,
                120.0,
            ),
            Err(PianoRollEditError::MismatchedTransformSettings)
        ));
    }

    #[test]
    fn scale_membership_repeats_across_octaves_and_respects_root() {
        let c_major = [0_u8, 2, 4, 5, 7, 9, 11];
        for note in 0..=127_u8 {
            assert_eq!(
                pitch_class_in_scale(note, 0, PianoScale::Major),
                c_major.contains(&(note % 12))
            );
            assert_eq!(
                pitch_class_in_scale(note, 2, PianoScale::Major),
                c_major.contains(&((note % 12 + 10) % 12))
            );
            assert!(pitch_class_in_scale(note, 9, PianoScale::Chromatic));
        }
    }

    #[test]
    fn scale_snap_is_nearest_tie_lower_and_midi_bounded() {
        assert_eq!(snap_pitch_to_scale(61, 0, PianoScale::Major), 60);
        assert_eq!(snap_pitch_to_scale(63, 0, PianoScale::Major), 62);
        assert_eq!(snap_pitch_to_scale(60, 0, PianoScale::Major), 60);
        assert_eq!(snap_pitch_to_scale(0, 11, PianoScale::Major), 1);
        assert_eq!(snap_pitch_to_scale(127, 11, PianoScale::Major), 126);
    }

    #[test]
    fn chord_stamp_intervals_follow_scale_and_deduplicate_at_midi_ceiling() {
        assert_eq!(
            chord_stamp_pitches(60, PianoChordStamp::Major, 0, PianoScale::Major, false),
            vec![60, 64, 67]
        );
        assert_eq!(
            chord_stamp_pitches(60, PianoChordStamp::Minor, 0, PianoScale::Major, true),
            vec![60, 62, 67]
        );
        assert_eq!(
            chord_stamp_pitches(126, PianoChordStamp::Add9, 0, PianoScale::Chromatic, false),
            vec![126, 127]
        );
    }

    #[test]
    fn chord_stamp_allocates_stable_ids_selects_insertions_and_preserves_existing_notes() {
        let mut first = note(2, 0.0, 1.0, true);
        first.note = 48;
        let mut second = note(9, 2.0, 0.5, false);
        second.note = 52;
        let original = vec![first.clone(), second.clone()];
        let result = stamp_chord_notes(
            &original,
            Some(1),
            60,
            4.0,
            0.75,
            0.8,
            PianoChordStamp::Major,
            0,
            PianoScale::Major,
            false,
        )
        .unwrap();
        assert_eq!(
            result
                .inserted_notes
                .iter()
                .map(|note| (note.id, note.note))
                .collect::<Vec<_>>(),
            vec![(10, 60), (11, 64), (12, 67)]
        );
        assert_eq!(result.selection_ids, HashSet::from([10, 11, 12]));
        assert_eq!(result.notes[0].note, first.note);
        assert_eq!(result.notes[1].note, second.note);
        assert!(result.notes[..2].iter().all(|note| !note.selected));
        assert!(
            result
                .inserted_notes
                .iter()
                .all(|note| note.group_id.is_none()
                    && note.channel_id == Some(1)
                    && note.start == 4.0
                    && note.length == 0.75
                    && note.velocity == 0.8)
        );
    }

    #[test]
    fn chord_stamp_suppresses_only_matching_channel_pitch_and_step() {
        let mut c = note(1, 4.0, 1.0, false);
        c.note = 60;
        let mut e_other_channel = note(7, 4.0, 1.0, false);
        e_other_channel.note = 64;
        e_other_channel.channel_id = Some(2);
        let result = stamp_chord_notes(
            &[c, e_other_channel],
            Some(1),
            60,
            4.0,
            1.0,
            0.75,
            PianoChordStamp::Major,
            0,
            PianoScale::Major,
            false,
        )
        .unwrap();
        assert_eq!(
            result
                .inserted_notes
                .iter()
                .map(|note| note.note)
                .collect::<Vec<_>>(),
            vec![64, 67]
        );
        assert_eq!(
            result
                .inserted_notes
                .iter()
                .map(|note| note.id)
                .collect::<Vec<_>>(),
            vec![8, 9]
        );
    }

    #[test]
    fn chord_stamp_rejects_invalid_or_oversized_candidates_without_mutation() {
        let original = vec![note(1, 0.0, 1.0, false)];
        assert!(matches!(
            stamp_chord_notes(
                &original,
                Some(1),
                60,
                f32::NAN,
                1.0,
                0.75,
                PianoChordStamp::Major,
                0,
                PianoScale::Major,
                false,
            ),
            Err(PianoRollEditError::InvalidNumber)
        ));

        let oversized = (0..MAX_TRANSFORM_NOTES - 1)
            .map(|index| {
                let mut candidate = note(index as u64 + 1, index as f32 + 10.0, 1.0, false);
                candidate.note = 0;
                candidate
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            stamp_chord_notes(
                &oversized,
                Some(1),
                60,
                1.0,
                1.0,
                0.75,
                PianoChordStamp::Major,
                0,
                PianoScale::Major,
                false,
            ),
            Err(PianoRollEditError::TransformTooLarge)
        ));
        assert_eq!(original.len(), 1);
        assert_eq!(original[0].id, 1);
        assert_eq!(original[0].start, 0.0);
        assert_eq!(original[0].length, 1.0);
    }

    #[test]
    fn piano_roll_preferences_round_trip_and_invalid_values_reset_state() {
        let preferences = PianoRollPreferences {
            scale_root: 9,
            scale: PianoScale::HarmonicMinor,
            scale_highlighting: false,
            snap_to_scale: true,
            chord_stamp: PianoChordStamp::Minor7,
            stamp_only_one: true,
            ..PianoRollPreferences::default()
        };
        let encoded = serde_json::to_string(&preferences).unwrap();
        let decoded: PianoRollPreferences = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, preferences);
        assert_eq!(
            PianoRollState::from_preferences(decoded).preferences(),
            preferences
        );

        let invalid = PianoRollPreferences {
            scale_root: 12,
            ..preferences
        };
        assert_eq!(
            PianoRollState::from_preferences(invalid).preferences(),
            PianoRollPreferences::default()
        );
    }
}

#[cfg(test)]
mod keyboard_edit_tests {
    use super::*;

    fn notes() -> Vec<PianoNote> {
        [
            (1, 1, 0.125, 0.625, 60),
            (2, 1, 0.5, 0.25, 64),
            (3, 2, 0.0, 0.25, 127),
        ]
        .into_iter()
        .map(|(id, channel, start, length, pitch)| PianoNote {
            id,
            channel_id: Some(channel),
            group_id: Some(7),
            note: pitch,
            start,
            length,
            velocity: 0.6,
            selected: false,
            muted: id == 2,
        })
        .collect()
    }
    fn run(
        notes: &[PianoNote],
        edit: PianoKeyboardEdit,
    ) -> Result<DuplicateNotesResult, PianoRollEditError> {
        edit_piano_notes_from_keyboard(notes, &HashSet::from([1, 2]), edit, 0.25, 100)
    }
    #[test]
    fn keyboard_targets_are_active_channel_selected_or_all_and_group_qualified() {
        let notes = notes();
        for selection in [
            HashSet::new(),
            HashSet::from([1]),
            HashSet::from([3]),
            HashSet::from([99]),
        ] {
            assert_eq!(
                piano_keyboard_targets(&notes, &selection, 1, true),
                HashSet::from([1, 2])
            );
        }
        assert_eq!(
            piano_keyboard_targets(&notes, &HashSet::from([1, 3]), 1, false),
            HashSet::from([1])
        );
        assert!(piano_keyboard_targets(&notes, &HashSet::new(), 999, true).is_empty());
    }
    #[test]
    fn keyboard_moves_preserve_offsets_and_clamp_the_whole_phrase() {
        let mut source = notes();
        let result = run(&source, PianoKeyboardEdit::MoveSteps(-1)).unwrap();
        assert_eq!((result.notes[0].start, result.notes[1].start), (0.0, 0.375));
        assert_eq!(result.notes[2].start, source[2].start);
        source[0].start = 4095.25;
        source[1].start = 4095.625;
        let result = run(&source, PianoKeyboardEdit::MoveSteps(1)).unwrap();
        assert_eq!(
            (result.notes[0].start, result.notes[1].start),
            (4095.375, 4095.75)
        );
        assert_eq!(
            serde_json::to_string(
                &run(&result.notes, PianoKeyboardEdit::MoveSteps(1))
                    .unwrap()
                    .notes
            )
            .unwrap(),
            serde_json::to_string(&result.notes).unwrap()
        );
    }
    #[test]
    fn keyboard_fractional_lengths_reach_representable_right_boundary() {
        for length in [0.15, 0.2, 0.1, 0.01] {
            let mut source = notes();
            source[0].start = 4095.75;
            source[0].length = length;
            source[1].start = 4095.25;
            source[1].length = length;
            let moved = run(&source, PianoKeyboardEdit::MoveSteps(1)).unwrap();
            assert!(moved.notes[0].start > source[0].start);
            assert!(f64::from(moved.notes[0].start) + f64::from(length) <= MAX_PIANO_BEAT);
            assert_eq!(
                moved.notes[0].start - source[0].start,
                moved.notes[1].start - source[1].start
            );
            let again = run(&moved.notes, PianoKeyboardEdit::MoveSteps(1)).unwrap();
            assert_eq!(again.notes[0].start, moved.notes[0].start);
            assert_eq!(again.notes[1].start, moved.notes[1].start);
        }
    }

    #[test]
    fn keyboard_transpose_is_chromatic_and_preserves_interval_at_both_pitch_edges() {
        let mut source = notes();
        source[0].note = 120;
        source[1].note = 124;
        let result = run(&source, PianoKeyboardEdit::Transpose(12)).unwrap();
        assert_eq!((result.notes[0].note, result.notes[1].note), (123, 127));
        source[0].note = 1;
        source[1].note = 5;
        let result = run(&source, PianoKeyboardEdit::Transpose(-12)).unwrap();
        assert_eq!((result.notes[0].note, result.notes[1].note), (0, 4));
        assert_eq!(result.notes[2].note, 127);
        assert_eq!(result.notes[0].group_id, Some(7));
    }
    #[test]
    fn keyboard_repeat_uses_extent_fresh_project_ids_and_is_chainable() {
        let source = notes();
        let first = run(&source, PianoKeyboardEdit::RepeatRight).unwrap();
        assert_eq!(first.selection_ids, HashSet::from([101, 102]));
        assert_eq!((first.notes[3].start, first.notes[4].start), (0.75, 1.125));
        assert_ne!(first.notes[3].group_id, source[0].group_id);
        assert_eq!(first.notes[3].group_id, first.notes[4].group_id);
        assert!(first.notes[4].muted);
        let second = edit_piano_notes_from_keyboard(
            &first.notes,
            &first.selection_ids,
            PianoKeyboardEdit::RepeatRight,
            0.25,
            102,
        )
        .unwrap();
        assert_eq!(
            (second.notes[5].start, second.notes[6].start),
            (1.375, 1.75)
        );
        assert_eq!(second.selection_ids, HashSet::from([103, 104]));
    }
    #[test]
    fn keyboard_quantize_variants_and_discard_have_distinct_lengths() {
        let source = notes();
        let starts = run(
            &source,
            PianoKeyboardEdit::QuickQuantize { starts_only: true },
        )
        .unwrap();
        let full = run(
            &source,
            PianoKeyboardEdit::QuickQuantize { starts_only: false },
        )
        .unwrap();
        let discard = run(&source, PianoKeyboardEdit::DiscardLengths).unwrap();
        assert_eq!(
            (starts.notes[0].start, starts.notes[0].length),
            (0.25, 0.625)
        );
        assert_eq!((full.notes[0].start, full.notes[0].length), (0.25, 0.75));
        assert_eq!(
            (discard.notes[0].start, discard.notes[0].length),
            (0.125, 0.25)
        );
        assert_eq!(
            serde_json::to_string(&discard.notes[2]).unwrap(),
            serde_json::to_string(&source[2]).unwrap()
        );
    }
    #[test]
    fn keyboard_operations_reject_invalid_source_or_target_atomically() {
        for damage in 0..9 {
            let mut source = notes();
            match damage {
                0 => source[0].start = f32::NAN,
                1 => source[0].start = -1.0,
                2 => source[0].length = f32::INFINITY,
                3 => source[0].length = 0.0,
                4 => source[0].note = 128,
                5 => source[0].velocity = f32::NAN,
                6 => source[0].start = 4096.0,
                7 => source[0].id = 2,
                _ => source[0].group_id = Some(0),
            }
            assert!(
                run(&source, PianoKeyboardEdit::Transpose(1)).is_err(),
                "damage {damage}"
            );
        }
        assert!(
            edit_piano_notes_from_keyboard(
                &notes(),
                &HashSet::from([999]),
                PianoKeyboardEdit::Transpose(1),
                0.25,
                100
            )
            .is_err()
        );
        for snap in [0.0, -1.0, f32::NAN, f32::INFINITY, 8192.0] {
            assert!(
                edit_piano_notes_from_keyboard(
                    &notes(),
                    &HashSet::from([1]),
                    PianoKeyboardEdit::RepeatRight,
                    snap,
                    100
                )
                .is_err()
            );
        }
    }
    #[test]
    fn keyboard_repeat_and_length_edits_fail_closed_at_time_and_allocation_limits() {
        let mut source = notes();
        source[0].start = 4095.375;
        assert!(run(&source, PianoKeyboardEdit::RepeatRight).is_err());
        source[0].start = 4095.9;
        source[0].length = 0.02;
        assert!(run(&source, PianoKeyboardEdit::DiscardLengths).is_err());
        assert!(
            edit_piano_notes_from_keyboard(
                &notes(),
                &HashSet::from([1, 2]),
                PianoKeyboardEdit::RepeatRight,
                0.25,
                u64::MAX
            )
            .is_err()
        );
        let mut source = notes();
        source[0].group_id = Some(u64::MAX);
        source[1].group_id = Some(u64::MAX);
        assert!(run(&source, PianoKeyboardEdit::RepeatRight).is_err());
    }
    #[test]
    fn keyboard_repeat_count_is_bounded_before_generating_notes() {
        let mut source = Vec::new();
        for id in 1..=MAX_TRANSFORM_NOTES {
            source.push(PianoNote {
                id: id as u64,
                ..notes()[0].clone()
            });
        }
        assert!(
            edit_piano_notes_from_keyboard(
                &source,
                &HashSet::from([1]),
                PianoKeyboardEdit::RepeatRight,
                0.25,
                MAX_TRANSFORM_NOTES as u64
            )
            .is_err()
        );
        let targets: HashSet<_> = source.iter().map(|n| n.id).collect();
        assert_eq!(
            edit_piano_notes_from_keyboard(
                &source,
                &targets,
                PianoKeyboardEdit::Transpose(1),
                0.25,
                0
            )
            .unwrap()
            .notes
            .len(),
            MAX_TRANSFORM_NOTES
        );
    }
}

#[cfg(test)]
mod repeat_range_tests {
    use super::*;
    #[test]
    fn explicit_range_overrides_extent_preserves_fields_and_can_overlap() {
        let source = vec![
            PianoNote {
                id: 10,
                channel_id: Some(1),
                group_id: Some(9),
                note: 64,
                start: 20.0,
                length: 3.0,
                velocity: 0.375,
                selected: false,
                muted: true,
            },
            PianoNote {
                id: 20,
                channel_id: Some(1),
                group_id: Some(9),
                note: 67,
                start: 24.0,
                length: 0.5,
                velocity: 0.75,
                selected: false,
                muted: false,
            },
        ];
        let range = PianoTimeRange::new(0.0, 1.0 / 3.0).unwrap();
        let result = edit_piano_notes_from_keyboard_with_range(
            &source,
            &HashSet::from([10, 20]),
            PianoKeyboardEdit::RepeatRight,
            0.25,
            100_000,
            Some(range),
        )
        .unwrap();
        assert_eq!(result.notes.len(), 4);
        assert_eq!(result.notes[2].id, 100_001);
        assert_eq!(result.notes[3].id, 100_002);
        for (original, copy) in source.iter().zip(&result.notes[2..]) {
            assert!((copy.start - original.start - 1.0 / 3.0).abs() < 0.00001);
            assert_eq!(
                (
                    original.note,
                    original.length,
                    original.velocity,
                    original.muted,
                    original.channel_id
                ),
                (
                    copy.note,
                    copy.length,
                    copy.velocity,
                    copy.muted,
                    copy.channel_id
                )
            );
            assert_ne!(original.group_id, copy.group_id);
        }
        assert_eq!(result.notes[2].group_id, result.notes[3].group_id);
        assert_eq!(result.selection_ids, HashSet::from([100_001, 100_002]));
    }
    #[test]
    fn invalid_range_and_repeat_overflow_fail_atomically() {
        let source = vec![PianoNote {
            id: 10,
            channel_id: Some(1),
            group_id: None,
            note: 64,
            start: 4095.0,
            length: 1.0,
            velocity: 0.5,
            selected: false,
            muted: false,
        }];
        for range in [
            PianoTimeRange {
                start: 0.0,
                end: f64::NAN,
            },
            PianoTimeRange {
                start: 3.0,
                end: 2.0,
            },
            PianoTimeRange {
                start: 0.0,
                end: 1.0,
            },
            PianoTimeRange {
                start: -1.0,
                end: 1.0,
            },
        ] {
            assert!(
                edit_piano_notes_from_keyboard_with_range(
                    &source,
                    &HashSet::from([10]),
                    PianoKeyboardEdit::RepeatRight,
                    0.25,
                    10,
                    Some(range)
                )
                .is_err()
            );
        }
        assert_eq!(source[0].start, 4095.0);
        assert_eq!(
            PianoTimeRange::new(-3.0, 5000.0),
            Some(PianoTimeRange {
                start: 0.0,
                end: 4096.0
            })
        );
    }
    #[test]
    fn old_snap_preferences_migrate_and_off_triplet_preferences_roundtrip() {
        let old: PianoRollPreferences =
            serde_json::from_str(r#"{"version":1,"scale_root":9,"snap_to_scale":true}"#).unwrap();
        assert_eq!(old.local_snap, PianoSnap::QuarterBeat);
        assert_eq!(old.scale_root, 9);
        for snap in PianoSnap::ALL {
            let prefs = PianoRollPreferences {
                local_snap: snap,
                ..old.clone()
            };
            let raw = serde_json::to_string(&prefs).unwrap();
            assert_eq!(
                PianoRollState::from_preferences(serde_json::from_str(&raw).unwrap()).preferences(),
                prefs
            );
        }
        assert!(
            serde_json::from_str::<PianoRollPreferences>(r#"{"local_snap":"not-a-grid"}"#).is_err()
        );
        assert!(serde_json::from_str::<PianoRollPreferences>(r#"{"local_snap":0}"#).is_err());
    }
}

#[cfg(test)]
mod fine_grid_transform_tests {
    use super::*;
    fn source(start: f32, length: f32) -> Vec<PianoNote> {
        vec![PianoNote {
            id: 1,
            channel_id: Some(1),
            group_id: None,
            note: 60,
            start,
            length,
            velocity: 0.5,
            selected: false,
            muted: false,
        }]
    }
    #[test]
    fn indexed_triplet_transforms_keep_rational_phase_and_bounds_at_late_times() {
        for snap in [
            PianoSnap::TwentyFourthBeat,
            PianoSnap::TwelfthBeat,
            PianoSnap::SixthBeat,
            PianoSnap::ThirdBeat,
        ] {
            let step = snap.step().unwrap();
            for length in [1.0, 2.0] {
                for start in [0.0_f32, 0.137, 4094.0, 4093.875] {
                    let notes = source(start, length);
                    for absolute in [false, true] {
                        if absolute && start.fract() != 0.0 {
                            continue;
                        }
                        let mut settings = ChopSettings::new(snap.keyboard_step());
                        settings.absolute_pattern = absolute;
                        let result = chop_notes(&notes, &HashSet::from([1]), settings).unwrap();
                        assert_eq!(
                            result.notes.len(),
                            (f64::from(length) / step).round() as usize
                        );
                        for (index, note) in result.notes.iter().enumerate() {
                            assert_eq!(note.start, (f64::from(start) + index as f64 * step) as f32);
                            assert!(
                                f64::from(note.start) + f64::from(note.length)
                                    <= f64::from(start) + f64::from(length)
                            );
                            assert!(note.length >= MIN_NOTE_LENGTH_BEATS);
                        }
                    }
                    let result = arpeggiate_notes(
                        &notes,
                        &HashSet::from([1]),
                        ArpeggiateSettings::new(snap.keyboard_step()),
                    )
                    .unwrap();
                    assert_eq!(
                        result.notes.len(),
                        (f64::from(length) / step).round() as usize
                    );
                    for (index, note) in result.notes.iter().enumerate() {
                        assert_eq!(note.start, (f64::from(start) + index as f64 * step) as f32);
                        assert!(
                            f64::from(note.start) + f64::from(note.length)
                                <= f64::from(start) + f64::from(length)
                        );
                        assert!(note.length >= MIN_NOTE_LENGTH_BEATS);
                    }
                }
            }
        }
    }
    #[test]
    fn minimum_chop_includes_last_legal_slice_and_transforms_reject_unbounded_generation() {
        let notes = source(0.0, 1.0);
        let chopped = chop_notes(
            &notes,
            &HashSet::from([1]),
            ChopSettings::new(PianoSnap::Off.transform_step()),
        )
        .unwrap();
        assert_eq!(chopped.notes.len(), 64);
        assert!(
            chopped
                .notes
                .iter()
                .all(|n| n.length == MIN_NOTE_LENGTH_BEATS)
        );
        let huge = source(0.0, 4096.0);
        assert!(matches!(
            chop_notes(&huge, &HashSet::from([1]), ChopSettings::new(1.0 / 24.0)),
            Err(PianoRollEditError::TransformTooLarge)
        ));
        assert!(matches!(
            arpeggiate_notes(
                &huge,
                &HashSet::from([1]),
                ArpeggiateSettings::new(1.0 / 24.0)
            ),
            Err(PianoRollEditError::TransformTooLarge)
        ));
    }
}

#[cfg(test)]
mod repeated_triplet_nudge_tests {
    use super::*;
    #[test]
    fn repeated_on_grid_nudges_keep_canonical_positions_and_off_grid_moves_do_not_quantize() {
        for snap in [
            PianoSnap::TwentyFourthBeat,
            PianoSnap::TwelfthBeat,
            PianoSnap::SixthBeat,
            PianoSnap::ThirdBeat,
        ] {
            let mut notes = vec![PianoNote {
                id: 1,
                channel_id: Some(1),
                group_id: None,
                note: 60,
                start: 4094.0,
                length: 0.02,
                velocity: 0.5,
                selected: false,
                muted: false,
            }];
            let count = (1.9 / snap.step().unwrap()).floor() as usize;
            for index in 1..=count {
                notes = edit_piano_notes_from_keyboard(
                    &notes,
                    &HashSet::from([1]),
                    PianoKeyboardEdit::MoveSteps(1),
                    snap.keyboard_step(),
                    1,
                )
                .unwrap()
                .notes;
                assert_eq!(
                    notes[0].start,
                    (4094.0 + index as f64 * snap.step().unwrap()) as f32
                );
            }
            for index in (0..count).rev() {
                notes = edit_piano_notes_from_keyboard(
                    &notes,
                    &HashSet::from([1]),
                    PianoKeyboardEdit::MoveSteps(-1),
                    snap.keyboard_step(),
                    1,
                )
                .unwrap()
                .notes;
                assert_eq!(
                    notes[0].start,
                    (4094.0 + index as f64 * snap.step().unwrap()) as f32
                );
            }
            notes[0].start = 0.137;
            let shifted = edit_piano_notes_from_keyboard(
                &notes,
                &HashSet::from([1]),
                PianoKeyboardEdit::MoveSteps(1),
                snap.keyboard_step(),
                1,
            )
            .unwrap();
            assert_eq!(
                shifted.notes[0].start,
                (f64::from(notes[0].start) + snap.step().unwrap()) as f32
            );
        }
    }
    #[test]
    fn repeated_triplet_nudges_apply_one_delta_to_mixed_offset_selection() {
        for start in [1.0_f32, 4094.0] {
            for snap in [PianoSnap::TwentyFourthBeat, PianoSnap::ThirdBeat] {
                let anchor = PianoNote {
                    id: 1,
                    channel_id: Some(1),
                    group_id: Some(7),
                    note: 60,
                    start,
                    length: 0.02,
                    velocity: 0.5,
                    selected: false,
                    muted: false,
                };
                let partner = PianoNote {
                    id: 2,
                    start: start + 0.137,
                    note: 67,
                    ..anchor.clone()
                };
                let initial_offset = partner.start - anchor.start;
                let mut notes = vec![anchor, partner];
                let targets = HashSet::from([1, 2]);
                let count = (1.7 / snap.step().unwrap()).floor() as usize;
                for direction in [1, -1] {
                    for _ in 0..count {
                        notes = edit_piano_notes_from_keyboard(
                            &notes,
                            &targets,
                            PianoKeyboardEdit::MoveSteps(direction),
                            snap.keyboard_step(),
                            2,
                        )
                        .unwrap()
                        .notes;
                        let error = ((notes[1].start - notes[0].start) - initial_offset).abs();
                        assert!(
                            error <= notes[1].start.abs().max(1.0) * f32::EPSILON * 2.0,
                            "common group offset changed: {notes:?}"
                        );
                        assert_eq!(notes[1].note - notes[0].note, 7);
                    }
                }
                assert_eq!(notes[0].start, start);
                assert!(
                    (notes[1].start - (start + initial_offset)).abs()
                        <= notes[1].start.abs().max(1.0) * f32::EPSILON * 2.0
                );
            }
        }
    }
}

#[cfg(test)]
mod repeated_range_precision_tests {
    use super::*;
    #[test]
    fn repeated_triplet_range_copy_has_bounded_f32_error_unique_ids_and_atomic_endpoint() {
        let mut notes = vec![PianoNote {
            id: 1,
            channel_id: Some(1),
            group_id: None,
            note: 60,
            start: 4094.0,
            length: 0.02,
            velocity: 0.5,
            selected: false,
            muted: false,
        }];
        let mut targets = HashSet::from([1]);
        let range = PianoTimeRange::new(0.0, 1.0 / 24.0).unwrap();
        let mut rounding_bound = 0.0;
        for index in 1..=47 {
            let floor = notes.iter().map(|n| n.id).max().unwrap();
            let result = edit_piano_notes_from_keyboard_with_range(
                &notes,
                &targets,
                PianoKeyboardEdit::RepeatRight,
                1.0 / 24.0,
                floor,
                Some(range),
            )
            .unwrap();
            let copy = result.notes.last().unwrap();
            // One f32 addition and one f32 width conversion per repeat. A copy
            // preserves arbitrary phrase phase; it is not silently re-quantized.
            rounding_bound += f64::from(copy.start.next_up() - copy.start) * 0.5
                + (f64::from((1.0 / 24.0) as f32) - 1.0 / 24.0).abs();
            let exact = 4094.0 + f64::from(index) / 24.0;
            assert!((f64::from(copy.start) - exact).abs() <= rounding_bound + 1e-10);
            assert!(f64::from(copy.start) + f64::from(copy.length) <= MAX_PIANO_BEAT);
            assert_eq!(result.notes.len(), index as usize + 1);
            assert_eq!(
                result
                    .notes
                    .iter()
                    .map(|n| n.id)
                    .collect::<HashSet<_>>()
                    .len(),
                result.notes.len()
            );
            targets = result.selection_ids;
            notes = result.notes;
        }
        let before = serde_json::to_string(&notes).unwrap();
        assert!(
            edit_piano_notes_from_keyboard_with_range(
                &notes,
                &targets,
                PianoKeyboardEdit::RepeatRight,
                1.0 / 24.0,
                48,
                Some(range)
            )
            .is_err()
        );
        assert_eq!(serde_json::to_string(&notes).unwrap(), before);
    }
}
