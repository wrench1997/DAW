//! Standard MIDI File (SMF) import and export.
//!
//! This module deliberately has no MIDI-specific dependency. It supports SMF
//! format 0 and 1 files that use metrical (PPQ) timing, including running
//! status, note-on/note-off events, tempo meta events, track names, and safe
//! skipping of events Citrus Studio does not currently consume.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail, ensure};

use crate::model::{PianoNote, Project};

/// The PPQ resolution used when the caller does not request another value.
pub const DEFAULT_PPQ: u16 = 480;

const MAX_SMF_TRACKS: usize = 4_096;
const MAX_VLQ: u64 = 0x0fff_ffff;
const DEFAULT_TEMPO_US_PER_QUARTER: u32 = 500_000;
const MIDI_EXPORT_TEMP_PREFIX: &str = ".citrus-midi-";
const MIDI_EXPORT_TEMP_ATTEMPTS: usize = 128;
static MIDI_EXPORT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "MoveFileExW"]
    fn move_file_ex_w(existing_file_name: *const u16, new_file_name: *const u16, flags: u32)
    -> i32;
}

struct MidiExportTemp {
    path: Option<PathBuf>,
}

impl MidiExportTemp {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for MidiExportTemp {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A decoded Standard MIDI File.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiFile {
    /// SMF format, currently either 0 or 1.
    pub format: u16,
    /// Pulses (ticks) per quarter note.
    pub ppq: u16,
    /// Musical tracks in file order.
    pub tracks: Vec<MidiTrack>,
    /// Tempo changes collected from all tracks, ordered by absolute tick.
    pub tempo_events: Vec<MidiTempo>,
}

impl MidiFile {
    /// Returns the first tempo in the file, or the MIDI default of 120 BPM.
    pub fn initial_tempo_bpm(&self) -> f32 {
        self.tempo_events.first().map_or(
            60_000_000.0 / DEFAULT_TEMPO_US_PER_QUARTER as f32,
            |tempo| tempo.bpm(),
        )
    }

    /// Number of note events after matching note-on and note-off messages.
    pub fn note_count(&self) -> usize {
        self.tracks.iter().map(|track| track.notes.len()).sum()
    }
}

/// Explicit routing from an SMF channel to a stable project Channel Rack id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiImportChannelMap {
    routes: BTreeMap<u8, u32>,
}

impl MidiImportChannelMap {
    pub fn try_new(routes: impl IntoIterator<Item = (u8, u32)>) -> Result<Self> {
        let mut map = Self::default();
        for (midi_channel, project_channel_id) in routes {
            ensure!(
                midi_channel < 16,
                "MIDI import channel {midi_channel} is outside 0..=15"
            );
            ensure!(
                project_channel_id != 0,
                "MIDI import channel {midi_channel} cannot route to project channel id zero"
            );
            ensure!(
                map.routes
                    .insert(midi_channel, project_channel_id)
                    .is_none(),
                "MIDI import channel {midi_channel} has more than one route"
            );
        }
        Ok(map)
    }

    #[must_use]
    pub fn project_channel_id(&self, midi_channel: u8) -> Option<u32> {
        self.routes.get(&midi_channel).copied()
    }
}

/// Explicit routing from a stable project Channel Rack id to an SMF channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiExportChannelMap {
    routes: BTreeMap<u32, u8>,
}

impl MidiExportChannelMap {
    pub fn try_new(routes: impl IntoIterator<Item = (u32, u8)>) -> Result<Self> {
        let mut map = Self::default();
        for (project_channel_id, midi_channel) in routes {
            ensure!(
                project_channel_id != 0,
                "project channel id zero cannot be exported to MIDI"
            );
            ensure!(
                midi_channel < 16,
                "MIDI export channel {midi_channel} is outside 0..=15"
            );
            ensure!(
                map.routes
                    .insert(project_channel_id, midi_channel)
                    .is_none(),
                "project channel {project_channel_id} has more than one MIDI route"
            );
        }
        Ok(map)
    }

    #[must_use]
    pub fn midi_channel(&self, project_channel_id: u32) -> Option<u8> {
        self.routes.get(&project_channel_id).copied()
    }
}

/// A decoded MIDI track.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiTrack {
    pub name: Option<String>,
    pub notes: Vec<MidiNote>,
}

/// A complete MIDI note expressed in absolute PPQ ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiNote {
    pub channel: u8,
    pub note: u8,
    pub start_ticks: u64,
    pub length_ticks: u64,
    pub velocity: u8,
}

/// A tempo change expressed as microseconds per quarter note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiTempo {
    pub tick: u64,
    pub microseconds_per_quarter: u32,
}

impl MidiTempo {
    pub fn bpm(self) -> f32 {
        60_000_000.0 / self.microseconds_per_quarter as f32
    }
}

/// Options for exporting the active pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiExportOptions {
    /// SMF format 0 (one merged track) or format 1 (conductor + note track).
    pub format: u16,
    pub ppq: u16,
    /// Required stable project-channel to SMF-channel mapping.
    pub channel_map: MidiExportChannelMap,
    pub track_name: String,
    /// MIDI has no standard muted-note flag. Muted notes are omitted unless
    /// this is explicitly enabled.
    pub include_muted: bool,
}

impl Default for MidiExportOptions {
    fn default() -> Self {
        Self {
            format: 1,
            ppq: DEFAULT_PPQ,
            channel_map: MidiExportChannelMap::default(),
            track_name: "Citrus Studio Pattern".into(),
            include_muted: false,
        }
    }
}

