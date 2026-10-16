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
    whole_song: bool,
    fixed_tail_confirmed: bool,
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
        self.open_inner(project_session, device_rate, running, false);
    }

    pub(super) fn open_whole_song(
        &mut self,
        project_session: u64,
        device_rate: u32,
        running: bool,
    ) {
        self.open_inner(project_session, device_rate, running, true);
    }

    fn open_inner(
        &mut self,
        project_session: u64,
        device_rate: u32,
        running: bool,
        whole_song: bool,
    ) {
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
            whole_song,
            fixed_tail_confirmed: false,
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
            draft.fixed_tail_confirmed = false;
        }
    }

    pub(super) fn begin_destination(
        &mut self,
        project_session: u64,
        running: bool,
    ) -> Option<WavExportOptions> {
        self.reconcile_session(project_session);
        let draft = self.draft.as_mut()?;
        if running
            || draft.step != ExportStep::Review
            || draft.options.validate().is_err()
            || draft.whole_song && !draft.fixed_tail_confirmed
        {
            return None;
        }
        draft.step = ExportStep::ChoosingDestination;
        Some(draft.options)
    }

    pub(super) fn confirmed_whole_fixed_tail(&self) -> bool {
        self.draft.is_some_and(|draft| {
            draft.whole_song
                && draft.fixed_tail_confirmed
                && matches!(
                    draft.step,
                    ExportStep::Review | ExportStep::ChoosingDestination
                )
        })
    }

    #[cfg(test)]
    pub(in crate::app) fn test_is_whole_review(&self) -> bool {
        self.draft
            .is_some_and(|draft| draft.whole_song && draft.step == ExportStep::Review)
    }

    #[cfg(test)]
    pub(in crate::app) fn test_set_options(&mut self, options: WavExportOptions) {
        let draft = self
            .draft
            .as_mut()
            .expect("actual export settings must be open");
        assert_eq!(draft.step, ExportStep::Settings);
        draft.options = options;
        draft.fixed_tail_confirmed = false;
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
    let observation = format!(
        "rect={:?}, interact={:?}, enabled={}, layer={:?}, pass={}, discard={}",
        response.rect,
        response.interact_rect,
        response.enabled(),
        response.layer_id,
        response.ctx.current_pass_index(),
        response.ctx.will_discard(),
    );
    response.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new(("export-test-button", label)), response.rect);
        data.insert_temp(
            egui::Id::new(("export-test-interact", label)),
            response.interact_rect,
        );
        data.insert_temp(
            egui::Id::new(("export-test-widget-observation", label)),
            observation,
        );
    });
}

fn settings_body(
    ui: &mut egui::Ui,
    draft: &mut ExportDraft,
    project_name: &str,
    error: Option<&str>,
) {
    ui.label(format!("Project: {project_name}"));
    ui.label("Full arrangement • Stereo • Immutable offline snapshot");
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
        ui.horizontal(|ui| {
            ui.label("Release tail (ms)");
            ui.add(
                egui::DragValue::new(&mut draft.options.tail_milliseconds)
                    .range(0..=WavExportOptions::MAX_TAIL_MILLISECONDS)
                    .speed(100),
            );
        });
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
        ui.label(format!(
            "Release tail: {} ms after the arrangement",
            draft.options.tail_milliseconds
        ));
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
    if draft.whole_song {
        ui.label(whole_fixed_window_review_text(draft.options));
        ui.label("Master Volume/Pan/Mute automation is rendered. The selected fixed tail holds the final master controls and starts no new notes. This export does not measure acoustic silence.");
        ui.label("Offline limits: one VST3 Instrument at the current sample rate, with optional fully wet VST3 Inserts in one static, ordered post-fader chain to Master. Master Volume/Pan/Mute automation is sample-exact; the source Generator's automatable parameters use Q128 block-start values and hold their last sent values through the fixed tail.");
        ui.label("Unavailable: branches, sends, sidechains, active partial-wet Inserts, off-path Inserts, other music/audio sources, Tempo, Channel/non-Master mixer controls, and other plug-in/slot parameter automation. Use Realtime Master Capture for those projects. Generator automation is not sample-accurate within Q128.");
        if draft.step == ExportStep::Review {
            let response = ui.checkbox(
                &mut draft.fixed_tail_confirmed,
                "I accept this fixed window, including possible truncation",
            );
            #[cfg(test)]
            remember_test_rect(&response, "Confirm fixed VST tail window");
            let _ = response;
        }
    } else {
        ui.label("Tempo, Master Volume, Master Pan and Master Mute automation are rendered. The fixed release tail drains existing native voices and holds the last master controls; it does not start new notes.");
        ui.label("Offline limits: active VST instruments/effects, sidechains and other automation are rejected. Disabled or bypassed instrument channels are silent. Use Realtime Master Capture for the live result.");
    }
    ui.label("Source WAVs use linear resampling when needed. Standard RIFF size limits apply; no MP3, RF64 or compressed export.");
    ui.label("Settings are remembered only for this app run. The next native Save dialog chooses the destination and confirms replacement of an existing file.");
    if let Some(error) = error {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }
}

