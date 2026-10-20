//! Native sampler editor. Decode and peak extraction run off the UI thread; drafts never
//! mutate Project until a complete gesture can be committed as one history transaction.
use super::*;
use crate::sampler::SamplerConfig;

// Native sample voices receive channel gain from their compiled base descriptor.
// The legacy oscillator keeps its established caller-applied gain convention.
pub(super) fn step_note(channel: &Channel, legacy_note: u8) -> u8 {
    channel
        .sampler
        .as_ref()
        .map_or(legacy_note, |s| s.root_note)
}
pub(super) fn note_velocity(channel: &Channel, velocity: f32, channel_gain: f32) -> f32 {
    if channel.sampler.is_some() {
        velocity
    } else {
        velocity * channel_gain
    }
}

#[derive(Default)]
pub(super) struct SamplerEditor {
    channel: Option<u32>,
    session: u64,
    draft: Option<SamplerDraft>,
    import: Option<SamplerImport>,
    waveform: Option<SamplerWaveformPreview>,
    notice: Option<String>,
    sealed_until_release: bool,
    #[cfg(test)]
    drop_rect: Option<Rect>,
}
struct SamplerDraft {
    session: u64,
    generation: u64,
    channel: u32,
    before: SamplerConfig,
    value: SamplerConfig,
}
type DecodedSamplerImport = Result<(PathBuf, wav::WavAsset, Vec<f32>), String>;

struct SamplerImport {
    session: u64,
    generation: u64,
    channel: u32,
    before: Option<SamplerConfig>,
    receiver: Receiver<DecodedSamplerImport>,
}

#[derive(Clone, PartialEq, Eq)]
struct SamplerWaveformKey {
    session: u64,
    generation: u64,
    channel: u32,
    media: crate::project_media::MediaIdentity,
}
struct SamplerWaveformPreview {
    key: SamplerWaveformKey,
    receiver: Option<Receiver<Result<Vec<f32>, String>>>,
    result: Option<Result<Vec<f32>, String>>,
}

impl CitrusApp {
    pub(super) fn new_sampler_channel(&mut self) {
        self.seal_sampler_for_action();
        if !self.sampler_mutation_available() {
            return;
        }
        let Some(id) = self
            .project
            .channels
            .iter()
            .map(|c| c.id)
            .chain(
                self.project
                    .patterns
                    .iter()
                    .flat_map(|p| p.notes.iter().filter_map(|n| n.channel_id)),
            )
            .max()
            .unwrap_or(0)
            .checked_add(1)
        else {
            self.notify("No Channel IDs remain".into());
            return;
        };
        let Some(mixer_track) = self.project.mixer_track_id_at_runtime_slot(0) else {
            self.notify("Master mixer destination is unavailable".into());
            return;
        };
        let mut candidate = self.project.clone();
        candidate.channels.push(Channel {
            id,
            name: format!("Sampler {id}"),
            color: [255, 160, 80],
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            mixer_track,
            instrument_plugin_instance_id: None,
            sampler: Some(SamplerConfig::empty()),
            steps: [false; 16],
        });
        for pattern in &mut candidate.patterns {
            pattern
                .channel_steps
                .resize(candidate.channels.len(), [false; 16]);
        }
        self.commit_sampler_project(candidate);
        self.selected_channel = self.project.channels.len() - 1;
        self.piano_roll_state.clear_selection();
        self.sync_piano_range_owner();
        self.open_sampler(id);
    }

    #[cfg(test)]
    pub(super) fn sampler_drop_rect(&self) -> Option<Rect> {
        self.sampler_editor.drop_rect
    }

    #[cfg(test)]
    pub(super) fn sampler_waveform_preview(&self) -> Option<Result<&[f32], &str>> {
        self.sampler_editor
            .waveform
            .as_ref()?
            .result
            .as_ref()
            .map(|result| result.as_ref().map(Vec::as_slice).map_err(String::as_str))
    }

    pub(super) fn open_sampler(&mut self, channel: u32) {
        self.finish_sampler_gesture();
        self.sampler_editor.channel = Some(channel);
        self.sampler_editor.session = self.project_session;
        self.sampler_editor.notice = None;
    }