/// Summary returned after importing a file into the active pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiImportSummary {
    pub format: u16,
    pub ppq: u16,
    pub track_count: usize,
    pub note_count: usize,
    pub tempo_bpm: f32,
}

/// Parse an SMF format 0 or format 1 byte stream using PPQ timing.
pub fn parse_smf(bytes: &[u8]) -> Result<MidiFile> {
    let mut input = Reader::new(bytes);
    ensure!(
        input.read_exact(4, "MIDI header signature")? == b"MThd",
        "Not a Standard MIDI File: missing MThd header"
    );

    let header_length = input.read_u32("MIDI header length")? as usize;
    ensure!(
        header_length >= 6,
        "Invalid MIDI header length {header_length}"
    );
    let header = input.read_exact(header_length, "MIDI header")?;
    let mut header = Reader::new(header);
    let format = header.read_u16("MIDI format")?;
    ensure!(
        matches!(format, 0 | 1),
        "Unsupported MIDI format {format}; only format 0 and 1 are supported"
    );
    let track_count = usize::from(header.read_u16("MIDI track count")?);
    ensure!(track_count > 0, "MIDI file contains no tracks");
    ensure!(
        track_count <= MAX_SMF_TRACKS,
        "MIDI file declares too many tracks ({track_count})"
    );
    ensure!(
        format != 0 || track_count == 1,
        "MIDI format 0 must contain exactly one track"
    );

    let division = header.read_u16("MIDI time division")?;
    ensure!(
        division & 0x8000 == 0,
        "SMPTE MIDI timing is not supported; a PPQ division is required"
    );
    ensure!(division > 0, "MIDI PPQ division cannot be zero");

    let mut tracks = Vec::with_capacity(track_count);
    let mut tempo_events = Vec::new();
    for track_index in 0..track_count {
        let signature = input
            .read_exact(4, "MIDI track signature")
            .with_context(|| format!("Unable to read MIDI track {}", track_index + 1))?;
        ensure!(
            signature == b"MTrk",
            "Invalid MIDI track {}: missing MTrk signature",
            track_index + 1
        );
        let length = input
            .read_u32("MIDI track length")
            .with_context(|| format!("Unable to read MIDI track {}", track_index + 1))?
            as usize;
        let data = input
            .read_exact(length, "MIDI track data")
            .with_context(|| format!("MIDI track {} is truncated", track_index + 1))?;
        tracks.push(
            parse_track(data, &mut tempo_events)
                .with_context(|| format!("Invalid MIDI track {}", track_index + 1))?,
        );
    }

    tempo_events.sort_by_key(|tempo| tempo.tick);
    Ok(MidiFile {
        format,
        ppq: division,
        tracks,
        tempo_events,
    })
}

/// Load and parse a Standard MIDI File from disk.
pub fn read_smf(path: &Path) -> Result<MidiFile> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Unable to read MIDI file {}", path.display()))?;
    parse_smf(&bytes).with_context(|| format!("Unable to import {}", path.display()))
}

/// Compatibility entry point. Note-bearing files require explicit routing and
/// are rejected instead of being assigned to a UI-selected or last channel.
pub fn import_into_active_pattern(
    project: &mut Project,
    bytes: &[u8],
) -> Result<MidiImportSummary> {
    let midi = parse_smf(bytes)?;
    let used_channels = used_midi_channels(&midi);
    ensure!(
        used_channels.is_empty(),
        "MIDI import uses channel(s) {}; supply an explicit MidiImportChannelMap",
        format_midi_channels(&used_channels)
    );
    import_decoded_into_active_pattern(project, midi, &MidiImportChannelMap::default())
}

/// Replace the active pattern while explicitly routing every used SMF channel
/// to a stable project Channel Rack id.
pub fn import_into_active_pattern_with_routing(
    project: &mut Project,
    bytes: &[u8],
    routing: &MidiImportChannelMap,
) -> Result<MidiImportSummary> {
    let midi = parse_smf(bytes)?;
    import_decoded_into_active_pattern(project, midi, routing)
}

fn import_decoded_into_active_pattern(
    project: &mut Project,
    midi: MidiFile,
    routing: &MidiImportChannelMap,
) -> Result<MidiImportSummary> {
    ensure!(
        !project.patterns.is_empty(),
        "the project has no active pattern for MIDI import"
    );
    let notes = routed_project_notes(project, &midi, routing)?;
    let tempo_bpm = midi.initial_tempo_bpm();
    let note_count = notes.len();

    project.tempo = tempo_bpm;
    project.active_pattern_mut().notes = notes;
    project.piano_notes.clear();

    Ok(MidiImportSummary {
        format: midi.format,
        ppq: midi.ppq,
        track_count: midi.tracks.len(),
        note_count,
        tempo_bpm,
    })
}

/// Load a file and replace the active pattern's piano-roll notes.
pub fn import_file_into_active_pattern(
    project: &mut Project,
    path: &Path,
) -> Result<MidiImportSummary> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Unable to read MIDI file {}", path.display()))?;
    import_into_active_pattern(project, &bytes)
        .with_context(|| format!("Unable to import {}", path.display()))
}

