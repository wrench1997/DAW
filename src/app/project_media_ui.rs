//! Project-media UI and runtime integration. Storage/validation lives in project_media.

use super::*;
use crate::project_media::{MediaIdentity, PreparedMediaRelink};

impl CitrusApp {
    fn project_media_actions_available(&self) -> bool {
        self.project_lifecycle.is_idle()
            && !self.project_lifecycle_barriers_active()
            && self.audio_import_receiver.is_none()
            && self.queued_save_after_midi_recording.is_none()
            && self.deferred_project_intent_after_midi.is_none()
            && self.deferred_recovery_project_after_midi.is_none()
            && !self.recovery_available
            && self.piano_roll_transform.is_none()
            && self.pending_midi_import.is_none()
            && self.pending_midi_export.is_none()
    }

    pub(super) fn open_project_media(&mut self) {
        if !self.project_media_actions_available() {
            self.notify(
                "Finish active recording, import, save, or project dialogs before managing media"
                    .into(),
            );
            return;
        }
        // Keep the playhead, but don't replace samples under a playing transport.
        self.pause_for_project_media();
        self.project_media
            .open(self.project_session, &self.project.audio_assets);
    }

    fn pause_for_project_media(&mut self) {
        self.playing = false;
        self.stop_all_plugin_notes();
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
    }

    pub(super) fn reload_project_media_runtime(&mut self) {
        self.pause_for_project_media();
        // This advances both loader and callback identities, prunes obsolete cached paths and
        // clears existing registrations before admitting the newly referenced samples.
        self.queue_project_audio_assets();
    }

