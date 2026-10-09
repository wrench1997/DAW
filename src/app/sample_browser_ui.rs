//! Sounds browser integration. Directory I/O and import decoding stay on control-side workers.

use super::*;
use crate::sample_browser::EntryKind;

impl CitrusApp {
    pub(super) fn browser_sounds(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open folder…").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_folder()
            {
                self.browser_search.clear();
                self.sample_browser.navigate(path);
            }
            let parent = self
                .sample_browser
                .directory
                .as_ref()
                .and_then(|path| path.parent())
                .map(Path::to_path_buf);
            if ui
                .add_enabled(parent.is_some(), egui::Button::new("Up"))
                .clicked()
                && let Some(parent) = parent
            {
                self.browser_search.clear();
                self.sample_browser.navigate(parent);
            }
        });
        if let Some(directory) = self.sample_browser.directory.clone() {
            // Full local paths are visible only on demand, never saved as browser preferences.
            let name = directory
                .file_name()
                .unwrap_or(directory.as_os_str())
                .to_string_lossy();
            ui.add(egui::Label::new(RichText::new(name).strong()).truncate())
                .on_hover_text(directory.to_string_lossy());
            ui.horizontal(|ui| {
                if ui.small_button("Refresh").clicked() {
                    self.sample_browser.navigate(directory.clone());
                }
                if self.sample_browser.busy() {
                    ui.spinner();
                    if ui.small_button("Cancel scan").clicked() {
                        self.sample_browser.cancel();
                    }
                }
            });
        } else {
            ui.label("Choose a local folder to browse WAV files.");
            ui.small("Only that folder is listed. No disk-wide search.");
            return;
        }
        if let Some(notice) = &self.sample_browser.notice {
            ui.label(RichText::new(notice).size(10.0).color(theme::AMBER));
        }
        if self.sample_browser.limited {
            ui.label(RichText::new("Listing limit reached. Choose a smaller folder; search filters only listed items.")
                .size(10.0).color(theme::AMBER));
        }
        ui.separator();
        let query = self.browser_search.to_lowercase();
        let mut navigate = None;
        let mut selected = None;
        let mut visible = 0;
        for entry in &self.sample_browser.entries {
            if !entry.matches(&query) {
                continue;
            }
            visible += 1;
            let label = match entry.kind {
                EntryKind::Folder => format!("▸  {}", entry.name),
                EntryKind::Wav => entry.name.clone(),
            };
            let response = ui
                .push_id(&entry.path, |ui| {
                    ui.add(
                        egui::Button::new(RichText::new(label).size(11.0).color(
                            if entry.kind == EntryKind::Folder {
                                theme::BLUE
                            } else {
                                theme::TEXT
                            },
                        ))
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .selected(self.sample_browser.selected.as_ref() == Some(&entry.path))
                        .min_size(Vec2::new(ui.available_width(), 26.0)),
                    )
                })
                .inner;
            let response = response.on_hover_text(match entry.kind {
                EntryKind::Folder => "Double-click to open this folder",
                EntryKind::Wav => {
                    "Select, then Import to Playlist. WAV content is validated during import."
                }
            });
            if entry.kind == EntryKind::Folder && response.double_clicked() {
                navigate = Some(entry.path.clone());
            } else if response.clicked() {
                selected = Some(entry.path.clone());
            }
        }
        if let Some(path) = navigate {
            self.browser_search.clear();
            self.sample_browser.navigate(path);
        } else if let Some(path) = selected {
            self.sample_browser.selected = Some(path);
            self.audio_import_error = None;
        }
        if visible == 0 && !self.sample_browser.busy() {
            ui.label(if query.is_empty() {
                "No folders or WAV candidates found."
            } else {
                "No matching listed items."
            });
        }
    }

    pub(super) fn browser_selection(&mut self, ui: &mut egui::Ui) {
        match self.browser_tab {
            BrowserTab::Sounds => {
                ui.label(
                    RichText::new("LOCAL WAV IMPORT")
                        .size(9.0)
                        .color(theme::MUTED),
                );
                let selected = self
                    .sample_browser
                    .selected_entry(&self.browser_search.to_lowercase())
                    .cloned();
                if let Some(entry) = &selected {
                    ui.add(egui::Label::new(RichText::new(&entry.name).strong()).truncate())
                        .on_hover_text(&entry.name);
                    if let Some(bytes) = entry.bytes {
                        ui.small(format!("{} bytes · validation on import", bytes));
                    }
                } else {
                    ui.small("Select a WAV file in the folder list.");
                }
                let importing = self.audio_import_receiver.is_some();
                let available = selected.is_some() && self.project_media_actions_available();
                if ui.add_enabled(available, egui::Button::new(if importing {
                    "Importing…"
                } else {
                    "Import to Playlist"
                })).on_hover_text("Adds one Audio Clip at the playhead, on the selected clip’s lane (otherwise lane 5). Undo removes the import; the source file is never modified.")
                    .clicked()
                    && let Some(entry) = selected
                {
                    self.start_audio_import_path(entry.path);
                }
                if let Some(error) = &self.audio_import_error {
                    ui.label(
                        RichText::new("Import failed. Hover for details.")
                            .size(10.0)
                            .color(theme::RED),
                    )
                    .on_hover_text(error);
                }
                ui.small("PCM 16/24/32-bit or float32 WAV. Audition is not available yet.");
            }
            BrowserTab::Plugins => {
                ui.small("Double-click to load into the selected Channel or Mixer.");
            }
            BrowserTab::Project => {
                ui.small("Project items reflect the current arrangement. Use Project media / relink to manage audio references.");
            }
        }
    }
}
