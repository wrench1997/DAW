use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::automation::{AutomationLane, AutomationPoint, AutomationTarget};
use crate::mixer_graph::{
    MASTER_MIXER_TRACK_ID, MIXER_GRAPH_MAX_NODES, MixerRoute, MixerRouteDestination, MixerRouteTap,
    MixerTrackId, compile_mixer_graph,
};

const PROJECT_SAVE_TEMP_PREFIX: &str = ".citrus-save-";
const PROJECT_SAVE_TEMP_ATTEMPTS: usize = 128;
static PROJECT_SAVE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Current on-disk project schema written by Citrus Studio.
pub const CURRENT_PROJECT_FORMAT_VERSION: u32 = 12;

const LEGACY_PROJECT_FORMAT_VERSION: u32 = 1;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "MoveFileExW"]
    fn move_file_ex_w(existing_file_name: *const u16, new_file_name: *const u16, flags: u32)
    -> i32;
}

struct ProjectSaveTemp {
    path: Option<PathBuf>,
}

impl ProjectSaveTemp {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for ProjectSaveTemp {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn create_project_save_temp(directory: &Path) -> io::Result<(PathBuf, File)> {
    let process_id = std::process::id();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    for _ in 0..PROJECT_SAVE_TEMP_ATTEMPTS {
        let sequence = PROJECT_SAVE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let file_name =
            format!("{PROJECT_SAVE_TEMP_PREFIX}{process_id}-{timestamp:032x}-{sequence:016x}.tmp");
        let path = directory.join(file_name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "could not reserve a unique project staging file after {PROJECT_SAVE_TEMP_ATTEMPTS} attempts"
        ),
    ))
}

#[cfg(windows)]
fn nul_terminated_wide_path(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if encoded.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains an embedded NUL",
        ));
    }
    encoded.push(0);
    Ok(encoded)
}

#[cfg(windows)]
fn commit_project_save(staged: &Path, target: &Path) -> io::Result<()> {
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    let staged = nul_terminated_wide_path(staged)?;
    let target = nul_terminated_wide_path(target)?;
    // SAFETY: both buffers are NUL-terminated and remain alive for the call. The
    // staging file is closed before this function is invoked.
    let result = unsafe {
        move_file_ex_w(
            staged.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn commit_project_save(staged: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(staged, target)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StudioView {
    #[default]
    Playlist,
    ChannelRack,
    PianoRoll,
    Mixer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub id: u32,
    pub name: String,
    pub color: [u8; 3],
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    /// Stable project identity of the destination mixer track. The callback
    /// resource index lives on `MixerTrack::runtime_slot` and is not inferred
    /// from vector order.
    #[serde(
        default = "default_master_mixer_track_id",
        rename = "mixer_track_id",
        alias = "mixer_track"
    )]
    pub mixer_track: MixerTrackId,
    /// Optional VST generator assigned to this channel. The native generator
    /// remains the runtime fallback until the plug-in audio graph is wired.
    #[serde(default)]
    pub instrument_plugin_instance_id: Option<u64>,
    pub steps: [bool; 16],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipKind {
    Pattern,
    Audio,
    Automation,
}

/// The original fade domain, expressed relative to this clip's start.
/// Splitting may put the domain start before the clip, so offsets can be negative.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioFadeReference {
    pub offset_beats: f64,
    pub length_beats: f64,
    /// A song boundary that already truncated the original domain when split.
    /// Relative to the child placement; absent domains can extend past song end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_limit_beats: Option<f64>,
    /// Present for a freshly edited side of an exact-length slice. Such sides
    /// use exact export endpoint math rather than legacy f32 beat arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_length_beats: Option<f64>,
}

impl AudioFadeReference {
    pub fn is_valid(&self) -> bool {
        if self
            .export_length_beats
            .is_some_and(|length| !length.is_finite() || length <= 0.0 || length > 1_000_000.0)
        {
            return false;
        }
        if self.end_limit_beats.is_some_and(|limit| {
            !limit.is_finite() || limit.abs() > 1_000_000.0 || limit <= self.offset_beats
        }) {
            return false;
        }
        self.offset_beats.is_finite()
            && self.offset_beats.abs() <= 1_000_000.0
            && self.length_beats.is_finite()
            && self.length_beats > 0.0
            && self.length_beats <= 1_000_000.0
    }
}

/// Absolute timeline seconds captured when an audio clip's source is trimmed
/// or split. Reversed endpoints represent revealing earlier source material.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSourceSpan {
    pub start_seconds: f64,
    pub end_seconds: f64,
}

impl AudioSourceSpan {
    pub fn is_valid(&self) -> bool {
        self.start_seconds.is_finite()
            && (0.0..=1_000_000_000.0).contains(&self.start_seconds)
            && self.end_seconds.is_finite()
            && (0.0..=1_000_000_000.0).contains(&self.end_seconds)
    }
}

/// Rate-independent source provenance. Compile each span as a difference of
/// rounded output-frame positions, then add it to the root source in-point.
/// Moving the clip or changing the tempo must not reinterpret these seconds.
/// References are bounded to 4096 spans and a cumulative duration within
/// +/-1 billion seconds, including every intermediate signed sum.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSourceReference {
    pub elapsed_spans_seconds: Vec<AudioSourceSpan>,
}

impl AudioSourceReference {
    pub fn is_valid(&self) -> bool {
        if self.elapsed_spans_seconds.len() > 4096 {
            return false;
        }
        let mut elapsed_seconds = 0.0;
        for span in &self.elapsed_spans_seconds {
            if !span.is_valid() {
                return false;
            }
            elapsed_seconds += span.end_seconds - span.start_seconds;
            if !elapsed_seconds.is_finite() || elapsed_seconds.abs() > 1_000_000_000.0 {
                return false;
            }
        }
        true
    }
}

/// Exact inherited playback extents. The UI keeps its f32 length; this
/// reference is used only while that displayed length is unchanged. Separate
/// legacy realtime/export extents preserve both v10 renderers' arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioLengthReference {
    pub stored_length_beats: f32,
    pub length_beats: f64,
    pub export_length_beats: f64,
}

impl AudioLengthReference {
    pub fn is_valid(&self) -> bool {
        self.stored_length_beats.is_finite()
            && self.stored_length_beats > 0.0
            && self.stored_length_beats <= 1_000_000.0
            && self.length_beats.is_finite()
            && self.length_beats > 0.0
            && self.length_beats <= 1_000_000.0
            && self.export_length_beats.is_finite()
            && self.export_length_beats > 0.0
            && self.export_length_beats <= 1_000_000.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub id: u32,
    pub track: usize,
    pub start: f32,
    pub length: f32,
    pub name: String,
    pub color: [u8; 3],
    pub kind: ClipKind,
    /// Stable Playlist-local Clip group identity. Zero is reserved and a
    /// group with fewer than two surviving members is normalized away.
    #[serde(default)]
    pub group_id: Option<u64>,
    #[serde(default = "default_pattern_id")]
    pub pattern_id: u32,
    #[serde(default)]
    pub automation_id: Option<u64>,
    #[serde(default)]
    pub audio_asset_id: Option<u64>,
    #[serde(default)]
    pub source_offset: f32,
    /// Source position for an audio clip, expressed in native asset frames.
    /// When `audio_source_reference` is present, this is the integer root
    /// in-point, before applying the captured elapsed source spans.
    ///
    /// `None` is intentionally distinct from frame zero: it means an older or
    /// damaged project could not be migrated without guessing.
    #[serde(default)]
    pub audio_source_offset_frame: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_source_reference: Option<AudioSourceReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_length_reference: Option<AudioLengthReference>,
    #[serde(default = "default_gain")]
    pub gain: f32,
    #[serde(default)]
    pub fade_in: f32,
    #[serde(default)]
    pub fade_out: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_in_reference: Option<AudioFadeReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_out_reference: Option<AudioFadeReference>,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PianoNote {
    /// Stable project-wide identity. Zero is reserved for legacy input and is
    /// repaired deterministically by [`Project::normalize`].
    #[serde(default)]
    pub id: u64,
    /// Stable destination Channel Rack id. Legacy projects leave this
    /// unresolved instead of guessing from session-only UI selection.
    #[serde(default)]
    pub channel_id: Option<u32>,
    /// Pattern-local note-group identity. Grouping behavior can be disabled in
    /// the editor without destroying this persisted relationship.
    #[serde(default)]
    pub group_id: Option<u64>,
    pub note: u8,
    pub start: f32,
    pub length: f32,
    pub velocity: f32,
    /// Editor selection is session UI state, never musical project data.
    #[serde(skip)]
    pub selected: bool,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pattern {
    pub id: u32,
    pub name: String,
    pub length_steps: usize,
    pub channel_steps: Vec<[bool; 16]>,
    pub notes: Vec<PianoNote>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectAutomation {
    pub id: u64,
    pub name: String,
    pub lane: AutomationLane,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioAsset {
    pub id: u64,
    pub name: String,
    pub path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub frames: u64,
    #[serde(skip, default)]
    pub waveform_peaks: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioOffsetMigrationIssue {
    InvalidLegacyOffset,
    MissingAssetReference,
    MissingOrAmbiguousAsset,
    InvalidAssetSampleRate,
    TempoAutomationRequiresTimeline,
    FramePositionOverflow,
    MissingPersistedFrameOffset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectMigrationDiagnostic {
    MidiRoutingDisabled {
        reason: String,
    },
    LegacyPianoMirrorIgnored {
        note_count: usize,
    },
    LegacyPianoRouteUnresolved {
        pattern_id: u32,
        note_ids: Vec<u64>,
    },
    PianoRouteMissingChannel {
        pattern_id: u32,
        note_id: u64,
        channel_id: u32,
    },
    AudioSourceOffsetUnresolved {
        clip_id: u32,
        issue: AudioOffsetMigrationIssue,
    },
}

impl AudioAsset {
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames as f64 / f64::from(self.sample_rate)
        }
    }
}

/// Number of serial effect positions available on each mixer track.
pub const MIXER_INSERT_SLOT_COUNT: usize = 10;

/// Canonical plug-in format stored in a project.
///
/// The scanner/host may have its own discovery types while the runtime graph is
/// being wired, but persisted projects use this clean-room enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    Vst2,
    Vst3,
}

/// Discovery-time classification persisted for presentation and routing hints.
///
/// This is deliberately descriptive rather than authoritative: scanners can
/// misclassify a plug-in, so normalization never changes placement based on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginRole {
    Instrument,
    Effect,
    #[default]
    Unknown,
}

/// Session-only state reported by the plug-in host.
///
/// Availability and crash results depend on the current machine and process,
/// so they are deliberately excluded from project serialization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginRuntimeStatus {
    #[default]
    Unloaded,
    Loaded,
    Missing,
    Crashed,
}

/// A persisted plug-in instance, independent from its mixer placement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInstance {
    #[serde(default)]
    pub midi_ports: crate::plugin_midi_routing::PluginMidiPorts,
    pub id: u64,
    pub format: PluginFormat,
    #[serde(default)]
    pub role: PluginRole,
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default = "default_wet")]
    pub wet: f32,
    #[serde(default)]
    pub parameters: BTreeMap<u32, f32>,
    /// Opaque vendor state. JSON stores this as a hexadecimal string so every
    /// byte round-trips without pretending the payload is text.
    #[serde(default, with = "opaque_state_hex")]
    pub opaque_state: Vec<u8>,
    #[serde(skip, default)]
    pub runtime_status: PluginRuntimeStatus,
}

