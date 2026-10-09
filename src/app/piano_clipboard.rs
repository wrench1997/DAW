//! Bounded, session-local Citrus note clipboard. No file paths, media or MIDI interchange.
use super::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

const PREFIX: &str = "CITRUS-NOTES/1\n";
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_NOTES: usize = crate::piano_roll::MAX_TRANSFORM_NOTES;
const MAX_BEAT: f64 = 4096.0;
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ClipboardOwner {
    process: u32,
    created_ns: u128,
    instance: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipboardNote {
    channel_id: Option<u32>,
    group_id: Option<u64>,
    pitch: u8,
    offset: f64,
    length: f32,
    velocity: f32,
    muted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotePayload {
    version: u32,
    owner: ClipboardOwner,
    project_session: u64,
    notes: Vec<ClipboardNote>,
}

pub(super) struct PianoClipboard {
    owner: ClipboardOwner,
    payload: Option<NotePayload>,
}

impl Default for PianoClipboard {
    fn default() -> Self {
        Self {
            // Compatibility identity only, not a cryptographic/security boundary.
            owner: ClipboardOwner {
                process: std::process::id(),
                created_ns: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            },
            payload: None,
        }
    }
}

impl PianoClipboard {
    pub(super) fn clear(&mut self) {
        self.payload = None;
    }

    fn decode(&self, text: &str, session: u64) -> Result<NotePayload, &'static str> {
        if text.len() > MAX_BYTES {
            return Err("Note clipboard exceeds the size limit");
        }
        let json = text
            .strip_prefix(PREFIX)
            .ok_or("Clipboard does not contain Citrus notes")?;
        let payload: NotePayload =
            serde_json::from_str(json).map_err(|_| "Invalid Citrus note clipboard")?;
        self.validate_identity(&payload, session)?;
        Ok(payload)
    }

    fn validate_identity(&self, payload: &NotePayload, session: u64) -> Result<(), &'static str> {
        if payload.version != 1 || payload.owner != self.owner || payload.project_session != session
        {
            return Err("Notes belong to another app or project session; copy them again");
        }
        if payload.notes.is_empty() || payload.notes.len() > MAX_NOTES {
            return Err("Invalid note clipboard count");
        }
        Ok(())
    }
}

fn clipboard_selection(
    notes: &[PianoNote],
    selected: &HashSet<u64>,
    grouping: bool,
) -> Result<HashSet<u64>, &'static str> {
    if selected.len() > MAX_NOTES {
        return Err("Copy exceeds the 65,536-note limit");
    }
    // A pair of linear scans avoids the general editor helper's per-selected-note scan.
    let groups: HashSet<_> = if grouping {
        notes
            .iter()
            .filter(|n| selected.contains(&n.id))
            .filter_map(|n| n.group_id.map(|g| (n.channel_id, g)))
            .collect()
    } else {
        HashSet::new()
    };
    let mut result = HashSet::new();
    for note in notes {
        if selected.contains(&note.id)
            || note
                .group_id
                .is_some_and(|g| groups.contains(&(note.channel_id, g)))
        {
            result.insert(note.id);
            if result.len() > MAX_NOTES {
                return Err("Copy exceeds the 65,536-note limit");
            }
        }
    }
    Ok(result)
}

fn channel_counts(project: &Project) -> HashMap<u32, usize> {
    let mut counts = HashMap::new();
    for channel in &project.channels {
        *counts.entry(channel.id).or_insert(0) += 1;
    }
    counts
}

fn validate_note(
    note: &ClipboardNote,
    channels: &HashMap<u32, usize>,
    anchor: f64,
) -> Result<(), &'static str> {
    let start = anchor + note.offset;
    let end = start + f64::from(note.length);
    if !note.offset.is_finite()
        || note.offset < 0.0
        || !start.is_finite()
        || !note.length.is_finite()
        || note.length < 0.01
        || !note.velocity.is_finite()
        || !(0.0..=1.0).contains(&note.velocity)
        || note.pitch > 127
        || !(0.0..MAX_BEAT).contains(&start)
        || end > MAX_BEAT
        || f64::from(start as f32) + f64::from(note.length) > MAX_BEAT
        || note.group_id == Some(0)
    {
        return Err("Notes would exceed valid pitch, velocity or 4096-beat bounds");
    }
    if note
        .channel_id
        .is_some_and(|id| id == 0 || channels.get(&id) != Some(&1))
    {
        return Err("A copied note channel is missing or ambiguous");
    }
    Ok(())
}

