//! File drops append notes to one explicitly hit instrument, never replace the score.
use super::*;
use std::io::Read;

const MAX_DROP_BYTES: usize = 16 * 1024 * 1024;

fn is_midi(file: &egui::DroppedFile) -> bool {
    file.bytes.as_ref().is_some_and(|b| b.starts_with(b"MThd"))
        || file.mime == "audio/midi"
        || file.mime == "audio/x-midi"
        || file
            .path
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new(&file.name))
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("mid") || s.eq_ignore_ascii_case("midi"))
}

fn read_drop(file: &egui::DroppedFile) -> anyhow::Result<Vec<u8>> {
    if let Some(bytes) = &file.bytes {
        anyhow::ensure!(bytes.len() <= MAX_DROP_BYTES, "MIDI exceeds 16 MiB");
        return Ok(bytes.to_vec());
    }
    let path = file
        .path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("MIDI drop has no bytes or local path"))?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_DROP_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= MAX_DROP_BYTES, "MIDI exceeds 16 MiB");
    Ok(bytes)
}

impl CitrusApp {
    pub(super) fn midi_drop_target(&mut self, ui: &egui::Ui, rect: Rect, channel: u32) {
        if !ui.is_enabled() || !ui.rect_contains_pointer(rect) {
            return;
        }
        let files = ui.input(|i| i.raw.dropped_files.clone());
        if !files.iter().any(is_midi) {
            return;
        }
        // Own this OS drop once. Read temporary files synchronously before any dialog/frame.
        ui.ctx().input_mut(|i| i.raw.dropped_files.clear());
        if files.len() != 1 {
            self.notify("Drop one MIDI file at a time; no notes were changed".into());
            return;
        }
        let result = read_drop(&files[0]).and_then(|bytes| self.append_midi_drop(&bytes, channel));
        match result {
            Ok(count) => self.notify(format!("Appended {count} MIDI notes at beat 1; BPM and existing notes kept. Undo removes this drop.")),
            Err(error) => self.notify(format!("MIDI drop failed: {error}")),
        }
    }

    fn append_midi_drop(&mut self, bytes: &[u8], channel: u32) -> anyhow::Result<usize> {
        anyhow::ensure!(
            !self.project_snapshot_transition_pending()
                && !self.project_lifecycle_barriers_active()
                && !self.plugin_topology_is_settling()
                && self.top_shortcut_modal().is_none()
                && !self.shortcut_blocking_layer_active()
                && !self.editor_pointer_gesture_active()
                && !self.sampler_gesture_active()
                && self.piano_roll_gesture_before.is_none()
                && self.playlist_gesture_before.is_none()
                && self.piano_note_properties.is_none(),
            "Finish the current edit or project operation, then drop again"
        );
        let mut candidate = self.project.clone();
        let count = midi::append_to_channel(&mut candidate, bytes, channel)?;
        self.cancel_all_keyboard_auditions();
        self.commit_editor_project(candidate);
        self.selected_channel = self
            .project
            .channels
            .iter()
            .position(|c| c.id == channel)
            .unwrap();
        self.piano_roll_state.clear_selection();
        self.sync_piano_range_owner();
        self.request_immediate_timeline_compile();
        Ok(count)
    }
}