pub(super) fn whole_fixed_window_review_text(options: WavExportOptions) -> String {
    let end = if options.tail_milliseconds == 0 {
        "With 0 ms, the file ends at the song end.".to_owned()
    } else {
        format!(
            "The file ends {} ms after the song end.",
            options.tail_milliseconds
        )
    };
    format!(
        "Fixed VST render window: {} ms of tail. {end} Unknown or longer reported plug-in tails may be truncated. A finite tail selection does not guarantee the entire release is captured.",
        options.tail_milliseconds
    )
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
        // Area starts from its previous measured height. Give the scroll body
        // a viewport budget before laying it out, including room for the
        // heading and footer, so the footer cannot grow the modal each pass.
        // Only set the maximum: shorter native reviews still shrink to fit.
        let body_height = (ctx.content_rect().height() - 180.0).max(120.0);
        let chrome_height = ui.text_style_height(&egui::TextStyle::Heading)
            + ui.spacing().interact_size.y
            + 3.0 * ui.spacing().item_spacing.y
            + 6.0;
        ui.set_max_height(body_height + chrome_height);
        ui.heading(if draft.step == ExportStep::Settings {
            "WAV export settings"
        } else {
            "Review WAV export"
        });
        let scroll = egui::ScrollArea::vertical()
            .id_salt("wav-export-review-scroll")
            .max_height(body_height)
            .show(ui, |ui| {
                settings_body(ui, draft, project_name, error);
            });
        #[cfg(test)]
        {
            let observation = format!(
                "content={:?}, inner={:?}, offset={:?}, outer_min={:?}, outer_max={:?}, clip={:?}, pass={}",
                scroll.content_size,
                scroll.inner_rect,
                scroll.state.offset,
                ui.min_rect(),
                ui.max_rect(),
                ui.clip_rect(),
                ctx.current_pass_index(),
            );
            ctx.data_mut(|data| {
                data.insert_temp(egui::Id::new("export-test-scroll-observation"), observation);
            });
        }
        let _ = scroll;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if draft.step == ExportStep::Settings {
                review = dialog_button(ui, "Review…");
            } else if draft.step == ExportStep::Review {
                back = dialog_button(ui, "Back");
                ui.add_enabled_ui(
                    !running && (!draft.whole_song || draft.fixed_tail_confirmed),
                    |ui| {
                        destination = dialog_button(ui, "Choose destination and export…");
                    },
                );
            }
            close = dialog_button(ui, "Close");
        });
    });
    #[cfg(test)]
    {
        remember_test_rect(&response.response, "Export modal");
        let observation = format!(
            "modal={:?}, interact={:?}, top={}, pass={}",
            response.response.rect,
            response.response.interact_rect,
            response.is_top_modal,
            ctx.current_pass_index(),
        );
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new("export-test-modal-observation"), observation);
        });
    }
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
pub(in crate::app) mod tests {
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
    pub(in crate::app) fn frame(
        ctx: &egui::Context,
        dialog: &mut ExportDialog,
        events: Vec<egui::Event>,
    ) -> bool {
        frame_at(
            ctx,
            dialog,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 900.0)),
            events,
        )
    }

    fn frame_at(
        ctx: &egui::Context,
        dialog: &mut ExportDialog,
        viewport: egui::Rect,
        events: Vec<egui::Event>,
    ) -> bool {
        let input = egui::RawInput {
            screen_rect: Some(viewport),
            events,
            ..Default::default()
        };
        let mut destination = false;
        let _ = ctx.run_ui(input, |ui| {
            let session = dialog.draft.map_or(1, |draft| draft.project_session);
            destination =
                settings_window(ui.ctx(), dialog, session, "Export review test", false, None);
        });
        destination
    }

    pub(in crate::app) fn click(
        ctx: &egui::Context,
        dialog: &mut ExportDialog,
        label: &'static str,
    ) -> bool {
        // A new App context has pending fonts/zoom, and a new modal first
        // performs an invisible Area sizing pass. Observe fresh geometry on
        // consecutive real frames before sending a pointer to that widget.
        let id = egui::Id::new(("export-test-button", label));
        let observation_id = egui::Id::new(("export-test-widget-observation", label));
        let mut previous = None;
        let mut settled = None;
        let mut observations = Vec::new();
        for attempt in 0..8 {
            ctx.data_mut(|data| {
                data.remove::<egui::Rect>(id);
                data.remove::<String>(observation_id);
                data.remove::<String>(egui::Id::new("export-test-scroll-observation"));
                data.remove::<String>(egui::Id::new("export-test-modal-observation"));
            });
            let pass_before = ctx.cumulative_pass_nr();
            frame(ctx, dialog, Vec::new());
            let (raw, widget, scroll, modal) = ctx.data(|data| {
                (
                    data.get_temp::<egui::Rect>(id),
                    data.get_temp::<String>(observation_id),
                    data.get_temp::<String>(egui::Id::new("export-test-scroll-observation")),
                    data.get_temp::<String>(egui::Id::new("export-test-modal-observation")),
                )
            });
            let current = raw.filter(|rect| rect.is_finite() && rect.is_positive());
            let coordinates = raw.map(|rect| [rect.min.x, rect.min.y, rect.max.x, rect.max.y]);
            observations.push(format!(
                "frame={attempt}, passes={}..{}, time={}, viewport={:?}, zoom={}, ppp={}, raw={raw:?}, coordinates={coordinates:?}, valid={current:?}, widget={widget:?}, scroll={scroll:?}, modal={modal:?}",
                pass_before,
                ctx.cumulative_pass_nr(),
                ctx.input(|input| input.time),
                ctx.content_rect(),
                ctx.zoom_factor(),
                ctx.pixels_per_point(),
            ));
            if current.is_some() && current == previous {
                settled = current;
                break;
            }
            previous = current;
        }
        let rect = settled.unwrap_or_else(|| {
            panic!(
                "export widget {label:?} did not render stable geometry; draft={:?}, viewport={:?}, zoom={}\n{}",
                dialog.draft,
                ctx.content_rect(),
                ctx.zoom_factor(),
                observations.join("\n"),
            )
        });
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
    fn whole_modal_geometry_settles_by_the_second_visible_frame_and_native_stays_compact() {
        for styled in [false, true] {
            let mut modal_heights = Vec::new();
            for whole in [false, true] {
                let ctx = egui::Context::default();
                if styled {
                    crate::theme::install(&ctx);
                }
                let mut dialog = ExportDialog::default();
                if whole {
                    dialog.open_whole_song(1, 48_000, false);
                } else {
                    dialog.open(1, 48_000, false);
                }
                let mut buttons = Vec::new();
                let mut modals = Vec::new();
                for _ in 0..3 {
                    let id = egui::Id::new(("export-test-button", "Review…"));
                    ctx.data_mut(|data| data.remove::<egui::Rect>(id));
                    frame(&ctx, &mut dialog, Vec::new());
                    buttons.push(ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap());
                    modals.push(
                        ctx.data(|data| {
                            data.get_temp::<egui::Rect>(egui::Id::new((
                                "export-test-button",
                                "Export modal",
                            )))
                        })
                        .unwrap(),
                    );
                }
                assert_eq!(buttons[1], buttons[2], "styled={styled}, whole={whole}");
                assert_eq!(modals[1], modals[2], "styled={styled}, whole={whole}");
                assert!(ctx.content_rect().contains_rect(buttons[2]));
                assert!(ctx.content_rect().contains_rect(modals[2]));
                modal_heights.push(modals[2].height());
            }
            assert!(
                modal_heights[0] < modal_heights[1],
                "styled={styled}: {modal_heights:?}"
            );
            assert!(
                modal_heights[0] < 720.0,
                "native dialog filled the viewport budget"
            );
        }
    }

    #[test]
    fn whole_review_checkbox_and_footer_stay_visible_and_use_real_events_in_small_viewport() {
        for styled in [false, true] {
            let ctx = egui::Context::default();
            if styled {
                crate::theme::install(&ctx);
            }
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1080.0, 680.0));
            let mut dialog = ExportDialog::default();
            dialog.open_whole_song(1, 48_000, false);
            let widget_rect = |label| {
                ctx.data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new(("export-test-button", label)))
                })
                .expect("actual widget must have been rendered")
            };
            let click_current = |dialog: &mut ExportDialog, label| {
                let rect = widget_rect(label);
                assert!(viewport.contains_rect(rect), "{label}: {rect:?}");
                let pos = rect.center();
                let pointer = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                frame_at(
                    &ctx,
                    dialog,
                    viewport,
                    vec![egui::Event::PointerMoved(pos), pointer(true)],
                );
                frame_at(
                    &ctx,
                    dialog,
                    viewport,
                    vec![egui::Event::PointerMoved(pos), pointer(false)],
                )
            };
            for _ in 0..3 {
                frame_at(&ctx, &mut dialog, viewport, Vec::new());
            }
            assert!(!click_current(&mut dialog, "Review…"));
            assert_eq!(dialog.draft.unwrap().step, ExportStep::Review);
            let mut checkboxes = Vec::new();
            for _ in 0..3 {
                frame_at(&ctx, &mut dialog, viewport, Vec::new());
                checkboxes.push(widget_rect("Confirm fixed VST tail window"));
            }
            assert_eq!(checkboxes[1], checkboxes[2], "styled={styled}");
            for label in [
                "Confirm fixed VST tail window",
                "Choose destination and export…",
                "Close",
            ] {
                let rect = widget_rect(label);
                assert!(viewport.contains_rect(rect), "{label}: {rect:?}");
                let interact = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new(("export-test-interact", label)))
                    })
                    .unwrap();
                assert!(
                    interact.contains_rect(rect),
                    "{label} is clipped: {interact:?} vs {rect:?}"
                );
            }
            assert!(!click_current(
                &mut dialog,
                "Choose destination and export…"
            ));
            assert!(dialog.begin_destination(1, false).is_none());
            assert!(!dialog.confirmed_whole_fixed_tail());
            assert!(!click_current(&mut dialog, "Confirm fixed VST tail window"));
            assert!(dialog.confirmed_whole_fixed_tail());
            assert!(click_current(&mut dialog, "Choose destination and export…"));
            assert_eq!(
                dialog.begin_destination(1, false),
                Some(WavExportOptions::legacy(48_000))
            );
        }
    }

    #[test]
    fn cold_app_theme_review_uses_real_checkbox_before_destination_admission() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut dialog = ExportDialog::default();
        dialog.open_whole_song(73, 48_000, false);
        assert!(!click(&ctx, &mut dialog, "Review…"));
        assert_eq!(dialog.draft.unwrap().step, ExportStep::Review);
        assert!(!dialog.confirmed_whole_fixed_tail());
        assert!(!click(&ctx, &mut dialog, "Choose destination and export…"));
        assert!(dialog.begin_destination(73, false).is_none());
        assert!(!click(&ctx, &mut dialog, "Confirm fixed VST tail window"));
        assert!(dialog.confirmed_whole_fixed_tail());
        assert!(click(&ctx, &mut dialog, "Choose destination and export…"));
        assert_eq!(
            dialog.begin_destination(73, false),
            Some(WavExportOptions::legacy(48_000))
        );
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
    fn whole_fixed_window_requires_actual_checkbox_and_back_invalidates_previous_consent() {
        let ctx = egui::Context::default();
        let mut dialog = ExportDialog::default();
        dialog.open_whole_song(1, 48_000, false);
        assert!(!click(&ctx, &mut dialog, "Review…"));
        assert!(!click(&ctx, &mut dialog, "Choose destination and export…"));
        assert!(dialog.begin_destination(1, false).is_none());
        assert!(!dialog.confirmed_whole_fixed_tail());
        assert!(!click(&ctx, &mut dialog, "Confirm fixed VST tail window"));
        assert!(dialog.confirmed_whole_fixed_tail());
        assert!(!click(&ctx, &mut dialog, "Back"));
        assert!(!dialog.confirmed_whole_fixed_tail());
        let mut options = WavExportOptions::legacy(48_000);
        options.tail_milliseconds = 3_000;
        dialog.test_set_options(options);
        assert!(!click(&ctx, &mut dialog, "Review…"));
        assert!(!click(&ctx, &mut dialog, "Choose destination and export…"));
        assert!(dialog.begin_destination(1, false).is_none());
        assert!(!click(&ctx, &mut dialog, "Confirm fixed VST tail window"));
        assert!(click(&ctx, &mut dialog, "Choose destination and export…"));
        assert_eq!(dialog.begin_destination(1, false), Some(options));
        assert!(dialog.confirmed_whole_fixed_tail());
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
