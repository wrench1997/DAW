//! Small native export surfaces; rendering and all filesystem work stay in the worker.

use crate::{
    export_job::ExportJob,
    export_options::{WavExportOptions, WavLevelPolicy, WavSampleFormat},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportStep {
    Settings,
    Review,
    ChoosingDestination,
}

#[derive(Clone, Copy, Debug)]
struct ExportDraft {
    project_session: u64,
    device_rate: u32,
    options: WavExportOptions,
    step: ExportStep,
}

/// Pending review belongs to a project session; only successfully started
/// choices are remembered, in memory for this app run. Never retain a path or
/// overwrite consent, and never modify the project's or device's settings.
#[derive(Default)]
pub(super) struct ExportDialog {
    last_used: Option<WavExportOptions>,
    draft: Option<ExportDraft>,
}

impl ExportDialog {
    pub(super) fn is_open(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn open(&mut self, project_session: u64, device_rate: u32, running: bool) {
        self.reconcile_session(project_session);
        if running || self.is_open() {
            return;
        }
        let device_rate = if (8_000..=192_000).contains(&device_rate) {
            device_rate
        } else {
            48_000
        };
        self.draft = Some(ExportDraft {
            project_session,
            device_rate,
            options: self
                .last_used
                .unwrap_or_else(|| WavExportOptions::legacy(device_rate)),
            step: ExportStep::Settings,
        });
    }

    pub(super) fn close(&mut self) {
        self.draft = None;
    }

    fn reconcile_session(&mut self, project_session: u64) {
        if self
            .draft
            .is_some_and(|draft| draft.project_session != project_session)
        {
            self.close();
        }
    }

    fn review(&mut self) {
        if let Some(draft) = self.draft.as_mut()
            && draft.step == ExportStep::Settings
            && draft.options.validate().is_ok()
        {
            draft.step = ExportStep::Review;
        }
    }

    fn back(&mut self) {
        if let Some(draft) = self.draft.as_mut()
            && draft.step == ExportStep::Review
        {
            draft.step = ExportStep::Settings;
        }
    }

    pub(super) fn begin_destination(
        &mut self,
        project_session: u64,
        running: bool,
    ) -> Option<WavExportOptions> {
        self.reconcile_session(project_session);
        let draft = self.draft.as_mut()?;
        if running || draft.step != ExportStep::Review || draft.options.validate().is_err() {
            return None;
        }
        draft.step = ExportStep::ChoosingDestination;
        Some(draft.options)
    }

    pub(super) fn finish_destination(&mut self, started: bool) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        if draft.step != ExportStep::ChoosingDestination {
            return;
        }
        if started {
            self.last_used = Some(draft.options);
            self.close();
        } else {
            // Native Save/overwrite cancellation or a failed worker launch
            // returns to review without discarding the chosen format.
            draft.step = ExportStep::Review;
        }
    }
}

fn dialog_button(ui: &mut egui::Ui, label: &'static str) -> bool {
    let response = ui.button(label);
    #[cfg(test)]
    remember_test_rect(&response, label);
    response.clicked()
}

#[cfg(test)]
fn remember_test_rect(response: &egui::Response, label: &'static str) {
    response.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new(("export-test-button", label)), response.rect)
    });
}