/// Placement of one persisted plug-in instance in a mixer insert chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MixerInsertSlotRef {
    #[serde(rename = "mixer_track_id", alias = "track")]
    pub track: MixerTrackId,
    pub slot: usize,
    pub plugin_instance_id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MixerTrack {
    /// Stable project identity. Zero is invalid and MASTER has the fixed
    /// `MASTER_MIXER_TRACK_ID` identity.
    #[serde(default)]
    pub id: MixerTrackId,
    /// Fixed callback bus slot. This remains unchanged when display order is
    /// rearranged. MASTER owns slot zero; inserts own slots 1..31.
    #[serde(default)]
    pub runtime_slot: u8,
    pub name: String,
    pub color: [u8; 3],
    pub volume: f32,
    pub pan: f32,
    pub muted: bool,
    pub solo: bool,
    pub peak: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioClipMixerDestination {
    pub clip_id: u32,
    pub mixer_track_id: MixerTrackId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    #[serde(default = "legacy_format_version")]
    pub format_version: u32,
    pub name: String,
    pub tempo: f32,
    pub swing: f32,
    #[serde(default = "default_song_length")]
    pub song_length_beats: f32,
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub patterns: Vec<Pattern>,
    #[serde(default)]
    pub active_pattern: usize,
    pub clips: Vec<Clip>,
    /// Pre-pattern Piano Roll payload accepted only as migration input.
    #[serde(default, skip_serializing)]
    pub piano_notes: Vec<PianoNote>,
    pub mixer_tracks: Vec<MixerTrack>,
    #[serde(default)]
    pub automation_lanes: Vec<ProjectAutomation>,
    #[serde(default)]
    pub audio_assets: Vec<AudioAsset>,
    #[serde(default)]
    pub plugin_instances: Vec<PluginInstance>,
    #[serde(default)]
    pub mixer_insert_slots: Vec<MixerInsertSlotRef>,
    /// Authoritative mixer destination for each Audio Playlist clip. Playlist
    /// row (`Clip::track`) remains a visual arrangement coordinate.
    #[serde(default)]
    pub audio_clip_mixer_destinations: Vec<AudioClipMixerDestination>,
    #[serde(default)]
    pub mixer_routes: Vec<MixerRoute>,
    /// Session-only report produced by project normalization.
    #[serde(skip, default)]
    pub migration_diagnostics: Vec<ProjectMigrationDiagnostic>,
}

impl Default for Project {
    fn default() -> Self {
        let channels = vec![
            channel(
                1,
                "Citrus Kick",
                [255, 142, 82],
                [
                    true, false, false, false, false, false, false, false, true, false, false,
                    false, false, false, false, false,
                ],
            ),
            channel(
                2,
                "Velvet Clap",
                [255, 207, 99],
                [
                    false, false, false, false, true, false, false, false, false, false, false,
                    false, true, false, false, false,
                ],
            ),
            channel(
                3,
                "Glass Hat",
                [93, 207, 177],
                [
                    true, false, true, false, true, false, true, false, true, false, true, false,
                    true, false, true, false,
                ],
            ),
            channel(
                4,
                "Sub Orchard",
                [113, 158, 255],
                [
                    true, false, false, false, false, false, true, false, true, false, false,
                    false, false, false, true, false,
                ],
            ),
            channel(5, "Neon Keys", [196, 142, 231], [false; 16]),
        ];

        let clips = vec![
            clip(
                1,
                0,
                0.0,
                4.0,
                "Intro drums",
                [255, 142, 82],
                ClipKind::Pattern,
            ),
            clip(
                2,
                0,
                4.0,
                8.0,
                "Main groove",
                [255, 142, 82],
                ClipKind::Pattern,
            ),
            clip(
                3,
                0,
                12.0,
                4.0,
                "Drum fill",
                [255, 166, 108],
                ClipKind::Pattern,
            ),
            clip(
                4,
                1,
                0.0,
                8.0,
                "Warm chords",
                [196, 142, 231],
                ClipKind::Pattern,
            ),
            clip(
                5,
                1,
                8.0,
                8.0,
                "Open voicing",
                [169, 128, 220],
                ClipKind::Pattern,
            ),
            clip(
                6,
                2,
                4.0,
                12.0,
                "Neon lead",
                [93, 207, 177],
                ClipKind::Pattern,
            ),
            clip(
                7,
                3,
                0.0,
                16.0,
                "SUB — A minor",
                [113, 158, 255],
                ClipKind::Pattern,
            ),
            clip(
                9,
                5,
                8.0,
                8.0,
                "Filter sweep",
                [238, 109, 153],
                ClipKind::Automation,
            ),
        ];

        let notes: Vec<PianoNote> = [
            (76, 1.0, 1.5, 0.78),
            (74, 3.0, 0.75, 0.64),
            (72, 4.0, 1.5, 0.82),
            (69, 6.0, 1.0, 0.72),
            (67, 8.0, 2.0, 0.86),
            (69, 10.5, 0.75, 0.67),
            (72, 12.0, 1.5, 0.80),
            (74, 14.0, 1.5, 0.74),
            (64, 0.0, 3.5, 0.58),
            (60, 4.0, 3.5, 0.61),
            (62, 8.0, 3.5, 0.56),
            (59, 12.0, 3.5, 0.63),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (note, start, length, velocity))| PianoNote {
            id: index as u64 + 1,
            channel_id: Some(5),
            group_id: None,
            note,
            start,
            length,
            velocity,
            selected: false,
            muted: false,
        })
        .collect();

        let mixer_names = [
            "MASTER", "DRUMS", "KICK", "CLAP", "HATS", "BASS", "KEYS", "LEAD", "VOCAL", "FX",
            "SEND A", "SEND B",
        ];
        let colors = [
            [255, 142, 82],
            [238, 109, 153],
            [255, 142, 82],
            [255, 207, 99],
            [93, 207, 177],
            [113, 158, 255],
            [196, 142, 231],
            [93, 207, 177],
        ];
        let mixer_tracks = (0..MIXER_GRAPH_MAX_NODES)
            .map(|i| MixerTrack {
                id: mixer_track_id_for_runtime_slot(i as u8),
                runtime_slot: i as u8,
                name: mixer_names
                    .get(i)
                    .map_or_else(|| format!("INSERT {i}"), |name| (*name).into()),
                color: colors[i % colors.len()],
                volume: if i == 0 {
                    0.84
                } else {
                    0.68 + (i % 3) as f32 * 0.06
                },
                pan: 0.0,
                muted: false,
                solo: false,
                peak: 0.18 + ((i * 17) % 55) as f32 / 100.0,
            })
            .collect();

        let pattern = Pattern {
            id: 1,
            name: "Pattern 1".into(),
            length_steps: 16,
            channel_steps: channels.iter().map(|channel| channel.steps).collect(),
            notes: notes.clone(),
        };
        let mut volume_automation = AutomationLane::new(AutomationTarget::MixerVolume { track: 7 });
        volume_automation.replace_points([
            AutomationPoint::with_tension(0.0, 0.22, 0.25),
            AutomationPoint::with_tension(4.0, 0.68, -0.15),
            AutomationPoint::new(8.0, 0.92),
        ]);

        Self {
            format_version: CURRENT_PROJECT_FORMAT_VERSION,
            name: "Night Orchard".into(),
            tempo: 128.0,
            swing: 0.12,
            song_length_beats: default_song_length(),
            channels,
            patterns: vec![pattern],
            active_pattern: 0,
            clips,
            piano_notes: Vec::new(),
            mixer_tracks,
            automation_lanes: vec![ProjectAutomation {
                id: 1,
                name: "Lead volume".into(),
                lane: volume_automation,
            }],
            audio_assets: Vec::new(),
            plugin_instances: Vec::new(),
            mixer_insert_slots: Vec::new(),
            audio_clip_mixer_destinations: Vec::new(),
            mixer_routes: default_mixer_routes(),
            migration_diagnostics: Vec::new(),
        }
    }
}

impl Project {
    /// Creates an operational empty project without any demo arrangement,
    /// notes, automation, media, or plug-in state.
    ///
    /// The single empty Channel and Pattern are structural editing targets, and
    /// the fixed 32-track mixer mirrors the native engine topology. `Default`
    /// intentionally remains the bundled showcase project.
    #[must_use]
    pub fn blank() -> Self {
        let channel = Channel {
            id: 1,
            name: "Channel 1".into(),
            color: [255, 142, 82],
            volume: 0.72,
            pan: 0.0,
            muted: false,
            solo: false,
            mixer_track: 1,
            instrument_plugin_instance_id: None,
            steps: [false; 16],
        };
        let pattern = Pattern {
            id: 1,
            name: "Pattern 1".into(),
            length_steps: 16,
            channel_steps: vec![[false; 16]],
            notes: Vec::new(),
        };
        let mixer_tracks = (0..32)
            .map(|index| MixerTrack {
                id: mixer_track_id_for_runtime_slot(index as u8),
                runtime_slot: index as u8,
                name: if index == 0 {
                    "MASTER".into()
                } else {
                    format!("INSERT {index}")
                },
                color: if index == 0 {
                    [255, 142, 82]
                } else {
                    [91, 105, 110]
                },
                volume: if index == 0 { 0.84 } else { 0.72 },
                pan: 0.0,
                muted: false,
                solo: false,
                peak: 0.0,
            })
            .collect();

        Self {
            format_version: CURRENT_PROJECT_FORMAT_VERSION,
            name: "Untitled".into(),
            tempo: 128.0,
            swing: 0.0,
            song_length_beats: default_song_length(),
            channels: vec![channel],
            patterns: vec![pattern],
            active_pattern: 0,
            clips: Vec::new(),
            piano_notes: Vec::new(),
            mixer_tracks,
            automation_lanes: Vec::new(),
            audio_assets: Vec::new(),
            plugin_instances: Vec::new(),
            mixer_insert_slots: Vec::new(),
            audio_clip_mixer_destinations: Vec::new(),
            mixer_routes: default_mixer_routes(),
            migration_diagnostics: Vec::new(),
        }
    }

    /// JSON encodes non-finite floats as `null`, which cannot be loaded into
    /// the project's required numeric fields. Reject them before touching the
    /// filesystem rather than replacing a recoverable project with corrupt data.
    fn validate_persisted_numbers(&self) -> Result<()> {
        fn finite(values: &[f32], location: impl std::fmt::Display) -> Result<()> {
            anyhow::ensure!(
                values.iter().all(|value| value.is_finite()),
                "Non-finite numeric value in {location}"
            );
            Ok(())
        }

        finite(
            &[self.tempo, self.swing, self.song_length_beats],
            "project settings",
        )?;
        for channel in &self.channels {
            finite(
                &[channel.volume, channel.pan],
                format_args!("channel {}", channel.id),
            )?;
        }
        for pattern in &self.patterns {
            for note in &pattern.notes {
                finite(
                    &[note.start, note.length, note.velocity],
                    format_args!("pattern {} note {}", pattern.id, note.id),
                )?;
            }
        }
        for clip in &self.clips {
            finite(
                &[
                    clip.start,
                    clip.length,
                    clip.source_offset,
                    clip.gain,
                    clip.fade_in,
                    clip.fade_out,
                ],
                format_args!("clip {}", clip.id),
            )?;
        }
        for track in &self.mixer_tracks {
            finite(
                &[track.volume, track.pan, track.peak],
                format_args!("mixer track {}", track.id),
            )?;
        }
        for route in &self.mixer_routes {
            finite(&[route.gain], format_args!("mixer route {}", route.id))?;
        }
        for plugin in &self.plugin_instances {
            finite(&[plugin.wet], format_args!("plug-in {} wet mix", plugin.id))?;
            for (parameter, value) in &plugin.parameters {
                finite(
                    &[*value],
                    format_args!("plug-in {} parameter {parameter}", plugin.id),
                )?;
            }
        }
        // AutomationLane's private fields are kept finite by its editing and
        // deserialization APIs. Legacy piano_notes and waveform_peaks are not
        // serialized, so they must not prevent saving the musical project.
        Ok(())
    }