fn fresh_id(used: &mut HashSet<u64>, next: &mut u64) -> Result<u64, &'static str> {
    loop {
        *next = next
            .checked_add(1)
            .ok_or("No stable note identity remains")?;
        if used.insert(*next) {
            return Ok(*next);
        }
    }
}

fn paste_candidate(
    project: &Project,
    payload: &NotePayload,
    anchor: f64,
) -> Result<(Project, HashSet<u64>), &'static str> {
    let target = project
        .patterns
        .get(project.active_pattern)
        .ok_or("No active pattern")?;
    if payload.notes.is_empty()
        || target.notes.len().saturating_add(payload.notes.len()) > MAX_NOTES
    {
        return Err("Paste exceeds the 65,536-note pattern limit");
    }
    if !anchor.is_finite() || !(0.0..MAX_BEAT).contains(&anchor) {
        return Err("Invalid Piano paste position");
    }
    // Validate the complete input before building any candidate or assigning identities.
    let channels = channel_counts(project);
    for note in &payload.notes {
        validate_note(note, &channels, anchor)?;
    }
    let mut ids = HashSet::new();
    for note in project.patterns.iter().flat_map(|pattern| &pattern.notes) {
        if note.id == 0 || !ids.insert(note.id) {
            return Err("Project contains duplicate or invalid note identities");
        }
    }
    let mut group_ids: HashSet<u64> = target
        .notes
        .iter()
        .filter_map(|note| note.group_id)
        .collect();
    let mut groups = HashMap::new();
    let mut counts = HashMap::new();
    for note in &payload.notes {
        if let Some(group) = note.group_id {
            *counts.entry((note.channel_id, group)).or_insert(0_usize) += 1;
        }
    }
    let mut candidate = project.clone();
    let notes = &mut candidate.active_pattern_mut().notes;
    let mut selected = HashSet::new();
    let mut next_note = 0;
    let mut next_group = 0;
    for note in &payload.notes {
        let id = fresh_id(&mut ids, &mut next_note)?;
        let group_id = if let Some(group) = note
            .group_id
            .filter(|g| counts[&(note.channel_id, *g)] >= 2)
        {
            let key = (note.channel_id, group);
            if let Some(mapped) = groups.get(&key) {
                Some(*mapped)
            } else {
                let mapped = fresh_id(&mut group_ids, &mut next_group)?;
                groups.insert(key, mapped);
                Some(mapped)
            }
        } else {
            None
        };
        notes.push(PianoNote {
            id,
            channel_id: note.channel_id,
            group_id,
            note: note.pitch,
            start: (anchor + note.offset) as f32,
            length: note.length,
            velocity: note.velocity,
            selected: false,
            muted: note.muted,
        });
        selected.insert(id);
    }
    Ok((candidate, selected))
}

impl CitrusApp {
    pub(super) fn piano_clipboard_ready(&self, ctx: &egui::Context) -> bool {
        self.piano_editor_command_ready(ctx, false)
    }

    // Explicitly clicked menu commands own their popup. Raw keys and clipboard
    // actions cannot bypass it; every other ownership/snapshot barrier is shared.
    pub(super) fn piano_editor_command_ready(
        &self,
        ctx: &egui::Context,
        menu_command: bool,
    ) -> bool {
        self.workspace.focused == StudioView::PianoRoll
            && self.workspace.windows[workspace::index(StudioView::PianoRoll)].visible
            && !ctx.text_edit_focused()
            && (menu_command || !ctx.any_popup_open())
            && !self.editor_pointer_gesture_active()
            && !piano_range::active(ctx)
            && self.playlist_gesture_before.is_none()
            && self.piano_roll_gesture_before.is_none()
            && self.top_shortcut_modal().is_none()
            && !self.shortcut_blocking_layer_active()
            && !self.project_snapshot_transition_pending()
            && !self.project_lifecycle_barriers_active()
    }