/// File-based form of [`import_into_active_pattern_with_routing`].
pub fn import_file_into_active_pattern_with_routing(
    project: &mut Project,
    path: &Path,
    routing: &MidiImportChannelMap,
) -> Result<MidiImportSummary> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Unable to read MIDI file {}", path.display()))?;
    import_into_active_pattern_with_routing(project, &bytes, routing)
        .with_context(|| format!("Unable to import {}", path.display()))
}

fn routed_project_notes(
    project: &Project,
    midi: &MidiFile,
    routing: &MidiImportChannelMap,
) -> Result<Vec<PianoNote>> {
    let channel_counts =
        project
            .channels
            .iter()
            .fold(BTreeMap::<u32, usize>::new(), |mut counts, channel| {
                *counts.entry(channel.id).or_default() += 1;
                counts
            });
    for midi_channel in used_midi_channels(midi) {
        let project_channel_id = routing.project_channel_id(midi_channel).ok_or_else(|| {
            anyhow!(
                "MIDI channel {} has no explicit project-channel route",
                midi_channel + 1
            )
        })?;
        ensure!(
            channel_counts.get(&project_channel_id).copied() == Some(1),
            "MIDI channel {} routes to missing or duplicate project channel {}",
            midi_channel + 1,
            project_channel_id
        );
    }

    let mut source_notes = midi
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(track_index, track)| {
            track
                .notes
                .iter()
                .copied()
                .enumerate()
                .map(move |(note_index, note)| (track_index, note_index, note))
        })
        .collect::<Vec<_>>();
    source_notes.sort_by(|left, right| {
        left.2
            .start_ticks
            .cmp(&right.2.start_ticks)
            .then_with(|| left.2.channel.cmp(&right.2.channel))
            .then_with(|| left.2.note.cmp(&right.2.note))
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.1.cmp(&right.1))
    });

    let mut used_note_ids = project
        .patterns
        .iter()
        .flat_map(|pattern| pattern.notes.iter())
        .filter_map(|note| (note.id != 0).then_some(note.id))
        .collect::<BTreeSet<_>>();
    let mut candidate = project
        .next_note_id()
        .ok_or_else(|| anyhow!("the project has no available Piano Roll note ids"))?;
    let ppq = f64::from(midi.ppq);
    let mut notes = Vec::with_capacity(source_notes.len());
    for (_, _, source) in source_notes {
        while candidate == 0 || used_note_ids.contains(&candidate) {
            candidate = candidate
                .checked_add(1)
                .ok_or_else(|| anyhow!("the project has no available Piano Roll note ids"))?;
        }
        let id = candidate;
        used_note_ids.insert(id);
        candidate = candidate.wrapping_add(1);
        notes.push(PianoNote {
            id,
            channel_id: Some(
                routing
                    .project_channel_id(source.channel)
                    .expect("used MIDI channels were validated above"),
            ),
            group_id: None,
            note: source.note,
            start: (source.start_ticks as f64 / ppq) as f32,
            length: (source.length_ticks as f64 / ppq) as f32,
            velocity: f32::from(source.velocity) / 127.0,
            selected: false,
            muted: false,
        });
    }
    Ok(notes)
}

fn used_midi_channels(midi: &MidiFile) -> BTreeSet<u8> {
    midi.tracks
        .iter()
        .flat_map(|track| track.notes.iter().map(|note| note.channel))
        .collect()
}

fn format_midi_channels(channels: &BTreeSet<u8>) -> String {
    channels
        .iter()
        .map(|channel| channel.saturating_add(1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Encode the active pattern as an SMF format 0 or format 1 file.
pub fn export_active_pattern(project: &Project, options: &MidiExportOptions) -> Result<Vec<u8>> {
    validate_export_options(options)?;
    let tempo = tempo_to_microseconds(project.tempo)?;
    let note_events = build_note_events(project, options)?;

    let mut output = Vec::new();
    output.extend_from_slice(b"MThd");
    output.extend_from_slice(&6_u32.to_be_bytes());
    output.extend_from_slice(&options.format.to_be_bytes());
    let track_count = if options.format == 0 { 1_u16 } else { 2_u16 };
    output.extend_from_slice(&track_count.to_be_bytes());
    output.extend_from_slice(&options.ppq.to_be_bytes());

    if options.format == 0 {
        let mut events = note_events;
        events.push(TimedEvent::new(0, 0, tempo_event(tempo)));
        events.push(TimedEvent::new(
            0,
            1,
            track_name_event(&options.track_name)?,
        ));
        append_track(&mut output, events)?;
    } else {
        let conductor_name = format!("{} — Conductor", options.track_name);
        append_track(
            &mut output,
            vec![
                TimedEvent::new(0, 0, tempo_event(tempo)),
                TimedEvent::new(0, 1, track_name_event(&conductor_name)?),
            ],
        )?;

        let mut events = note_events;
        events.push(TimedEvent::new(
            0,
            0,
            track_name_event(&options.track_name)?,
        ));
        append_track(&mut output, events)?;
    }

    Ok(output)
}

/// Encode and atomically write the active pattern to disk.
pub fn write_active_pattern(
    path: &Path,
    project: &Project,
    options: &MidiExportOptions,
) -> Result<()> {
    let bytes = export_active_pattern(project, options)?;
    ensure!(
        path.file_name().is_some(),
        "MIDI export path does not name a file: {}",
        path.display()
    );
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory)
        .with_context(|| format!("Unable to create {}", directory.display()))?;
    let (temporary, mut staged) = create_midi_export_temp(directory)
        .with_context(|| format!("Unable to stage MIDI file {}", path.display()))?;
    let mut cleanup = MidiExportTemp::new(temporary.clone());
    staged
        .write_all(&bytes)
        .with_context(|| format!("Unable to write staged MIDI file {}", temporary.display()))?;
    staged
        .flush()
        .with_context(|| format!("Unable to flush staged MIDI file {}", temporary.display()))?;
    staged
        .sync_all()
        .with_context(|| format!("Unable to sync staged MIDI file {}", temporary.display()))?;
    drop(staged);
    commit_midi_export(&temporary, path).with_context(|| {
        format!(
            "Unable to atomically commit MIDI file {} to {}",
            temporary.display(),
            path.display()
        )
    })?;
    cleanup.disarm();
    Ok(())
}

fn create_midi_export_temp(directory: &Path) -> io::Result<(PathBuf, File)> {
    let process_id = std::process::id();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..MIDI_EXPORT_TEMP_ATTEMPTS {
        let sequence = MIDI_EXPORT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name =
            format!("{MIDI_EXPORT_TEMP_PREFIX}{process_id}-{timestamp:032x}-{sequence:016x}.tmp");
        let temporary = directory.join(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "could not reserve a unique MIDI staging file after {MIDI_EXPORT_TEMP_ATTEMPTS} attempts"
        ),
    ))
}

#[cfg(windows)]
fn commit_midi_export(staged: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut value = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains an embedded NUL",
            ));
        }
        value.push(0);
        Ok(value)
    }

    let staged = wide(staged)?;
    let target = wide(target)?;
    // SAFETY: both buffers are NUL terminated, remain alive for the call, and
    // the staged file handle was closed before this function was entered.
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
fn commit_midi_export(staged: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(staged, target)
}