    pub(super) fn poll_project_media(&mut self, ctx: &egui::Context) {
        if self.project_media.open
            && let Some(audio) = &self.audio
            && audio.snapshot().transport_playing
        {
            // A previously queued transport activation must not resume behind this modal.
            audio.set_playing(false);
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        if self.project_media.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if let Some(prepared) = self
            .project_media
            .poll(self.project_session, &self.project.audio_assets)
        {
            self.apply_project_media_candidate(prepared);
        }
    }

    fn apply_project_media_candidate(&mut self, prepared: PreparedMediaRelink) {
        if !self.project_media_actions_available() {
            self.project_media.candidate = Some(prepared);
            self.project_media.notice = Some(
                "A save or project operation is active. Wait for it to finish, then Apply again."
                    .into(),
            );
            return;
        }
        let mut candidate = self.project.clone();
        if let Err(error) = prepared.apply_to(self.project_session, &mut candidate) {
            self.project_media.notice = Some(error.to_string());
            return;
        }
        let changed = commit_explicit_project_history_transaction(
            &mut self.project,
            &mut self.history_snapshot,
            &mut self.history_fingerprint,
            &mut self.undo_stack,
            &mut self.redo_stack,
            &mut self.dirty,
            candidate,
        );
        self.reload_project_media_runtime();
        self.register_audio_asset(prepared.expected.id, prepared.decoded);
        self.refresh_timeline_fingerprint(Instant::now(), true);
        self.sync_history_observer();
        self.project_media.notice = Some(if changed {
            "Media relinked. Save the project to keep the new path. Undo restores the previous reference.".into()
        } else {
            "Media reloaded from the reviewed file. The saved reference is unchanged.".into()
        });
        self.project_media
            .refresh(self.project_session, &self.project.audio_assets);
    }

    pub(super) fn project_media_dialog(&mut self, ctx: &egui::Context) {
        if !self.project_media.open {
            return;
        }
        let mut locate = None;
        let mut close = false;
        let mut cancel = false;
        let mut apply = false;
        let mut refresh = false;
        let available = self.project_media_actions_available();
        let busy = self.project_media.busy();
        let reviewing = self.project_media.candidate.is_some();
        egui::Modal::new(Id::new("project-media-modal"))
            .backdrop_color(Color32::from_black_alpha(180))
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 80.0).clamp(200.0, 670.0));
                ui.heading("Project media");
                egui::ScrollArea::vertical()
                    .id_salt("project-media-body")
                    .max_height((ctx.content_rect().height() - 150.0).max(160.0))
                    .show(ui, |ui| {
                        ui.label("Locate moved or unavailable WAV files. Playback stays paused; source files are never changed.");
                        ui.label(RichText::new("Relink requires the original sample rate, channel count and frame count to preserve Clip timing.").color(theme::MUTED));
                        ui.add_space(8.0);
                        if self.project.audio_assets.is_empty() {
                            ui.label("This project has no imported or recorded audio assets.");
                        }
                        ui.vertical(|ui| {
                            for asset in &self.project.audio_assets {
                                ui.push_id(asset.id, |ui| {
                                    ui.group(|ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(RichText::new(&asset.name).strong());
                                            ui.label(format!("{} Hz / {} ch / {} frames", asset.sample_rate, asset.channels, asset.frames));
                                            if ui.add_enabled(available && !busy && !reviewing, egui::Button::new("Locate…")).clicked() {
                                                locate = Some(asset.id);
                                            }
                                        });
                                        ui.label(asset.path.display().to_string());
                                        let clips = self.project.clips.iter().filter(|clip| clip.audio_asset_id == Some(asset.id)).count();
                                        ui.label(RichText::new(format!("Asset {} · used by {clips} Clip(s)", asset.id)).size(11.0).color(theme::MUTED));
                                        let report = self.project_media.reports.iter().find(|report| report.identity == MediaIdentity::from(asset));
                                        let failure = self.failed_audio_assets.get(&asset.id).filter(|failure| failure.generation == self.audio_asset_generation);
                                        if let Some(error) = report.and_then(|report| report.error.as_ref()) {
                                            ui.colored_label(theme::ORANGE, format!("Unavailable: {error}"));
                                        } else if let Some(failure) = failure {
                                            ui.colored_label(theme::ORANGE, &failure.message);
                                        } else if self.cached_audio_assets.get(&asset.id).is_some_and(|cached| cached.matches(self.project_session, asset)) {
                                            ui.label("Decoded audio is available in this session.");
                                        } else if report.is_some() {
                                            ui.label("File is available; not yet decoded in this session.");
                                        } else {
                                            ui.label("Path has not been checked. Use Refresh paths.");
                                        }
                                    });
                                });
                            }
                        });
                        if let Some(candidate) = &self.project_media.candidate {
                            ui.separator();
                            ui.label(RichText::new(format!("Review replacement for asset {}", candidate.expected.id)).strong());
                            ui.label(format!("From: {}", candidate.expected.path.display()));
                            ui.label(format!("To: {}", candidate.replacement.display()));
                            ui.label("WAV validated; timing properties match. Matching properties do not prove identical audio. Apply only if this is the file you intend to use.");
                            ui.horizontal(|ui| {
                                apply = ui.add_enabled(available && !busy, egui::Button::new("Apply relink")).clicked();
                                cancel = ui.button("Cancel relink").clicked();
                            });
                        }
                        if busy {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(self.project_media.busy_label());
                                if ui.button("Cancel check").clicked() { cancel = true; }
                            });
                        }
                        if let Some(notice) = &self.project_media.notice {
                            ui.separator();
                            ui.label(notice);
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    refresh = ui.add_enabled(!busy && !reviewing, egui::Button::new("Refresh paths")).clicked();
                    close = ui.button("Close").clicked();
                });
            });
        if close {
            self.project_media.close();
        } else if cancel {
            self.project_media.cancel();
        } else if apply {
            self.project_media
                .apply(self.project_session, &self.project.audio_assets);
        } else if refresh {
            self.project_media
                .refresh(self.project_session, &self.project.audio_assets);
        } else if let Some(asset_id) = locate {
            let Some(asset) = self
                .project
                .audio_assets
                .iter()
                .find(|asset| asset.id == asset_id)
            else {
                return;
            };
            let mut picker = rfd::FileDialog::new().add_filter("WAV audio", &["wav", "wave"]);
            if let Some(name) = asset.path.file_name().and_then(|name| name.to_str()) {
                picker = picker.set_file_name(name);
            }
            if let Some(path) = picker.pick_file() {
                self.project_media.locate(self.project_session, asset, path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_reference_edit_is_one_undo_step_and_reverts_runtime_cache_identity() {
        let mut project = Project::blank();
        project.audio_assets.push(AudioAsset {
            id: 17,
            name: "Take".into(),
            path: "old.wav".into(),
            sample_rate: 48_000,
            channels: 1,
            bits_per_sample: 16,
            frames: 100,
            waveform_peaks: vec![0.3],
        });
        let mut snapshot = project.clone();
        let mut fingerprint = project_fingerprint(&project);
        let mut undo = Vec::new();
        let mut redo = Vec::new();
        let mut dirty = false;
        let old_cache = CachedAudioAssetRegistration {
            project_session: 5,
            project_asset_id: 17,
            path: "old.wav".into(),
            samples: Arc::from([0.0_f32; 100]),
            sample_rate: 48_000,
            channels: 1,
        };
        let mut candidate = project.clone();
        candidate.audio_assets[0].path = "relinked.wav".into();
        assert!(commit_explicit_project_history_transaction(
            &mut project,
            &mut snapshot,
            &mut fingerprint,
            &mut undo,
            &mut redo,
            &mut dirty,
            candidate
        ));
        assert!(dirty);
        assert_eq!(undo.len(), 1);
        assert!(!old_cache.matches(5, &project.audio_assets[0]));
        let previous = undo.pop().unwrap();
        assert!(media_references_changed(
            &project.audio_assets,
            &previous.audio_assets
        ));
        redo.push(std::mem::replace(&mut project, previous));
        assert_eq!(project.audio_assets[0].path, PathBuf::from("old.wav"));
        let next = redo.pop().unwrap();
        assert!(media_references_changed(
            &project.audio_assets,
            &next.audio_assets
        ));
        project = next;
        assert_eq!(project.audio_assets[0].path, PathBuf::from("relinked.wav"));
    }

    #[test]
    fn media_modal_never_applies_a_replacement_with_an_unreviewed_enter_shortcut() {
        let policy = ShortcutPolicy::new(ShortcutContext {
            top_modal: Some(ShortcutModal::ProjectMedia),
            ..ShortcutContext::default()
        });
        let plain = ShortcutModifiers {
            command: false,
            non_command_control: false,
            shift: false,
            alt: false,
        };
        assert_eq!(
            policy.resolve(ShortcutChord {
                key: ShortcutKey::Enter,
                modifiers: plain
            }),
            None
        );
        assert_eq!(
            policy.resolve(ShortcutChord {
                key: ShortcutKey::Escape,
                modifiers: plain
            }),
            Some(ShortcutAction::DismissModal(ShortcutModal::ProjectMedia))
        );
    }
}
