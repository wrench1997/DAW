//! Narrow App entry, source-restoration and mutation fences for selected conversion.
use super::*;
use crate::instrument_bounce::BouncePluginMetadata;
use crate::plugins::plugin_runtime::{
    PluginChain, RetainedShutdownPoll, SourceRestorationExpectation,
};

#[cfg(test)]
pub(super) type RestorationTestFactory = fn(
    &Project,
    u64,
    PluginLoadSpec,
    crate::plugins::plugin_runtime::PluginPrepareConfig,
    &BouncePluginMetadata,
) -> Result<PluginChain, String>;

#[derive(Clone)]
pub(super) struct ClipRenderRestorationBinding {
    project_session: u64,
    instance_id: u64,
    descriptor_id: String,
    module_path: PathBuf,
    metadata: BouncePluginMetadata,
    fresh_load: bool,
}

impl ClipRenderRestorationBinding {
    pub(super) fn project_session(&self) -> u64 {
        self.project_session
    }
    pub(super) fn matches_playback_instance(
        &self,
        session: u64,
        instance: &PluginInstance,
    ) -> bool {
        self.project_session == session
            && self.instance_id == instance.id
            && self.descriptor_id == instance.uid
            && self.module_path == instance.path
            && instance.format == ProjectPluginFormat::Vst3
            && instance.role == PluginRole::Instrument
    }
}

pub(super) struct PendingClipRestoration {
    project_session: u64,
    instance_id: u64,
    engine_session: u64,
    live_snapshot: Arc<[u8]>,
    live_revision: u64,
    candidate: Option<Project>,
    previous: Option<ChannelGeneratorState>,
    chain: Option<PluginChain>,
    transferred_control: Option<PluginChainControl>,
    transferred_guard: Option<PluginWorkerGuard>,
    expected_timing: (u32, u32),
    ready: bool,
    started_at: Instant,
}

