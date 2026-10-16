//! Raw Whole PCM passes through each disposable clone before one final crop.
use super::*;
use crate::{
    export::serial_master::StaticSerialMasterPlan,
    instrument_bounce::effect::{
        CapturedEffectSlot, EffectChainStateReceipt, EffectSourceMetadata,
    },
    model::MixerInsertSlotRef,
};
#[cfg(test)]
#[path = "whole_serial_render_tests.rs"]
mod tests;

pub(crate) struct CapturedSerialEffectChain {
    pub runtime_track: usize,
    pub placements: Vec<MixerInsertSlotRef>,
    pub receipt: EffectChainStateReceipt,
}

pub(crate) trait EffectRenderBackend: 'static {
    fn prepare(
        &mut self,
        slot: &CapturedEffectSlot,
        control: &RenderControl,
    ) -> Result<EffectSourceMetadata, BackendFailure>;
    fn process(
        &mut self,
        input: &[Vec<f32>],
        transport: PluginTransport,
    ) -> Result<BlockReply, BackendFailure>;
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation;
}

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
impl EffectRenderBackend for crate::plugins::strict_backend::StrictEffectBackend {
    fn prepare(
        &mut self,
        slot: &CapturedEffectSlot,
        control: &RenderControl,
    ) -> Result<EffectSourceMetadata, BackendFailure> {
        self.prepare(slot, control)
    }
    fn process(
        &mut self,
        input: &[Vec<f32>],
        transport: PluginTransport,
    ) -> Result<BlockReply, BackendFailure> {
        self.process(input, transport)
    }
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        self.dispose(deadline)
    }
}