    /// Exact split metadata cannot be repaired without changing what is heard.
    /// Validate it before normalization or any save-side filesystem changes.
    fn validate_audio_references(&self) -> Result<()> {
        for clip in &self.clips {
            anyhow::ensure!(
                clip.audio_length_reference
                    .is_none_or(|reference| reference.is_valid()),
                "Clip {} has invalid exact length metadata",
                clip.id
            );
            for (label, reference) in [
                ("fade-in", clip.fade_in_reference),
                ("fade-out", clip.fade_out_reference),
            ] {
                anyhow::ensure!(
                    reference.is_none_or(|reference| reference.is_valid()),
                    "Invalid {label} reference in clip {}",
                    clip.id
                );
            }
            anyhow::ensure!(
                clip.audio_source_reference
                    .as_ref()
                    .is_none_or(AudioSourceReference::is_valid),
                "Invalid audio source reference in clip {}",
                clip.id
            );
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_with_commit(path, commit_project_save)
    }

    fn save_with_commit<Commit>(&self, path: &Path, commit: Commit) -> Result<()>
    where
        Commit: FnOnce(&Path, &Path) -> io::Result<()>,
    {
        self.validate_persisted_numbers()
            .context("Refusing to save non-finite project data")?;
        self.validate_audio_references()
            .context("Refusing to save invalid audio split metadata")?;
        self.validate_mixer_graph()
            .context("Refusing to save an invalid v8 mixer graph")?;
        if path.file_name().is_none() {
            anyhow::bail!("Project save path does not name a file: {}", path.display());
        }

        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(directory).with_context(|| {
            format!("Unable to create project directory {}", directory.display())
        })?;

        let (temporary, file) = create_project_save_temp(directory).with_context(|| {
            format!(
                "Unable to create a staging file for project {} in {}",
                path.display(),
                directory.display()
            )
        })?;
        let mut cleanup = ProjectSaveTemp::new(temporary.clone());
        // Drop the open handle before the cleanup guard on every error path.
        // This also avoids relying on platform-specific open-file deletion.
        let mut staged_file = file;

        serde_json::to_writer_pretty(&mut staged_file, self)
            .with_context(|| format!("Unable to serialize project into {}", temporary.display()))?;
        staged_file
            .write_all(b"\n")
            .with_context(|| format!("Unable to finish writing {}", temporary.display()))?;
        staged_file
            .flush()
            .with_context(|| format!("Unable to flush {}", temporary.display()))?;
        staged_file
            .sync_all()
            .with_context(|| format!("Unable to sync {}", temporary.display()))?;
        drop(staged_file);

        commit(&temporary, path).with_context(|| {
            format!(
                "Unable to atomically commit staged project {} to {}",
                temporary.display(),
                path.display()
            )
        })?;
        cleanup.disarm();
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Unable to read {}", path.display()))?;
        let mut project: Self =
            serde_json::from_str(&content).context("The project file is invalid")?;
        anyhow::ensure!(
            project.format_version <= CURRENT_PROJECT_FORMAT_VERSION,
            "Project format version {} is newer than this Citrus Studio build (maximum supported version {})",
            project.format_version,
            CURRENT_PROJECT_FORMAT_VERSION
        );
        project
            .validate_audio_references()
            .context("The project audio split metadata is invalid")?;
        project.normalize();
        if let Err(reason) = crate::plugin_midi_routing::compile_midi_port_routes(&project) {
            for plugin in &mut project.plugin_instances {
                plugin.midi_ports.input = None;
            }
            project
                .migration_diagnostics
                .push(ProjectMigrationDiagnostic::MidiRoutingDisabled { reason });
        }
        project
            .validate_mixer_graph()
            .context("The project mixer graph is invalid")?;
        Ok(project)
    }

    pub fn normalize(&mut self) {
        let source_version = self.format_version;
        let legacy_routing = source_version < 2;
        let migrating_to_v7 = source_version < 7;
        let migrating_to_v8 = source_version < 8;
        if migrating_to_v8 {
            self.migration_diagnostics.clear();
        }
        self.tempo = if self.tempo.is_finite() {
            self.tempo.clamp(20.0, 400.0)
        } else {
            128.0
        };
        self.swing = if self.swing.is_finite() {
            self.swing.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if legacy_routing {
            for (index, channel) in self.channels.iter_mut().enumerate() {
                channel.mixer_track = (index + 2).min(31) as MixerTrackId;
            }
        }
        if self.patterns.is_empty() {
            self.patterns.push(Pattern {
                id: 1,
                name: "Pattern 1".into(),
                length_steps: 16,
                channel_steps: self.channels.iter().map(|channel| channel.steps).collect(),
                notes: std::mem::take(&mut self.piano_notes),
            });
        } else if migrating_to_v7 && !self.piano_notes.is_empty() {
            self.migration_diagnostics
                .push(ProjectMigrationDiagnostic::LegacyPianoMirrorIgnored {
                    note_count: self.piano_notes.len(),
                });
            self.piano_notes.clear();
        }
        self.normalize_timeline_entity_ids();
        // Audio mixer destinations are keyed by stable Clip IDs, so the v8
        // graph migration must run only after legacy zero/duplicate Clip IDs
        // have been repaired.
        self.normalize_mixer_schema(source_version);
        let channel_counts = self
            .channels
            .iter()
            .fold(BTreeMap::new(), |mut counts, channel| {
                *counts.entry(channel.id).or_insert(0_usize) += 1;
                counts
            });
        for pattern in &mut self.patterns {
            pattern
                .channel_steps
                .resize(self.channels.len(), [false; 16]);
            pattern.length_steps = pattern.length_steps.clamp(1, 16);
            let mut unresolved_note_ids = Vec::new();
            for note in &mut pattern.notes {
                note.start = finite_clamp(note.start, 0.0, 4096.0, 0.0);
                note.length = finite_clamp(note.length, 0.01, 4096.0, 0.25);
                note.velocity = finite_clamp(note.velocity, 0.0, 1.0, 0.8);
                if migrating_to_v7 {
                    note.channel_id = None;
                    unresolved_note_ids.push(note.id);
                } else if let Some(channel_id) = note.channel_id
                    && channel_counts.get(&channel_id).copied() != Some(1)
                {
                    self.migration_diagnostics.push(
                        ProjectMigrationDiagnostic::PianoRouteMissingChannel {
                            pattern_id: pattern.id,
                            note_id: note.id,
                            channel_id,
                        },
                    );
                    note.channel_id = None;
                }
            }
            normalize_piano_note_groups(&mut pattern.notes);
            if !unresolved_note_ids.is_empty() {
                self.migration_diagnostics.push(
                    ProjectMigrationDiagnostic::LegacyPianoRouteUnresolved {
                        pattern_id: pattern.id,
                        note_ids: unresolved_note_ids,
                    },
                );
            }
        }
        for automation in &mut self.automation_lanes {
            automation.lane.normalize();
        }
        self.active_pattern = self
            .active_pattern
            .min(self.patterns.len().saturating_sub(1));
        let fallback_pattern_id = self.active_pattern().id;
        let pattern_ids = self
            .patterns
            .iter()
            .map(|pattern| pattern.id)
            .collect::<Vec<_>>();
        for channel in &mut self.channels {
            channel.volume = finite_clamp(channel.volume, 0.0, 1.0, 0.72);
            channel.pan = finite_clamp(channel.pan, -1.0, 1.0, 0.0);
        }
        for track in &mut self.mixer_tracks {
            track.volume = finite_clamp(track.volume, 0.0, 1.5, 0.72);
            track.pan = finite_clamp(track.pan, -1.0, 1.0, 0.0);
            track.peak = finite_clamp(track.peak, 0.0, 1.0, 0.0);
        }
        self.normalize_plugin_graph(source_version);
        let invalid_legacy_audio_offsets = self
            .clips
            .iter()
            .filter(|clip| {
                migrating_to_v7
                    && clip.kind == ClipKind::Audio
                    && (!clip.source_offset.is_finite() || clip.source_offset < 0.0)
            })
            .map(|clip| clip.id)
            .collect::<BTreeSet<_>>();
        for clip in &mut self.clips {
            clip.track = clip.track.min(31);
            clip.start = finite_clamp(clip.start, 0.0, 4096.0, 0.0);
            clip.length = finite_clamp(clip.length, 0.0625, 4096.0, 4.0);
            clip.source_offset = finite_clamp(clip.source_offset, 0.0, 4096.0, 0.0);
            clip.gain = finite_clamp(clip.gain, 0.0, 1.5, 1.0);
            clip.fade_in = finite_clamp(clip.fade_in, 0.0, 1.0, 0.0);
            clip.fade_out = finite_clamp(clip.fade_out, 0.0, 1.0, 0.0);
            if !pattern_ids.contains(&clip.pattern_id) {
                clip.pattern_id = fallback_pattern_id;
            }
        }
        normalize_playlist_clip_groups(&mut self.clips);
        if migrating_to_v7 {
            for clip in &mut self.clips {
                if clip.kind == ClipKind::Automation {
                    // v6 lanes were evaluated in absolute project beats. This
                    // source offset preserves that result under v7 placement-
                    // local evaluation without rewriting any lane points.
                    clip.source_offset = clip.start;
                }
            }
            self.migrate_legacy_audio_source_offsets(&invalid_legacy_audio_offsets);
        }
        let required_length = self
            .clips
            .iter()
            .map(|clip| clip.start + clip.length)
            .fold(16.0_f32, f32::max);
        self.song_length_beats = self
            .song_length_beats
            .max(required_length)
            .clamp(4.0, 4096.0);
        for clip in self
            .clips
            .iter()
            .filter(|clip| clip.kind == ClipKind::Audio)
        {
            if clip.audio_source_offset_frame.is_some() {
                self.migration_diagnostics.retain(|diagnostic| {
                    !matches!(
                        diagnostic,
                        ProjectMigrationDiagnostic::AudioSourceOffsetUnresolved {
                            clip_id,
                            issue: AudioOffsetMigrationIssue::MissingPersistedFrameOffset,
                        } if *clip_id == clip.id
                    )
                });
            } else if !self.migration_diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic,
                    ProjectMigrationDiagnostic::AudioSourceOffsetUnresolved { clip_id, .. }
                        if *clip_id == clip.id
                )
            }) {
                self.migration_diagnostics.push(
                    ProjectMigrationDiagnostic::AudioSourceOffsetUnresolved {
                        clip_id: clip.id,
                        issue: AudioOffsetMigrationIssue::MissingPersistedFrameOffset,
                    },
                );
            }
        }
        self.format_version = CURRENT_PROJECT_FORMAT_VERSION;
    }

    fn normalize_mixer_schema(&mut self, source_version: u32) {
        if source_version >= 8 {
            return;
        }

        for (index, track) in self.mixer_tracks.iter_mut().enumerate() {
            if index >= MIXER_GRAPH_MAX_NODES {
                break;
            }
            track.runtime_slot = index as u8;
            track.id = mixer_track_id_for_runtime_slot(track.runtime_slot);
        }
        while self.mixer_tracks.len() < MIXER_GRAPH_MAX_NODES {
            let runtime_slot = self.mixer_tracks.len() as u8;
            self.mixer_tracks.push(MixerTrack {
                id: mixer_track_id_for_runtime_slot(runtime_slot),
                runtime_slot,
                name: format!("INSERT {runtime_slot}"),
                color: [91, 105, 110],
                volume: 0.72,
                pan: 0.0,
                muted: false,
                solo: false,
                peak: 0.0,
            });
        }

        for channel in &mut self.channels {
            let legacy_slot = channel.mixer_track.min(31) as u8;
            channel.mixer_track = mixer_track_id_for_runtime_slot(legacy_slot);
        }
        for slot_ref in &mut self.mixer_insert_slots {
            let legacy_slot = slot_ref.track.min(31) as u8;
            slot_ref.track = mixer_track_id_for_runtime_slot(legacy_slot);
        }
        for automation in &mut self.automation_lanes {
            let migrated = match automation.lane.target().clone() {
                AutomationTarget::MixerVolume { track } => Some(AutomationTarget::MixerVolume {
                    track: mixer_track_id_for_runtime_slot(track.min(31) as u8),
                }),
                AutomationTarget::MixerPan { track } => Some(AutomationTarget::MixerPan {
                    track: mixer_track_id_for_runtime_slot(track.min(31) as u8),
                }),
                AutomationTarget::MixerMute { track } => Some(AutomationTarget::MixerMute {
                    track: mixer_track_id_for_runtime_slot(track.min(31) as u8),
                }),
                _ => None,
            };
            if let Some(target) = migrated {
                automation.lane.set_target(target);
            }
        }
        self.audio_clip_mixer_destinations = self
            .clips
            .iter()
            .filter(|clip| clip.kind == ClipKind::Audio)
            .map(|clip| AudioClipMixerDestination {
                clip_id: clip.id,
                mixer_track_id: mixer_track_id_for_runtime_slot(
                    clip.track.saturating_add(1).min(31) as u8,
                ),
            })
            .collect();
        self.mixer_routes = default_mixer_routes();
    }

    pub fn validate_mixer_graph(&self) -> Result<()> {
        crate::plugin_midi_routing::compile_midi_port_routes(self).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            self.mixer_tracks.len() == MIXER_GRAPH_MAX_NODES,
            "v8 requires exactly {MIXER_GRAPH_MAX_NODES} mixer tracks, found {}",
            self.mixer_tracks.len()
        );
        compile_mixer_graph(self)?;
        let track_ids = self
            .mixer_tracks
            .iter()
            .map(|track| track.id)
            .collect::<BTreeSet<_>>();
        let mut plugin_ids = BTreeSet::new();
        for (project_index, instance) in self.plugin_instances.iter().enumerate() {
            anyhow::ensure!(
                instance.id != 0,
                "plug-in at project index {project_index} has reserved zero identity"
            );
            anyhow::ensure!(
                plugin_ids.insert(instance.id),
                "plug-in identity {} is duplicated",
                instance.id
            );
        }
        let mut placed_plugin_ids = BTreeSet::new();
        for channel in &self.channels {
            anyhow::ensure!(
                track_ids.contains(&channel.mixer_track),
                "Channel {} references missing mixer track {}",
                channel.id,
                channel.mixer_track
            );
            if let Some(instance_id) = channel.instrument_plugin_instance_id {
                anyhow::ensure!(
                    plugin_ids.contains(&instance_id),
                    "Channel {} references missing plug-in {}",
                    channel.id,
                    instance_id
                );
                anyhow::ensure!(
                    placed_plugin_ids.insert(instance_id),
                    "plug-in {instance_id} is placed more than once"
                );
            }
        }
        let mut occupied_mixer_slots = BTreeSet::new();
        for slot_ref in &self.mixer_insert_slots {
            anyhow::ensure!(
                track_ids.contains(&slot_ref.track),
                "plug-in {} references missing mixer track {}",
                slot_ref.plugin_instance_id,
                slot_ref.track
            );
            anyhow::ensure!(
                slot_ref.slot < MIXER_INSERT_SLOT_COUNT,
                "mixer track {} plug-in slot {} is outside 0..{}",
                slot_ref.track,
                slot_ref.slot,
                MIXER_INSERT_SLOT_COUNT
            );
            anyhow::ensure!(
                occupied_mixer_slots.insert((slot_ref.track, slot_ref.slot)),
                "mixer track {} plug-in slot {} is occupied more than once",
                slot_ref.track,
                slot_ref.slot
            );
            anyhow::ensure!(
                plugin_ids.contains(&slot_ref.plugin_instance_id),
                "mixer track {} slot {} references missing plug-in {}",
                slot_ref.track,
                slot_ref.slot,
                slot_ref.plugin_instance_id
            );
            anyhow::ensure!(
                placed_plugin_ids.insert(slot_ref.plugin_instance_id),
                "plug-in {} is placed more than once",
                slot_ref.plugin_instance_id
            );
        }
        let audio_clip_ids = self
            .clips
            .iter()
            .filter(|clip| clip.kind == ClipKind::Audio)
            .map(|clip| clip.id)
            .collect::<BTreeSet<_>>();
        let mut routed_audio_clips = BTreeSet::new();
        for destination in &self.audio_clip_mixer_destinations {
            anyhow::ensure!(
                audio_clip_ids.contains(&destination.clip_id),
                "mixer destination references non-Audio clip {}",
                destination.clip_id
            );
            anyhow::ensure!(
                routed_audio_clips.insert(destination.clip_id),
                "Audio clip {} has multiple mixer destinations",
                destination.clip_id
            );
            anyhow::ensure!(
                track_ids.contains(&destination.mixer_track_id),
                "Audio clip {} references missing mixer track {}",
                destination.clip_id,
                destination.mixer_track_id
            );
        }
        anyhow::ensure!(
            routed_audio_clips == audio_clip_ids,
            "every Audio clip must have exactly one stable mixer destination"
        );
        for automation in &self.automation_lanes {
            let mixer_track_id = match automation.lane.target() {
                AutomationTarget::MixerVolume { track }
                | AutomationTarget::MixerPan { track }
                | AutomationTarget::MixerMute { track } => Some(*track),
                _ => None,
            };
            if let Some(mixer_track_id) = mixer_track_id {
                anyhow::ensure!(
                    track_ids.contains(&mixer_track_id),
                    "automation {} references missing mixer track {}",
                    automation.id,
                    mixer_track_id
                );
            }
            if let AutomationTarget::PluginParameter { instance, .. } = automation.lane.target() {
                anyhow::ensure!(
                    plugin_ids.contains(instance),
                    "automation {} references missing plug-in {}",
                    automation.id,
                    instance
                );
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn mixer_track_index(&self, id: MixerTrackId) -> Option<usize> {
        self.mixer_tracks.iter().position(|track| track.id == id)
    }

    #[must_use]
    pub fn mixer_track_by_id(&self, id: MixerTrackId) -> Option<&MixerTrack> {
        self.mixer_tracks.iter().find(|track| track.id == id)
    }

    #[must_use]
    pub fn mixer_runtime_slot(&self, id: MixerTrackId) -> Option<usize> {
        self.mixer_track_by_id(id)
            .map(|track| usize::from(track.runtime_slot))
    }

    #[must_use]
    pub fn mixer_track_id_at_runtime_slot(&self, runtime_slot: usize) -> Option<MixerTrackId> {
        self.mixer_tracks
            .iter()
            .find(|track| usize::from(track.runtime_slot) == runtime_slot)
            .map(|track| track.id)
    }

    #[must_use]
    pub fn audio_clip_mixer_track_id(&self, clip_id: u32) -> Option<MixerTrackId> {
        let mut destinations = self
            .audio_clip_mixer_destinations
            .iter()
            .filter(|destination| destination.clip_id == clip_id);
        let destination = destinations.next()?;
        destinations
            .next()
            .is_none()
            .then_some(destination.mixer_track_id)
    }

    /// Returns an unused stable Piano Roll note id without mutating the project.
    /// Callers allocate before taking a mutable borrow of a pattern.
    #[must_use]
    pub fn next_note_id(&self) -> Option<u64> {
        let used = self
            .patterns
            .iter()
            .flat_map(|pattern| pattern.notes.iter())
            .filter_map(|note| (note.id != 0).then_some(note.id))
            .collect::<BTreeSet<_>>();
        next_available_u64(&used)
    }

    fn normalize_timeline_entity_ids(&mut self) {
        normalize_piano_note_ids(&mut self.patterns);
        normalize_clip_ids(&mut self.clips);
        normalize_automation_ids(&mut self.automation_lanes, &mut self.clips);
    }

    fn migrate_legacy_audio_source_offsets(&mut self, invalid_offsets: &BTreeSet<u32>) {
        let migrations = self
            .clips
            .iter()
            .enumerate()
            .filter(|(_, clip)| clip.kind == ClipKind::Audio)
            .map(|(index, clip)| {
                (
                    index,
                    migrate_legacy_audio_source_offset(self, clip, invalid_offsets),
                )
            })
            .collect::<Vec<_>>();

        for (index, migration) in migrations {
            match migration {
                Ok(frame) => {
                    self.clips[index].audio_source_offset_frame = Some(frame);
                    self.clips[index].source_offset = 0.0;
                }
                Err(issue) => {
                    self.clips[index].audio_source_offset_frame = None;
                    self.migration_diagnostics.push(
                        ProjectMigrationDiagnostic::AudioSourceOffsetUnresolved {
                            clip_id: self.clips[index].id,
                            issue,
                        },
                    );
                }
            }
        }
    }

    fn normalize_plugin_graph(&mut self, source_version: u32) {
        if source_version >= 8 {
            // Stable v8 identities and placements are authoritative project
            // structure. Metadata remains safe to normalize, but changing an
            // identity, reference, placement, or placement order here would
            // hide file corruption before the explicit save/load validator can
            // reject it.
            for instance in &mut self.plugin_instances {
                normalize_plugin_instance_metadata(instance);
            }
            return;
        }

        let reserved_ids = self
            .plugin_instances
            .iter()
            .filter_map(|instance| (instance.id != 0).then_some(instance.id))
            .collect::<BTreeSet<_>>();
        let mut used_ids = BTreeSet::new();
        let mut first_id_mapping = BTreeMap::new();
        let mut next_id = 1_u64;

        for instance in &mut self.plugin_instances {
            let original_id = instance.id;
            if original_id == 0 || !used_ids.insert(original_id) {
                instance.id = allocate_plugin_instance_id(&reserved_ids, &used_ids, &mut next_id);
                used_ids.insert(instance.id);
            }
            first_id_mapping.entry(original_id).or_insert(instance.id);

            normalize_plugin_instance_metadata(instance);
        }

        for automation in &mut self.automation_lanes {
            if let AutomationTarget::PluginParameter {
                instance,
                parameter,
            } = automation.lane.target().clone()
                && let Some(mapped) = first_id_mapping.get(&instance)
            {
                automation
                    .lane
                    .set_target(AutomationTarget::PluginParameter {
                        instance: *mapped,
                        parameter,
                    });
            }
        }

        let valid_instance_ids = self
            .plugin_instances
            .iter()
            .map(|instance| instance.id)
            .collect::<BTreeSet<_>>();
        let valid_mixer_track_ids = self
            .mixer_tracks
            .iter()
            .map(|track| track.id)
            .collect::<BTreeSet<_>>();
        let mut placed_instances = BTreeSet::new();
        // A generator placement is more specific than a mixer insert. Process
        // channels in stable project order: the first valid channel reference
        // wins, and later channel/insert references to that instance are
        // cleared. Role metadata is intentionally not consulted here because
        // scanner classification is only a hint.
        for channel in &mut self.channels {
            let Some(original_id) = channel.instrument_plugin_instance_id else {
                continue;
            };
            let Some(mapped_id) = first_id_mapping.get(&original_id).copied() else {
                channel.instrument_plugin_instance_id = None;
                continue;
            };
            if !valid_instance_ids.contains(&mapped_id) || !placed_instances.insert(mapped_id) {
                channel.instrument_plugin_instance_id = None;
                continue;
            }
            channel.instrument_plugin_instance_id = Some(mapped_id);
        }

        let mut occupied_slots = BTreeSet::new();
        let mut normalized_slots = Vec::with_capacity(self.mixer_insert_slots.len());
        for mut slot_ref in self.mixer_insert_slots.drain(..).rev() {
            let Some(mapped_id) = first_id_mapping.get(&slot_ref.plugin_instance_id) else {
                continue;
            };
            slot_ref.plugin_instance_id = *mapped_id;
            if !valid_mixer_track_ids.contains(&slot_ref.track) {
                continue;
            }
            if slot_ref.slot >= MIXER_INSERT_SLOT_COUNT
                || !occupied_slots.insert((slot_ref.track, slot_ref.slot))
                || !placed_instances.insert(slot_ref.plugin_instance_id)
            {
                continue;
            }
            normalized_slots.push(slot_ref);
        }
        let runtime_slot_by_id = self
            .mixer_tracks
            .iter()
            .map(|track| (track.id, track.runtime_slot))
            .collect::<BTreeMap<_, _>>();
        normalized_slots.sort_by_key(|slot_ref| {
            (
                runtime_slot_by_id
                    .get(&slot_ref.track)
                    .copied()
                    .unwrap_or(u8::MAX),
                slot_ref.slot,
            )
        });
        self.mixer_insert_slots = normalized_slots;
    }

    pub fn active_pattern(&self) -> &Pattern {
        &self.patterns[self
            .active_pattern
            .min(self.patterns.len().saturating_sub(1))]
    }

    pub fn active_pattern_mut(&mut self) -> &mut Pattern {
        let index = self
            .active_pattern
            .min(self.patterns.len().saturating_sub(1));
        &mut self.patterns[index]
    }
}

fn normalize_piano_note_ids(patterns: &mut [Pattern]) {
    let reserved = patterns
        .iter()
        .flat_map(|pattern| pattern.notes.iter())
        .filter_map(|note| (note.id != 0).then_some(note.id))
        .collect::<BTreeSet<_>>();
    let mut used = BTreeSet::new();
    let mut next = 1_u64;
    for note in patterns
        .iter_mut()
        .flat_map(|pattern| pattern.notes.iter_mut())
    {
        if note.id != 0 && used.insert(note.id) {
            continue;
        }
        note.id = allocate_reserved_u64(&reserved, &used, &mut next);
        used.insert(note.id);
    }
}

pub(crate) fn normalize_piano_note_groups(notes: &mut [PianoNote]) {
    for note in notes.iter_mut() {
        if note.group_id == Some(0) {
            note.group_id = None;
        }
    }
    let counts = notes.iter().fold(BTreeMap::new(), |mut counts, note| {
        if let Some(group_id) = note.group_id {
            *counts.entry((group_id, note.channel_id)).or_insert(0_usize) += 1;
        }
        counts
    });
    for note in notes {
        if note.group_id.is_some_and(|group_id| {
            counts
                .get(&(group_id, note.channel_id))
                .copied()
                .unwrap_or(0)
                < 2
        }) {
            note.group_id = None;
        }
    }
}

pub(crate) fn normalize_playlist_clip_groups(clips: &mut [Clip]) {
    for clip in clips.iter_mut() {
        if clip.group_id == Some(0) {
            clip.group_id = None;
        }
    }
    let counts = clips.iter().fold(BTreeMap::new(), |mut counts, clip| {
        if let Some(group_id) = clip.group_id {
            *counts.entry(group_id).or_insert(0_usize) += 1;
        }
        counts
    });
    for clip in clips {
        if clip
            .group_id
            .is_some_and(|group_id| counts.get(&group_id).copied().unwrap_or(0) < 2)
        {
            clip.group_id = None;
        }
    }
}

fn normalize_clip_ids(clips: &mut [Clip]) {
    let reserved = clips
        .iter()
        .filter_map(|clip| (clip.id != 0).then_some(clip.id))
        .collect::<BTreeSet<_>>();
    let mut used = BTreeSet::new();
    let mut next = 1_u32;
    for clip in clips {
        if clip.id != 0 && used.insert(clip.id) {
            continue;
        }
        clip.id = allocate_reserved_u32(&reserved, &used, &mut next);
        used.insert(clip.id);
    }
}

fn normalize_automation_ids(automation: &mut [ProjectAutomation], clips: &mut [Clip]) {
    let reserved = automation
        .iter()
        .filter_map(|lane| (lane.id != 0).then_some(lane.id))
        .collect::<BTreeSet<_>>();
    let mut used = BTreeSet::new();
    let mut first_mapping = BTreeMap::new();
    let mut next = 1_u64;
    for lane in automation {
        let original = lane.id;
        if original == 0 || !used.insert(original) {
            lane.id = allocate_reserved_u64(&reserved, &used, &mut next);
            used.insert(lane.id);
        }
        first_mapping.entry(original).or_insert(lane.id);
    }
    for clip in clips {
        if let Some(id) = clip.automation_id
            && let Some(repaired) = first_mapping.get(&id)
        {
            clip.automation_id = Some(*repaired);
        }
    }
}

fn allocate_reserved_u64(reserved: &BTreeSet<u64>, used: &BTreeSet<u64>, next: &mut u64) -> u64 {
    loop {
        let candidate = *next;
        *next = next.wrapping_add(1).max(1);
        if candidate != 0 && !reserved.contains(&candidate) && !used.contains(&candidate) {
            return candidate;
        }
    }
}

fn allocate_reserved_u32(reserved: &BTreeSet<u32>, used: &BTreeSet<u32>, next: &mut u32) -> u32 {
    loop {
        let candidate = *next;
        *next = next.wrapping_add(1).max(1);
        if candidate != 0 && !reserved.contains(&candidate) && !used.contains(&candidate) {
            return candidate;
        }
    }
}

fn next_available_u64(used: &BTreeSet<u64>) -> Option<u64> {
    let mut candidate = 1_u64;
    for id in used.iter().copied().filter(|id| *id != 0) {
        if id > candidate {
            return Some(candidate);
        }
        if id == candidate {
            candidate = candidate.checked_add(1)?;
        }
    }
    Some(candidate)
}

fn migrate_legacy_audio_source_offset(
    project: &Project,
    clip: &Clip,
    invalid_offsets: &BTreeSet<u32>,
) -> std::result::Result<u64, AudioOffsetMigrationIssue> {
    if invalid_offsets.contains(&clip.id) {
        return Err(AudioOffsetMigrationIssue::InvalidLegacyOffset);
    }
    if let Some(frame) = clip.audio_source_offset_frame {
        return Ok(frame);
    }
    if clip.source_offset == 0.0 {
        return Ok(0);
    }
    let asset_id = clip
        .audio_asset_id
        .ok_or(AudioOffsetMigrationIssue::MissingAssetReference)?;
    let mut matches = project
        .audio_assets
        .iter()
        .filter(|asset| asset.id == asset_id);
    let asset = matches
        .next()
        .ok_or(AudioOffsetMigrationIssue::MissingOrAmbiguousAsset)?;
    if matches.next().is_some() {
        return Err(AudioOffsetMigrationIssue::MissingOrAmbiguousAsset);
    }
    if asset.sample_rate == 0 {
        return Err(AudioOffsetMigrationIssue::InvalidAssetSampleRate);
    }
    let start = f64::from(clip.start);
    let end = start + f64::from(clip.source_offset);
    if legacy_interval_has_tempo_automation(project, start, end) {
        return Err(AudioOffsetMigrationIssue::TempoAutomationRequiresTimeline);
    }
    let frame = f64::from(clip.source_offset) * 60.0 / f64::from(project.tempo)
        * f64::from(asset.sample_rate);
    if !frame.is_finite() || frame < 0.0 || frame > u64::MAX as f64 {
        return Err(AudioOffsetMigrationIssue::FramePositionOverflow);
    }
    Ok(frame.round() as u64)
}

fn legacy_interval_has_tempo_automation(project: &Project, start: f64, end: f64) -> bool {
    project.automation_lanes.iter().any(|automation| {
        let lane = &automation.lane;
        if !lane.is_enabled()
            || lane.points().is_empty()
            || !matches!(lane.target(), AutomationTarget::Tempo)
        {
            return false;
        }
        let placements = project.clips.iter().filter(|clip| {
            clip.kind == ClipKind::Automation && clip.automation_id == Some(automation.id)
        });
        let mut has_placement = false;
        let mut intersects = false;
        for placement in placements {
            has_placement = true;
            let placement_start = f64::from(placement.start);
            let placement_end = placement_start + f64::from(placement.length);
            if !placement.muted && placement_start < end && placement_end > start {
                intersects = true;
            }
        }
        !has_placement || intersects
    })
}

fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

fn allocate_plugin_instance_id(
    reserved_ids: &BTreeSet<u64>,
    used_ids: &BTreeSet<u64>,
    next_id: &mut u64,
) -> u64 {
    loop {
        let candidate = *next_id;
        *next_id = next_id.wrapping_add(1);
        if candidate != 0 && !reserved_ids.contains(&candidate) && !used_ids.contains(&candidate) {
            return candidate;
        }
    }
}

fn path_is_blank(path: &Path) -> bool {
    path.as_os_str().is_empty() || path.to_string_lossy().trim().is_empty()
}

fn normalize_plugin_instance_metadata(instance: &mut PluginInstance) {
    instance.uid = instance.uid.trim().to_owned();
    instance.vendor = instance.vendor.trim().to_owned();
    instance.name = instance.name.trim().to_owned();
    instance.wet = finite_clamp(instance.wet, 0.0, 1.0, 1.0);
    instance.parameters.retain(|_, value| {
        if value.is_finite() {
            *value = value.clamp(0.0, 1.0);
            true
        } else {
            false
        }
    });

    if path_is_blank(&instance.path) {
        instance.path.clear();
        instance.runtime_status = PluginRuntimeStatus::Missing;
    }
    // Do not probe non-empty paths here. Availability is machine- and
    // session-specific and belongs to the runtime host boundary.
    if instance.name.is_empty() {
        instance.name = instance
            .path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| default_plugin_name(instance.format).to_owned());
    }
}