impl CitrusApp {
    pub(super) fn advance_clip_model_revision(&mut self) {
        if self.clip_render_model_revision != 0 {
            self.clip_render_model_revision =
                self.clip_render_model_revision.checked_add(1).unwrap_or(0);
        }
    }
    pub(super) fn advance_clip_control_revision(&mut self) {
        if self.clip_render_control_revision != 0 {
            self.clip_render_control_revision = self
                .clip_render_control_revision
                .checked_add(1)
                .unwrap_or(0);
        }
    }
    pub(super) fn clip_render_reject_mutation(&mut self) -> bool {
        if self.clip_operation_fence_active() {
            self.notify("Wait for the selected-clip operation to finish, or cancel it".into());
            true
        } else {
            self.advance_clip_model_revision();
            false
        }
    }
    pub(super) fn clip_render_reject_control(&mut self) -> bool {
        if self.clip_operation_fence_active() {
            self.notify(
                "Source controls are held while the selected-clip operation finishes".into(),
            );
            true
        } else {
            self.advance_clip_control_revision();
            false
        }
    }
    pub(super) fn commit_plugin_with_reserved_source_ids(
        &self,
        candidate: &mut Project,
        target: PluginPickerTarget,
        descriptor: &PluginDescriptor,
    ) -> Result<u64, String> {
        let id = commit_loaded_plugin(candidate, target, descriptor)?;
        if !self.fresh_bounce_sources.contains_key(&id)
            && !self.clip_render_restorations.contains_key(&id)
        {
            return Ok(id);
        }
        let highest = candidate
            .plugin_instances
            .iter()
            .map(|item| item.id)
            .chain(self.fresh_bounce_sources.keys().copied())
            .chain(self.clip_render_restorations.keys().copied())
            .max()
            .unwrap_or(0);
        let replacement = highest
            .checked_add(1)
            .filter(|value| *value != 0)
            .ok_or("Session source identities are exhausted")?;
        candidate
            .plugin_instances
            .iter_mut()
            .find(|item| item.id == id)
            .expect("newly allocated source")
            .id = replacement;
        match target {
            PluginPickerTarget::ChannelDevice { channel } => {
                candidate.channels[channel].instrument_plugin_instance_id = Some(replacement)
            }
            PluginPickerTarget::MixerSlot { .. } => {
                for slot in &mut candidate.mixer_insert_slots {
                    if slot.plugin_instance_id == id {
                        slot.plugin_instance_id = replacement;
                    }
                }
            }
        }
        Ok(replacement)
    }
    pub(super) fn record_fresh_clip_source_for_new_load(
        &mut self,
        instance_id: u64,
        descriptor: &PluginDescriptor,
    ) -> Result<(), String> {
        if !cfg!(all(
            target_os = "linux",
            feature = "experimental-fresh-helper"
        )) || descriptor.format != ScannedPluginFormat::Vst3
        {
            return Err(
                "Experimental dry-bounce preparation requires this Linux VST3 build".into(),
            );
        }
        if instance_id == 0
            || self
                .project
                .plugin_instances
                .iter()
                .any(|item| item.id == instance_id)
            || self.clip_render_restorations.contains_key(&instance_id)
            || self.fresh_bounce_sources.contains_key(&instance_id)
        {
            return Err("Fresh-source opt-in requires a new, unbound instance identity".into());
        }
        let uid = descriptor.vst3_metadata.as_ref().map(|metadata| metadata.class_uid.as_str())
            .filter(|uid| uid.len() == 32 && uid.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or("Rescan this VST3 to obtain its actual factory class before fresh-source preparation")?;
        if descriptor.id.is_empty()
            || descriptor.id.len() > 4096
            || descriptor.path.as_os_str().len() > 4096
        {
            return Err("Fresh-source descriptor binding exceeds its bound".into());
        }
        if self.fresh_bounce_sources.len() >= 4096 {
            return Err("Session fresh-source metadata limit reached (4096 entries)".into());
        }
        self.fresh_bounce_sources
            .try_reserve(1)
            .map_err(|_| "Unable to reserve fresh-source intent")?;
        self.fresh_bounce_sources.insert(
            instance_id,
            (
                descriptor.id.clone(),
                descriptor.path.clone(),
                uid.to_owned(),
            ),
        );
        Ok(())
    }
    pub(super) fn selected_clip_render_button(&mut self, ui: &mut egui::Ui) {
        let reason = self.clip_render_unavailable_reason();
        let button = ui.add_enabled(
            reason.is_none(),
            egui::Button::new("Render selected clip to audio…"),
        );
        let button = if let Some(reason) = reason {
            button.on_hover_text(reason)
        } else {
            button.on_hover_text("Experimental Linux dry render. Keeps muted MIDI and disables its sole-use generator in one undoable conversion.")
        };
        if button.clicked() {
            self.open_selected_clip_render_review();
        }
    }
    pub(super) fn retain_clip_source_before_engine_loss(&mut self) {
        // Transfer before any generic map clear. The job owns this exact guard
        // until a finished-only join, including an unknown queued-removal result.
        let channel = self
            .running_generator_chains
            .iter()
            .find_map(|(&channel, source)| {
                self.clip_render_job
                    .as_ref()
                    .and_then(|job| job.owns_source(source).then_some(channel))
            });
        if let Some(channel) = channel
            && let Some(source) = self.running_generator_chains.remove(&channel)
            && let Err(source) = self.retain_clip_render_source_on_engine_loss(source)
        {
            self.running_generator_chains.insert(channel, source);
        }
    }
    pub(super) fn prepare_clip_render_restoration(
        &mut self,
        instance_id: u64,
        metadata: &BouncePluginMetadata,
    ) -> Result<ClipRenderRestorationBinding, String> {
        if self.clip_timeline_projection_revision == u64::MAX {
            return Err(
                "Converted playback projection identities are exhausted; open a new session".into(),
            );
        }
        let instance = self
            .project
            .plugin_instances
            .iter()
            .find(|item| item.id == instance_id)
            .ok_or("The captured source instance disappeared")?;
        let sample_rate = self
            .audio
            .as_ref()
            .ok_or("Audio is unavailable")?
            .snapshot()
            .sample_rate;
        metadata.validate(sample_rate)?;
        if metadata.name.len() > 4096
            || metadata.version.len() > 4096
            || metadata.category.len() > 4096
            || instance.uid.len() > 4096
            || instance.path.as_os_str().len() > 4096
        {
            return Err("Restoration identity exceeds its bounded storage".into());
        }
        if !self.clip_render_restorations.contains_key(&instance_id)
            && self.clip_render_restorations.len() >= 4096
        {
            return Err("Session restoration metadata limit reached (4096 entries)".into());
        }
        self.clip_render_restorations
            .try_reserve(1)
            .map_err(|_| "Unable to reserve restoration identity")?;
        self.retired_clip_restorations
            .try_reserve(2)
            .map_err(|_| "Unable to reserve retained source cleanup")?;
        Ok(ClipRenderRestorationBinding {
            project_session: self.project_session,
            instance_id,
            descriptor_id: instance.uid.clone(),
            module_path: instance.path.clone(),
            metadata: metadata.clone(),
            fresh_load: self.fresh_bounce_sources.contains_key(&instance_id),
        })
    }
    pub(super) fn install_clip_render_restoration(
        &mut self,
        binding: ClipRenderRestorationBinding,
    ) {
        // Capacity was reserved before CommitAccepted; the moved binding contains
        // no opaque state. The authoritative history images own that state.
        self.clip_render_restorations
            .insert(binding.instance_id, binding);
        self.clip_timeline_projection_revision = self
            .clip_timeline_projection_revision
            .checked_add(1)
            .expect("projection identity preflighted before CommitAccepted");
    }
    pub(super) fn clip_operation_fence_active(&self) -> bool {
        self.clip_session_retirement_pending()
            || self.clip_render_fence_active()
            || self.clip_restoration_pending()
    }
    pub(super) fn clip_restoration_resources_pending(&self) -> bool {
        !self.pending_clip_restorations.is_empty()
            || !self.retired_clip_restorations.is_empty()
            || self.pending_generator_removals.values().any(|removal| {
                removal
                    .plugin_instance_id
                    .is_some_and(|id| self.clip_render_restorations.contains_key(&id))
            })
            || self.pending_generator_chains.values().any(|chain| {
                self.clip_render_restorations
                    .contains_key(&chain.instance_id)
            })
    }
    pub(super) fn clip_restoration_pending(&self) -> bool {
        self.clip_restoration_resources_pending()
            || self.clip_restoration_playback_pending && !self.clip_restoration_failed
    }
    pub(super) fn source_restoration_expectation(
        &self,
        candidate: &Project,
        instance_id: u64,
        spec: &mut PluginLoadSpec,
        sample_rate: u32,
    ) -> Result<Option<SourceRestorationExpectation>, String> {
        let Some(binding) = self.clip_render_restorations.get(&instance_id) else {
            return Ok(None);
        };
        let instance = candidate
            .plugin_instances
            .iter()
            .find(|item| item.id == instance_id)
            .ok_or("Restoration instance disappeared")?;
        if binding.project_session != self.project_session
            || binding.descriptor_id != instance.uid
            || binding.module_path != instance.path
            || instance.format != ProjectPluginFormat::Vst3
        {
            return Err(
                "Restoration class binding does not match this Project instance/path/session"
                    .into(),
            );
        }
        spec.class_uid = Some(binding.metadata.class_uid.clone());
        if !instance.enabled {
            return Ok(None);
        }
        if instance.bypass || instance.wet.to_bits() != 1.0f32.to_bits() {
            return Err(
                "Restoration requires the retained enabled, dry generator configuration".into(),
            );
        }
        let mut bases = Vec::new();
        bases
            .try_reserve_exact(instance.parameters.len())
            .map_err(|_| "Unable to reserve restoration bases")?;
        bases.extend(instance.parameters.iter().map(|(&id, &value)| (id, value)));
        SourceRestorationExpectation::new(
            self.project_session,
            instance_id,
            binding.descriptor_id.clone(),
            binding.module_path.clone(),
            sample_rate,
            binding.metadata.clone(),
            bases,
        )
        .map(Some)
    }
    pub(super) fn spawn_clip_bound_source(
        &self,
        instance_id: u64,
        mut spec: PluginLoadSpec,
        candidate: &Project,
        sample_rate: f32,
    ) -> Result<(PluginChain, Option<(u32, u32)>), String> {
        if let Some(expectation) = self.source_restoration_expectation(
            candidate,
            instance_id,
            &mut spec,
            sample_rate as u32,
        )? {
            let timing = expectation.expected_timing();
            let fresh = self.clip_render_restorations[&instance_id].fresh_load;
            #[cfg(test)]
            if let Some(factory) = self.clip_restoration_test_factory {
                let config = crate::plugins::plugin_runtime::PluginPrepareConfig {
                    sample_rate: f64::from(sample_rate),
                    max_block_frames: MAX_MIXER_BLOCK_FRAMES,
                };
                expectation.validate_spec(self.project_session, instance_id, &spec, config)?;
                return factory(
                    candidate,
                    instance_id,
                    spec,
                    config,
                    &self.clip_render_restorations[&instance_id].metadata,
                )
                .map(|chain| (chain, Some(timing)));
            }
            return self
                .plugin_host
                .spawn_recreated_source(
                    self.project_session,
                    instance_id,
                    spec,
                    sample_rate,
                    MAX_MIXER_BLOCK_FRAMES as u32,
                    expectation,
                    fresh,
                )
                .map(|chain| (chain, Some(timing)));
        }
        if let Some((descriptor_id, path, class_uid)) = self.fresh_bounce_sources.get(&instance_id)
        {
            if spec.descriptor.id != *descriptor_id || spec.descriptor.path != *path {
                return Err("Fresh-source intent belongs to a different descriptor/path".into());
            }
            spec.class_uid = Some(class_uid.clone());
            if spec.enabled && !spec.bypassed && spec.wet.to_bits() == 1.0f32.to_bits() {
                return self
                    .plugin_host
                    .spawn_fresh_bounce_source(
                        instance_id,
                        spec,
                        sample_rate,
                        MAX_MIXER_BLOCK_FRAMES as u32,
                    )
                    .map(|chain| (chain, None));
            }
        }
        self.plugin_host
            .spawn_identified_chain(
                vec![(instance_id, spec)],
                sample_rate,
                MAX_MIXER_BLOCK_FRAMES as u32,
            )
            .map(|chain| (chain, None))
    }
    #[allow(clippy::too_many_arguments)] // exact source, candidate and owned chain binding
    pub(super) fn defer_clip_restoration(
        &mut self,
        channel_id: u32,
        instance_id: u64,
        candidate: Project,
        previous: Option<ChannelGeneratorState>,
        chain: PluginChain,
        expected_timing: (u32, u32),
        live_snapshot: Arc<[u8]>,
    ) {
        self.pending_clip_restorations.insert(
            channel_id,
            PendingClipRestoration {
                project_session: self.project_session,
                instance_id,
                engine_session: self.audio.as_ref().map_or(0, AudioEngine::engine_session),
                live_snapshot,
                live_revision: self.clip_render_model_revision,
                candidate: Some(candidate),
                previous,
                chain: Some(chain),
                transferred_control: None,
                transferred_guard: None,
                expected_timing,
                ready: false,
                started_at: Instant::now(),
            },
        );
        self.clip_restoration_playback_pending = true;
        self.clip_restoration_failed = false;
        self.clip_restoration_timeline = None;
        self.playing = false;
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
    }
    pub(super) fn poll_clip_restorations(&mut self) {
        if self.clip_session_retirement_pending() {
            return;
        }
        let channels = self
            .pending_clip_restorations
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for channel in channels {
            // The owner is outside the catch boundary. No unwind can detach it.
            let mut pending = self
                .pending_clip_restorations
                .remove(&channel)
                .expect("listed pending source");
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.drive_clip_restoration(channel, &mut pending)
            }));
            match result {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => {
                    self.pending_clip_restorations.insert(channel, pending);
                }
                outcome => {
                    if let Some(chain) = pending.chain.take() {
                        let PluginChain {
                            audio,
                            control,
                            guard,
                        } = chain;
                        drop(audio);
                        drop(control);
                        self.retired_clip_restorations.push(guard);
                    }
                    pending.transferred_control = None;
                    if let Some(guard) = pending.transferred_guard.take() {
                        self.retired_clip_restorations.push(guard);
                    }
                    self.clip_restoration_failed = true;
                    self.playing = false;
                    if let Some(audio) = &self.audio {
                        audio.set_playing(false);
                    }
                    let error = match outcome {
                        Ok(Err(error)) => error,
                        _ => "Restoration panicked before admission".into(),
                    };
                    self.notify(format!(
                        "Restoration refused; playback stays paused: {error}"
                    ));
                }
            }
        }
        let mut shutdown_panicked = false;
        self.retired_clip_restorations.retain_mut(|guard| {
            match guard.poll_shutdown_retained() {
                RetainedShutdownPoll::Pending => true,
                RetainedShutdownPoll::Joined => false,
                RetainedShutdownPoll::WorkerPanicked => {
                    // Terminal join proves ownership ended, not healthy shutdown.
                    shutdown_panicked = true;
                    false
                }
            }
        });
        if shutdown_panicked {
            self.clip_restoration_failed = true;
            self.clip_restoration_playback_pending = true;
            self.playing = false;
            if let Some(audio) = &self.audio {
                audio.set_playing(false);
            }
            self.notify("Source shutdown panicked; restoration remains paused".into());
        }
        if self.retired_clip_restorations.is_empty() && !self.clip_restoration_failed {
            let channels = self
                .deferred_generator_rebuilds
                .iter()
                .copied()
                .filter(|channel| {
                    self.project
                        .channels
                        .iter()
                        .find(|item| item.id == *channel)
                        .and_then(|item| item.instrument_plugin_instance_id)
                        .is_some_and(|id| self.clip_render_restorations.contains_key(&id))
                })
                .collect::<Vec<_>>();
            for channel in channels {
                self.deferred_generator_rebuilds.remove(&channel);
                self.request_channel_generator_rebuild(channel);
            }
        }
        if self.clip_restoration_playback_pending
            && !self.clip_restoration_failed
            && self.pending_clip_restorations.is_empty()
            && self.retired_clip_restorations.is_empty()
            && self.pending_generator_chains.is_empty()
            && self.pending_generator_removals.is_empty()
            && self.clip_restored_sources_ready()
        {
            if self.clip_restoration_timeline.is_none() {
                let Some(generation) = self
                    .timeline_desired_generation
                    .checked_add(1)
                    .filter(|value| *value != 0)
                else {
                    self.clip_restoration_failed = true;
                    self.notify("Timeline identities exhausted; restoration stays paused".into());
                    return;
                };
                self.timeline_desired_generation = generation;
                self.timeline_observed_fingerprint =
                    self.clip_timeline_fingerprint(self.timeline_sample_rate());
                self.timeline_compile_deadline = Instant::now();
                self.timeline_deferred_job = None;
                self.invalidate_pending_timeline_audio_generation();
                if let Some(audio) = &self.audio {
                    self.clip_restoration_timeline = Some((audio.engine_session(), generation));
                }
            }
            let exact = self
                .clip_restoration_timeline
                .is_some_and(|(engine, generation)| {
                    self.audio.as_ref().is_some_and(|audio| {
                        audio.engine_session() == engine
                            && audio.project_session() == self.project_session
                            && audio
                                .confirmed_timeline_identity()
                                .is_some_and(|(actual, epoch)| {
                                    actual == generation
                                        && epoch == audio.snapshot().transport_epoch
                                        && !audio.snapshot().transport_playing
                                })
                    }) && self.timeline_desired_generation == generation
                        && self.timeline_audio_matches_desired()
                });
            if exact {
                self.clip_restoration_playback_pending = false;
                self.clear_clip_render_playback_failure_after_restoration();
            }
        }
    }
    fn drive_clip_restoration(
        &mut self,
        channel: u32,
        pending: &mut PendingClipRestoration,
    ) -> Result<bool, String> {
        let chain = pending
            .chain
            .as_mut()
            .ok_or("Restoration owner is already transferred")?;
        while let Some(event) = chain.control.try_next_event() {
            // This restored source is still unpublished; its endpoint ID is
            // allocated only after the readiness checks below succeed.
            #[cfg(test)]
            crate::genuine_app_observer::record_runtime_event(
                0,
                pending.project_session,
                pending.instance_id,
                &event,
            );
            match &event {
                RuntimeEvent::SlotReady { slot: 0, .. } => {
                    pending.ready = true;
                    let _ = apply_plugin_runtime_event_for_session(
                        pending.candidate.as_mut().expect("candidate retained"),
                        self.project_session,
                        pending.project_session,
                        &[pending.instance_id],
                        &event,
                    );
                }
                RuntimeEvent::SlotFault { message, .. } => return Err(message.clone()),
                RuntimeEvent::ShutdownComplete | RuntimeEvent::StateRejected { .. } => {
                    return Err("Recreated source stopped before admission".into());
                }
                _ => {}
            }
        }
        if pending.project_session != self.project_session
            || self.audio.as_ref().is_none_or(|audio| {
                audio.engine_session() != pending.engine_session
                    || audio.project_session() != pending.project_session
            })
            || pending.started_at.elapsed() > Duration::from_secs(35)
            || chain.control.stats().faults != 0
            || chain.control.stats().stopped
        {
            return Err(
                "Recreated source failed, expired, or changed session before admission".into(),
            );
        }
        let Some(snapshot) = chain.control.plugin_endpoint_snapshot() else {
            return Ok(false);
        };
        if !pending.ready {
            return Ok(false);
        }
        if snapshot.slot_count() != 1
            || snapshot.manifest().instance_ids[0] != Some(pending.instance_id)
            || snapshot.active_mask() != 1
            || (
                snapshot.total_plugin_latency_samples(),
                snapshot.tail_samples(),
            ) != pending.expected_timing
        {
            return Err(
                "Recreated source has changed coherent identity/timing before endpoint transfer"
                    .into(),
            );
        }
        if pending.live_revision != self.clip_render_model_revision
            || !clip_render::project_matches_snapshot(&self.project, &pending.live_snapshot)
                .map_err(|error| error.to_string())?
        {
            return Err(
                "The live Project changed during source restoration; candidate refused".into(),
            );
        }
        let candidate = pending.candidate.as_ref().expect("candidate retained");
        crate::plugin_midi_routing::compile_midi_port_routes(candidate)?;
        if !self.audio_restart_state.allows_realtime_session_actions()
            && !self.audio_restart_state.is_restoring()
        {
            return Err("Audio is degraded during source restoration".into());
        }
        if !self.audio_restart_state.is_restoring() {
            self.native_editor_topology_guard()?;
        }
        if self.pending_generator_chains.contains_key(&channel)
            || self.pending_generator_removals.contains_key(&channel)
            || self
                .midi_input
                .state
                .owner()
                .is_some_and(|owner| owner.stamp.channel_id == channel)
        {
            return Err("Source admission changed while the replacement was being prepared".into());
        }
        let (_, mixer_track, _) = channel_generator_runtime_spec(candidate, channel)?
            .ok_or("Restoration Channel disappeared")?;
        let candidate_state = snapshot_channel_generator(candidate, channel)
            .ok_or("Restoration Channel disappeared")?;
        self.running_generator_chains
            .try_reserve(1)
            .map_err(|_| "Unable to reserve installed restoration ownership")?;
        self.retired_clip_restorations
            .try_reserve(self.pending_generator_chains.len() + 2)
            .map_err(|_| "Unable to reserve restored callback retirement")?;
        self.pending_generator_chains
            .try_reserve(1)
            .map_err(|_| "Unable to reserve restoration callback ownership")?;
        self.clip_restoration_installs
            .try_reserve(1)
            .map_err(|_| "Unable to reserve exact restoration receipt")?;
        if self.audio.is_none() {
            return Err("Audio disappeared before source admission".into());
        }
        // Retain this exact ready owner while ordinary commands are backed up.
        // No other command is produced before the synchronous install below.
        if !self
            .audio
            .as_ref()
            .expect("audio checked")
            .ordinary_command_capacity_available()
        {
            return Ok(false);
        }
        let endpoint_id = self.allocate_generator_endpoint_id();
        let PluginChain {
            audio: endpoint,
            control,
            guard,
        } = pending.chain.take().expect("retained source");
        pending.transferred_control = Some(control);
        pending.transferred_guard = Some(guard);
        let (latency_samples, tail_samples) = pending.expected_timing;
        let queued = self
            .audio
            .as_mut()
            .expect("audio checked")
            .install_generator_endpoint_for_project_with_timing(
                self.project_session,
                channel,
                endpoint_id,
                pending.instance_id,
                mixer_track,
                endpoint,
                audio::GeneratorInstallTimingExpectation {
                    latency_samples,
                    tail_samples,
                },
            );
        if !queued {
            return Err("Restoration callback installation queue is full".into());
        }
        self.clip_restoration_installs
            .insert(pending.instance_id, (endpoint_id, false));
        self.clip_restoration_timeline = None;
        // Capacity and all fallible preparation precede endpoint transfer. The
        // guard is installed before the candidate Project can become visible.
        self.pending_generator_chains.insert(
            channel,
            PendingGeneratorChain {
                project_session: self.project_session,
                endpoint_id,
                instance_id: pending.instance_id,
                mixer_track,
                control: pending
                    .transferred_control
                    .take()
                    .expect("retained control"),
                guard: pending.transferred_guard.take().expect("retained guard"),
                previous_state: pending.previous.take(),
                candidate_state,
            },
        );
        if !self.audio_restart_state.is_restoring() {
            self.project = pending.candidate.take().expect("retained candidate");
        }
        self.pending_generator_routes.remove(&channel);
        self.pending_generator_note_offs
            .retain(|note| note.channel_id != channel);
        Ok(true)
    }
    pub(super) fn prepare_clip_history_restoration(&mut self) -> Result<(), String> {
        self.retired_clip_restorations
            .try_reserve(
                self.running_generator_chains.len() + self.pending_generator_chains.len() + 2,
            )
            .map_err(|_| "Unable to reserve source shutdown ownership for Undo/Redo")?;
        self.pending_clip_restorations
            .try_reserve(self.clip_render_restorations.len())
            .map_err(|_| "Unable to reserve restored sources for Undo/Redo")?;
        self.clip_restoration_playback_pending = true;
        self.clip_restoration_failed = false;
        self.clip_restoration_installs.clear();
        self.clip_restoration_timeline = None;
        self.playing = false;
        self.transport_clock_playing = false;
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
        Ok(())
    }
    pub(super) fn retire_clip_bound_generator(&mut self, source: RunningGeneratorChain) {
        let source = match self.retain_clip_session_source(source) {
            Ok(()) => return,
            Err(source) => source,
        };
        if self
            .clip_render_restorations
            .contains_key(&source.instance_id)
        {
            self.retired_clip_restorations.push(source.guard);
            self.clip_restoration_playback_pending = true;
        } else {
            drop(source.guard);
        }
    }
    pub(super) fn retire_failed_clip_candidate(
        &mut self,
        instance_id: u64,
        guard: PluginWorkerGuard,
    ) {
        if self.clip_render_restorations.contains_key(&instance_id) {
            self.retired_clip_restorations.push(guard);
            self.clip_restoration_playback_pending = true;
            self.clip_restoration_failed = true;
            self.playing = false;
            if let Some(audio) = &self.audio {
                audio.set_playing(false);
            }
        } else {
            drop(guard);
        }
    }
    pub(super) fn retain_pending_recreated_sources_before_clear(&mut self) {
        let channels = self
            .pending_generator_chains
            .iter()
            .filter_map(|(&channel, source)| {
                self.clip_render_restorations
                    .contains_key(&source.instance_id)
                    .then_some(channel)
            })
            .collect::<Vec<_>>();
        for channel in channels {
            if let Some(source) = self.pending_generator_chains.remove(&channel) {
                self.retire_failed_clip_candidate(source.instance_id, source.guard);
            }
        }
    }
    pub(super) fn retain_recreated_sources_before_clear(&mut self) {
        let channels = self
            .running_generator_chains
            .iter()
            .filter_map(|(&channel, source)| {
                self.clip_render_restorations
                    .contains_key(&source.instance_id)
                    .then_some(channel)
            })
            .collect::<Vec<_>>();
        for channel in channels {
            if let Some(source) = self.running_generator_chains.remove(&channel) {
                self.retire_clip_bound_generator(source);
            }
        }
    }
    /// Bound callback confirmations keep every guard in App-owned maps until
    /// panic-capable validation/cancellation has finished. Transfers afterwards
    /// move directly into storage reserved before endpoint admission.
    pub(super) fn consume_bound_generator_event(&mut self, event: GeneratorEndpointEvent) -> bool {
        let bound = match event {
            GeneratorEndpointEvent::Installed {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                ..
            } => self
                .pending_generator_chains
                .get(&channel_id)
                .is_some_and(|pending| {
                    pending.endpoint_id == endpoint_id
                        && pending.instance_id == plugin_instance_id
                        && self
                            .clip_render_restorations
                            .contains_key(&pending.instance_id)
                }),
            GeneratorEndpointEvent::Removed {
                channel_id,
                endpoint_id,
                plugin_instance_id,
            } => self
                .pending_generator_removals
                .get(&channel_id)
                .is_some_and(|pending| {
                    pending.endpoint_id == endpoint_id
                        && pending.plugin_instance_id == plugin_instance_id
                        && pending
                            .plugin_instance_id
                            .is_some_and(|id| self.clip_render_restorations.contains_key(&id))
                }),
            _ => false,
        };
        if !bound {
            return false;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.apply_bound_generator_event(event)
        }));
        if !matches!(result, Ok(Ok(()))) {
            self.clip_restoration_failed = true;
            self.clip_restoration_playback_pending = true;
            self.playing = false;
            self.transport_clock_playing = false;
            if let Some(audio) = &self.audio {
                audio.set_playing(false);
            }
            let reason = match result {
                Ok(Err(reason)) => reason,
                _ => "Source callback confirmation panicked; exact source owners remain retained"
                    .into(),
            };
            self.notify(format!("Restoration stays paused: {reason}"));
        }
        true
    }
    fn apply_bound_generator_event(&mut self, event: GeneratorEndpointEvent) -> Result<(), String> {
        match event {
            GeneratorEndpointEvent::Installed {
                channel_id,
                endpoint_id,
                plugin_instance_id,
                mixer_track,
                replaced_endpoint_id,
                replaced_plugin_instance_id,
                success,
            } => {
                #[cfg(test)]
                if self.clip_restoration_event_panic == Some("installed") {
                    self.clip_restoration_event_panic = None;
                    panic!("injected bound installed confirmation panic");
                }
                let pending = self
                    .pending_generator_chains
                    .get(&channel_id)
                    .ok_or("Exact restored candidate disappeared")?;
                if pending.project_session != self.project_session
                    || pending.mixer_track != mixer_track
                {
                    return Err("Restored callback session or mixer identity changed".into());
                }
                if !success {
                    // Callback refusal leaves the old endpoint in place. Do not
                    // replay a whole candidate or claim a successful restoration.
                    mark_channel_generator_runtime_error(
                        &mut self.project,
                        channel_id,
                        "audio callback rejected the restored generator endpoint",
                    );
                    self.clip_restoration_failed = true;
                    self.clip_restoration_playback_pending = true;
                    let pending = self
                        .pending_generator_chains
                        .remove(&channel_id)
                        .expect("owned candidate");
                    self.retired_clip_restorations.push(pending.guard);
                    self.notify(format!(
                        "Channel {channel_id} restored endpoint was rejected; playback stays paused"
                    ));
                    return Ok(());
                }
                if replaced_endpoint_id
                    != self
                        .running_generator_chains
                        .get(&channel_id)
                        .map(|old| old.endpoint_id)
                    || replaced_plugin_instance_id
                        != self
                            .running_generator_chains
                            .get(&channel_id)
                            .map(|old| old.instance_id)
                {
                    return Err("Restored callback replacement identity changed".into());
                }
                if self.clip_restoration_installs.get(&plugin_instance_id)
                    != Some(&(endpoint_id, false))
                {
                    return Err(
                        "Restored callback receipt does not match its issued endpoint".into(),
                    );
                }
                if let Some((session, old_endpoint)) = self
                    .running_generator_chains
                    .get(&channel_id)
                    .map(|old| (old.project_session, old.endpoint_id))
                {
                    self.cancel_plugin_parameter_endpoint_edits(
                        session,
                        ParameterEndpoint {
                            kind: ParameterEndpointKind::Generator,
                            id: old_endpoint,
                        },
                        ParameterEditCancelReason::EndpointReplaced,
                    );
                }
                // No assertion, callback or allocation is interposed between
                // taking each guard and transferring it into its reserved owner.
                if let Some(old) = self.running_generator_chains.remove(&channel_id) {
                    self.retire_clip_bound_generator(old);
                }
                let pending = self
                    .pending_generator_chains
                    .remove(&channel_id)
                    .expect("owned candidate");
                self.running_generator_chains.insert(
                    channel_id,
                    RunningGeneratorChain {
                        project_session: pending.project_session,
                        endpoint_id: pending.endpoint_id,
                        instance_id: pending.instance_id,
                        mixer_track,
                        control: pending.control,
                        guard: pending.guard,
                    },
                );
                if let Some(receipt) = self.clip_restoration_installs.get_mut(&plugin_instance_id) {
                    receipt.1 = true;
                }
                self.automation_evaluator.reset();
                self.deferred_generator_rebuilds.remove(&channel_id);
            }
            GeneratorEndpointEvent::Removed {
                channel_id,
                endpoint_id,
                plugin_instance_id,
            } => {
                #[cfg(test)]
                if self.clip_restoration_event_panic == Some("removed") {
                    self.clip_restoration_event_panic = None;
                    panic!("injected bound removed confirmation panic");
                }
                let pending = self
                    .pending_generator_removals
                    .get(&channel_id)
                    .ok_or("Exact source removal disappeared")?;
                if pending.project_session != self.project_session {
                    return Err("Retained source removal belongs to a different session".into());
                }
                if let Some(old) = self.running_generator_chains.get(&channel_id) {
                    if Some(old.endpoint_id) != endpoint_id
                        || Some(old.instance_id) != plugin_instance_id
                    {
                        return Err(
                            "Retained source removal does not own the running endpoint".into()
                        );
                    }
                    let (session, endpoint) = (old.project_session, old.endpoint_id);
                    self.cancel_plugin_parameter_endpoint_edits(
                        session,
                        ParameterEndpoint {
                            kind: ParameterEndpointKind::Generator,
                            id: endpoint,
                        },
                        ParameterEditCancelReason::EndpointReplaced,
                    );
                }
                if let Some(old) = self.running_generator_chains.remove(&channel_id) {
                    self.retire_clip_bound_generator(old);
                }
                self.pending_generator_removals.remove(&channel_id);
                self.deferred_generator_rebuilds.remove(&channel_id);
            }
            _ => return Err("Unexpected bound generator confirmation".into()),
        }
        Ok(())
    }
    pub(super) fn clip_restored_sources_ready(&self) -> bool {
        self.clip_render_restorations.iter().all(|(&id, binding)| {
            let Some(instance) = self
                .project
                .plugin_instances
                .iter()
                .find(|item| item.id == id)
            else {
                return true;
            };
            if !instance.enabled {
                return !self
                    .running_generator_chains
                    .values()
                    .any(|chain| chain.instance_id == id);
            }
            self.running_generator_chains.values().any(|chain| {
                chain.project_session == binding.project_session
                    && chain.instance_id == id
                    && self.clip_restoration_installs.get(&id) == Some(&(chain.endpoint_id, true))
                    && chain.control.stats().faults == 0
                    && chain
                        .control
                        .plugin_endpoint_snapshot()
                        .is_some_and(|snapshot| {
                            snapshot.slot_count() == 1
                                && snapshot.manifest().instance_ids[0] == Some(id)
                                && snapshot.active_mask() == 1
                        })
            })
        })
    }
}

#[cfg(test)]
#[path = "clip_render_glue_tests.rs"]
pub(in crate::app) mod tests;