impl RenderAdmission {
    #[cfg(all(
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_master_song_serial(
        &self,
        project: Project,
        capture: CapturedInstrumentBounce,
        chains: Vec<CapturedSerialEffectChain>,
        options: WavExportOptions,
        path: PathBuf,
        control: RenderControl,
        reviewed_fixed_tail: bool,
    ) -> Result<MasterSongRenderReceipt, String> {
        self.start_serial_with_backends_inner(
            project,
            capture,
            chains,
            options,
            path,
            control,
            reviewed_fixed_tail,
            crate::plugins::strict_backend::StrictBackend::new,
            || crate::plugins::strict_backend::StrictEffectBackend::new(None),
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(crate) fn start_serial_with_backends<G: RenderBackend, E: EffectRenderBackend>(
        &self,
        project: Project,
        capture: CapturedInstrumentBounce,
        chains: Vec<CapturedSerialEffectChain>,
        options: WavExportOptions,
        path: PathBuf,
        control: RenderControl,
        reviewed_fixed_tail: bool,
        generator: impl FnOnce() -> G + Send + 'static,
        effects: impl FnMut() -> E + Send + 'static,
    ) -> Result<MasterSongRenderReceipt, String> {
        self.start_serial_with_backends_inner(
            project,
            capture,
            chains,
            options,
            path,
            control,
            reviewed_fixed_tail,
            generator,
            effects,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_serial_with_backends_inner<G: RenderBackend, E: EffectRenderBackend>(
        &self,
        project: Project,
        capture: CapturedInstrumentBounce,
        chains: Vec<CapturedSerialEffectChain>,
        options: WavExportOptions,
        path: PathBuf,
        control: RenderControl,
        reviewed_fixed_tail: bool,
        generator: impl FnOnce() -> G + Send + 'static,
        effects: impl FnMut() -> E + Send + 'static,
    ) -> Result<MasterSongRenderReceipt, String> {
        self.0
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "An instrument renderer still owns this job")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let terminated = Arc::new(AtomicBool::new(false));
        let ack = Arc::clone(&terminated);
        let slot = Arc::clone(&self.0);
        let progress = Arc::new(JobProgress::new());
        *self.0.job.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::clone(&progress));
        let worker_progress = Arc::clone(&progress);
        let worker_control = control.clone();
        let spawn = std::thread::Builder::new()
            .name("strict-serial-master-render".into())
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    run_serial_owned(
                        project,
                        capture,
                        chains,
                        options,
                        path,
                        &worker_control,
                        &worker_progress,
                        reviewed_fixed_tail,
                        generator,
                        effects,
                    )
                }))
                .unwrap_or_else(|_| {
                    MasterSongRenderOutcome::Failed(RenderFailure::new(
                        FailureKind::Panic,
                        "Serial Whole owner panicked",
                    ))
                });
                let _ = sender.try_send(outcome);
                drop(sender);
                worker_progress.set(RenderStatus::Terminated);
                slot.busy.store(false, Ordering::Release);
                ack.store(true, Ordering::Release);
            });
        if let Err(error) = spawn {
            progress.set(RenderStatus::Terminated);
            self.0.busy.store(false, Ordering::Release);
            return Err(error.to_string());
        }
        Ok(MasterSongRenderReceipt {
            receiver,
            control,
            terminated,
            progress,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn run_serial_owned<G: RenderBackend, E: EffectRenderBackend>(
    project: Project,
    capture: CapturedInstrumentBounce,
    chains: Vec<CapturedSerialEffectChain>,
    options: WavExportOptions,
    path: PathBuf,
    control: &RenderControl,
    progress: &JobProgress,
    reviewed_fixed_tail: bool,
    generator: impl FnOnce() -> G,
    mut effects: impl FnMut() -> E,
) -> MasterSongRenderOutcome {
    let result = (|| {
        let mut prepared = PreparedRender::from_capture(&capture, reviewed_fixed_tail, control)?;
        let invalid = |s: &str| RenderFailure::new(FailureKind::Invalid, s);
        if !capture.plan().is_serial_master()
            || options.sample_rate != prepared.sample_rate
            || options.tail_frames().map_err(|e| invalid(&e.to_string()))?
                != prepared.tail_frames as usize
        {
            return Err(invalid(
                "Serial Whole options differ from the frozen capture",
            ));
        }
        let validated = super::super::InstrumentBouncePlan::prepare_master_song_serial(
            &project,
            capture.plan().channel_id(),
            prepared.sample_rate,
            prepared.tail_frames,
        )
        .map_err(|e| invalid(&e))?;
        if validated.project_fingerprint() != capture.identity().project_fingerprint
            || validated.project_fingerprint() != capture.plan().project_fingerprint()
        {
            return Err(invalid(
                "Serial Whole frozen project differs from the captured source",
            ));
        }
        let tempo_map = crate::tempo_map::TempoMap::from_project(&project, prepared.sample_rate)
            .map_err(|e| invalid(&e.to_string()))?;
        let master_controls = crate::export::OfflineMasterControls::prepare(&project, &tempo_map)
            .map_err(|e| invalid(&e.to_string()))?;
        let mix = StaticSerialMasterPlan::prepare(&project, capture.plan().channel_id())
            .map_err(|e| invalid(&e.to_string()))?;
        let endpoints: Vec<_> = mix
            .nodes
            .iter()
            .filter(|n| !n.placements.is_empty())
            .collect();
        if endpoints.len() != chains.len() {
            return Err(invalid(
                "Serial Whole requires every ordered source endpoint receipt",
            ));
        }
        let mut latency = u64::from(prepared.metadata.latency_samples);
        let mut reported_tail = prepared.metadata.tail_samples;
        for (node, chain) in endpoints.iter().zip(&chains) {
            if node.runtime_track != chain.runtime_track
                || node.placements != chain.placements
                || chain.receipt.slots.len() != node.placements.len()
                || chain.receipt.endpoint.slot_count() != node.placements.len()
                || chain.receipt.epoch != capture.identity().epoch
            {
                return Err(invalid(
                    "Serial Whole endpoint receipt order or epoch differs from the frozen graph",
                ));
            }
            let mut chain_tail = 0_u32;
            for (index, (placement, slot)) in
                node.placements.iter().zip(&chain.receipt.slots).enumerate()
            {
                let instance = project
                    .plugin_instances
                    .iter()
                    .find(|p| p.id == placement.plugin_instance_id)
                    .ok_or_else(|| invalid("Serial Whole captured Insert is missing"))?;
                if slot.request.instance_id != placement.plugin_instance_id
                    || slot.request.config.enabled != instance.enabled
                    || slot.request.config.bypassed != instance.bypass
                    || slot.request.config.wet != instance.wet
                    || slot.request.path != instance.path
                    || slot
                        .request
                        .parameter_ids
                        .iter()
                        .copied()
                        .ne(instance.parameters.keys().copied())
                    || chain.receipt.endpoint.manifest().instance_id(index)
                        != Some(slot.request.instance_id)
                    || chain.receipt.endpoint.slot(index).is_none_or(|endpoint| {
                        endpoint.is_active() != (instance.enabled && !instance.bypass)
                            || endpoint.is_active()
                                && endpoint.latency_samples() != slot.state.metadata.latency_samples
                    })
                {
                    return Err(invalid(
                        "Serial Whole captured Insert config differs from the frozen graph",
                    ));
                }
                slot.state
                    .validate(&slot.request.parameter_ids)
                    .map_err(|e| invalid(&e))?;
                if slot.state.metadata.sample_rate != f64::from(prepared.sample_rate)
                    || slot.state.metadata.tempo != prepared.tempo
                {
                    return Err(invalid(
                        "Serial Whole source timing differs across endpoints",
                    ));
                }
                if slot.request.config.enabled && !slot.request.config.bypassed {
                    chain_tail = chain_tail.saturating_add(slot.state.metadata.tail_samples);
                    latency = latency
                        .checked_add(u64::from(slot.state.metadata.latency_samples))
                        .ok_or_else(|| invalid("Serial Whole total DSP latency overflow"))?;
                    reported_tail = reported_tail.saturating_add(slot.state.metadata.tail_samples);
                }
            }
            if chain_tail != chain.receipt.endpoint.tail_samples() {
                return Err(invalid(
                    "Serial Whole captured tail differs from its exact source endpoint",
                ));
            }
        }
        let total_latency = u32::try_from(latency)
            .map_err(|_| invalid("Serial Whole total DSP latency exceeds its bounded range"))?;
        let (_, generated) =
            render_frame_bounds(prepared.song_frames, prepared.tail_frames, total_latency)?;
        prepared.generated_frames = generated;
        let raw_frames = usize::try_from(generated)
            .map_err(|_| invalid("Serial Whole raw frames exceed platform capacity"))?;
        // The original cropped Instrument/Occurrence entry points are unchanged.
        let mut raw = run_with_disposal(
            control,
            progress,
            generator,
            |backend| render_audio_interval(&capture, &prepared, backend, control, 0, raw_frames),
            |backend, deadline| backend.dispose(deadline),
        )?;
        control.checkpoint()?;
        let mut spare = Vec::new();
        spare
            .try_reserve_exact(raw_frames)
            .map_err(|e| invalid(&format!("Serial Whole spare allocation failed: {e}")))?;
        apply_controls(
            &mut raw,
            mix.channel_volume,
            mix.channel_pan_left,
            mix.channel_pan_right,
            mix.channel_audible,
            true,
            control,
        )?;
        let mut chain_iter = chains.iter();
        let mut upstream_latency = u64::from(prepared.metadata.latency_samples);
        for node in &mix.nodes {
            if !node.placements.is_empty() {
                let chain = chain_iter
                    .next()
                    .ok_or_else(|| invalid("Serial Whole endpoint receipt disappeared"))?;
                for slot in &chain.receipt.slots {
                    if !slot.request.config.enabled || slot.request.config.bypassed {
                        continue;
                    }
                    spare.clear();
                    run_with_disposal(
                        control,
                        progress,
                        &mut effects,
                        |backend| {
                            render_effect(
                                slot,
                                &raw,
                                &mut spare,
                                upstream_latency,
                                &prepared,
                                backend,
                                control,
                            )
                        },
                        |backend, deadline| backend.dispose(deadline),
                    )?;
                    // run_with_disposal has completed native reap AND backend destruction.
                    std::mem::swap(&mut raw, &mut spare);
                    upstream_latency += u64::from(slot.state.metadata.latency_samples);
                }
                apply_gain(&mut raw, 1.0, 1.0, true, control)?;
            }
            if !node.is_master {
                apply_controls(
                    &mut raw,
                    node.volume,
                    node.pan_left,
                    node.pan_right,
                    node.audible,
                    false,
                    control,
                )?;
                apply_gain(
                    &mut raw,
                    node.outgoing_gain,
                    node.outgoing_gain,
                    false,
                    control,
                )?;
            }
        }
        let start =
            usize::try_from(latency).map_err(|_| invalid("Serial Whole crop offset overflow"))?;
        let end = start
            .checked_add(prepared.retained_frames)
            .filter(|end| *end <= raw.len())
            .ok_or_else(|| invalid("Serial Whole final crop exceeds raw PCM"))?;
        raw.copy_within(start..end, 0);
        raw.truncate(prepared.retained_frames);
        drop(spare);
        let peak = checked_master_dry_peak(&raw, control)?;
        let dry_render = RenderReport {
            identity: prepared.identity,
            origin_frame: 0,
            musical_frames: prepared.song_frames,
            frames: raw.len(),
            native_latency: total_latency,
            reported_tail,
            fixed_tail: prepared.tail_frames,
            generated_frames: generated,
            sample_peak: peak,
        };
        let master_output = mix
            .publish(
                raw,
                options,
                &path,
                reported_tail,
                master_controls,
                &control.export,
                control.deadline,
            )
            .map_err(writer_failure)?;
        Ok(MasterSongRenderReport {
            dry_render,
            master_output,
            sample_format: options.sample_format,
        })
    })();
    match result {
        Ok(report) => MasterSongRenderOutcome::Complete(report),
        Err(error) => MasterSongRenderOutcome::Failed(error),
    }
}

fn apply_gain(
    audio: &mut [StereoFrame],
    left: f32,
    right: f32,
    clamp: bool,
    control: &RenderControl,
) -> Result<(), RenderFailure> {
    for block in audio.chunks_mut(4096) {
        control.checkpoint()?;
        for sample in block {
            if clamp {
                sample.left = sample.left.clamp(-8.0, 8.0);
                sample.right = sample.right.clamp(-8.0, 8.0);
            }
            sample.left *= left;
            sample.right *= right;
            if !sample.left.is_finite() || !sample.right.is_finite() {
                return Err(RenderFailure::new(
                    FailureKind::Invalid,
                    "Serial Whole controls produced non-finite PCM",
                ));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_controls(
    audio: &mut [StereoFrame],
    volume: f32,
    left: f32,
    right: f32,
    audible: bool,
    clamp: bool,
    control: &RenderControl,
) -> Result<(), RenderFailure> {
    for block in audio.chunks_mut(4096) {
        control.checkpoint()?;
        for sample in block {
            if !audible {
                *sample = StereoFrame::default();
                continue;
            }
            if clamp {
                sample.left = sample.left.clamp(-8.0, 8.0);
                sample.right = sample.right.clamp(-8.0, 8.0);
            }
            sample.left = sample.left * volume * left;
            sample.right = sample.right * volume * right;
            if !sample.left.is_finite() || !sample.right.is_finite() {
                return Err(RenderFailure::new(
                    FailureKind::Invalid,
                    "Serial Whole controls produced non-finite PCM",
                ));
            }
        }
    }
    Ok(())
}

fn render_effect<E: EffectRenderBackend>(
    slot: &CapturedEffectSlot,
    raw: &[StereoFrame],
    spare: &mut Vec<StereoFrame>,
    upstream_latency: u64,
    prepared: &PreparedRender,
    backend: &mut E,
    control: &RenderControl,
) -> Result<(), RenderFailure> {
    control.call_checked(
        || backend.prepare(slot, control),
        |metadata| {
            let mut expected = slot.state.metadata.clone();
            expected.async_started_revision = metadata.async_started_revision;
            if metadata != &expected {
                return Err(RenderFailure::new(
                    FailureKind::Invalid,
                    "Strict Effect metadata differs from the captured source",
                ));
            }
            Ok(())
        },
    )?;
    let mut input = vec![vec![0.0; BOUNCE_QUANTUM]; 2];
    for (block_index, frames) in raw.as_chunks::<BOUNCE_QUANTUM>().0.iter().enumerate() {
        for (index, sample) in frames.iter().enumerate() {
            input[0][index] = sample.left;
            input[1][index] = sample.right;
        }
        let position = i64::try_from(block_index * BOUNCE_QUANTUM)
            .ok()
            .and_then(|p| p.checked_sub(upstream_latency as i64))
            .ok_or_else(|| {
                RenderFailure::new(FailureKind::Invalid, "Serial Effect transport overflow")
            })?;
        let transport = PluginTransport {
            sample_position: position,
            quarter_note_position: position as f64 * prepared.tempo
                / (f64::from(prepared.sample_rate) * 60.0),
            tempo: prepared.tempo,
            playing: true,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
        };
        let block = control.call_checked(
            || backend.process(&input, transport),
            |block| {
                if !block.transport_applied
                    || block.output_events != 0
                    || block.output_events_lost
                    || block.channels.len() != 2
                    || block
                        .channels
                        .iter()
                        .any(|c| c.len() != BOUNCE_QUANTUM || c.iter().any(|v| !v.is_finite()))
                {
                    return Err(RenderFailure::new(
                        FailureKind::Invalid,
                        "Strict Effect returned invalid stereo/transport/events/PCM",
                    ));
                }
                Ok(())
            },
        )?;
        for index in 0..BOUNCE_QUANTUM {
            spare.push(StereoFrame {
                left: block.channels[0][index],
                right: block.channels[1][index],
            });
        }
    }
    Ok(())
}