fn parse_track(data: &[u8], tempo_events: &mut Vec<MidiTempo>) -> Result<MidiTrack> {
    let mut input = Reader::new(data);
    let mut absolute_tick = 0_u64;
    let mut running_status = None;
    let mut active_notes: HashMap<(u8, u8), VecDeque<(u64, u8)>> = HashMap::new();
    let mut notes = Vec::new();
    let mut name = None;

    while !input.is_empty() {
        let event_offset = input.position();
        let delta = input
            .read_vlq("MIDI event delta")
            .with_context(|| format!("Invalid event at byte {event_offset}"))?;
        absolute_tick = absolute_tick
            .checked_add(u64::from(delta))
            .ok_or_else(|| anyhow!("MIDI event time overflow at byte {event_offset}"))?;

        let lead = input
            .read_u8("MIDI event status")
            .with_context(|| format!("Missing event at byte {event_offset}"))?;
        let (status, first_data) = if lead < 0x80 {
            let status = running_status.ok_or_else(|| {
                anyhow!(
                    "Running-status data without a previous channel status at byte {event_offset}"
                )
            })?;
            (status, Some(lead))
        } else {
            (lead, None)
        };

        match status {
            0x80..=0xef => {
                running_status = Some(status);
                let kind = status & 0xf0;
                let channel = status & 0x0f;
                let data_length = match kind {
                    0xc0 | 0xd0 => 1,
                    _ => 2,
                };
                let mut event_data = [0_u8; 2];
                if let Some(first) = first_data {
                    event_data[0] = first;
                    for slot in event_data.iter_mut().take(data_length).skip(1) {
                        *slot = input.read_data_byte("running-status event data")?;
                    }
                } else {
                    for slot in event_data.iter_mut().take(data_length) {
                        *slot = input.read_data_byte("MIDI channel event data")?;
                    }
                }

                match kind {
                    0x80 => finish_note(
                        &mut active_notes,
                        &mut notes,
                        channel,
                        event_data[0],
                        absolute_tick,
                    ),
                    0x90 if event_data[1] == 0 => finish_note(
                        &mut active_notes,
                        &mut notes,
                        channel,
                        event_data[0],
                        absolute_tick,
                    ),
                    0x90 => {
                        active_notes
                            .entry((channel, event_data[0]))
                            .or_default()
                            .push_back((absolute_tick, event_data[1]));
                    }
                    _ => {
                        // Poly pressure, control changes, program changes,
                        // channel pressure and pitch bend are valid but are not
                        // yet represented by the Citrus Studio project model.
                    }
                }
            }
            0xff => {
                running_status = None;
                ensure!(
                    first_data.is_none(),
                    "Internal parser error while reading a meta event"
                );
                let meta_type = input.read_u8("MIDI meta-event type")?;
                let length = input.read_vlq("MIDI meta-event length")? as usize;
                let payload = input.read_exact(length, "MIDI meta-event payload")?;
                match meta_type {
                    0x03 => name = Some(String::from_utf8_lossy(payload).into_owned()),
                    0x2f => break,
                    0x51 if payload.len() == 3 => {
                        let microseconds_per_quarter =
                            u32::from_be_bytes([0, payload[0], payload[1], payload[2]]);
                        ensure!(
                            microseconds_per_quarter > 0,
                            "Tempo meta event contains a zero duration"
                        );
                        tempo_events.push(MidiTempo {
                            tick: absolute_tick,
                            microseconds_per_quarter,
                        });
                    }
                    _ => {
                        // Unknown and currently unused meta events are length
                        // delimited, so they can be skipped without guessing.
                    }
                }
            }
            0xf0 | 0xf7 => {
                running_status = None;
                let length = input.read_vlq("MIDI SysEx length")? as usize;
                input.read_exact(length, "MIDI SysEx payload")?;
            }
            0xf1 | 0xf3 => {
                running_status = None;
                input.read_data_byte("MIDI system-common event")?;
            }
            0xf2 => {
                running_status = None;
                input.read_data_byte("MIDI song-position event")?;
                input.read_data_byte("MIDI song-position event")?;
            }
            0xf6 | 0xf8..=0xfe => {
                running_status = None;
                // Tune request, real-time and undefined system status bytes do
                // not carry data in an SMF track. They are ignored safely.
            }
            _ => bail!("Unsupported MIDI status 0x{status:02X} at byte {event_offset}"),
        }
    }

    // A surprising number of otherwise usable MIDI files omit note-off events
    // at the end of a track. Preserve those notes by closing them at the final
    // event tick, with a minimum duration of one tick.
    for ((channel, note), starts) in active_notes {
        for (start, velocity) in starts {
            notes.push(MidiNote {
                channel,
                note,
                start_ticks: start,
                length_ticks: absolute_tick.saturating_sub(start).max(1),
                velocity,
            });
        }
    }
    notes.sort_by_key(|note| (note.start_ticks, note.note, note.channel));

    Ok(MidiTrack { name, notes })
}