    fn sampler_mutation_available(&self) -> bool {
        self.project_media_actions_available()
            && self.top_shortcut_modal().is_none()
            && !self.shortcut_blocking_layer_active()
            && !self.project_snapshot_transition_pending()
            && !self.project_media.open
            && self.piano_note_properties.is_none()
            && self.playlist_gesture_before.is_none()
            && self.piano_roll_gesture_before.is_none()
            && !self.plugin_topology_is_settling()
    }

    fn commit_sampler_project(&mut self, candidate: Project) {
        self.cancel_all_keyboard_auditions();
        if commit_explicit_project_history_transaction(
            &mut self.project,
            &mut self.history_snapshot,
            &mut self.history_fingerprint,
            &mut self.undo_stack,
            &mut self.redo_stack,
            &mut self.dirty,
            candidate,
        ) {
            self.sync_history_observer();
            self.refresh_timeline_fingerprint(Instant::now(), true);
        }
    }

    pub(super) fn finish_sampler_gesture(&mut self) {
        let Some(draft) = self.sampler_editor.draft.take() else {
            return;
        };
        if draft.session != self.project_session || draft.generation != self.audio_asset_generation
        {
            return;
        }
        let Some(index) = self
            .project
            .channels
            .iter()
            .position(|c| c.id == draft.channel)
        else {
            return;
        };
        if self.project.channels[index].sampler.as_ref() != Some(&draft.before)
            || self.project.channels[index]
                .instrument_plugin_instance_id
                .is_some()
        {
            self.sampler_editor.notice =
                Some("Sampler changed elsewhere; the unfinished edit was cancelled.".into());
            return;
        }
        // Lifecycle entrypoints seal the current draft before capturing a snapshot. If another
        // transaction already owns the Project, do not write through its snapshot.
        if !self.sampler_mutation_available() {
            self.sampler_editor.notice =
                Some("Sampler edit cancelled because a project operation is active.".into());
            return;
        }
        let mut candidate = self.project.clone();
        candidate.channels[index].sampler = Some(draft.value);
        self.commit_sampler_project(candidate);
    }

    pub(super) fn sampler_gesture_active(&self) -> bool {
        self.sampler_editor.draft.is_some()
    }

    pub(super) fn cancel_sampler_gesture(&mut self) {
        self.sampler_editor.sealed_until_release |= self.sampler_editor.draft.take().is_some();
    }

    pub(super) fn seal_sampler_for_action(&mut self) {
        let had_draft = self.sampler_editor.draft.is_some();
        self.finish_sampler_gesture();
        self.sampler_editor.sealed_until_release |= had_draft;
    }