    pub(super) fn piano_paste_anchor(&self) -> Result<f64, &'static str> {
        let left = self.piano_viewport.x.origin();
        if !left.is_finite() || !(0.0..MAX_BEAT).contains(&left) {
            return Err("Invalid Piano paste position");
        }
        Ok((left / 4.0).floor() * 4.0)
    }

    pub(super) fn piano_clipboard_action(&mut self, ctx: &egui::Context, action: ShortcutAction) {
        self.piano_clipboard_action_impl(ctx, action, false);
    }

    fn piano_clipboard_action_impl(
        &mut self,
        ctx: &egui::Context,
        action: ShortcutAction,
        menu_command: bool,
    ) {
        if !self.piano_editor_command_ready(ctx, menu_command)
            || ctx.input(|i| i.pointer.any_down())
        {
            return;
        }
        if action == ShortcutAction::SelectAllNotes {
            let channel = self
                .project
                .channels
                .get(self.selected_channel)
                .map(|c| c.id);
            self.piano_roll_state.selection_ids = self
                .project
                .active_pattern()
                .notes
                .iter()
                .filter(|n| n.channel_id == channel)
                .map(|n| n.id)
                .collect();
            return;
        }
        let result = match action {
            ShortcutAction::CopyNotes | ShortcutAction::CutNotes => {
                self.copy_piano_notes(ctx, action == ShortcutAction::CutNotes)
            }
            ShortcutAction::PasteNotes => {
                // A semantic OS paste must validate its own text; never fall back to stale local
                // notes when the user pastes unrelated text. Buttons/raw keys use the local copy.
                let payload = ctx.input(|i| {
                    match i.events.iter().find_map(|e| match e {
                        egui::Event::Paste(text) => Some(text.as_str()),
                        _ => None,
                    }) {
                        Some(text) => self.piano_clipboard.decode(text, self.project_session),
                        None => self
                            .piano_clipboard
                            .payload
                            .clone()
                            .ok_or("Copy Piano notes before pasting"),
                    }
                });
                payload.and_then(|payload| {
                    self.piano_clipboard
                        .validate_identity(&payload, self.project_session)?;
                    let (candidate, selected) =
                        paste_candidate(&self.project, &payload, self.piano_paste_anchor()?)?;
                    self.commit_editor_project(candidate);
                    self.piano_roll_state.selection_ids = selected;
                    Ok(())
                })
            }
            _ => return,
        };
        if let Err(message) = result {
            self.notify(message.into());
        }
    }

    fn copy_piano_notes(&mut self, ctx: &egui::Context, cut: bool) -> Result<(), &'static str> {
        let notes = &self.project.active_pattern().notes;
        let selected = clipboard_selection(
            notes,
            &self.piano_roll_state.selection_ids,
            self.piano_roll_state.grouping_enabled,
        )?;
        let selected_notes: Vec<_> = notes
            .iter()
            .filter(|n| selected.contains(&n.id))
            .take(MAX_NOTES + 1)
            .collect();
        if selected_notes.is_empty() {
            return Ok(());
        } // Preserve both clipboards and history.
        if selected_notes.len() > MAX_NOTES {
            return Err("Copy exceeds the 65,536-note limit");
        }
        let start = selected_notes
            .iter()
            .map(|n| f64::from(n.start))
            .reduce(f64::min)
            .unwrap();
        if !start.is_finite() || start < 0.0 {
            return Err("Invalid source note time");
        }
        let channels = channel_counts(&self.project);
        let mut copied = Vec::with_capacity(selected_notes.len());
        let mut seen = HashSet::new();
        for note in selected_notes {
            if note.id == 0 || !seen.insert(note.id) || !note.start.is_finite() {
                return Err("Invalid source note identity or time");
            }
            let copy = ClipboardNote {
                channel_id: note.channel_id,
                group_id: note.group_id,
                pitch: note.note,
                offset: f64::from(note.start) - start,
                length: note.length,
                velocity: note.velocity,
                muted: note.muted,
            };
            validate_note(&copy, &channels, start)?;
            copied.push(copy);
        }
        let payload = NotePayload {
            version: 1,
            owner: self.piano_clipboard.owner.clone(),
            project_session: self.project_session,
            notes: copied,
        };
        let text = format!(
            "{PREFIX}{}",
            serde_json::to_string(&payload).map_err(|_| "Cannot encode note clipboard")?
        );
        if text.len() > MAX_BYTES {
            return Err("Note clipboard exceeds the size limit");
        }
        if cut {
            let mut candidate = self.project.clone();
            candidate
                .active_pattern_mut()
                .notes
                .retain(|n| !selected.contains(&n.id));
            normalize_piano_note_groups(&mut candidate.active_pattern_mut().notes);
            self.commit_editor_project(candidate);
            self.piano_roll_state.clear_selection();
        }
        self.piano_clipboard.payload = Some(payload);
        ctx.copy_text(text);
        Ok(())
    }

    pub(super) fn piano_clipboard_toolbar(&mut self, ui: &mut egui::Ui, menu_command: bool) {
        let ready = self.piano_editor_command_ready(ui.ctx(), menu_command);
        let selected = !self.piano_roll_state.selection_ids.is_empty();
        let copied = self.piano_clipboard.payload.is_some();
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            for (label, command, enabled) in [
                ("Select all notes", ShortcutAction::SelectAllNotes, true),
                ("Copy notes", ShortcutAction::CopyNotes, selected),
                ("Cut notes", ShortcutAction::CutNotes, selected),
                ("Paste notes", ShortcutAction::PasteNotes, copied),
            ] {
                if ui.add_enabled(ready && enabled, egui::Button::new(label).small()).clicked() { action = Some(command); }
            }
            let anchor = self.piano_paste_anchor().map(|beat| format!("Paste beat {beat:.2}")).unwrap_or_else(|_| "Paste position invalid".into());
            ui.label(RichText::new(anchor).size(9.0).color(theme::MUTED)).on_hover_text(
                "Session-local Citrus notes. Both transport modes: start of the bar containing the left edge of the Piano grid. Range and playhead do not change this anchor. Repeated paste uses the same anchor. Original channels and pitches are preserved; TARGET does not remap them. Copy/Cut writes Citrus text to the OS clipboard; Paste notes uses the last local copy. Not MIDI interchange.");
        });
        if let Some(action) = action {
            if menu_command {
                ui.close();
            }
            self.piano_clipboard_action_impl(ui.ctx(), action, menu_command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Project, PianoClipboard, NotePayload) {
        let project = Project::default();
        let clipboard = PianoClipboard::default();
        let payload = NotePayload {
            version: 1,
            owner: clipboard.owner.clone(),
            project_session: 1,
            notes: vec![
                ClipboardNote {
                    channel_id: Some(project.channels[0].id),
                    group_id: Some(7),
                    pitch: 60,
                    offset: 0.0,
                    length: 0.75,
                    velocity: 0.625,
                    muted: true,
                },
                ClipboardNote {
                    channel_id: Some(project.channels[0].id),
                    group_id: Some(7),
                    pitch: 64,
                    offset: 0.375,
                    length: 0.25,
                    velocity: 0.25,
                    muted: false,
                },
            ],
        };
        (project, clipboard, payload)
    }

    fn encoded(payload: &NotePayload) -> String {
        format!("{PREFIX}{}", serde_json::to_string(payload).unwrap())
    }

    #[test]
    fn selection_expansion_is_bounded_channel_qualified_and_handles_the_full_limit() {
        let (project, _, _) = fixture();
        let source = project.patterns[0].notes[0].clone();
        let notes: Vec<_> = (1..=MAX_NOTES)
            .map(|id| PianoNote {
                id: id as u64,
                group_id: Some(1),
                ..source.clone()
            })
            .collect();
        let all: HashSet<_> = notes.iter().map(|n| n.id).collect();
        assert_eq!(clipboard_selection(&notes, &all, true).unwrap(), all);
        assert_eq!(clipboard_selection(&notes, &all, false).unwrap(), all);
        assert_eq!(
            clipboard_selection(&notes, &HashSet::from([1]), true).unwrap(),
            all
        );
        assert_eq!(
            clipboard_selection(&notes, &HashSet::from([1]), false).unwrap(),
            HashSet::from([1])
        );
        let mut overflow = notes;
        overflow.push(PianoNote {
            id: MAX_NOTES as u64 + 1,
            group_id: Some(1),
            ..source.clone()
        });
        assert!(clipboard_selection(&overflow, &HashSet::from([1]), true).is_err());
        overflow.last_mut().unwrap().channel_id = None;
        assert_eq!(
            clipboard_selection(&overflow, &HashSet::from([1]), true).unwrap(),
            all
        );
    }

    #[test]
    fn payload_roundtrip_is_bounded_versioned_and_session_local() {
        let (_, clipboard, mut payload) = fixture();
        let text = encoded(&payload);
        assert_eq!(clipboard.decode(&text, 1).unwrap().notes.len(), 2);
        for text in [
            String::new(),
            "plain text".into(),
            format!("{PREFIX}{{}}"),
            format!("{PREFIX}{{\"notes\":["),
            "x".repeat(MAX_BYTES + 1),
        ] {
            assert!(clipboard.decode(&text, 1).is_err());
        }
        assert!(clipboard.decode(&encoded(&payload), 2).is_err());
        assert!(
            PianoClipboard::default()
                .decode(&encoded(&payload), 1)
                .is_err()
        );
        payload.version = 2;
        assert!(clipboard.decode(&encoded(&payload), 1).is_err());
        payload.version = 1;
        let text = encoded(&payload).replacen("\"version\":1", "\"extra\":0,\"version\":1", 1);
        assert!(clipboard.decode(&text, 1).is_err());
        payload.notes.clear();
        assert!(clipboard.decode(&encoded(&payload), 1).is_err());
    }

    #[test]
    fn paste_preserves_musical_data_with_global_ids_and_isolated_groups() {
        let (mut project, _, mut payload) = fixture();
        project.patterns[0].notes[0].id = u64::MAX; // Allocate gaps instead of max + 1.
        payload.notes.push(ClipboardNote {
            channel_id: None,
            group_id: Some(7),
            pitch: 127,
            offset: 1.25,
            length: 0.5,
            velocity: 1.0,
            muted: false,
        });
        let count = project.active_pattern().notes.len();
        let before = project_fingerprint(&project);
        let (first, selected) = paste_candidate(&project, &payload, 2.25).unwrap();
        assert_eq!(project_fingerprint(&project), before);
        assert_eq!(selected.len(), 3);
        let notes = &first.active_pattern().notes[count..];
        for (note, source) in notes.iter().zip(&payload.notes) {
            assert_eq!(note.start, (2.25 + source.offset) as f32);
            assert_eq!(note.channel_id, source.channel_id);
            assert_eq!(
                (note.note, note.length, note.velocity, note.muted),
                (source.pitch, source.length, source.velocity, source.muted)
            );
            assert!(selected.contains(&note.id));
        }
        assert_eq!(notes[0].group_id, notes[1].group_id);
        assert!(notes[0].group_id.is_some());
        assert!(notes[2].group_id.is_none(), "singleton group is detached");
        let (second, next_selection) = paste_candidate(&first, &payload, 2.25).unwrap();
        assert!(selected.is_disjoint(&next_selection));
        let repeated = &second.active_pattern().notes[count + 3..];
        assert_eq!(repeated[0].start, notes[0].start);
        assert_ne!(repeated[0].group_id, notes[0].group_id);
        assert_eq!(
            piano_note_ids(&second).len(),
            second.patterns.iter().map(|p| p.notes.len()).sum::<usize>()
        );
    }

    #[test]
    fn paste_refuses_bad_data_channels_bounds_and_existing_duplicate_ids_atomically() {
        let (project, _, payload) = fixture();
        let before = project_fingerprint(&project);
        for mutate in [
            |n: &mut ClipboardNote| n.offset = f64::NAN,
            |n: &mut ClipboardNote| n.offset = f64::INFINITY,
            |n: &mut ClipboardNote| n.offset = -0.1,
            |n: &mut ClipboardNote| n.length = f32::NAN,
            |n: &mut ClipboardNote| n.length = 0.0,
            |n: &mut ClipboardNote| n.velocity = f32::INFINITY,
            |n: &mut ClipboardNote| n.velocity = 1.01,
            |n: &mut ClipboardNote| n.pitch = 128,
            |n: &mut ClipboardNote| n.channel_id = Some(u32::MAX),
            |n: &mut ClipboardNote| n.channel_id = Some(0),
            |n: &mut ClipboardNote| n.group_id = Some(0),
        ] {
            let mut invalid = payload.clone();
            mutate(&mut invalid.notes[1]);
            assert!(paste_candidate(&project, &invalid, 0.0).is_err());
            assert_eq!(project_fingerprint(&project), before);
        }
        for anchor in [-1.0, f64::NAN, f64::INFINITY, 4095.5, 4096.0] {
            assert!(paste_candidate(&project, &payload, anchor).is_err());
        }
        let mut invalid = project.clone();
        invalid.channels.push(invalid.channels[0].clone());
        assert!(paste_candidate(&invalid, &payload, 0.0).is_err());
        invalid = project.clone();
        let duplicate = invalid.patterns[0].notes[0].clone();
        invalid.patterns[0].notes.push(duplicate);
        assert!(paste_candidate(&invalid, &payload, 0.0).is_err());
        invalid = project.clone();
        invalid.active_pattern = invalid.patterns.len();
        assert!(paste_candidate(&invalid, &payload, 0.0).is_err());
        let mut oversized = payload.clone();
        oversized.notes = vec![payload.notes[0].clone(); MAX_NOTES];
        assert!(paste_candidate(&project, &oversized, 0.0).is_err());
    }
}