fn finish_note(
    active_notes: &mut HashMap<(u8, u8), VecDeque<(u64, u8)>>,
    notes: &mut Vec<MidiNote>,
    channel: u8,
    note: u8,
    end_tick: u64,
) {
    let key = (channel, note);
    let mut remove_entry = false;
    if let Some(starts) = active_notes.get_mut(&key) {
        if let Some((start_tick, velocity)) = starts.pop_front() {
            notes.push(MidiNote {
                channel,
                note,
                start_ticks: start_tick,
                length_ticks: end_tick.saturating_sub(start_tick).max(1),
                velocity,
            });
        }
        remove_entry = starts.is_empty();
    }
    if remove_entry {
        active_notes.remove(&key);
    }
}

fn validate_export_options(options: &MidiExportOptions) -> Result<()> {
    ensure!(
        matches!(options.format, 0 | 1),
        "MIDI export format must be 0 or 1"
    );
    ensure!(options.ppq > 0, "MIDI export PPQ cannot be zero");
    ensure!(options.ppq & 0x8000 == 0, "MIDI export PPQ is too large");
    ensure!(
        options.track_name.len() <= 0x0fff_ffff,
        "MIDI track name is too long"
    );
    Ok(())
}

fn tempo_to_microseconds(tempo_bpm: f32) -> Result<u32> {
    ensure!(
        tempo_bpm.is_finite() && tempo_bpm > 0.0,
        "Project tempo must be a positive finite BPM value"
    );
    let microseconds = (60_000_000.0_f64 / f64::from(tempo_bpm)).round();
    ensure!(
        (1.0..=16_777_215.0).contains(&microseconds),
        "Project tempo {tempo_bpm} BPM cannot be represented by a MIDI tempo event"
    );
    Ok(microseconds as u32)
}

fn build_note_events(project: &Project, options: &MidiExportOptions) -> Result<Vec<TimedEvent>> {
    let mut events = Vec::new();
    let channel_counts =
        project
            .channels
            .iter()
            .fold(BTreeMap::<u32, usize>::new(), |mut counts, channel| {
                *counts.entry(channel.id).or_default() += 1;
                counts
            });
    for note in &project.active_pattern().notes {
        if note.muted && !options.include_muted {
            continue;
        }
        ensure!(
            note.note < 128,
            "Piano-roll note {} is not a valid MIDI key",
            note.note
        );
        ensure!(
            note.start.is_finite() && note.start >= 0.0,
            "Piano-roll note {} has an invalid start time",
            note.note
        );
        ensure!(
            note.length.is_finite() && note.length > 0.0,
            "Piano-roll note {} has an invalid length",
            note.note
        );
        ensure!(
            note.velocity.is_finite(),
            "Piano-roll note {} has an invalid velocity",
            note.note
        );
        let project_channel_id = note.channel_id.ok_or_else(|| {
            anyhow!(
                "Piano-roll note {} (id {}) has no project channel assignment",
                note.note,
                note.id
            )
        })?;
        ensure!(
            channel_counts.get(&project_channel_id).copied() == Some(1),
            "Piano-roll note {} (id {}) references missing or duplicate project channel {}",
            note.note,
            note.id,
            project_channel_id
        );
        let midi_channel = options
            .channel_map
            .midi_channel(project_channel_id)
            .ok_or_else(|| {
                anyhow!(
                    "project channel {} used by Piano-roll note id {} has no MIDI export route",
                    project_channel_id,
                    note.id
                )
            })?;

        let start = beats_to_ticks(note.start, options.ppq)?;
        let raw_length = beats_to_ticks(note.length, options.ppq)?;
        let end = start
            .checked_add(raw_length.max(1))
            .ok_or_else(|| anyhow!("Piano-roll note {} exceeds the MIDI time range", note.note))?;
        let velocity = ((note.velocity.clamp(0.0, 1.0) * 127.0).round() as u8).max(1);
        let status = 0x90 | midi_channel;
        let off_status = 0x80 | midi_channel;

        // Note-offs sort before note-ons at the same tick. This prevents a
        // retriggered key from being immediately silenced by the previous note.
        events.push(TimedEvent::new(end, 1, vec![off_status, note.note, 0]));
        events.push(TimedEvent::new(start, 2, vec![status, note.note, velocity]));
    }
    Ok(events)
}