    pub(super) fn sampler_frame_start(&mut self, ctx: &egui::Context) {
        if self.sampler_editor.session != self.project_session {
            self.sampler_editor = SamplerEditor::default();
            return;
        }
        if ctx.input(|i| !i.pointer.any_down()) {
            self.sampler_editor.sealed_until_release = false;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.sampler_editor.draft = None;
            self.sampler_editor.sealed_until_release = true;
        } else if ctx.input(|i| !i.focused) {
            self.finish_sampler_gesture();
        }
        self.poll_sampler_import();
        self.drive_sampler_waveform();
        if self.sampler_editor.import.is_some()
            || self
                .sampler_editor
                .waveform
                .as_ref()
                .is_some_and(|preview| preview.receiver.is_some())
        {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    // Waveform visualization is independent of audio-device residency. The only
    // payload returned by this worker is a tiny, transient peak vector; PCM never
    // enters the UI thread, callback queues, saved Project, or history snapshots.
    fn drive_sampler_waveform(&mut self) {
        let source = self.sampler_editor.channel.and_then(|channel| {
            let sampler = self
                .project
                .channels
                .iter()
                .find(|c| c.id == channel)?
                .sampler?;
            let asset_id = sampler.asset_id?;
            let asset = self
                .project
                .audio_assets
                .iter()
                .find(|a| a.id == asset_id)?;
            asset.waveform_peaks.is_empty().then(|| {
                (
                    SamplerWaveformKey {
                        session: self.project_session,
                        generation: self.audio_asset_generation,
                        channel,
                        media: crate::project_media::MediaIdentity::from(asset),
                    },
                    asset.clone(),
                )
            })
        });
        let Some((key, asset)) = source else {
            self.sampler_editor.waveform = None;
            return;
        };
        if self
            .sampler_editor
            .waveform
            .as_ref()
            .is_none_or(|preview| preview.key != key)
        {
            let (sender, receiver) = mpsc::channel();
            let started = std::thread::Builder::new().name("sampler-waveform".into()).spawn(move || {
                let result = wav::read_wav(&asset.path).map_err(|error| format!("{error:#}"))
                    .and_then(|decoded| {
                        if decoded.metadata.sample_rate != asset.sample_rate
                            || decoded.metadata.channels != asset.channels
                            || decoded.metadata.frames != asset.frames {
                            return Err("Source properties changed. Restore the original WAV or explicitly Load WAV to replace it.".into());
                        }
                        Ok(build_waveform_peaks(&decoded.samples, decoded.metadata.channels, 256))
                    });
                let _ = sender.send(result);
            });
            self.sampler_editor.waveform = Some(SamplerWaveformPreview {
                key,
                receiver: started.as_ref().ok().map(|_| receiver),
                result: started
                    .err()
                    .map(|error| Err(format!("Cannot start waveform loader: {error}"))),
            });
        }
        let Some(preview) = self.sampler_editor.waveform.as_mut() else {
            return;
        };
        let Some(receiver) = &preview.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                preview.result = Some(result);
                preview.receiver = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                preview.result = Some(Err("Waveform loader stopped before completing.".into()));
                preview.receiver = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn start_sampler_import(&mut self, channel: u32, path: PathBuf) {
        if !self.sampler_mutation_available() || self.sampler_editor.import.is_some() {
            return;
        }
        let Some(target) = self.project.channels.iter().find(|c| c.id == channel) else {
            return;
        };
        if target.instrument_plugin_instance_id.is_some() {
            self.sampler_editor.notice = Some(
                "Unload this Channel's VST first, or select a native Channel, then load a sample."
                    .into(),
            );
            return;
        }
        let before = target.sampler;
        let session = self.project_session;
        let generation = self.audio_asset_generation;
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("sampler-import".into())
            .spawn(move || {
                let result = audio_import::decode_audio_import(generation, path, 0.0, 0)
                    .map(|(_, path, asset, _, _)| {
                        let peaks =
                            build_waveform_peaks(&asset.samples, asset.metadata.channels, 256);
                        (path, asset, peaks)
                    })
                    .map_err(|(_, error)| error);
                let _ = sender.send(result);
            }) {
            Ok(_) => {
                self.sampler_editor.import = Some(SamplerImport {
                    session,
                    generation,
                    channel,
                    before,
                    receiver,
                });
                self.sampler_editor.notice = Some("Loading WAV in the background…".into());
            }
            Err(error) => self.sampler_editor.notice = Some(format!("Cannot load WAV: {error}")),
        }
    }

    fn poll_sampler_import(&mut self) {
        if !self.sampler_mutation_available() || self.sampler_editor.draft.is_some() {
            return;
        }
        let Some(pending) = self.sampler_editor.import.as_ref() else {
            return;
        };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("WAV worker stopped before completing.".into())
            }
        };
        let pending = self.sampler_editor.import.take().unwrap();
        let current = self
            .project
            .channels
            .iter()
            .find(|c| c.id == pending.channel);
        if pending.session != self.project_session
            || pending.generation != self.audio_asset_generation
            || !current.is_some_and(|c| {
                c.sampler == pending.before && c.instrument_plugin_instance_id.is_none()
            })
        {
            self.sampler_editor.notice = Some(
                "Import cancelled: the project or target Channel changed. Load the WAV again."
                    .into(),
            );
            return;
        }
        match result {
            Err(error) => self.sampler_editor.notice = Some(format!("WAV import failed: {error}")),
            Ok((path, asset, peaks)) => {
                let Some(id) = self
                    .project
                    .audio_assets
                    .iter()
                    .map(|a| a.id)
                    .chain(self.retained_clip_media.keys().copied())
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                else {
                    self.sampler_editor.notice = Some("No audio asset IDs remain.".into());
                    return;
                };
                let mut candidate = self.project.clone();
                let metadata = &asset.metadata;
                if metadata.frames == 0 {
                    self.sampler_editor.notice = Some("The WAV contains no sample frames.".into());
                    return;
                }
                candidate.audio_assets.push(AudioAsset {
                    id,
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    path,
                    sample_rate: metadata.sample_rate,
                    channels: metadata.channels,
                    bits_per_sample: metadata.bits_per_sample,
                    frames: metadata.frames,
                    waveform_peaks: peaks.clone(),
                });
                candidate
                    .channels
                    .iter_mut()
                    .find(|c| c.id == pending.channel)
                    .unwrap()
                    .sampler = Some(SamplerConfig::new(id));
                self.commit_sampler_project(candidate);
                self.register_audio_asset_with_peaks(id, asset, peaks);
                self.sampler_editor.notice =
                    Some("Sample loaded. Play notes in Piano Roll or click Preview.".into());
            }
        }
    }

    pub(super) fn sampler_window(&mut self, ctx: &egui::Context) {
        // Foreground instrument controls must yield to the same dialog owners as
        // editor shortcuts. This includes Export, Settings, MIDI, and plug-in dialogs.
        if self.top_shortcut_modal().is_some() || self.shortcut_blocking_layer_active() {
            self.finish_sampler_gesture();
            return;
        }
        let Some(channel_id) = self.sampler_editor.channel else {
            return;
        };
        let Some(channel) = self
            .project
            .channels
            .iter()
            .find(|c| c.id == channel_id)
            .cloned()
        else {
            self.sampler_editor.channel = None;
            return;
        };
        let mut open = true;
        let mut load = false;
        let mut remove = false;
        let mut unload = false;
        let mut preview = false;
        let mut drop_path = None;
        let available = self.sampler_mutation_available()
            && self.sampler_editor.import.is_none()
            && !self.sampler_editor.sealed_until_release;
        let mut config = self
            .sampler_editor
            .draft
            .as_ref()
            .map(|d| d.value)
            .or(channel.sampler);
        let initial = config;
        egui::Window::new("Sampler")
            // Instrument controls must stay above workspace editors, including their
            // multi-frame stacking restoration after a narrow viewport resize.
            .order(egui::Order::Foreground)
            .id(Id::new("native-sampler-editor")).open(&mut open)
            .default_width(390.0_f32.min(ctx.content_rect().width() - 24.0)).min_width(260.0)
            .default_height((ctx.content_rect().height() - 48.0).clamp(120.0, 560.0))
            .max_height((ctx.content_rect().height() - 48.0).max(120.0)).vscroll(true).resizable(true).show(ctx, |ui| {
                ui.add(egui::Label::new(RichText::new(&channel.name).strong()).truncate());
                ui.horizontal_wrapped(|ui| {
                    load = ui.add_enabled(available, egui::Button::new("Load WAV…")).clicked();
                    remove = ui.add_enabled(available && config.is_some_and(|config| config.asset_id.is_some()), egui::Button::new("Remove sample")).on_hover_text("Clear the WAV and leave this Sampler silent").clicked();
                    unload = ui.add_enabled(available && config.is_some(), egui::Button::new("Use native synth")).on_hover_text("Unload Sampler and restore the native synthesizer. This is undoable; VST instruments can then be assigned.").clicked();
                    if self.sampler_editor.import.is_some() { ui.spinner(); }
                });
                let (drop_rect, drop_response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::hover());
                #[cfg(test)] { self.sampler_editor.drop_rect = Some(drop_rect); }
                ui.painter().rect_filled(drop_rect, 4.0, theme::PANEL_ALT);
                ui.painter().text(drop_rect.center(), egui::Align2::CENTER_CENTER,
                    "Drop one WAV here → this Channel", egui::FontId::proportional(12.0), theme::TEXT);
                // File drops are consumed only by this explicit target, never Playlist.
                if available && drop_response.contains_pointer() {
                    let files = ctx.input(|i| i.raw.dropped_files.clone());
                    if !files.is_empty() { ctx.input_mut(|i| i.raw.dropped_files.clear()); }
                    if files.len() == 1 {
                        drop_path = files[0].path.clone();
                        if drop_path.is_none() { self.sampler_editor.notice = Some("This drop has no local WAV path. Use Load WAV instead.".into()); }
                    }
                    else if files.len() > 1 { self.sampler_editor.notice = Some("Drop one WAV at a time.".into()); }
                }
                if channel.instrument_plugin_instance_id.is_some() {
                    ui.colored_label(theme::AMBER, "VST assigned. Unload it first or select a native Channel.");
                }
                if let Some(notice) = &self.sampler_editor.notice { ui.label(notice); }
                if let Some(value) = config.as_mut() {
                    let asset = value.asset_id.and_then(|asset_id| self.project.audio_assets.iter().find(|a| a.id == asset_id));
                    if let Some(asset) = asset {
                        ui.add(egui::Label::new(&asset.name).truncate()).on_hover_text(asset.path.to_string_lossy());
                        ui.small(format!("{:.2} s · {} Hz · {} ch", asset.frames as f64 / asset.sample_rate.max(1) as f64, asset.sample_rate, asset.channels));
                        let preview = self.sampler_editor.waveform.as_ref().filter(|preview|
                            preview.key.session == self.project_session
                                && preview.key.generation == self.audio_asset_generation
                                && preview.key.channel == channel_id
                                && preview.key.media == crate::project_media::MediaIdentity::from(asset));
                        let peaks = preview.and_then(|preview| preview.result.as_ref())
                            .and_then(|result| result.as_ref().ok()).map(Vec::as_slice)
                            .unwrap_or(&asset.waveform_peaks);
                        draw_waveform(ui, asset, value, peaks);
                        if let Some(preview) = preview {
                            if preview.receiver.is_some() { ui.small("Loading waveform…"); }
                            if let Some(Err(error)) = &preview.result {
                                ui.colored_label(theme::RED, "Waveform unavailable. Restore/relink the sample or Load WAV.").on_hover_text(error);
                                if ui.small_button("Retry waveform").clicked() { self.sampler_editor.waveform = None; }
                            }
                        }
                        if let Some(failure) = self.failed_audio_assets.get(&asset.id) {
                            ui.colored_label(theme::RED, "Sample unavailable. Use Project media / relink or Load WAV.").on_hover_text(&failure.message);
                        }
                        ui.add_enabled_ui(available, |ui| { sampler_controls(ui, value, asset.frames); });
                    } else if value.asset_id.is_some() {
                        ui.colored_label(theme::RED, "Missing sample reference. Load WAV to replace it.");
                    } else {
                        ui.label("No sample loaded. This Sampler stays silent until you Load WAV.");
                    }
                    let ready = self.audio.is_some() && self.timeline_audio_matches_desired() && asset.is_some_and(|a| self.loaded_audio_assets.get(&a.id) == Some(&a.id) && !self.failed_audio_assets.contains_key(&a.id));
                    ui.horizontal_wrapped(|ui| {
                        preview = ui.add_enabled(ready && available, egui::Button::new("Preview (short note)")).clicked();
                        if self.audio.is_none() { ui.small("Audio offline"); }
                        else if value.asset_id.is_none() { ui.small("No sample loaded"); }
                        else if !ready { ui.small("Sample not ready"); }
                    });
                    ui.small("Play melodies in Piano Roll. Edits apply when released; Esc cancels a drag.");
                } else { ui.label("Native synth selected. Load a WAV to switch this Channel to Sampler."); }
            });
        if config != initial
            && let (Some(before), Some(value)) = (channel.sampler, config)
        {
            let draft = self.sampler_editor.draft.get_or_insert(SamplerDraft {
                session: self.project_session,
                generation: self.audio_asset_generation,
                channel: channel_id,
                before,
                value,
            });
            draft.value = value;
        }
        if !ctx.input(|i| i.pointer.any_down()) || !open {
            self.finish_sampler_gesture();
        }
        if remove || unload {
            self.sampler_editor.draft = None;
            let mut candidate = self.project.clone();
            candidate
                .channels
                .iter_mut()
                .find(|c| c.id == channel_id)
                .unwrap()
                .sampler = if unload {
                None
            } else {
                Some(SamplerConfig::empty())
            };
            self.commit_sampler_project(candidate);
        }
        if preview && let Some(slot) = self.project.mixer_runtime_slot(channel.mixer_track) {
            let note = channel.sampler.as_ref().map_or(60, |s| s.root_note);
            self.trigger_channel_audition_note(channel.id, note, 1.0, 1.0, slot);
        }
        if load {
            drop_path = rfd::FileDialog::new()
                .add_filter("WAV audio", &["wav", "wave"])
                .pick_file();
        }
        if let Some(path) = drop_path {
            self.start_sampler_import(channel_id, path);
        }
        if !open {
            self.sampler_editor.channel = None;
        }
    }
}

fn sampler_controls(ui: &mut egui::Ui, value: &mut SamplerConfig, frames: u64) {
    let frames = frames.max(1);
    ui.horizontal_wrapped(|ui| {
        ui.label("Root note");
        ui.add(egui::DragValue::new(&mut value.root_note).range(0..=127));
        ui.label(format!(
            "{}{}",
            pitch_class_label(value.root_note % 12),
            i16::from(value.root_note) / 12 - 1
        ));
    });
    // Displaying a saved configuration must never normalize or clamp it.
    let mut end = value.end_frame.unwrap_or(frames).clamp(1, frames);
    let mut start = value.start_frame.min(end - 1);
    let start_changed = ui
        .add(
            egui::Slider::new(&mut start, 0..=end - 1)
                .clamping(egui::SliderClamping::Edits)
                .text("Start frame"),
        )
        .changed();
    let end_changed = ui
        .add(
            egui::Slider::new(&mut end, start + 1..=frames)
                .clamping(egui::SliderClamping::Edits)
                .text("End frame"),
        )
        .changed();
    if start_changed {
        value.start_frame = start;
    }
    if end_changed {
        value.end_frame = if end == frames { None } else { Some(end) };
    }
    let mut looping = value.loop_start_frame.is_some() && value.loop_end_frame.is_some();
    let loop_changed = ui.checkbox(&mut looping, "Loop while held").changed();
    if looping {
        let mut loop_start = value
            .loop_start_frame
            .unwrap_or(start)
            .clamp(start, end - 1);
        let mut loop_end = value
            .loop_end_frame
            .unwrap_or(end)
            .clamp(loop_start + 1, end);
        let loop_start_changed = ui
            .add(
                egui::Slider::new(&mut loop_start, start..=loop_end - 1)
                    .clamping(egui::SliderClamping::Edits)
                    .text("Loop start"),
            )
            .changed();
        let loop_end_changed = ui
            .add(
                egui::Slider::new(&mut loop_end, loop_start + 1..=end)
                    .clamping(egui::SliderClamping::Edits)
                    .text("Loop end"),
            )
            .changed();
        if loop_changed || start_changed || end_changed || loop_start_changed || loop_end_changed {
            value.loop_start_frame = Some(loop_start);
            value.loop_end_frame = Some(loop_end);
        }
    } else if loop_changed {
        value.loop_start_frame = None;
        value.loop_end_frame = None;
    }
    ui.separator();
    for (seconds, label) in [
        (&mut value.attack_seconds, "Attack · s"),
        (&mut value.decay_seconds, "Decay · s"),
    ] {
        ui.add(
            egui::Slider::new(seconds, 0.0..=60.0)
                .logarithmic(true)
                .smallest_positive(0.001)
                .clamping(egui::SliderClamping::Edits)
                .text(label),
        );
    }
    ui.add(
        egui::Slider::new(&mut value.sustain, 0.0..=1.0)
            .clamping(egui::SliderClamping::Edits)
            .text("Sustain"),
    );
    ui.add(
        egui::Slider::new(&mut value.release_seconds, 0.0..=60.0)
            .logarithmic(true)
            .smallest_positive(0.001)
            .clamping(egui::SliderClamping::Edits)
            .text("Release · s"),
    );
}

fn draw_waveform(ui: &mut egui::Ui, asset: &AudioAsset, config: &SamplerConfig, peaks: &[f32]) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 76.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, theme::PANEL_ALT);
    let x = |frame: u64| {
        rect.left()
            + rect.width() * (frame as f64 / asset.frames.max(1) as f64).clamp(0.0, 1.0) as f32
    };
    let selected = Rect::from_min_max(
        Pos2::new(x(config.start_frame), rect.top()),
        Pos2::new(x(config.end_frame.unwrap_or(asset.frames)), rect.bottom()),
    );
    painter.rect_filled(
        selected,
        0.0,
        Color32::from_rgba_unmultiplied(255, 160, 40, 30),
    );
    for (index, peak) in peaks.iter().enumerate() {
        let px = rect.left() + (index as f32 + 0.5) / peaks.len().max(1) as f32 * rect.width();
        let height = peak.abs().min(1.0) * rect.height() * 0.45;
        painter.line_segment(
            [
                Pos2::new(px, rect.center().y - height),
                Pos2::new(px, rect.center().y + height),
            ],
            Stroke::new(1.0, theme::ORANGE),
        );
    }
    for frame in [config.loop_start_frame, config.loop_end_frame]
        .into_iter()
        .flatten()
    {
        painter.line_segment(
            [
                Pos2::new(x(frame), rect.top()),
                Pos2::new(x(frame), rect.bottom()),
            ],
            Stroke::new(2.0, theme::BLUE),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Box<CitrusApp> {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        app.project.channels[0].instrument_plugin_instance_id = None;
        app.sync_history_observer();
        app.undo_stack.clear();
        app
    }
    fn wav_fixture() -> wav::WavAsset {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&8000_u32.to_le_bytes());
        bytes.extend_from_slice(&16000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 64]);
        wav::decode_wav(&bytes).unwrap()
    }
    fn queued(app: &mut CitrusApp) {
        let (sender, receiver) = mpsc::channel();
        let channel = app.project.channels[0].id;
        app.open_sampler(channel);
        app.sampler_editor.import = Some(SamplerImport {
            channel,
            session: app.project_session,
            generation: app.audio_asset_generation,
            before: app.project.channels[0].sampler,
            receiver,
        });
        sender
            .send(Ok((
                PathBuf::from("melody.wav"),
                wav_fixture(),
                vec![0.0, 0.5],
            )))
            .unwrap();
    }
    #[test]
    fn sampler_import_is_one_undo_without_playlist_mutation() {
        let mut app = app();
        let clips = app.project.clips.clone();
        let before = project_fingerprint(&app.project);
        queued(&mut app);
        app.poll_sampler_import();
        assert!(app.project.channels[0].sampler.is_some());
        assert_eq!(
            serde_json::to_value(&app.project.clips).unwrap(),
            serde_json::to_value(&clips).unwrap()
        );
        assert_eq!(app.undo_stack.len(), 1);
        app.undo();
        assert_eq!(project_fingerprint(&app.project), before);
    }
    #[test]
    fn sampler_import_waits_for_snapshot_owner_and_rejects_deleted_or_changed_target() {
        let mut app = app();
        queued(&mut app);
        app.project_media.open = true;
        app.poll_sampler_import();
        assert!(app.sampler_editor.import.is_some());
        assert!(app.project.channels[0].sampler.is_none());
        app.project_media.open = false;
        app.project.channels.remove(0);
        let before = project_fingerprint(&app.project);
        app.poll_sampler_import();
        assert_eq!(project_fingerprint(&app.project), before);
        assert!(app.sampler_editor.import.is_none());
    }
    #[test]
    fn sampler_import_rejects_old_generation_and_session() {
        for session_change in [false, true] {
            let mut app = app();
            queued(&mut app);
            if session_change {
                app.project_session += 1;
            } else {
                app.audio_asset_generation += 1;
            }
            let before = project_fingerprint(&app.project);
            app.poll_sampler_import();
            assert_eq!(project_fingerprint(&app.project), before);
            assert!(app.undo_stack.is_empty());
        }
    }
    #[test]
    fn sampler_import_decode_failure_keeps_project_and_reports_error() {
        let mut app = app();
        let (sender, receiver) = mpsc::channel();
        app.sampler_editor.import = Some(SamplerImport {
            channel: app.project.channels[0].id,
            session: app.project_session,
            generation: app.audio_asset_generation,
            before: None,
            receiver,
        });
        sender.send(Err("Not a WAV file".into())).unwrap();
        let before = project_fingerprint(&app.project);
        app.poll_sampler_import();
        assert_eq!(project_fingerprint(&app.project), before);
        assert!(
            app.sampler_editor
                .notice
                .as_ref()
                .unwrap()
                .contains("Not a WAV")
        );
    }
    #[test]
    fn stale_sampler_draft_cannot_overwrite_undo_or_new_project() {
        let mut app = app();
        app.project.channels[0].sampler = Some(SamplerConfig::new(1));
        let before = app.project.channels[0].sampler.unwrap();
        let mut value = before;
        value.root_note = 72;
        app.sampler_editor.draft = Some(SamplerDraft {
            session: app.project_session,
            generation: app.audio_asset_generation,
            channel: app.project.channels[0].id,
            before,
            value,
        });
        app.audio_asset_generation += 1;
        app.finish_sampler_gesture();
        assert_eq!(app.project.channels[0].sampler, Some(before));
        assert!(app.undo_stack.is_empty());
    }
    #[test]
    fn sampler_restore_rejects_changed_source_properties_without_rewriting_metadata() {
        for mismatch in 0..3 {
            let mut app = app();
            queued(&mut app);
            app.poll_sampler_import();
            let id = app.project.channels[0].sampler.unwrap().asset_id.unwrap();
            let before = project_fingerprint(&app.project);
            let mut decoded = wav_fixture();
            match mismatch {
                0 => decoded.metadata.sample_rate *= 2,
                1 => decoded.metadata.channels = 2,
                _ => decoded.metadata.frames += 1,
            }
            app.register_audio_asset_with_peaks(id, decoded, vec![0.1]);
            assert_eq!(project_fingerprint(&app.project), before);
            assert!(
                app.failed_audio_assets
                    .get(&id)
                    .unwrap()
                    .message
                    .contains("Sampler source properties changed")
            );
        }
    }

    #[test]
    fn sampler_step_pitch_and_velocity_match_timeline_without_double_gain() {
        let mut channel = Project::default().channels.remove(0);
        channel.volume = 0.25;
        assert_eq!(step_note(&channel, 36), 36);
        assert_eq!(note_velocity(&channel, 0.8, channel.volume), 0.2);
        let mut config = SamplerConfig::new(1);
        config.root_note = 67;
        channel.sampler = Some(config);
        assert_eq!(step_note(&channel, 36), 67);
        assert_eq!(note_velocity(&channel, 0.8, channel.volume), 0.8);
    }
    #[test]
    fn sampler_waveform_completion_cannot_cross_session_generation_asset_or_channel_changes() {
        for mutation in 0..4 {
            let mut app = app();
            queued(&mut app);
            app.poll_sampler_import();
            let channel = app.project.channels[0].id;
            let asset_id = app.project.channels[0].sampler.unwrap().asset_id.unwrap();
            let asset = app
                .project
                .audio_assets
                .iter_mut()
                .find(|a| a.id == asset_id)
                .unwrap();
            asset.waveform_peaks.clear();
            let key = SamplerWaveformKey {
                session: app.project_session,
                generation: app.audio_asset_generation,
                channel,
                media: crate::project_media::MediaIdentity::from(&*asset),
            };
            let (sender, receiver) = mpsc::channel();
            sender.send(Ok(vec![0.9876])).unwrap();
            app.sampler_editor.waveform = Some(SamplerWaveformPreview {
                key,
                receiver: Some(receiver),
                result: None,
            });
            match mutation {
                0 => app.project_session += 1,
                1 => app.audio_asset_generation += 1,
                2 => {
                    app.project
                        .audio_assets
                        .iter_mut()
                        .find(|a| a.id == asset_id)
                        .unwrap()
                        .path = PathBuf::from("other-source.wav")
                }
                _ => {
                    app.project.channels.remove(0);
                }
            }
            let before = project_fingerprint(&app.project);
            app.drive_sampler_waveform();
            assert_ne!(
                app.sampler_waveform_preview(),
                Some(Ok([0.9876].as_slice()))
            );
            assert_eq!(project_fingerprint(&app.project), before);
        }
    }
    #[test]
    fn sampler_loading_undo_returns_to_empty_sampler_and_does_not_restore_oscillator() {
        let mut app = app();
        app.project.channels[0].sampler = Some(SamplerConfig::empty());
        app.sync_history_observer();
        queued(&mut app);
        app.poll_sampler_import();
        assert!(app.project.channels[0].sampler.unwrap().asset_id.is_some());
        app.undo();
        assert_eq!(
            app.project.channels[0].sampler,
            Some(SamplerConfig::empty())
        );
        app.drive_sampler_waveform();
        assert!(app.sampler_editor.waveform.is_none());
    }

    #[test]
    fn sampler_failed_load_leaves_empty_sampler_silent_without_waveform_worker() {
        let mut app = app();
        app.project.channels[0].sampler = Some(SamplerConfig::empty());
        app.open_sampler(app.project.channels[0].id);
        let (sender, receiver) = mpsc::channel();
        app.sampler_editor.import = Some(SamplerImport {
            channel: app.project.channels[0].id,
            session: app.project_session,
            generation: app.audio_asset_generation,
            before: Some(SamplerConfig::empty()),
            receiver,
        });
        sender.send(Err("Missing WAV".into())).unwrap();
        let before = project_fingerprint(&app.project);
        app.poll_sampler_import();
        app.drive_sampler_waveform();
        assert_eq!(project_fingerprint(&app.project), before);
        assert_eq!(
            app.project.channels[0].sampler,
            Some(SamplerConfig::empty())
        );
        assert!(app.sampler_editor.waveform.is_none());
        assert!(
            app.sampler_editor
                .notice
                .as_ref()
                .unwrap()
                .contains("Missing WAV")
        );
    }
}