fn settings_body(
    ui: &mut egui::Ui,
    draft: &mut ExportDraft,
    project_name: &str,
    error: Option<&str>,
) {
    ui.label(format!("Project: {project_name}"));
    ui.label("Full arrangement • Stereo • Offline project snapshot");
    ui.separator();
    if draft.step == ExportStep::Settings {
        ui.label("Sample rate");
        egui::ComboBox::from_id_salt("wav-export-rate")
            .selected_text(format!("{} Hz", draft.options.sample_rate))
            .show_ui(ui, |ui| {
                let mut rates = WavExportOptions::STANDARD_SAMPLE_RATES.to_vec();
                rates.extend([draft.device_rate, draft.options.sample_rate]);
                rates.sort_unstable();
                rates.dedup();
                for rate in rates {
                    ui.selectable_value(&mut draft.options.sample_rate, rate, format!("{rate} Hz"));
                }
            });
        ui.label("Sample format");
        let format_response = egui::ComboBox::from_id_salt("wav-export-format")
            .selected_text(draft.options.sample_format.label())
            .show_ui(ui, |ui| {
                for format in WavSampleFormat::ALL {
                    let response = ui.selectable_value(
                        &mut draft.options.sample_format,
                        format,
                        format.label(),
                    );
                    #[cfg(test)]
                    remember_test_rect(&response, format.label());
                    let _ = response;
                }
            });
        #[cfg(test)]
        remember_test_rect(&format_response.response, "Sample format");
        let _ = format_response;
        for policy in [
            WavLevelPolicy::AttenuatePeaks,
            WavLevelPolicy::PreserveLevel,
        ] {
            let response = ui.radio_value(&mut draft.options.level_policy, policy, policy.label());
            #[cfg(test)]
            remember_test_rect(&response, policy.label());
            let _ = response;
        }
        if dialog_button(ui, "Reset to legacy / device-rate defaults") {
            draft.options = WavExportOptions::legacy(draft.device_rate);
        }
    } else {
        ui.label(format!(
            "{} Hz • {}",
            draft.options.sample_rate,
            draft.options.sample_format.label()
        ));
        ui.label(draft.options.level_policy.label());
    }
    match draft.options.level_policy {
        WavLevelPolicy::AttenuatePeaks => {
            ui.label("A single gain reduction is applied only if the rendered sample peak exceeds 0.95 (about −0.45 dBFS). Quieter mixes are never boosted. This is not loudness normalization or a true-peak limiter.");
        }
        WavLevelPolicy::PreserveLevel => {
            ui.label("No additional export gain is applied. Integer PCM export fails safely if any sample exceeds ±1.0; lower the mix level or choose peak attenuation / float instead.");
        }
    }
    if draft.options.sample_format == WavSampleFormat::Float32 {
        ui.colored_label(egui::Color32::YELLOW, "With Preserve level, float export preserves finite samples above ±1.0. Caution: Citrus currently clamps those samples on WAV reimport; over-unity reimport is not lossless.");
    } else {
        ui.label("Integer PCM is rounded to the selected bit depth. No dither is applied.");
    }
    ui.separator();
    ui.label("Offline limits: active VST instruments/effects, sidechains and non-Tempo automation are rejected. Use Realtime Master Capture for the live result.");
    ui.label("Source WAVs use linear resampling when needed. Standard RIFF size limits apply; no MP3, RF64 or compressed export.");
    ui.label("Settings are remembered only for this app run. The next native Save dialog chooses the destination and confirms replacement of an existing file.");
    if let Some(error) = error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
}

/// Returns true only for the final review action. The caller then opens the
/// native Save dialog, retaining its existing overwrite-confirmation semantics.
pub(super) fn settings_window(
    ctx: &egui::Context,
    dialog: &mut ExportDialog,
    project_session: u64,
    project_name: &str,
    running: bool,
    error: Option<&str>,
) -> bool {
    dialog.reconcile_session(project_session);
    let Some(draft) = dialog.draft.as_mut() else {
        return false;
    };
    let mut review = false;
    let mut back = false;
    let mut close = false;
    let mut destination = false;
    let response = egui::Modal::new(egui::Id::new("wav-export-settings")).show(ctx, |ui| {
        ui.set_width(500.0);
        ui.heading(if draft.step == ExportStep::Settings {
            "WAV export settings"
        } else {
            "Review WAV export"
        });
        egui::ScrollArea::vertical()
            .id_salt("wav-export-review-scroll")
            .max_height((ctx.content_rect().height() - 180.0).max(120.0))
            .show(ui, |ui| {
                settings_body(ui, draft, project_name, error);
            });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if draft.step == ExportStep::Settings {
                review = dialog_button(ui, "Review…");
            } else if draft.step == ExportStep::Review {
                back = dialog_button(ui, "Back");
                ui.add_enabled_ui(!running, |ui| {
                    destination = dialog_button(ui, "Choose destination and export…");
                });
            }
            close = dialog_button(ui, "Close");
        });
    });
    if close || response.should_close() {
        dialog.close();
        return false;
    }
    if back {
        dialog.back();
    }
    if review {
        dialog.review();
    }
    destination
}