fn beats_to_ticks(beats: f32, ppq: u16) -> Result<u64> {
    let ticks = f64::from(beats) * f64::from(ppq);
    ensure!(
        ticks.is_finite() && ticks >= 0.0 && ticks <= u64::MAX as f64,
        "Musical time is outside the MIDI tick range"
    );
    Ok(ticks.round() as u64)
}

#[derive(Clone, Debug)]
struct TimedEvent {
    tick: u64,
    priority: u8,
    data: Vec<u8>,
}

impl TimedEvent {
    fn new(tick: u64, priority: u8, data: Vec<u8>) -> Self {
        Self {
            tick,
            priority,
            data,
        }
    }
}

fn append_track(output: &mut Vec<u8>, mut events: Vec<TimedEvent>) -> Result<()> {
    events.sort_by(|left, right| {
        left.tick
            .cmp(&right.tick)
            .then_with(|| left.priority.cmp(&right.priority))
    });

    let mut track = Vec::new();
    let mut previous_tick = 0_u64;
    for event in events {
        let delta = event
            .tick
            .checked_sub(previous_tick)
            .ok_or_else(|| anyhow!("MIDI events are not in chronological order"))?;
        write_vlq(&mut track, delta)?;
        track.extend_from_slice(&event.data);
        previous_tick = event.tick;
    }
    write_vlq(&mut track, 0)?;
    track.extend_from_slice(&[0xff, 0x2f, 0x00]);

    let length = u32::try_from(track.len()).context("MIDI track is larger than 4 GiB")?;
    output.extend_from_slice(b"MTrk");
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&track);
    Ok(())
}

fn tempo_event(microseconds_per_quarter: u32) -> Vec<u8> {
    let bytes = microseconds_per_quarter.to_be_bytes();
    vec![0xff, 0x51, 0x03, bytes[1], bytes[2], bytes[3]]
}

fn track_name_event(name: &str) -> Result<Vec<u8>> {
    let mut event = vec![0xff, 0x03];
    write_vlq(&mut event, name.len() as u64)?;
    event.extend_from_slice(name.as_bytes());
    Ok(event)
}

fn write_vlq(output: &mut Vec<u8>, value: u64) -> Result<()> {
    ensure!(value <= MAX_VLQ, "MIDI VLQ value {value} exceeds 28 bits");
    let mut buffer = [0_u8; 4];
    let mut index = buffer.len() - 1;
    buffer[index] = (value & 0x7f) as u8;
    let mut remaining = value >> 7;
    while remaining > 0 {
        index -= 1;
        buffer[index] = ((remaining & 0x7f) as u8) | 0x80;
        remaining >>= 7;
    }
    output.extend_from_slice(&buffer[index..]);
    Ok(())
}

