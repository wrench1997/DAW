//! Guarded, discrete melody keyboard edits share the note clipboard's ownership barriers.
use super::*;
use crate::piano_roll::{
    edit_piano_notes_from_keyboard, edit_piano_notes_from_keyboard_with_range,
    piano_keyboard_targets,
};

impl CitrusApp {
    pub(super) fn piano_immediate_action(&mut self, ctx: &egui::Context, action: ShortcutAction) {
        if !self.piano_clipboard_ready(ctx) || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        self.apply_ready_piano_immediate_action(action);
    }

    pub(super) fn piano_menu_action(&mut self, ctx: &egui::Context, action: ShortcutAction) {
        if !self.piano_editor_command_ready(ctx, true) || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        match action {
            ShortcutAction::Delete => self.delete_selection(),
            ShortcutAction::QuickLegato => self.quick_legato_piano_roll(),
            _ => self.apply_ready_piano_immediate_action(action),
        }
    }

    fn apply_ready_piano_immediate_action(&mut self, action: ShortcutAction) {
        self.sync_piano_range_owner();
        match action {
            ShortcutAction::PianoRange(action) => self.apply_piano_range_action(action),
            ShortcutAction::DeselectNotes => {
                self.piano_roll_state.clear_selection();
            }
            ShortcutAction::ToggleGhostNotes => {
                self.piano_roll_state.ghosts_visible = !self.piano_roll_state.ghosts_visible;
            }
            ShortcutAction::PianoEdit(edit) => {
                let Some(channel_id) = self
                    .project
                    .channels
                    .get(self.selected_channel)
                    .map(|c| c.id)
                else {
                    return;
                };
                if channel_id == 0
                    || self
                        .project
                        .channels
                        .iter()
                        .filter(|c| c.id == channel_id)
                        .count()
                        != 1
                {
                    self.notify("Piano edit needs one unambiguous active Channel".into());
                    return;
                }
                let notes = &self.project.active_pattern().notes;
                let targets = piano_keyboard_targets(
                    notes,
                    &self.piano_roll_state.selection_ids,
                    channel_id,
                    self.piano_roll_state.grouping_enabled,
                );
                let id_floor = self
                    .project
                    .patterns
                    .iter()
                    .flat_map(|p| &p.notes)
                    .map(|n| n.id)
                    .max()
                    .unwrap_or(0);
                let snap = self.piano_roll_state.local_snap;
                if snap == PianoSnap::Off && matches!(edit, PianoKeyboardEdit::QuickQuantize { .. })
                {
                    self.notify("Quick quantize needs a Piano snap grid. Choose a grid or use the Quantize dialog.".into());
                    return;
                }
                let result = match if let Some(range) = self.piano_roll_state.repeat_range {
                    edit_piano_notes_from_keyboard_with_range(
                        notes,
                        &targets,
                        edit,
                        snap.keyboard_step(),
                        id_floor,
                        Some(range),
                    )
                } else {
                    edit_piano_notes_from_keyboard(
                        notes,
                        &targets,
                        edit,
                        snap.keyboard_step(),
                        id_floor,
                    )
                } {
                    Ok(result) => result,
                    Err(error) => {
                        self.notify(format!("Piano edit was not applied: {error}"));
                        return;
                    }
                };
                let mut candidate = self.project.clone();
                candidate.active_pattern_mut().notes = result.notes;
                self.commit_editor_project(candidate);
                // Preserve the selected-or-all scope through repeated edits. Only
                // phrase repeat selects its new copies, ready for the next repeat.
                if edit == PianoKeyboardEdit::RepeatRight {
                    self.piano_roll_state.selection_ids = result.selection_ids;
                }
            }
            _ => {}
        }
    }
}