pub(super) fn progress_panel(root: &mut egui::Ui, job: &ExportJob) {
    let Some((path, progress)) = job.status() else {
        return;
    };
    root.ctx()
        .request_repaint_after(std::time::Duration::from_millis(50));
    egui::Panel::bottom("wav-export-progress")
        .exact_size(36.0)
        .show(root, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label("WAV snapshot:");
                ui.add_sized(
                    [180.0, 20.0],
                    egui::Label::new(path.file_name().unwrap_or_default().to_string_lossy())
                        .truncate(),
                )
                .on_hover_text(path.display().to_string());
                ui.add(
                    egui::ProgressBar::new(progress.basis_points as f32 / 10000.0)
                        .desired_width(260.0)
                        .text(format!(
                            "{} · {}%",
                            progress.phase(),
                            progress.basis_points / 100
                        )),
                );
                if ui
                    .add_enabled(progress.can_cancel, egui::Button::new("Cancel export"))
                    .clicked()
                {
                    job.cancel();
                }
                if !progress.can_cancel && !progress.cancelling && progress.basis_points < 10000 {
                    ui.label("Finalizing; cancellation is no longer available.");
                }
            });
        });
}

pub(super) fn error_window(ctx: &egui::Context, error: &mut Option<String>) {
    let Some(message) = error.as_ref() else {
        return;
    };
    let mut open = true;
    let mut dismiss = false;
    egui::Window::new("WAV export could not complete")
        .id(egui::Id::new("wav-export-error"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(500.0)
        .show(ctx, |ui| {
            ui.label(message);
            ui.add_space(8.0);
            dismiss = ui.button("Dismiss").clicked();
        });
    if !open || dismiss {
        *error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export_job::ExportOutcome;

    #[test]
    fn export_surfaces_render_active_cancelled_and_error_states_without_a_device() {
        let ctx = egui::Context::default();
        let mut job = ExportJob::default();
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        job.start(
            1,
            "a-very-long-export-destination-name.wav".into(),
            move |_| {
                wait.recv().unwrap();
                ExportOutcome::Cancelled
            },
        )
        .unwrap();
        let mut error = Some("Unsupported automation 'Lead volume' (lane 900, target Mixer 7 volume). Use Realtime Master Capture.".into());
        let _ = ctx.run_ui(egui::RawInput::default(), |root| {
            progress_panel(root, &job);
            error_window(root.ctx(), &mut error);
        });
        assert!(job.cancel());
        let _ = ctx.run_ui(egui::RawInput::default(), |root| {
            progress_panel(root, &job);
            error_window(root.ctx(), &mut error);
        });
        assert!(job.status().unwrap().1.cancelling);
        assert!(error.is_some());
        release.send(()).unwrap();
    }
    fn frame(ctx: &egui::Context, dialog: &mut ExportDialog, events: Vec<egui::Event>) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 900.0),
            )),
            events,
            ..Default::default()
        };
        let mut destination = false;
        let _ = ctx.run_ui(input, |ui| {
            destination = settings_window(ui.ctx(), dialog, 1, "Export review test", false, None);
        });
        destination
    }

    fn click(ctx: &egui::Context, dialog: &mut ExportDialog, label: &'static str) -> bool {
        frame(ctx, dialog, Vec::new());
        let rect = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("export-test-button", label))))
            .expect("button must be rendered");
        let pos = rect.center();
        let pointer = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            ctx,
            dialog,
            vec![egui::Event::PointerMoved(pos), pointer(true)],
        );
        frame(
            ctx,
            dialog,
            vec![egui::Event::PointerMoved(pos), pointer(false)],
        )
    }

    #[test]
    fn settings_review_back_close_and_escape_use_actual_egui_events() {
        let ctx = egui::Context::default();
        let mut dialog = ExportDialog::default();
        dialog.open(1, 48000, false);
        frame(&ctx, &mut dialog, Vec::new());
        frame(&ctx, &mut dialog, Vec::new());
        dialog.draft.as_mut().unwrap().options.sample_format = WavSampleFormat::Float32;
        assert!(!click(&ctx, &mut dialog, "Review…"));
        assert_eq!(dialog.draft.unwrap().step, ExportStep::Review);
        assert!(!click(&ctx, &mut dialog, "Back"));
        assert_eq!(dialog.draft.unwrap().step, ExportStep::Settings);
        assert_eq!(
            dialog.draft.unwrap().options.sample_format,
            WavSampleFormat::Float32
        );
        assert!(!click(&ctx, &mut dialog, "Close"));
        assert!(!dialog.is_open());
        dialog.open(1, 44100, false);
        assert_eq!(
            dialog.draft.unwrap().options,
            WavExportOptions::legacy(44100)
        );
        frame(&ctx, &mut dialog, Vec::new());
        frame(&ctx, &mut dialog, Vec::new());
        frame(
            &ctx,
            &mut dialog,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!dialog.is_open());
    }

    #[test]
    fn actual_review_button_requests_destination_once_and_native_cancel_returns_to_review() {
        let ctx = egui::Context::default();
        let mut dialog = ExportDialog::default();
        dialog.open(1, 48000, false);
        frame(&ctx, &mut dialog, Vec::new());
        frame(&ctx, &mut dialog, Vec::new());
        assert!(!click(&ctx, &mut dialog, "Review…"));
        assert!(click(&ctx, &mut dialog, "Choose destination and export…"));
        let options = dialog.begin_destination(1, false).unwrap();
        assert_eq!(options, WavExportOptions::legacy(48000));
        assert!(dialog.begin_destination(1, false).is_none());
        dialog.finish_destination(false);
        assert_eq!(dialog.draft.unwrap().step, ExportStep::Review);
        assert!(click(&ctx, &mut dialog, "Choose destination and export…"));
        assert!(dialog.begin_destination(1, false).is_some());
        dialog.finish_destination(true);
        assert!(!dialog.is_open());
        assert!(dialog.begin_destination(1, false).is_none());
    }

    #[test]
    fn repeated_open_and_busy_worker_cannot_reset_or_start_another_export() {
        let mut dialog = ExportDialog::default();
        dialog.open(1, 48000, true);
        assert!(!dialog.is_open());
        dialog.open(1, 44100, false);
        dialog.draft.as_mut().unwrap().options.sample_format = WavSampleFormat::Pcm16;
        dialog.open(1, 192000, false);
        assert_eq!(dialog.draft.unwrap().options.sample_rate, 44100);
        dialog.review();
        assert!(dialog.begin_destination(1, true).is_none());
        assert_eq!(dialog.draft.unwrap().step, ExportStep::Review);
        dialog.close();
        assert!(dialog.begin_destination(1, false).is_none());
    }

    #[test]
    fn new_project_invalidates_pending_review_without_saving_cancelled_settings() {
        let mut dialog = ExportDialog::default();
        dialog.open(1, 96000, false);
        dialog.draft.as_mut().unwrap().options.level_policy = WavLevelPolicy::PreserveLevel;
        dialog.review();
        assert!(dialog.begin_destination(2, false).is_none());
        assert!(!dialog.is_open());
        assert!(dialog.last_used.is_none());
        dialog.open(2, 48000, false);
        assert_eq!(
            dialog.draft.unwrap().options,
            WavExportOptions::legacy(48000)
        );
        dialog.draft.as_mut().unwrap().options.sample_format = WavSampleFormat::Pcm16;
        dialog.review();
        dialog.begin_destination(2, false).unwrap();
        dialog.finish_destination(true);
        dialog.open(3, 96000, false);
        assert_eq!(
            dialog.draft.unwrap().options.sample_format,
            WavSampleFormat::Pcm16
        );
        assert_eq!(dialog.draft.unwrap().options.sample_rate, 48000);
    }

    #[test]
    fn invalid_device_rate_falls_back_safely_and_invalid_option_cannot_be_reviewed() {
        let mut dialog = ExportDialog::default();
        dialog.open(1, 0, false);
        assert_eq!(dialog.draft.unwrap().options, WavExportOptions::default());
        dialog.draft.as_mut().unwrap().options.sample_rate = u32::MAX;
        dialog.review();
        assert_eq!(dialog.draft.unwrap().step, ExportStep::Settings);
        assert!(dialog.begin_destination(1, false).is_none());
    }

    #[test]
    fn format_and_level_choices_work_with_pointer_events_and_reset_to_legacy() {
        let ctx = egui::Context::default();
        let mut dialog = ExportDialog::default();
        dialog.open(1, 44100, false);
        frame(&ctx, &mut dialog, Vec::new());
        frame(&ctx, &mut dialog, Vec::new());
        click(&ctx, &mut dialog, "Sample format");
        click(&ctx, &mut dialog, WavSampleFormat::Float32.label());
        assert_eq!(
            dialog.draft.unwrap().options.sample_format,
            WavSampleFormat::Float32
        );
        click(&ctx, &mut dialog, WavLevelPolicy::PreserveLevel.label());
        assert_eq!(
            dialog.draft.unwrap().options.level_policy,
            WavLevelPolicy::PreserveLevel
        );
        click(&ctx, &mut dialog, "Reset to legacy / device-rate defaults");
        assert_eq!(
            dialog.draft.unwrap().options,
            WavExportOptions::legacy(44100)
        );
    }

    #[test]
    fn long_preflight_errors_keep_footer_inside_small_viewport_and_closeable() {
        let ctx = egui::Context::default();
        let mut dialog = ExportDialog::default();
        dialog.open(1, 48000, false);
        let error = "Unsupported automation lane. ".repeat(250);
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 680.0));
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(viewport),
                    ..Default::default()
                },
                |ui| {
                    settings_window(ui.ctx(), &mut dialog, 1, "Long error", false, Some(&error));
                },
            );
        }
        for label in ["Close", "Review…"] {
            let rect = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new(("export-test-button", label)))
                })
                .unwrap();
            assert!(
                viewport.contains_rect(rect),
                "{label} outside viewport: {rect:?}"
            );
        }
    }

    #[test]
    fn popup_escape_closes_only_the_dropdown_before_the_export_dialog() {
        let ctx = egui::Context::default();
        let mut dialog = ExportDialog::default();
        dialog.open(1, 48000, false);
        frame(&ctx, &mut dialog, Vec::new());
        frame(&ctx, &mut dialog, Vec::new());
        click(&ctx, &mut dialog, "Sample format");
        assert!(egui::Popup::is_any_open(&ctx));
        let escape = || egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut popup_blocks_shortcuts = false;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 900.0),
                )),
                events: vec![escape()],
                ..Default::default()
            },
            |ui| {
                settings_window(ui.ctx(), &mut dialog, 1, "Popup test", false, None);
                popup_blocks_shortcuts = ui.ctx().any_popup_open();
            },
        );
        assert!(dialog.is_open());
        // The app shortcut policy also sees the popup for this pass, so it
        // cannot dismiss the dialog or a different underlying window.
        assert!(popup_blocks_shortcuts);
        frame(&ctx, &mut dialog, Vec::new());
        assert!(!egui::Popup::is_any_open(&ctx));
        frame(&ctx, &mut dialog, vec![escape()]);
        assert!(!dialog.is_open());
    }
}