#[derive(Clone, Copy)]
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    const fn is_empty(self) -> bool {
        self.position >= self.bytes.len()
    }

    const fn position(self) -> usize {
        self.position
    }

    fn read_u8(&mut self, field: &str) -> Result<u8> {
        let byte = self
            .bytes
            .get(self.position)
            .copied()
            .ok_or_else(|| anyhow!("Unexpected end of file while reading {field}"))?;
        self.position += 1;
        Ok(byte)
    }

    fn read_data_byte(&mut self, field: &str) -> Result<u8> {
        let byte = self.read_u8(field)?;
        ensure!(
            byte < 0x80,
            "Expected a MIDI data byte while reading {field}, found status 0x{byte:02X}"
        );
        Ok(byte)
    }

    fn read_u16(&mut self, field: &str) -> Result<u16> {
        let bytes: [u8; 2] = self
            .read_exact(2, field)?
            .try_into()
            .expect("slice length was checked");
        Ok(u16::from_be_bytes(bytes))
    }

    fn read_u32(&mut self, field: &str) -> Result<u32> {
        let bytes: [u8; 4] = self
            .read_exact(4, field)?
            .try_into()
            .expect("slice length was checked");
        Ok(u32::from_be_bytes(bytes))
    }

    fn read_exact(&mut self, length: usize, field: &str) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| anyhow!("Length overflow while reading {field}"))?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| anyhow!("Unexpected end of file while reading {field}"))?;
        self.position = end;
        Ok(bytes)
    }

    fn read_vlq(&mut self, field: &str) -> Result<u32> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let byte = self.read_u8(field)?;
            value = (value << 7) | u32::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        bail!("MIDI VLQ is longer than four bytes while reading {field}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smf(format: u16, ppq: u16, tracks: &[&[u8]]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(b"MThd");
        output.extend_from_slice(&6_u32.to_be_bytes());
        output.extend_from_slice(&format.to_be_bytes());
        output.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
        output.extend_from_slice(&ppq.to_be_bytes());
        for track in tracks {
            output.extend_from_slice(b"MTrk");
            output.extend_from_slice(&(track.len() as u32).to_be_bytes());
            output.extend_from_slice(track);
        }
        output
    }

    #[test]
    fn vlq_round_trips_boundaries() {
        for expected in [0, 0x7f, 0x80, 0x3fff, 0x4000, 0x1f_ffff, MAX_VLQ] {
            let mut encoded = Vec::new();
            write_vlq(&mut encoded, expected).unwrap();
            let decoded = Reader::new(&encoded).read_vlq("test VLQ").unwrap();
            assert_eq!(u64::from(decoded), expected);
        }
        assert!(write_vlq(&mut Vec::new(), MAX_VLQ + 1).is_err());
    }

    #[test]
    fn parses_running_status_notes_and_tempo() {
        let track = [
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // 120 BPM
            0x00, 0x90, 0x3c, 0x64, // C4 on at 0
            0x83, 0x60, 0x3e, 0x50, // E4 on at 480, running status
            0x00, 0x3c, 0x00, // C4 off via velocity zero
            0x83, 0x60, 0x80, 0x3e, 0x40, // E4 off at 960
            0x00, 0xff, 0x2f, 0x00,
        ];
        let midi = parse_smf(&smf(0, 480, &[&track])).unwrap();
        assert_eq!(midi.initial_tempo_bpm(), 120.0);
        assert_eq!(midi.note_count(), 2);
        assert_eq!(
            midi.tracks[0].notes[0],
            MidiNote {
                channel: 0,
                note: 60,
                start_ticks: 0,
                length_ticks: 480,
                velocity: 100,
            }
        );
        assert_eq!(midi.tracks[0].notes[1].start_ticks, 480);
        assert_eq!(midi.tracks[0].notes[1].length_ticks, 480);
    }

    #[test]
    fn safely_skips_unknown_events() {
        let track = [
            0x00, 0xff, 0x7f, 0x03, 1, 2, 3, // sequencer-specific meta
            0x00, 0xf0, 0x02, 0x7d, 0xf7, // length-delimited SysEx
            0x00, 0xb2, 0x07, 0x64, // controller event
            0x00, 0x92, 60, 100, // note on
            0x78, 0x82, 60, 0, // note off
            0x00, 0xff, 0x2f, 0x00,
        ];
        let midi = parse_smf(&smf(0, 120, &[&track])).unwrap();
        assert_eq!(midi.note_count(), 1);
        assert_eq!(midi.tracks[0].notes[0].channel, 2);
        assert_eq!(midi.tracks[0].notes[0].length_ticks, 120);
    }

    #[test]
    fn active_pattern_exports_format_zero_and_one() {
        let mut project = Project {
            tempo: 137.0,
            ..Project::default()
        };
        project.active_pattern_mut().notes = vec![PianoNote {
            id: 900,
            channel_id: Some(1),
            group_id: None,
            note: 64,
            start: 1.25,
            length: 0.75,
            velocity: 0.5,
            selected: true,
            muted: false,
        }];

        for (format, expected_tracks) in [(0, 1), (1, 2)] {
            let bytes = export_active_pattern(
                &project,
                &MidiExportOptions {
                    format,
                    channel_map: MidiExportChannelMap::try_new([(1, 2)]).unwrap(),
                    ..MidiExportOptions::default()
                },
            )
            .unwrap();
            let decoded = parse_smf(&bytes).unwrap();
            assert_eq!(decoded.format, format);
            assert_eq!(decoded.tracks.len(), expected_tracks);
            assert!((decoded.initial_tempo_bpm() - 137.0).abs() < 0.01);
            assert_eq!(decoded.note_count(), 1);
            let note = decoded
                .tracks
                .iter()
                .find_map(|track| track.notes.first())
                .unwrap();
            assert_eq!(note.note, 64);
            assert_eq!(note.start_ticks, 600);
            assert_eq!(note.length_ticks, 360);
            assert_eq!(note.velocity, 64);
            assert_eq!(note.channel, 2);
        }
    }

    #[test]
    fn import_replaces_active_pattern_and_tempo() {
        let track = [
            0x00, 0xff, 0x51, 0x03, 0x06, 0x1a, 0x80, // 150 BPM
            0x00, 0x90, 72, 127, 0x81, 0x70, 0x80, 72, 0, 0x00, 0xff, 0x2f, 0,
        ];
        let mut project = Project::default();
        let bytes = smf(0, 480, &[&track]);
        let error = import_into_active_pattern(&mut project, &bytes).unwrap_err();
        assert!(error.to_string().contains("explicit MidiImportChannelMap"));
        let destination = project.channels[2].id;
        let routing = MidiImportChannelMap::try_new([(0, destination)]).unwrap();
        let summary =
            import_into_active_pattern_with_routing(&mut project, &bytes, &routing).unwrap();
        assert_eq!(summary.note_count, 1);
        assert!((project.tempo - 150.0).abs() < f32::EPSILON);
        assert_eq!(project.active_pattern().notes[0].note, 72);
        assert!((project.active_pattern().notes[0].length - 0.5).abs() < f32::EPSILON);
        assert_eq!(
            project.active_pattern().notes[0].channel_id,
            Some(destination)
        );
        assert_ne!(project.active_pattern().notes[0].id, 0);
        assert!(project.piano_notes.is_empty());
    }

    #[test]
    fn multichannel_import_requires_complete_valid_routing_and_stable_ids() {
        let track = [
            0x00, 0x91, 60, 100, 0x78, 0x81, 60, 0, // MIDI channel 2
            0x00, 0x94, 67, 90, 0x78, 0x84, 67, 0, // MIDI channel 5
            0x00, 0xff, 0x2f, 0x00,
        ];
        let bytes = smf(0, 120, &[&track]);
        let mut project = Project::default();
        let first_destination = project.channels[0].id;
        let second_destination = project.channels[2].id;
        let incomplete = MidiImportChannelMap::try_new([(1, first_destination)]).unwrap();
        assert!(
            import_into_active_pattern_with_routing(&mut project, &bytes, &incomplete)
                .unwrap_err()
                .to_string()
                .contains("channel 5 has no explicit")
        );

        let routing =
            MidiImportChannelMap::try_new([(1, first_destination), (4, second_destination)])
                .unwrap();
        import_into_active_pattern_with_routing(&mut project, &bytes, &routing).unwrap();
        let notes = &project.active_pattern().notes;
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].channel_id, Some(first_destination));
        assert_eq!(notes[1].channel_id, Some(second_destination));
        assert_ne!(notes[0].id, notes[1].id);
        assert!(notes.iter().all(|note| note.id > 12));
    }

    #[test]
    fn multichannel_export_rejects_unassigned_or_unmapped_notes() {
        let mut project = Project::default();
        let first = project.channels[0].id;
        let second = project.channels[1].id;
        project.active_pattern_mut().notes = vec![
            PianoNote {
                id: 1_001,
                channel_id: Some(first),
                group_id: None,
                note: 60,
                start: 0.0,
                length: 1.0,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
            PianoNote {
                id: 1_002,
                channel_id: Some(second),
                group_id: None,
                note: 67,
                start: 1.0,
                length: 1.0,
                velocity: 0.8,
                selected: false,
                muted: false,
            },
        ];
        let incomplete = MidiExportOptions {
            channel_map: MidiExportChannelMap::try_new([(first, 1)]).unwrap(),
            ..MidiExportOptions::default()
        };
        assert!(
            export_active_pattern(&project, &incomplete)
                .unwrap_err()
                .to_string()
                .contains("has no MIDI export route")
        );

        let options = MidiExportOptions {
            channel_map: MidiExportChannelMap::try_new([(first, 1), (second, 4)]).unwrap(),
            ..MidiExportOptions::default()
        };
        let decoded = parse_smf(&export_active_pattern(&project, &options).unwrap()).unwrap();
        let channels = decoded
            .tracks
            .iter()
            .flat_map(|track| track.notes.iter().map(|note| note.channel))
            .collect::<BTreeSet<_>>();
        assert_eq!(channels, BTreeSet::from([1, 4]));

        project.active_pattern_mut().notes[0].channel_id = None;
        assert!(
            export_active_pattern(&project, &options)
                .unwrap_err()
                .to_string()
                .contains("has no project channel assignment")
        );
    }

    #[test]
    fn same_tick_note_off_sorts_before_retrigger_note_on() {
        let mut project = Project::default();
        let channel_id = project.channels[0].id;
        project.active_pattern_mut().notes = vec![
            PianoNote {
                id: 2_001,
                channel_id: Some(channel_id),
                group_id: None,
                note: 60,
                start: 0.0,
                length: 1.0,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
            PianoNote {
                id: 2_002,
                channel_id: Some(channel_id),
                group_id: None,
                note: 60,
                start: 1.0,
                length: 1.0,
                velocity: 1.0,
                selected: false,
                muted: false,
            },
        ];
        let options = MidiExportOptions {
            channel_map: MidiExportChannelMap::try_new([(channel_id, 3)]).unwrap(),
            ..MidiExportOptions::default()
        };
        let mut boundary = build_note_events(&project, &options)
            .unwrap()
            .into_iter()
            .filter(|event| event.tick == u64::from(options.ppq))
            .collect::<Vec<_>>();
        boundary.sort_by_key(|event| event.priority);
        assert_eq!(boundary.len(), 2);
        assert_eq!(boundary[0].data[0] & 0xf0, 0x80);
        assert_eq!(boundary[1].data[0] & 0xf0, 0x90);
    }

    #[test]
    fn midi_export_atomically_replaces_an_existing_target_and_cleans_staging() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "citrus-midi-atomic-{}-{unique:032x}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let target = directory.join("pattern.mid");
        std::fs::write(&target, b"old MIDI bytes").unwrap();

        let mut project = Project::default();
        let channel_id = project.channels[0].id;
        project.active_pattern_mut().notes = vec![PianoNote {
            id: 3_001,
            channel_id: Some(channel_id),
            group_id: None,
            note: 60,
            start: 0.0,
            length: 1.0,
            velocity: 1.0,
            selected: false,
            muted: false,
        }];
        let options = MidiExportOptions {
            channel_map: MidiExportChannelMap::try_new([(channel_id, 0)]).unwrap(),
            ..MidiExportOptions::default()
        };
        write_active_pattern(&target, &project, &options).unwrap();

        assert_eq!(&std::fs::read(&target).unwrap()[..4], b"MThd");
        let entries = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], target.file_name().unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_truncated_and_invalid_files() {
        assert!(parse_smf(b"not midi").is_err());

        let unterminated_vlq = [0x80, 0x80, 0x80, 0x80];
        assert!(parse_smf(&smf(0, 480, &[&unterminated_vlq])).is_err());

        let invalid_running_status = [0x00, 60, 100];
        assert!(parse_smf(&smf(0, 480, &[&invalid_running_status])).is_err());

        let smpte_division = smf(0, 0xe728, &[&[0x00, 0xff, 0x2f, 0x00]]);
        assert!(parse_smf(&smpte_division).is_err());
    }
}