const fn default_plugin_name(format: PluginFormat) -> &'static str {
    match format {
        PluginFormat::Vst2 => "Missing VST2 plug-in",
        PluginFormat::Vst3 => "Missing VST3 plug-in",
    }
}

const fn legacy_format_version() -> u32 {
    LEGACY_PROJECT_FORMAT_VERSION
}

const fn default_song_length() -> f32 {
    64.0
}

const fn default_master_mixer_track_id() -> MixerTrackId {
    MASTER_MIXER_TRACK_ID
}

/// Deterministic v8 identity assigned to the callback slot inherited from v7.
/// Insert identities intentionally equal their historical 1..31 slots; MASTER
/// uses a fixed out-of-band nonzero identity.
#[must_use]
pub const fn mixer_track_id_for_runtime_slot(runtime_slot: u8) -> MixerTrackId {
    if runtime_slot == 0 {
        MASTER_MIXER_TRACK_ID
    } else {
        runtime_slot as MixerTrackId
    }
}

fn default_mixer_routes() -> Vec<MixerRoute> {
    (1..MIXER_GRAPH_MAX_NODES)
        .map(|runtime_slot| MixerRoute {
            id: runtime_slot as u64,
            runtime_slot: (runtime_slot - 1) as u8,
            source_mixer_track_id: mixer_track_id_for_runtime_slot(runtime_slot as u8),
            destination: MixerRouteDestination::MainInput {
                mixer_track_id: MASTER_MIXER_TRACK_ID,
            },
            tap: MixerRouteTap::PostFader,
            gain: 1.0,
            enabled: true,
        })
        .collect()
}

fn channel(id: u32, name: &str, color: [u8; 3], steps: [bool; 16]) -> Channel {
    Channel {
        id,
        name: name.into(),
        color,
        volume: 0.72,
        pan: 0.0,
        muted: false,
        solo: false,
        mixer_track: u64::from(id) + 1,
        instrument_plugin_instance_id: None,
        steps,
    }
}

fn clip(
    id: u32,
    track: usize,
    start: f32,
    length: f32,
    name: &str,
    color: [u8; 3],
    kind: ClipKind,
) -> Clip {
    Clip {
        id,
        track,
        start,
        length,
        name: name.into(),
        color,
        kind,
        group_id: None,
        pattern_id: default_pattern_id(),
        automation_id: (kind == ClipKind::Automation).then_some(1),
        audio_asset_id: None,
        source_offset: 0.0,
        audio_source_offset_frame: (kind == ClipKind::Audio).then_some(0),
        audio_source_reference: None,
        audio_length_reference: None,
        gain: default_gain(),
        fade_in: 0.0,
        fade_out: 0.0,
        fade_in_reference: None,
        fade_out_reference: None,
        muted: false,
    }
}

const fn default_gain() -> f32 {
    1.0
}

const fn default_true() -> bool {
    true
}

const fn default_wet() -> f32 {
    1.0
}

const fn default_pattern_id() -> u32 {
    1
}

