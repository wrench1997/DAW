//! Small native export surfaces; rendering and all filesystem work stay in the worker.

use crate::export_job::ExportJob;

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
}