mod opaque_state_hex {
    use serde::{Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum EncodedState {
        Hex(String),
        Bytes(Vec<u8>),
    }

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
        for byte in bytes {
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match EncodedState::deserialize(deserializer)? {
            EncodedState::Bytes(bytes) => Ok(bytes),
            EncodedState::Hex(encoded) => {
                decode_hex(&encoded).map_err(<D::Error as serde::de::Error>::custom)
            }
        }
    }

    fn decode_hex(encoded: &str) -> Result<Vec<u8>, &'static str> {
        if !encoded.len().is_multiple_of(2) {
            return Err("opaque plug-in state must contain an even number of hex digits");
        }
        let mut bytes = Vec::with_capacity(encoded.len() / 2);
        for pair in encoded.as_bytes().as_chunks::<2>().0 {
            let high = decode_nibble(pair[0]).ok_or("opaque plug-in state contains invalid hex")?;
            let low = decode_nibble(pair[1]).ok_or("opaque plug-in state contains invalid hex")?;
            bytes.push((high << 4) | low);
        }
        Ok(bytes)
    }

    const fn decode_nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "citrus-model-{label}-{}-{timestamp:032x}-{sequence:016x}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn assert_no_project_save_temps(directory: &Path) {
        let staged_files = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().starts_with(PROJECT_SAVE_TEMP_PREFIX))
            .collect::<Vec<_>>();
        assert!(
            staged_files.is_empty(),
            "staging files were not cleaned up: {staged_files:?}"
        );
    }

    #[test]
    fn project_save_atomically_replaces_an_existing_file() {
        let directory = TestDirectory::new("replace");
        let target = directory.path().join("existing.citrus");
        std::fs::write(&target, b"original project bytes").unwrap();

        let project = Project {
            name: "Safely replaced".into(),
            tempo: 137.25,
            ..Project::default()
        };
        project.save(&target).unwrap();

        let restored = Project::load(&target).unwrap();
        assert_eq!(restored.name, "Safely replaced");
        assert_eq!(restored.tempo, 137.25);
        assert_ne!(std::fs::read(&target).unwrap(), b"original project bytes");
        assert_no_project_save_temps(directory.path());
    }

    #[test]
    fn project_save_rejects_non_finite_numbers_without_replacing_good_data() {
        let directory = TestDirectory::new("non-finite-save");
        let target = directory.path().join("session.citrus");
        let mut baseline = Project::default();
        baseline
            .plugin_instances
            .push(test_plugin(42, PluginFormat::Vst3, "synth.vst3"));
        baseline.plugin_instances[0].parameters.insert(7, 0.5);
        baseline.save(&target).unwrap();
        let original = std::fs::read(&target).unwrap();

        type CorruptField = fn(&mut Project, f32);
        let cases: &[(&str, CorruptField)] = &[
            ("tempo", |p, v| p.tempo = v),
            ("swing", |p, v| p.swing = v),
            ("song length", |p, v| p.song_length_beats = v),
            ("channel volume", |p, v| p.channels[0].volume = v),
            ("channel pan", |p, v| p.channels[0].pan = v),
            ("note start", |p, v| p.patterns[0].notes[0].start = v),
            ("note length", |p, v| p.patterns[0].notes[0].length = v),
            ("note velocity", |p, v| p.patterns[0].notes[0].velocity = v),
            ("clip start", |p, v| p.clips[0].start = v),
            ("clip length", |p, v| p.clips[0].length = v),
            ("clip source offset", |p, v| p.clips[0].source_offset = v),
            ("clip gain", |p, v| p.clips[0].gain = v),
            ("clip fade in", |p, v| p.clips[0].fade_in = v),
            ("clip fade out", |p, v| p.clips[0].fade_out = v),
            ("mixer volume", |p, v| p.mixer_tracks[0].volume = v),
            ("mixer pan", |p, v| p.mixer_tracks[0].pan = v),
            ("mixer peak", |p, v| p.mixer_tracks[0].peak = v),
            ("route gain", |p, v| p.mixer_routes[0].gain = v),
            ("plugin wet", |p, v| p.plugin_instances[0].wet = v),
            ("plugin parameter", |p, v| {
                p.plugin_instances[0].parameters.insert(7, v);
            }),
        ];
        for (label, corrupt) in cases {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut project = baseline.clone();
                corrupt(&mut project, value);
                let error = project.save(&target).unwrap_err();
                assert!(
                    format!("{error:#}").contains("Non-finite numeric value"),
                    "{label}: {error:#}"
                );
                assert_eq!(std::fs::read(&target).unwrap(), original, "{label}");
                Project::load(&target).unwrap();
                assert_no_project_save_temps(directory.path());
            }
        }
    }

    #[test]
    fn invalid_numeric_save_does_not_create_directories() {
        let directory = TestDirectory::new("non-finite-new-save");
        let target = directory.path().join("not-created/session.citrus");
        let project = Project {
            tempo: f32::NAN,
            ..Project::default()
        };
        assert!(project.save(&target).is_err());
        assert!(!target.parent().unwrap().exists());
    }

    #[test]
    fn non_finite_session_only_data_does_not_prevent_saving() {
        let directory = TestDirectory::new("session-only-floats");
        let target = directory.path().join("session.citrus");
        let mut project = Project::default();
        let mut legacy_note = project.patterns[0].notes[0].clone();
        legacy_note.velocity = f32::NAN;
        project.piano_notes.push(legacy_note);
        project.save(&target).unwrap();
        assert!(Project::load(&target).unwrap().piano_notes.is_empty());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn project_serialization_failure_preserves_old_data_and_removes_staging_file() {
        let directory = TestDirectory::new("serialization-failure");
        let target = directory.path().join("session.citrus");
        let mut project = Project::default();
        project.save(&target).unwrap();
        let original = std::fs::read(&target).unwrap();
        #[cfg(unix)]
        let invalid_path = {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]))
        };
        #[cfg(windows)]
        let invalid_path = {
            use std::os::windows::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_wide(&[0xd800]))
        };
        let mut plugin = test_plugin(42, PluginFormat::Vst3, "synth.vst3");
        plugin.path = invalid_path;
        project.plugin_instances.push(plugin);

        let error = project.save(&target).unwrap_err();
        assert!(format!("{error:#}").contains("Unable to serialize project"));
        assert_eq!(std::fs::read(&target).unwrap(), original);
        Project::load(&target).unwrap();
        assert_no_project_save_temps(directory.path());
    }

    #[test]
    fn failed_project_commit_preserves_the_old_file_and_cleans_the_stage() {
        let directory = TestDirectory::new("commit-failure");
        let target = directory.path().join("protected.citrus");
        let original = b"known-good project";
        std::fs::write(&target, original).unwrap();

        let project = Project {
            name: "Must not replace the old project".into(),
            ..Project::default()
        };
        let error = project
            .save_with_commit(&target, |staged, commit_target| {
                assert_eq!(staged.parent(), target.parent());
                assert_eq!(commit_target, target);
                assert!(staged.exists());
                assert!(
                    std::fs::read_to_string(staged)
                        .unwrap()
                        .contains("Must not replace the old project")
                );
                Err(io::Error::other("injected commit failure"))
            })
            .unwrap_err();

        assert!(
            format!("{error:#}").contains("injected commit failure"),
            "unexpected error chain: {error:#}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert_no_project_save_temps(directory.path());
    }

    #[test]
    fn project_save_creates_missing_parents_and_round_trips() {
        let directory = TestDirectory::new("round-trip");
        let target = directory
            .path()
            .join("new")
            .join("nested")
            .join("session.citrus");
        assert!(!target.parent().unwrap().exists());

        let mut project = Project {
            name: "Round-trip session".into(),
            swing: 0.42,
            ..Project::default()
        };
        project.channels[0].name = "Persisted channel".into();
        project.save(&target).unwrap();

        let serialized = std::fs::read_to_string(&target).unwrap();
        assert!(serialized.ends_with('\n'));
        let restored = Project::load(&target).unwrap();
        assert_eq!(restored.name, project.name);
        assert_eq!(restored.swing, project.swing);
        assert_eq!(restored.channels[0].name, project.channels[0].name);
        assert_no_project_save_temps(target.parent().unwrap());
    }

    #[test]
    fn project_load_rejects_a_newer_format_without_rewriting_it() {
        let directory = TestDirectory::new("future-version");
        let target = directory.path().join("future.citrus");
        let mut value = serde_json::to_value(Project::default()).unwrap();
        value["format_version"] = serde_json::json!(CURRENT_PROJECT_FORMAT_VERSION + 1);
        let original = serde_json::to_vec_pretty(&value).unwrap();
        std::fs::write(&target, &original).unwrap();

        let error = Project::load(&target).unwrap_err();
        assert!(
            format!("{error:#}").contains("newer than this Citrus Studio build"),
            "unexpected error: {error:#}"
        );
        assert_eq!(std::fs::read(&target).unwrap(), original);
    }

    #[test]
    fn audio_split_references_round_trip_without_changing_their_domains() {
        let directory = TestDirectory::new("audio-split-references");
        let target = directory.path().join("session.citrus");
        let mut project = Project::default();
        let audio_clip = &mut project.clips[0];
        audio_clip.kind = ClipKind::Audio;
        audio_clip.audio_source_offset_frame = Some(12_345);
        audio_clip.fade_in_reference = Some(AudioFadeReference {
            offset_beats: -2.125,
            length_beats: 7.75,
            end_limit_beats: None,
            export_length_beats: None,
        });
        audio_clip.fade_out_reference = Some(AudioFadeReference {
            offset_beats: -1.0625,
            length_beats: 4.5,
            end_limit_beats: None,
            export_length_beats: None,
        });
        audio_clip.audio_source_reference = Some(AudioSourceReference {
            elapsed_spans_seconds: vec![
                AudioSourceSpan {
                    // This value requires serde_json's exact float parser to
                    // avoid moving by an ULP across a save/load boundary.
                    start_seconds: 971.3278064597747,
                    end_seconds: 973.2340564597747,
                },
                AudioSourceSpan {
                    start_seconds: 973.2340564597747,
                    end_seconds: 972.8278064597747,
                },
            ],
        });
        let original = audio_clip.clone();
        project
            .audio_clip_mixer_destinations
            .push(AudioClipMixerDestination {
                clip_id: original.id,
                mixer_track_id: MASTER_MIXER_TRACK_ID,
            });
        project.save(&target).unwrap();

        let restored = Project::load(&target).unwrap();
        let restored_clip = restored
            .clips
            .iter()
            .find(|clip| clip.id == original.id)
            .unwrap();
        assert_eq!(restored.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert_eq!(
            restored_clip.audio_source_offset_frame,
            original.audio_source_offset_frame
        );
        assert_eq!(restored_clip.fade_in_reference, original.fade_in_reference);
        assert_eq!(
            restored_clip.fade_out_reference,
            original.fade_out_reference
        );
        assert_eq!(
            restored_clip.audio_source_reference,
            original.audio_source_reference
        );
    }

    #[test]
    fn v10_project_without_audio_split_references_loads_identically() {
        let directory = TestDirectory::new("v10-audio-clips");
        let target = directory.path().join("legacy.citrus");
        let mut expected = Project::default();
        expected.normalize();
        let mut legacy = serde_json::to_value(&expected).unwrap();
        legacy["format_version"] = serde_json::json!(10);
        for clip in legacy["clips"].as_array().unwrap() {
            for field in [
                "fade_in_reference",
                "fade_out_reference",
                "audio_source_reference",
            ] {
                assert!(clip.get(field).is_none(), "{field} should be omitted");
            }
        }
        std::fs::write(&target, serde_json::to_vec(&legacy).unwrap()).unwrap();

        let restored = Project::load(&target).unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn audio_fade_reference_validation_bounds_exact_domains() {
        for offset in [-1_000_000.0, -2.5, 0.0, 1_000_000.0] {
            assert!(
                AudioFadeReference {
                    offset_beats: offset,
                    length_beats: 1_000_000.0,
                    end_limit_beats: None,
                    export_length_beats: None,
                }
                .is_valid()
            );
        }
        for length in [0.0, -1.0, 1_000_001.0, f64::NAN, f64::INFINITY] {
            assert!(
                !AudioFadeReference {
                    offset_beats: 0.0,
                    length_beats: length,
                    end_limit_beats: None,
                    export_length_beats: None,
                }
                .is_valid()
            );
        }
        for offset in [-1_000_001.0, 1_000_001.0, f64::NAN, f64::NEG_INFINITY] {
            assert!(
                !AudioFadeReference {
                    offset_beats: offset,
                    length_beats: 1.0,
                    end_limit_beats: None,
                    export_length_beats: None,
                }
                .is_valid()
            );
        }
    }

    #[test]
    fn audio_source_reference_validation_bounds_spans_and_cumulative_duration() {
        let span = AudioSourceSpan {
            start_seconds: 0.0,
            end_seconds: 1_000_000_000.0,
        };
        let reverse = AudioSourceSpan {
            start_seconds: span.end_seconds,
            end_seconds: span.start_seconds,
        };
        assert!(span.is_valid());
        assert!(reverse.is_valid());
        assert!(
            AudioSourceReference {
                elapsed_spans_seconds: vec![span, reverse, reverse],
            }
            .is_valid()
        );
        for spans in [vec![span, span], vec![reverse, reverse], vec![span; 4097]] {
            assert!(
                !AudioSourceReference {
                    elapsed_spans_seconds: spans,
                }
                .is_valid()
            );
        }
        let zero = AudioSourceSpan {
            start_seconds: 0.0,
            end_seconds: 0.0,
        };
        assert!(
            AudioSourceReference {
                elapsed_spans_seconds: vec![zero; 4096],
            }
            .is_valid()
        );
        assert!(
            !AudioSourceReference {
                elapsed_spans_seconds: vec![zero; 4097],
            }
            .is_valid()
        );
        for invalid in [-0.001, 1_000_000_001.0, f64::NAN, f64::INFINITY] {
            for span in [
                AudioSourceSpan {
                    start_seconds: invalid,
                    end_seconds: 1.0,
                },
                AudioSourceSpan {
                    start_seconds: 1.0,
                    end_seconds: invalid,
                },
            ] {
                assert!(!span.is_valid());
                assert!(
                    !AudioSourceReference {
                        elapsed_spans_seconds: vec![span],
                    }
                    .is_valid()
                );
            }
        }
    }

    #[test]
    fn invalid_audio_split_metadata_cannot_replace_a_saved_project() {
        let directory = TestDirectory::new("invalid-audio-split-save");
        let target = directory.path().join("session.citrus");
        let baseline = Project::default();
        baseline.save(&target).unwrap();
        let original = std::fs::read(&target).unwrap();

        let cases: &[fn(&mut Clip)] = &[
            |clip| {
                clip.fade_in_reference = Some(AudioFadeReference {
                    offset_beats: 0.0,
                    length_beats: 0.0,
                    end_limit_beats: None,
                    export_length_beats: None,
                });
            },
            |clip| {
                clip.fade_out_reference = Some(AudioFadeReference {
                    offset_beats: f64::NAN,
                    length_beats: 4.0,
                    end_limit_beats: None,
                    export_length_beats: None,
                });
            },
            |clip| {
                clip.fade_in_reference = Some(AudioFadeReference {
                    offset_beats: 0.0,
                    length_beats: f64::NAN,
                    end_limit_beats: None,
                    export_length_beats: None,
                });
            },
            |clip| {
                clip.audio_source_reference = Some(AudioSourceReference {
                    elapsed_spans_seconds: vec![AudioSourceSpan {
                        start_seconds: -0.25,
                        end_seconds: 1.0,
                    }],
                });
            },
            |clip| {
                clip.audio_source_reference = Some(AudioSourceReference {
                    elapsed_spans_seconds: vec![AudioSourceSpan {
                        start_seconds: 0.0,
                        end_seconds: f64::NAN,
                    }],
                });
            },
            |clip| {
                clip.audio_source_reference = Some(AudioSourceReference {
                    elapsed_spans_seconds: vec![
                        AudioSourceSpan {
                            start_seconds: 0.0,
                            end_seconds: 1.0,
                        };
                        4097
                    ],
                });
            },
        ];
        for corrupt in cases {
            let mut project = baseline.clone();
            corrupt(&mut project.clips[0]);
            let error = project.save(&target).unwrap_err();
            assert!(format!("{error:#}").contains("invalid audio split metadata"));
            assert_eq!(std::fs::read(&target).unwrap(), original);
            assert_no_project_save_temps(directory.path());
            let new_target = directory.path().join("not-created/session.citrus");
            assert!(project.save(&new_target).is_err());
            assert!(!new_target.parent().unwrap().exists());
        }
    }

    #[test]
    fn project_load_rejects_malformed_audio_split_metadata() {
        let directory = TestDirectory::new("invalid-audio-split-load");
        let target = directory.path().join("session.citrus");
        let baseline = serde_json::to_value(Project::default()).unwrap();
        let invalid_metadata = [
            (
                "fade_in_reference",
                serde_json::json!({
                    "offset_beats": -2.0, "length_beats": 0.0
                }),
            ),
            (
                "fade_out_reference",
                serde_json::json!({
                    "offset_beats": 0.0, "length_beats": -1.0
                }),
            ),
            (
                "fade_in_reference",
                serde_json::json!({
                    "offset_beats": 1_000_001.0, "length_beats": 1.0
                }),
            ),
            (
                "audio_source_reference",
                serde_json::json!({
                    "elapsed_spans_seconds": [{"start_seconds": -0.25, "end_seconds": 1.0}]
                }),
            ),
            (
                "audio_source_reference",
                serde_json::json!({
                    "elapsed_spans_seconds": [{"start_seconds": 0.0, "end_seconds": 1_000_000_001.0}]
                }),
            ),
            (
                "audio_source_reference",
                serde_json::json!({
                    "elapsed_spans_seconds": vec![serde_json::json!({"start_seconds": 0.0, "end_seconds": 0.0}); 4097]
                }),
            ),
            (
                "audio_source_reference",
                serde_json::json!({
                    "elapsed_spans_seconds": vec![serde_json::json!({"start_seconds": 0.0, "end_seconds": 1_000_000_000.0}); 2]
                }),
            ),
        ];
        for (field, invalid) in invalid_metadata {
            let mut value = baseline.clone();
            value["clips"][0][field] = invalid;
            let original = serde_json::to_vec(&value).unwrap();
            std::fs::write(&target, &original).unwrap();
            let error = Project::load(&target).unwrap_err();
            assert!(
                format!("{error:#}").contains("audio split metadata is invalid"),
                "{field}: {error:#}"
            );
            assert_eq!(std::fs::read(&target).unwrap(), original);
        }
    }

    #[test]
    fn default_project_has_independent_pattern_data() {
        let mut project = Project::default();
        let original = project.active_pattern().channel_steps[0][0];
        project.patterns.push(Pattern {
            id: 2,
            name: "Pattern 2".into(),
            length_steps: 16,
            channel_steps: vec![[false; 16]; project.channels.len()],
            notes: Vec::new(),
        });
        project.active_pattern = 1;
        project.active_pattern_mut().channel_steps[0][0] = !original;
        assert_eq!(project.patterns[0].channel_steps[0][0], original);
        assert_eq!(project.patterns[1].channel_steps[0][0], !original);
        assert!(
            project
                .clips
                .iter()
                .filter(|clip| clip.kind == ClipKind::Audio)
                .all(|clip| clip.audio_asset_id.is_some_and(|asset_id| project
                    .audio_assets
                    .iter()
                    .any(|asset| asset.id == asset_id)))
        );
    }

    #[test]
    fn blank_project_is_operational_without_demo_or_session_content() {
        let project = Project::blank();

        assert_eq!(project.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert_eq!(project.name, "Untitled");
        assert_eq!(project.channels.len(), 1);
        assert!(project.channels[0].steps.iter().all(|step| !step));
        assert_eq!(project.patterns.len(), 1);
        assert_eq!(project.patterns[0].channel_steps, vec![[false; 16]]);
        assert!(project.patterns[0].notes.is_empty());
        assert_eq!(project.mixer_tracks.len(), 32);
        assert_eq!(project.mixer_tracks[0].name, "MASTER");
        assert!(project.mixer_tracks.iter().all(|track| track.peak == 0.0));
        assert!(project.clips.is_empty());
        assert!(project.automation_lanes.is_empty());
        assert!(project.audio_assets.is_empty());
        assert!(project.plugin_instances.is_empty());
        assert!(project.mixer_insert_slots.is_empty());
        assert!(project.migration_diagnostics.is_empty());

        let mut normalized = project.clone();
        normalized.normalize();
        assert_eq!(
            serde_json::to_vec(&normalized).unwrap(),
            serde_json::to_vec(&project).unwrap()
        );
    }

    #[test]
    fn v8_notes_without_group_identity_migrate_cleanly_to_current_format() {
        let mut value = serde_json::to_value(Project::default()).unwrap();
        value["format_version"] = serde_json::json!(8);
        for pattern in value["patterns"].as_array_mut().unwrap() {
            for note in pattern["notes"].as_array_mut().unwrap() {
                note.as_object_mut().unwrap().remove("group_id");
            }
        }

        let mut migrated: Project = serde_json::from_value(value).unwrap();
        assert_eq!(migrated.format_version, 8);
        assert!(
            migrated
                .patterns
                .iter()
                .flat_map(|pattern| &pattern.notes)
                .all(|note| note.group_id.is_none())
        );

        migrated.normalize();

        assert_eq!(migrated.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert!(
            migrated
                .patterns
                .iter()
                .flat_map(|pattern| &pattern.notes)
                .all(|note| note.group_id.is_none())
        );
    }

    #[test]
    fn v9_clips_without_group_identity_migrate_cleanly_to_v10() {
        let mut value = serde_json::to_value(Project::default()).unwrap();
        value["format_version"] = serde_json::json!(9);
        for clip in value["clips"].as_array_mut().unwrap() {
            clip.as_object_mut().unwrap().remove("group_id");
        }

        let mut migrated: Project = serde_json::from_value(value).unwrap();
        assert_eq!(migrated.format_version, 9);
        assert!(migrated.clips.iter().all(|clip| clip.group_id.is_none()));

        migrated.normalize();

        assert_eq!(migrated.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert!(migrated.clips.iter().all(|clip| clip.group_id.is_none()));
    }

    #[test]
    fn note_group_normalization_keeps_only_multi_note_same_channel_groups() {
        let project = Project::default();
        let mut notes = project.patterns[0].notes[..5].to_vec();
        let original_channel = notes[0].channel_id;
        let other_channel = project
            .channels
            .iter()
            .map(|channel| Some(channel.id))
            .find(|channel_id| *channel_id != original_channel)
            .unwrap();
        notes[0].group_id = Some(7);
        notes[1].group_id = Some(7);
        notes[2].group_id = Some(8);
        notes[3].group_id = Some(7);
        notes[3].channel_id = other_channel;
        notes[4].group_id = Some(0);

        normalize_piano_note_groups(&mut notes);

        assert_eq!(notes[0].group_id, Some(7));
        assert_eq!(notes[1].group_id, Some(7));
        assert_eq!(notes[2].group_id, None);
        assert_eq!(notes[3].group_id, None);
        assert_eq!(notes[4].group_id, None);
    }

    #[test]
    fn note_group_identity_round_trips_in_the_project_file() {
        let mut project = Project::default();
        project.patterns[0].notes[0].group_id = Some(42);
        project.patterns[0].notes[1].group_id = Some(42);

        let encoded = serde_json::to_vec(&project).unwrap();
        let mut restored: Project = serde_json::from_slice(&encoded).unwrap();
        restored.normalize();

        assert_eq!(restored.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert_eq!(restored.patterns[0].notes[0].group_id, Some(42));
        assert_eq!(restored.patterns[0].notes[1].group_id, Some(42));
    }

    #[test]
    fn clip_group_normalization_and_project_round_trip_keep_only_real_groups() {
        let mut project = Project::default();
        project.clips[0].group_id = Some(42);
        project.clips[1].group_id = Some(42);
        project.clips[2].group_id = Some(7);
        project.clips[3].group_id = Some(0);

        let encoded = serde_json::to_vec(&project).unwrap();
        let mut restored: Project = serde_json::from_slice(&encoded).unwrap();
        restored.normalize();

        assert_eq!(restored.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert_eq!(restored.clips[0].group_id, Some(42));
        assert_eq!(restored.clips[1].group_id, Some(42));
        assert_eq!(restored.clips[2].group_id, None);
        assert_eq!(restored.clips[3].group_id, None);
    }

    #[test]
    fn legacy_project_normalizes_into_pattern_one() {
        let project = Project::default();
        let mut value = serde_json::to_value(&project).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("patterns");
        object.remove("active_pattern");
        let mut migrated: Project = serde_json::from_value(value).unwrap();
        migrated.normalize();
        assert_eq!(migrated.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert_eq!(migrated.patterns.len(), 1);
        assert_eq!(
            migrated.patterns[0].channel_steps[0],
            migrated.channels[0].steps
        );
    }

    #[test]
    fn normalization_repairs_non_finite_editable_values() {
        let mut project = Project {
            tempo: f32::NAN,
            swing: f32::INFINITY,
            ..Project::default()
        };
        project.channels[0].volume = f32::NEG_INFINITY;
        project.mixer_tracks[0].pan = f32::NAN;
        project.clips[0].start = f32::NAN;
        project.clips[0].length = -10.0;
        project.clips[0].source_offset = f32::INFINITY;
        project.patterns[0].notes[0].velocity = f32::NAN;

        project.normalize();

        assert_eq!(project.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert_eq!(project.tempo, 128.0);
        assert_eq!(project.swing, 0.0);
        assert!(project.channels[0].volume.is_finite());
        assert!(project.mixer_tracks[0].pan.is_finite());
        assert_eq!(project.clips[0].start, 0.0);
        assert!(project.clips[0].length >= 0.0625);
        assert_eq!(project.clips[0].source_offset, 0.0);
        assert!(project.patterns[0].notes[0].velocity.is_finite());
    }

    #[test]
    fn version_five_projects_migrate_with_an_empty_plugin_graph() {
        let project = Project::default();
        let mut value = serde_json::to_value(project).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("format_version".into(), serde_json::json!(5));
        object.remove("plugin_instances");
        object.remove("mixer_insert_slots");
        for channel in value["channels"].as_array_mut().unwrap() {
            channel
                .as_object_mut()
                .unwrap()
                .remove("instrument_plugin_instance_id");
        }

        let mut migrated: Project = serde_json::from_value(value).unwrap();
        assert_eq!(migrated.format_version, 5);
        assert!(migrated.plugin_instances.is_empty());
        assert!(migrated.mixer_insert_slots.is_empty());
        assert!(
            migrated
                .channels
                .iter()
                .all(|channel| channel.instrument_plugin_instance_id.is_none())
        );

        migrated.normalize();

        assert_eq!(migrated.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert!(migrated.plugin_instances.is_empty());
        assert!(migrated.mixer_insert_slots.is_empty());
        assert!(
            migrated
                .channels
                .iter()
                .all(|channel| channel.instrument_plugin_instance_id.is_none())
        );
    }

    #[test]
    fn plugin_state_round_trips_as_hex_and_runtime_status_is_session_only() {
        let mut project = Project::default();
        let mut instance = test_plugin(42, PluginFormat::Vst3, r"C:\VST3\Citrus.vst3");
        instance.role = PluginRole::Effect;
        instance.enabled = false;
        instance.bypass = true;
        instance.wet = 0.35;
        instance.parameters.insert(7, 0.625);
        instance.opaque_state = vec![0x00, 0x01, 0x7f, 0x80, 0xff];
        instance.runtime_status = PluginRuntimeStatus::Crashed;
        project.plugin_instances.push(instance);
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 0,
            slot: 0,
            plugin_instance_id: 42,
        });

        let value = serde_json::to_value(&project).unwrap();
        let serialized = value["plugin_instances"][0].as_object().unwrap();
        assert_eq!(serialized["format"], "vst3");
        assert_eq!(serialized["role"], "effect");
        assert_eq!(serialized["opaque_state"], "00017f80ff");
        assert!(!serialized.contains_key("runtime_status"));

        let restored: Project = serde_json::from_value(value).unwrap();
        let restored = &restored.plugin_instances[0];
        assert_eq!(restored.opaque_state, [0x00, 0x01, 0x7f, 0x80, 0xff]);
        assert_eq!(restored.runtime_status, PluginRuntimeStatus::Unloaded);
        assert_eq!(restored.role, PluginRole::Effect);
        assert!(!restored.enabled);
        assert!(restored.bypass);
        assert_eq!(restored.wet, 0.35);
        assert_eq!(restored.parameters.get(&7), Some(&0.625));
    }

    #[test]
    fn plugin_state_accepts_legacy_byte_arrays_and_rejects_bad_hex() {
        let mut value = serde_json::to_value(Project::default()).unwrap();
        value["plugin_instances"] = serde_json::json!([{
            "id": 1,
            "format": "vst2",
            "path": "C:\\VstPlugins\\Legacy.dll",
            "opaque_state": [0, 127, 255]
        }]);

        let restored: Project = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(restored.plugin_instances[0].opaque_state, [0, 127, 255]);
        assert!(restored.plugin_instances[0].enabled);
        assert_eq!(restored.plugin_instances[0].role, PluginRole::Unknown);
        assert_eq!(restored.plugin_instances[0].wet, 1.0);

        value["plugin_instances"][0]["opaque_state"] = serde_json::json!("0xz1");
        let error = serde_json::from_value::<Project>(value).unwrap_err();
        assert!(error.to_string().contains("invalid hex"));
    }

    #[test]
    fn plugin_normalization_repairs_ids_metadata_values_and_blank_paths() {
        let mut project = Project {
            format_version: 7,
            ..Project::default()
        };
        let mut first = test_plugin(7, PluginFormat::Vst2, r"C:\VstPlugins\Citrus EQ.dll");
        first.uid = "  1234  ".into();
        first.vendor = "  Citrus Labs ".into();
        first.name = "  Citrus EQ  ".into();
        first.wet = f32::NAN;
        first.parameters = BTreeMap::from([(1, -0.5), (2, 0.4), (3, f32::NAN), (4, 2.0)]);

        let mut duplicate = test_plugin(7, PluginFormat::Vst3, "   ");
        duplicate.runtime_status = PluginRuntimeStatus::Crashed;
        duplicate.wet = -1.0;

        // Basename fallback follows native Path semantics. Keep the Windows
        // backslash fixture on Windows and use an absolute POSIX path elsewhere.
        let zero_path = if cfg!(windows) {
            r"C:\VST3\Orchard Synth.vst3"
        } else {
            "/VST3/Orchard Synth.vst3"
        };
        let mut zero = test_plugin(0, PluginFormat::Vst3, zero_path);
        zero.wet = f32::INFINITY;

        project.plugin_instances = vec![first, duplicate, zero];
        project.normalize();

        let ids = project
            .plugin_instances
            .iter()
            .map(|instance| instance.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), 3);
        assert!(!ids.contains(&0));
        assert_eq!(project.plugin_instances[0].id, 7);

        let first = &project.plugin_instances[0];
        assert_eq!(first.uid, "1234");
        assert_eq!(first.vendor, "Citrus Labs");
        assert_eq!(first.name, "Citrus EQ");
        assert_eq!(first.wet, 1.0);
        assert_eq!(first.parameters.get(&1), Some(&0.0));
        assert_eq!(first.parameters.get(&2), Some(&0.4));
        assert!(!first.parameters.contains_key(&3));
        assert_eq!(first.parameters.get(&4), Some(&1.0));

        let duplicate = &project.plugin_instances[1];
        assert!(duplicate.path.as_os_str().is_empty());
        assert_eq!(duplicate.name, "Missing VST3 plug-in");
        assert_eq!(duplicate.runtime_status, PluginRuntimeStatus::Missing);
        assert_eq!(duplicate.wet, 0.0);

        let zero = &project.plugin_instances[2];
        assert_eq!(zero.name, "Orchard Synth");
        assert_eq!(zero.runtime_status, PluginRuntimeStatus::Unloaded);
        assert_eq!(zero.wet, 1.0);
    }

    #[test]
    fn v8_plugin_normalization_preserves_all_structural_identity_and_order() {
        let mut project = Project::default();
        let mut first = test_plugin(0, PluginFormat::Vst3, r"C:\VST3\First.vst3");
        first.uid = "  stable uid  ".into();
        first.wet = f32::INFINITY;
        let second = test_plugin(0, PluginFormat::Vst2, r"C:\VstPlugins\Second.dll");
        project.plugin_instances = vec![first, second];
        project.channels[0].instrument_plugin_instance_id = Some(0);
        project.channels[1].instrument_plugin_instance_id = Some(0);
        project.mixer_insert_slots = vec![
            MixerInsertSlotRef {
                track: 2,
                slot: MIXER_INSERT_SLOT_COUNT,
                plugin_instance_id: 0,
            },
            MixerInsertSlotRef {
                track: 1,
                slot: 3,
                plugin_instance_id: 0,
            },
        ];
        project.automation_lanes[0]
            .lane
            .set_target(AutomationTarget::PluginParameter {
                instance: 0,
                parameter: 9,
            });

        let instance_ids = project
            .plugin_instances
            .iter()
            .map(|instance| instance.id)
            .collect::<Vec<_>>();
        let channel_references = project
            .channels
            .iter()
            .map(|channel| channel.instrument_plugin_instance_id)
            .collect::<Vec<_>>();
        let slots = project.mixer_insert_slots.clone();
        let automation_target = project.automation_lanes[0].lane.target().clone();

        project.normalize();

        assert_eq!(project.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert_eq!(
            project
                .plugin_instances
                .iter()
                .map(|instance| instance.id)
                .collect::<Vec<_>>(),
            instance_ids
        );
        assert_eq!(
            project
                .channels
                .iter()
                .map(|channel| channel.instrument_plugin_instance_id)
                .collect::<Vec<_>>(),
            channel_references
        );
        assert_eq!(project.mixer_insert_slots, slots);
        assert_eq!(
            project.automation_lanes[0].lane.target(),
            &automation_target
        );
        assert_eq!(project.plugin_instances[0].uid, "stable uid");
        assert_eq!(project.plugin_instances[0].wet, 1.0);
        assert!(project.validate_mixer_graph().is_err());
    }

    #[test]
    fn slot_normalization_drops_invalid_placements_and_uses_latest_placement() {
        let mut project = Project {
            format_version: 7,
            ..Project::default()
        };
        project.plugin_instances = vec![
            test_plugin(10, PluginFormat::Vst2, r"C:\VstPlugins\A.dll"),
            test_plugin(20, PluginFormat::Vst2, r"C:\VstPlugins\B.dll"),
            test_plugin(30, PluginFormat::Vst3, r"C:\VST3\C.vst3"),
        ];
        project.mixer_insert_slots = vec![
            MixerInsertSlotRef {
                track: 2,
                slot: 1,
                plugin_instance_id: 10,
            },
            MixerInsertSlotRef {
                track: 1,
                slot: 2,
                plugin_instance_id: 10,
            },
            MixerInsertSlotRef {
                track: 0,
                slot: 3,
                plugin_instance_id: 20,
            },
            MixerInsertSlotRef {
                track: 0,
                slot: 3,
                plugin_instance_id: 30,
            },
            MixerInsertSlotRef {
                track: 0,
                slot: MIXER_INSERT_SLOT_COUNT,
                plugin_instance_id: 20,
            },
            MixerInsertSlotRef {
                track: 0,
                slot: 4,
                plugin_instance_id: 999,
            },
        ];

        project.normalize();

        assert_eq!(
            project.mixer_insert_slots,
            [
                MixerInsertSlotRef {
                    track: MASTER_MIXER_TRACK_ID,
                    slot: 3,
                    plugin_instance_id: 30,
                },
                MixerInsertSlotRef {
                    track: 1,
                    slot: 2,
                    plugin_instance_id: 10,
                },
            ]
        );
        assert_eq!(project.plugin_instances.len(), 3);

        let once = serde_json::to_value(&project).unwrap();
        project.normalize();
        assert_eq!(serde_json::to_value(project).unwrap(), once);
    }

    #[test]
    fn v8_plugin_structure_validation_is_explicit_and_role_agnostic() {
        fn plugin_project() -> Project {
            let mut project = Project::blank();
            project.plugin_instances = vec![
                test_plugin(10, PluginFormat::Vst3, r"C:\VST3\Ten.vst3"),
                test_plugin(20, PluginFormat::Vst2, r"C:\VstPlugins\Twenty.dll"),
            ];
            project
        }

        fn assert_invalid(project: &Project, expected: &str) {
            let error = project.validate_mixer_graph().unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.contains(expected),
                "expected '{expected}' in '{message}'"
            );
        }

        let mut zero = plugin_project();
        zero.plugin_instances[0].id = 0;
        assert_invalid(&zero, "reserved zero identity");

        let mut duplicate = plugin_project();
        duplicate.plugin_instances[1].id = 10;
        assert_invalid(&duplicate, "identity 10 is duplicated");

        let mut missing_automation = plugin_project();
        let mut lane = AutomationLane::new(AutomationTarget::PluginParameter {
            instance: 999,
            parameter: 1,
        });
        lane.replace_points([AutomationPoint::new(0.0, 0.5)]);
        missing_automation.automation_lanes.push(ProjectAutomation {
            id: 900,
            name: "Missing plug-in automation".into(),
            lane,
        });
        assert_invalid(
            &missing_automation,
            "automation 900 references missing plug-in 999",
        );

        let mut missing_generator = plugin_project();
        missing_generator.channels[0].instrument_plugin_instance_id = Some(999);
        assert_invalid(&missing_generator, "references missing plug-in 999");

        let mut duplicate_placement = plugin_project();
        duplicate_placement.channels[0].instrument_plugin_instance_id = Some(10);
        duplicate_placement
            .mixer_insert_slots
            .push(MixerInsertSlotRef {
                track: 1,
                slot: 0,
                plugin_instance_id: 10,
            });
        assert_invalid(&duplicate_placement, "plug-in 10 is placed more than once");

        let mut invalid_slot = plugin_project();
        invalid_slot.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 1,
            slot: MIXER_INSERT_SLOT_COUNT,
            plugin_instance_id: 10,
        });
        assert_invalid(&invalid_slot, "is outside 0..10");

        let mut duplicate_slot = plugin_project();
        duplicate_slot.mixer_insert_slots.extend([
            MixerInsertSlotRef {
                track: 1,
                slot: 2,
                plugin_instance_id: 10,
            },
            MixerInsertSlotRef {
                track: 1,
                slot: 2,
                plugin_instance_id: 20,
            },
        ]);
        assert_invalid(&duplicate_slot, "slot 2 is occupied more than once");

        let mut missing_insert = plugin_project();
        missing_insert.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 1,
            slot: 0,
            plugin_instance_id: 999,
        });
        assert_invalid(&missing_insert, "references missing plug-in 999");

        // Scanner classification is descriptive only: placement, not role,
        // defines whether an instance is a generator or an insert.
        let mut role_agnostic = plugin_project();
        role_agnostic.plugin_instances[0].role = PluginRole::Effect;
        role_agnostic.plugin_instances[1].role = PluginRole::Instrument;
        role_agnostic.channels[0].instrument_plugin_instance_id = Some(10);
        role_agnostic.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 1,
            slot: 0,
            plugin_instance_id: 20,
        });
        role_agnostic.validate_mixer_graph().unwrap();
    }

    #[test]
    fn invalid_v8_plugin_structure_is_rejected_on_load_and_before_atomic_save() {
        let directory = TestDirectory::new("invalid-v8-plugin-structure");
        let damaged = directory.path().join("damaged.citrus");
        let mut project = Project::blank();
        project
            .plugin_instances
            .push(test_plugin(10, PluginFormat::Vst3, r"C:\VST3\Damaged.vst3"));
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 1,
            slot: MIXER_INSERT_SLOT_COUNT,
            plugin_instance_id: 10,
        });
        std::fs::write(&damaged, serde_json::to_vec_pretty(&project).unwrap()).unwrap();

        let error = Project::load(&damaged).unwrap_err();
        assert!(format!("{error:#}").contains("is outside 0..10"));

        let protected = directory.path().join("protected.citrus");
        let original = b"keep this project";
        std::fs::write(&protected, original).unwrap();
        let error = project.save(&protected).unwrap_err();
        assert!(format!("{error:#}").contains("is outside 0..10"));
        assert_eq!(std::fs::read(&protected).unwrap(), original);
        assert_no_project_save_temps(directory.path());
    }

    #[test]
    fn generator_normalization_is_stable_and_takes_placement_priority() {
        let mut project = Project {
            format_version: 7,
            ..Project::default()
        };
        let mut classified_effect =
            test_plugin(10, PluginFormat::Vst3, r"C:\VST3\Misclassified.vst3");
        classified_effect.role = PluginRole::Effect;
        let mut classified_instrument =
            test_plugin(20, PluginFormat::Vst2, r"C:\VstPlugins\Synth.dll");
        classified_instrument.role = PluginRole::Instrument;
        project.plugin_instances = vec![classified_effect, classified_instrument];
        project.channels[0].instrument_plugin_instance_id = Some(10);
        project.channels[1].instrument_plugin_instance_id = Some(10);
        project.channels[2].instrument_plugin_instance_id = Some(999);
        project.mixer_insert_slots = vec![
            MixerInsertSlotRef {
                track: 0,
                slot: 0,
                plugin_instance_id: 10,
            },
            MixerInsertSlotRef {
                track: 0,
                slot: 1,
                plugin_instance_id: 20,
            },
        ];

        project.normalize();

        assert_eq!(project.channels[0].instrument_plugin_instance_id, Some(10));
        assert_eq!(project.channels[1].instrument_plugin_instance_id, None);
        assert_eq!(project.channels[2].instrument_plugin_instance_id, None);
        assert_eq!(
            project.mixer_insert_slots,
            [MixerInsertSlotRef {
                track: MASTER_MIXER_TRACK_ID,
                slot: 1,
                plugin_instance_id: 20,
            }]
        );
        assert_eq!(project.plugin_instances[0].role, PluginRole::Effect);
        assert_eq!(project.plugin_instances[1].role, PluginRole::Instrument);

        let once = serde_json::to_value(&project).unwrap();
        project.normalize();
        assert_eq!(serde_json::to_value(project).unwrap(), once);
    }

    #[test]
    fn generator_reference_follows_duplicate_zero_id_migration() {
        let mut project = Project {
            format_version: 7,
            plugin_instances: vec![
                test_plugin(0, PluginFormat::Vst3, r"C:\VST3\Generator.vst3"),
                test_plugin(0, PluginFormat::Vst2, r"C:\VstPlugins\Other.dll"),
            ],
            ..Project::default()
        };
        project.channels[0].instrument_plugin_instance_id = Some(0);

        project.normalize();

        let normalized_id = project.plugin_instances[0].id;
        assert_ne!(normalized_id, 0);
        assert_ne!(project.plugin_instances[1].id, normalized_id);
        assert_eq!(
            project.channels[0].instrument_plugin_instance_id,
            Some(normalized_id)
        );
    }

    #[test]
    fn zero_id_migration_updates_slots_and_plugin_automation() {
        let mut project = Project {
            format_version: 7,
            plugin_instances: vec![test_plugin(0, PluginFormat::Vst3, r"C:\VST3\Zero.vst3")],
            mixer_insert_slots: vec![MixerInsertSlotRef {
                track: 0,
                slot: 0,
                plugin_instance_id: 0,
            }],
            ..Project::default()
        };
        project.automation_lanes[0]
            .lane
            .set_target(AutomationTarget::PluginParameter {
                instance: 0,
                parameter: 9,
            });

        project.normalize();

        let normalized_id = project.plugin_instances[0].id;
        assert_ne!(normalized_id, 0);
        assert_eq!(
            project.mixer_insert_slots[0].plugin_instance_id,
            normalized_id
        );
        assert_eq!(
            project.automation_lanes[0].lane.target(),
            &AutomationTarget::PluginParameter {
                instance: normalized_id,
                parameter: 9,
            }
        );
    }

    #[test]
    fn missing_version_is_v1_and_piano_route_remains_unresolved() {
        let mut value = serde_json::to_value(Project::default()).unwrap();
        value.as_object_mut().unwrap().remove("format_version");
        for pattern in value["patterns"].as_array_mut().unwrap() {
            for note in pattern["notes"].as_array_mut().unwrap() {
                note.as_object_mut().unwrap().remove("id");
                note.as_object_mut().unwrap().remove("channel_id");
            }
        }
        let mut project: Project = serde_json::from_value(value).unwrap();
        assert_eq!(project.format_version, LEGACY_PROJECT_FORMAT_VERSION);

        project.normalize();

        assert_eq!(project.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert!(
            project
                .patterns
                .iter()
                .flat_map(|pattern| &pattern.notes)
                .all(|note| note.id != 0 && note.channel_id.is_none())
        );
        assert!(project.migration_diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic,
                ProjectMigrationDiagnostic::LegacyPianoRouteUnresolved { .. }
            )
        }));
    }

    #[test]
    fn timeline_ids_are_stable_unique_and_idempotent() {
        let mut project = Project::default();
        project.patterns[0].notes.truncate(3);
        project.patterns[0].notes[0].id = 0;
        project.patterns[0].notes[1].id = 1;
        project.patterns[0].notes[2].id = 1;
        project.clips[0].id = 0;
        project.clips[1].id = 1;
        project.clips[2].id = 1;
        let duplicate = project.automation_lanes[0].clone();
        let mut zero = duplicate.clone();
        zero.id = 0;
        project.automation_lanes.extend([duplicate, zero]);

        project.normalize();

        let note_ids = project.patterns[0]
            .notes
            .iter()
            .map(|note| note.id)
            .collect::<BTreeSet<_>>();
        let clip_ids = project
            .clips
            .iter()
            .map(|clip| clip.id)
            .collect::<BTreeSet<_>>();
        let automation_ids = project
            .automation_lanes
            .iter()
            .map(|lane| lane.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(note_ids.len(), project.patterns[0].notes.len());
        assert_eq!(clip_ids.len(), project.clips.len());
        assert_eq!(automation_ids.len(), project.automation_lanes.len());
        assert!(!note_ids.contains(&0));
        assert!(!clip_ids.contains(&0));
        assert!(!automation_ids.contains(&0));
        assert_eq!(project.patterns[0].notes[1].id, 1);
        assert_eq!(project.next_note_id(), Some(4));

        let once = serde_json::to_value(&project).unwrap();
        let diagnostics = project.migration_diagnostics.clone();
        project.normalize();
        assert_eq!(serde_json::to_value(&project).unwrap(), once);
        assert_eq!(project.migration_diagnostics, diagnostics);
    }

    #[test]
    fn legacy_piano_mirror_is_migration_input_only() {
        let mut canonical = Project {
            format_version: 6,
            ..Project::default()
        };
        canonical.piano_notes = vec![canonical.patterns[0].notes[0].clone()];
        canonical.normalize();
        assert!(canonical.piano_notes.is_empty());
        assert!(canonical.migration_diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic,
                ProjectMigrationDiagnostic::LegacyPianoMirrorIgnored { note_count: 1 }
            )
        }));
        assert!(
            !serde_json::to_value(&canonical)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("piano_notes")
        );

        let mut mirror_only = Project {
            format_version: 1,
            ..Project::default()
        };
        let expected = mirror_only.patterns[0].notes[0].note;
        mirror_only.piano_notes = vec![mirror_only.patterns[0].notes[0].clone()];
        mirror_only.patterns.clear();
        mirror_only.normalize();
        assert_eq!(mirror_only.patterns[0].notes.len(), 1);
        assert_eq!(mirror_only.patterns[0].notes[0].note, expected);
        assert!(mirror_only.patterns[0].notes[0].channel_id.is_none());
    }

    #[test]
    fn v6_placement_and_constant_tempo_audio_offsets_migrate() {
        let mut project = Project {
            format_version: 6,
            tempo: 120.0,
            ..Project::default()
        };
        project.audio_assets.push(AudioAsset {
            id: 77,
            name: "Take".into(),
            path: PathBuf::from("Take.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 24,
            frames: 480_000,
            waveform_peaks: Vec::new(),
        });
        let mut audio = clip(77, 0, 2.0, 2.0, "Take", [1, 2, 3], ClipKind::Audio);
        audio.audio_asset_id = Some(77);
        audio.audio_source_offset_frame = None;
        audio.source_offset = 1.0;
        project.clips.push(audio);

        project.normalize();

        let placement = project
            .clips
            .iter()
            .find(|clip| clip.kind == ClipKind::Automation)
            .unwrap();
        assert_eq!(placement.source_offset, placement.start);
        let audio = project.clips.iter().find(|clip| clip.id == 77).unwrap();
        assert_eq!(audio.audio_source_offset_frame, Some(24_000));
        assert_eq!(audio.source_offset, 0.0);
    }

    #[test]
    fn tempo_automated_v6_audio_offset_is_explicitly_unresolved() {
        let mut project = Project {
            format_version: 6,
            ..Project::default()
        };
        project.automation_lanes[0]
            .lane
            .set_target(AutomationTarget::Tempo);
        project.audio_assets.push(AudioAsset {
            id: 88,
            name: "Tempo take".into(),
            path: PathBuf::from("Tempo take.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 24,
            frames: 480_000,
            waveform_peaks: Vec::new(),
        });
        let mut audio = clip(88, 0, 8.0, 2.0, "Tempo take", [1, 2, 3], ClipKind::Audio);
        audio.audio_asset_id = Some(88);
        audio.audio_source_offset_frame = None;
        audio.source_offset = 1.0;
        project.clips.push(audio);

        project.normalize();

        let audio = project.clips.iter().find(|clip| clip.id == 88).unwrap();
        assert_eq!(audio.audio_source_offset_frame, None);
        assert_eq!(audio.source_offset, 1.0);
        assert!(project.migration_diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic,
                ProjectMigrationDiagnostic::AudioSourceOffsetUnresolved {
                    clip_id: 88,
                    issue: AudioOffsetMigrationIssue::TempoAutomationRequiresTimeline,
                }
            )
        }));
    }

    #[test]
    fn v7_mixer_indices_migrate_to_stable_ids_slots_routes_without_losing_piano_routes() {
        let mut project = Project {
            format_version: 7,
            ..Project::default()
        };
        project.mixer_tracks.truncate(12);
        for track in &mut project.mixer_tracks {
            track.id = 0;
            track.runtime_slot = 0;
        }
        project.channels[0].mixer_track = 0;
        let piano_routes = project.patterns[0]
            .notes
            .iter()
            .map(|note| note.channel_id)
            .collect::<Vec<_>>();
        project
            .plugin_instances
            .push(test_plugin(500, PluginFormat::Vst3, "Legacy.vst3"));
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 7,
            slot: 2,
            plugin_instance_id: 500,
        });
        let mut audio = clip(900, 5, 0.0, 4.0, "Legacy audio", [1, 2, 3], ClipKind::Audio);
        audio.audio_asset_id = Some(44);
        audio.audio_source_offset_frame = Some(0);
        project.clips.push(audio);
        project.mixer_routes.clear();
        project.audio_clip_mixer_destinations.clear();

        project.normalize();

        assert_eq!(project.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        assert_eq!(project.mixer_tracks.len(), MIXER_GRAPH_MAX_NODES);
        for runtime_slot in 0_u8..32 {
            let track = project
                .mixer_tracks
                .iter()
                .find(|track| track.runtime_slot == runtime_slot)
                .unwrap();
            assert_eq!(track.id, mixer_track_id_for_runtime_slot(runtime_slot));
        }
        assert_eq!(project.channels[0].mixer_track, MASTER_MIXER_TRACK_ID);
        assert_eq!(project.mixer_insert_slots[0].track, 7);
        assert_eq!(project.mixer_routes.len(), 31);
        assert!(project.mixer_routes.iter().all(|route| {
            route.destination
                == MixerRouteDestination::MainInput {
                    mixer_track_id: MASTER_MIXER_TRACK_ID,
                }
                && route.tap == MixerRouteTap::PostFader
                && route.enabled
        }));
        assert_eq!(
            project.audio_clip_mixer_track_id(900),
            Some(mixer_track_id_for_runtime_slot(6))
        );
        assert_eq!(
            project.patterns[0]
                .notes
                .iter()
                .map(|note| note.channel_id)
                .collect::<Vec<_>>(),
            piano_routes,
            "v7 already has authoritative Piano Channel IDs"
        );
        compile_mixer_graph(&project).unwrap();
    }

    #[test]
    fn v8_serialization_uses_only_stable_mixer_identity_fields_and_survives_reorder() {
        let mut project = Project::blank();
        project
            .plugin_instances
            .push(test_plugin(700, PluginFormat::Vst3, "Stable.vst3"));
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 3,
            slot: 1,
            plugin_instance_id: 700,
        });
        let mut lane = AutomationLane::new(AutomationTarget::MixerPan { track: 3 });
        lane.replace_points([AutomationPoint::new(0.0, 0.25)]);
        project.automation_lanes.push(ProjectAutomation {
            id: 701,
            name: "Stable mixer target".into(),
            lane,
        });
        let channel_destination = project.channels[0].mixer_track;
        let slot_destination = project.mixer_insert_slots[0].track;
        let serialized = serde_json::to_value(&project).unwrap();
        let channel = &serialized["channels"][0];
        assert!(channel.get("mixer_track_id").is_some());
        assert!(channel.get("mixer_track").is_none());
        let slot = &serialized["mixer_insert_slots"][0];
        assert!(slot.get("mixer_track_id").is_some());
        assert!(slot.get("track").is_none());
        let target = &serialized["automation_lanes"][0]["lane"]["target"];
        assert!(target.get("mixer_track_id").is_some());
        assert!(target.get("track").is_none());
        assert!(
            serialized["mixer_tracks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|track| { track.get("id").is_some() && track.get("runtime_slot").is_some() })
        );

        let mut restored: Project = serde_json::from_value(serialized).unwrap();
        restored.mixer_tracks.reverse();
        assert_eq!(restored.channels[0].mixer_track, channel_destination);
        assert_eq!(restored.mixer_insert_slots[0].track, slot_destination);
        assert_eq!(restored.mixer_runtime_slot(channel_destination), Some(1));
        assert_eq!(restored.mixer_runtime_slot(slot_destination), Some(3));
        compile_mixer_graph(&restored).unwrap();
    }

    #[test]
    fn invalid_v8_graph_is_rejected_before_save_can_replace_the_target() {
        let directory = TestDirectory::new("invalid-mixer-save");
        let target = directory.path().join("protected.citrus");
        let original = b"keep this project";
        std::fs::write(&target, original).unwrap();
        let mut project = Project::blank();
        project.mixer_routes[0].destination = MixerRouteDestination::MainInput {
            mixer_track_id: 999_999,
        };

        let error = project.save(&target).unwrap_err();
        assert!(format!("{error:#}").contains("missing destination"));
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert_no_project_save_temps(directory.path());
    }

    #[test]
    fn v8_dangling_mixer_insert_identity_survives_normalization_and_load_is_rejected() {
        let directory = TestDirectory::new("dangling-mixer-insert");
        let target = directory.path().join("damaged.citrus");
        let mut project = Project::blank();
        project
            .plugin_instances
            .push(test_plugin(702, PluginFormat::Vst3, "Damaged.vst3"));
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: 999_999,
            slot: 0,
            plugin_instance_id: 702,
        });
        std::fs::write(&target, serde_json::to_vec_pretty(&project).unwrap()).unwrap();

        let error = Project::load(&target).unwrap_err();
        assert!(format!("{error:#}").contains("references missing mixer track 999999"));
    }

    fn test_plugin(id: u64, format: PluginFormat, path: impl Into<PathBuf>) -> PluginInstance {
        PluginInstance {
            midi_ports: crate::plugin_midi_routing::PluginMidiPorts::default(),
            id,
            format,
            role: PluginRole::Unknown,
            path: path.into(),
            uid: String::new(),
            vendor: String::new(),
            name: String::new(),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: BTreeMap::new(),
            opaque_state: Vec::new(),
            runtime_status: PluginRuntimeStatus::Unloaded,
        }
    }
    #[test]
    fn exact_length_and_cropped_fade_metadata_are_bounded_and_persisted() {
        let mut project = Project::default();
        project.clips[0].audio_length_reference = Some(AudioLengthReference {
            stored_length_beats: project.clips[0].length,
            length_beats: 3.123_456_789_123,
            export_length_beats: 3.123_456_7,
        });
        project.clips[0].fade_in_reference = Some(AudioFadeReference {
            offset_beats: -1.0,
            length_beats: 4.0,
            end_limit_beats: Some(2.0),
            export_length_beats: Some(4.000_000_001),
        });
        let directory = TestDirectory::new("precise-audio-geometry");
        let target = directory.path().join("session.citrus");
        project.save(&target).unwrap();
        let restored = Project::load(&target).unwrap();
        assert_eq!(
            restored.clips[0].audio_length_reference,
            project.clips[0].audio_length_reference
        );
        assert_eq!(
            restored.clips[0].fade_in_reference,
            project.clips[0].fade_in_reference
        );
        let original = std::fs::read(&target).unwrap();
        project.clips[0]
            .audio_length_reference
            .as_mut()
            .unwrap()
            .export_length_beats = f64::NAN;
        assert!(project.save(&target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), original);
        let mut invalid: serde_json::Value = serde_json::from_slice(&original).unwrap();
        invalid["clips"][0]["audio_length_reference"]["length_beats"] = serde_json::json!(-1.0);
        std::fs::write(&target, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(Project::load(&target).is_err());
        let mut invalid: serde_json::Value = serde_json::from_slice(&original).unwrap();
        invalid["clips"][0]["fade_in_reference"]["end_limit_beats"] = serde_json::json!(-2.0);
        std::fs::write(&target, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(Project::load(&target).is_err());
    }
    #[test]
    fn v12_midi_ports_save_load_and_incompatible_load_is_explicitly_disabled() {
        let directory = TestDirectory::new("midi-port-save-load");
        let path = directory.path().join("ports.citrus");
        let mut project = Project {
            plugin_instances: vec![
                test_plugin(1001, PluginFormat::Vst3, "source.vst3"),
                test_plugin(1002, PluginFormat::Vst3, "sink.vst3"),
            ],
            ..Project::default()
        };
        project.channels[0].instrument_plugin_instance_id = Some(1001);
        project.channels[1].instrument_plugin_instance_id = Some(1002);
        project.plugin_instances[0].midi_ports.output = Some(0);
        project.plugin_instances[0].midi_ports.audio_monitor_muted = true;
        project.plugin_instances[1].midi_ports.input = Some(0);
        project.save(&path).unwrap();
        let restored = Project::load(&path).unwrap();
        assert_eq!(restored.format_version, 12);
        assert_eq!(
            restored.plugin_instances[0].midi_ports,
            project.plugin_instances[0].midi_ports
        );
        assert_eq!(restored.plugin_instances[1].midi_ports.input, Some(0));
        let mut invalid = serde_json::to_value(&restored).unwrap();
        invalid["plugin_instances"][1]["midi_ports"]["input"] = 9.into();
        std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        let reopened = Project::load(&path).unwrap();
        assert_eq!(reopened.plugin_instances[1].midi_ports.input, None);
        assert_eq!(reopened.channels.len(), restored.channels.len());
        assert_eq!(
            serde_json::to_value(&reopened.patterns).unwrap(),
            serde_json::to_value(&restored.patterns).unwrap()
        );
        assert!(reopened.migration_diagnostics.iter().any(|diagnostic| matches!(diagnostic, ProjectMigrationDiagnostic::MidiRoutingDisabled { reason } if reason.contains("no Generator"))));
    }
}
