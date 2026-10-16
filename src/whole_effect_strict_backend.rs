//! Renderer-owned strict Effect adapter. The generator backend remains a separate contract.

use super::StrictBackend;
use crate::{
    instrument_bounce::{
        BOUNCE_QUANTUM, BounceAudioBus,
        effect::{CapturedEffectSlot, EffectSourceIdentity, EffectSourceMetadata},
        render::{BackendFailure, BlockReply, DisposalObservation, RenderBackend, RenderControl},
    },
    plugins::plugin_runtime::{
        MAX_PLUGIN_PARAMETER_CATALOG_ITEMS, MAX_PLUGIN_PARAMETER_NAME_BYTES,
        MAX_PLUGIN_PARAMETER_UNIT_BYTES, PluginParameterDescriptor, PluginTransport,
    },
};
use std::{path::PathBuf, time::Instant};
use vst3_host::{
    ProcessTransport,
    parameters::Parameter,
    strict_offline::{Client, EffectIdentity, StereoEffectMetadata},
};

/// Empty until prepare runs on the renderer owner. The retained StrictBackend
/// supplies the same cancellation, absolute budget and concrete-child cleanup.
pub(crate) struct StrictEffectBackend {
    owner: StrictBackend,
    helper_override: Option<PathBuf>,
}

impl StrictEffectBackend {
    pub(crate) fn new(helper_override: Option<PathBuf>) -> Self {
        Self {
            owner: StrictBackend::new(),
            helper_override,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        captured: &CapturedEffectSlot,
        control: &RenderControl,
    ) -> Result<EffectSourceMetadata, BackendFailure> {
        let result = self.prepare_inner(captured, control);
        self.seal_failure(result)
    }

    fn prepare_inner(
        &mut self,
        captured: &CapturedEffectSlot,
        control: &RenderControl,
    ) -> Result<EffectSourceMetadata, BackendFailure> {
        StrictBackend::checkpoint(control)?;
        if self.owner.prepare_attempted
            || self.owner.terminal_exchange
            || self.owner.disposal_begun
            || self.owner.terminated
        {
            return Err(StrictBackend::admission(
                "Strict Effect preparation is one-shot",
            ));
        }
        self.owner.prepare_attempted = true;
        self.owner.control = Some(control.clone());
        captured
            .state
            .validate(&captured.request.parameter_ids)
            .map_err(StrictBackend::admission)?;
        let source = &captured.state.metadata;
        if captured.request.path != source.path
            || captured.request.requested_class_uid != source.requested_class_uid
            || !captured.request.config.enabled
            || captured.request.config.bypassed
            || captured.request.config.wet != 1.0
        {
            return Err(StrictBackend::admission(
                "Strict Effect requires its captured source binding and an enabled, unbypassed, fully wet slot",
            ));
        }
        self.owner.sample_rate = source.sample_rate as u32;
        let helper = match &self.helper_override {
            Some(helper) => helper.clone(),
            None => {
                crate::plugins::installed_vst3_helper_path().map_err(StrictBackend::admission)?
            }
        };
        if !helper.is_file() {
            return Err(StrictBackend::admission("Strict Effect helper is absent"));
        }
        if !source.path.exists() {
            return Err(StrictBackend::admission(
                "Strict Effect captured plug-in path no longer exists",
            ));
        }
        let path = source
            .path
            .to_str()
            .ok_or_else(|| StrictBackend::admission("Strict Effect plug-in path is not UTF-8"))?;
        let identity = EffectIdentity {
            class_id: source.identity.class_uid.clone(),
            vendor: source.identity.vendor.clone(),
            name: source.identity.name.clone(),
            version: source.identity.version.clone(),
            category: source.identity.category.clone(),
        };
        self.owner.spawn_owned_client(&helper, control)?;
        let loaded = self
            .owner
            .exchange_at("effect.prepare.load_plugin", |client, deadline| {
                client.load_stereo_effect(
                    vst3_host::strict_offline::LoadSpec {
                        path,
                        sample_rate: source.sample_rate,
                        tempo: source.tempo,
                        time_sig_numerator: source.time_sig_numerator,
                        time_sig_denominator: source.time_sig_denominator,
                        class_id: &source.identity.class_uid,
                    },
                    &identity,
                    deadline,
                )
            })?;
        self.owner.loaded = Some(loaded);
        self.owner
            .exchange_at("effect.prepare.set_offline", Client::set_offline)?;
        self.owner
            .exchange_at("effect.prepare.load_state", |client, deadline| {
                client.load_state(&captured.state.opaque_state, deadline)
            })?;
        self.owner
            .wait_async_state_ready("effect.prepare.post_state_async_snapshot")?;

        // Prove the complete schema before the first base setter. Catalog
        // current values are not schema: read-only meters may change at rest.
        self.verify_catalog(captured, false)?;
        for (&id, &value) in &captured.state.parameter_bases {
            self.owner
                .exchange_at("effect.prepare.set_parameter_f64", |client, deadline| {
                    client.set_parameter_f64(id, value, deadline)
                })?;
        }
        let async_revision = self
            .owner
            .wait_async_state_ready("effect.prepare.post_bases_async_snapshot")?;
        let dirty_revision = self.owner.exchange_at(
            "effect.prepare.native_dirty_revision",
            Client::native_dirty_revision,
        )?;
        self.verify_bases(captured)?;
        self.verify_catalog(captured, true)?;
        self.verify_clone_revisions(async_revision, dirty_revision)?;

        let declared = self.owner.exchange_at(
            "effect.prepare.checked_audio_layout",
            Client::checked_audio_layout,
        )?;
        if !audio_descriptors_match(&declared.inputs, &source.input_buses)
            || !audio_descriptors_match(&declared.outputs, &source.output_buses)
        {
            return Err(StrictBackend::admission(
                "Strict Effect complete audio descriptors differ from the source",
            ));
        }
        let selected = self
            .owner
            .exchange_at("effect.prepare.select_stereo", |client, deadline| {
                client.select_main_stereo_output(&declared, deadline)
            })?;
        if audio_buses(&selected.inputs) != source.input_buses
            || audio_buses(&selected.outputs) != source.output_buses
        {
            return Err(StrictBackend::admission(
                "Strict Effect acknowledged audio participation differs from the source",
            ));
        }
        let admitted = self
            .owner
            .exchange_at("effect.prepare.admit_stopped", Client::admit_stereo_effect)?;
        let metadata = self.clone_metadata(&admitted, source)?;
        if !source_configuration_matches(&metadata, source)
            || admitted.state_bytes != captured.state.opaque_state.len()
            || admitted.async_started_revision != async_revision
            || admitted.native_dirty_revision != dirty_revision
        {
            return Err(StrictBackend::admission(
                "Strict Effect stopped clone identity, configuration, topology, latency, tail or revision differs",
            ));
        }
        self.owner.prepared_dirty_revision = Some(dirty_revision);
        self.owner.prepared_async_revision = Some(async_revision);

        // The Client has armed its own fresh revision. Source revision numbers
        // are instance-local and are never substituted for this clone's guard.
        self.owner
            .exchange_at("effect.prepare.start_processing", Client::start_processing)?;
        self.owner.started = true;
        self.verify_bases(captured)?;
        self.verify_catalog(captured, true)?;
        self.verify_clone_revisions(async_revision, dirty_revision)?;
        self.owner.input_channels = 2;
        self.owner.output_channels = 2;
        self.owner.prepared = true;
        Ok(metadata)
    }

    fn verify_catalog(
        &mut self,
        captured: &CapturedEffectSlot,
        restored: bool,
    ) -> Result<(), BackendFailure> {
        let parameters = self
            .owner
            .exchange_at("effect.prepare.parameters", Client::parameters)?;
        let catalog = bounded_catalog(parameters)?;
        if !catalog_schema_matches(&catalog, &captured.state.catalog)
            || (restored
                && catalog.iter().filter(|p| !p.read_only).any(|p| {
                    captured
                        .state
                        .parameter_bases
                        .get(&p.id)
                        .is_none_or(|value| {
                            (*value as f32).to_bits() != p.current_normalized.to_bits()
                        })
                }))
        {
            return Err(StrictBackend::admission(
                "Strict Effect complete catalog schema or restored writable values differ",
            ));
        }
        Ok(())
    }

    fn verify_bases(&mut self, captured: &CapturedEffectSlot) -> Result<(), BackendFailure> {
        // Read every base after all setters so cross-parameter side effects
        // cannot hide behind an earlier successful readback.
        for (&id, &expected) in &captured.state.parameter_bases {
            let actual = self
                .owner
                .exchange_at("effect.prepare.parameter_readback", |client, deadline| {
                    client.parameter(id, deadline)
                })?;
            if actual.to_bits() != expected.to_bits() {
                return Err(StrictBackend::admission(format!(
                    "Strict Effect captured f64 base {id} did not read back exactly"
                )));
            }
        }
        Ok(())
    }

    fn verify_clone_revisions(
        &mut self,
        async_revision: u64,
        dirty_revision: u64,
    ) -> Result<(), BackendFailure> {
        let actual_dirty = self.owner.exchange_at(
            "effect.prepare.verify_native_dirty_revision",
            Client::native_dirty_revision,
        )?;
        let actual_async = self.owner.exchange_at(
            "effect.prepare.verify_async_snapshot",
            Client::async_state_snapshot,
        )?;
        actual_async
            .ensure_ready()
            .map_err(|error| StrictBackend::admission(error.to_string()))?;
        if actual_dirty != dirty_revision || actual_async.async_started_revision != async_revision {
            return Err(StrictBackend::admission(
                "Strict Effect clone changed during exact restoration",
            ));
        }
        Ok(())
    }

    fn clone_metadata(
        &self,
        admitted: &StereoEffectMetadata,
        source: &EffectSourceMetadata,
    ) -> Result<EffectSourceMetadata, BackendFailure> {
        let loaded = self.owner.loaded.as_ref().ok_or_else(|| {
            StrictBackend::admission("Strict Effect has no actual loaded factory identity")
        })?;
        let metadata = EffectSourceMetadata {
            identity: EffectSourceIdentity {
                class_uid: loaded.uid.clone(),
                vendor: loaded.vendor.clone(),
                name: loaded.name.clone(),
                version: loaded.version.clone(),
                category: loaded.category.clone(),
            },
            path: PathBuf::from(&admitted.path),
            requested_class_uid: source.requested_class_uid.clone(),
            sample_rate: admitted.sample_rate,
            max_block_frames: admitted.max_block_frames as usize,
            tempo: admitted.tempo,
            time_sig_numerator: admitted.time_sig_numerator,
            time_sig_denominator: admitted.time_sig_denominator,
            input_buses: audio_buses(&admitted.audio.inputs),
            output_buses: audio_buses(&admitted.audio.outputs),
            event_input_buses: admitted
                .events
                .inputs
                .iter()
                .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                .collect(),
            event_output_buses: admitted
                .events
                .outputs
                .iter()
                .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                .collect(),
            latency_samples: admitted.latency_samples,
            tail_samples: admitted.tail_samples,
            async_started_revision: admitted.async_started_revision,
        };
        metadata.validate().map_err(StrictBackend::admission)?;
        Ok(metadata)
    }

    pub(crate) fn process(
        &mut self,
        input: &[Vec<f32>],
        transport: PluginTransport,
    ) -> Result<BlockReply, BackendFailure> {
        let result = (|| {
            self.owner.ready()?;
            if input.len() != 2
                || input.iter().any(|channel| channel.len() != BOUNCE_QUANTUM)
                || input.iter().flatten().any(|sample| !sample.is_finite())
            {
                return Err(StrictBackend::admission(
                    "Strict Effect requires finite genuine stereo Q128 PCM",
                ));
            }
            let transport = ProcessTransport {
                sample_position: transport.sample_position,
                quarter_note_position: transport.quarter_note_position,
                tempo: transport.tempo,
                playing: transport.playing,
                time_sig_numerator: transport.time_sig_numerator,
                time_sig_denominator: transport.time_sig_denominator,
            };
            // The strict Effect client rechecks its admitted complete metadata
            // and clone-local native/async guards before and after each block.
            let audio = self
                .owner
                .exchange_at("effect.render.process", |client, deadline| {
                    client.process_stereo_effect(input, transport, deadline)
                })?;
            Ok(BlockReply {
                channels: audio.outputs,
                transport_applied: audio.transport_applied,
                output_events: audio.output_event_count,
                output_events_lost: audio.output_events_lost,
            })
        })();
        self.seal_failure(result)
    }

    /// A local refusal seals admission just like a failed exchange. The same
    /// owner remains available to the renderer's checked disposal loop.
    fn seal_failure<T>(&mut self, result: Result<T, BackendFailure>) -> Result<T, BackendFailure> {
        if result.is_err() {
            self.owner.terminal_exchange = true;
        }
        result
    }

    pub(crate) fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        RenderBackend::dispose(&mut self.owner, deadline)
    }

    #[cfg(test)]
    pub(crate) fn test_child_completion(&self) -> (usize, Option<std::process::ExitStatus>) {
        self.owner.test_child_completion()
    }

    /// Read-only SDK evidence on this exact renderer owner, with the original
    /// readiness, cancellation, deadline and terminal-exchange rules intact.
    #[cfg(test)]
    pub(crate) fn test_parameter_readback(&mut self, id: u32) -> Result<f64, BackendFailure> {
        let result = (|| {
            self.owner.ready()?;
            self.owner
                .exchange_at("effect.sdk_probe.parameter", |client, deadline| {
                    client.parameter(id, deadline)
                })
        })();
        self.seal_failure(result)
    }
}

fn audio_buses(buses: &[vst3_host::plugin::CheckedAudioBusConfig]) -> Vec<BounceAudioBus> {
    buses
        .iter()
        .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
        .collect()
}

fn audio_descriptors_match(
    clone: &[vst3_host::plugin::CheckedAudioBusConfig],
    source: &[BounceAudioBus],
) -> bool {
    clone.len() == source.len()
        && clone.iter().zip(source).all(|(clone, source)| {
            clone.channel_count == source.channel_count && clone.is_main == source.is_main
        })
}

fn source_configuration_matches(
    actual: &EffectSourceMetadata,
    source: &EffectSourceMetadata,
) -> bool {
    // Async revisions belong to their own instances. Every other source field
    // has an actual clone authority or is the preserved requested-class binding.
    actual.identity == source.identity
        && actual.path == source.path
        && actual.requested_class_uid == source.requested_class_uid
        && actual.sample_rate == source.sample_rate
        && actual.max_block_frames == source.max_block_frames
        && actual.tempo == source.tempo
        && actual.time_sig_numerator == source.time_sig_numerator
        && actual.time_sig_denominator == source.time_sig_denominator
        && actual.input_buses == source.input_buses
        && actual.output_buses == source.output_buses
        && actual.event_input_buses == source.event_input_buses
        && actual.event_output_buses == source.event_output_buses
        && actual.latency_samples == source.latency_samples
        && actual.tail_samples == source.tail_samples
}

pub(super) fn bounded_catalog(
    parameters: Vec<Parameter>,
) -> Result<Vec<PluginParameterDescriptor>, BackendFailure> {
    if parameters.len() > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
        return Err(StrictBackend::admission(
            "Strict Effect parameter catalog exceeds the capture bound",
        ));
    }
    let mut catalog = Vec::with_capacity(parameters.len());
    for parameter in parameters {
        if !parameter.value.is_finite()
            || !(0.0..=1.0).contains(&parameter.value)
            || !parameter.default.is_finite()
            || !(0.0..=1.0).contains(&parameter.default)
        {
            return Err(StrictBackend::admission(
                "Strict Effect catalog contains invalid normalized values",
            ));
        }
        let mut descriptor = PluginParameterDescriptor {
            id: parameter.id,
            name: parameter.name,
            unit: parameter.unit,
            current_normalized: parameter.value as f32,
            default_normalized: Some(parameter.default as f32),
            step_count: u32::try_from(parameter.step_count).ok(),
            automatable: parameter.can_automate,
            read_only: parameter.is_read_only,
            bypass: parameter.is_bypass,
        };
        truncate_text(&mut descriptor.name, MAX_PLUGIN_PARAMETER_NAME_BYTES);
        truncate_text(&mut descriptor.unit, MAX_PLUGIN_PARAMETER_UNIT_BYTES);
        catalog.push(descriptor);
    }
    catalog.sort_by_key(|parameter| parameter.id);
    if catalog.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return Err(StrictBackend::admission(
            "Strict Effect catalog has duplicate parameter identities",
        ));
    }
    Ok(catalog)
}

fn truncate_text(text: &mut String, maximum_bytes: usize) {
    let mut end = text.len().min(maximum_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

pub(super) fn catalog_schema_matches(
    actual: &[PluginParameterDescriptor],
    source: &[PluginParameterDescriptor],
) -> bool {
    actual.len() == source.len()
        && actual.iter().zip(source).all(|(actual, source)| {
            actual.id == source.id
                && actual.name == source.name
                && actual.unit == source.unit
                && actual.default_normalized.map(f32::to_bits)
                    == source.default_normalized.map(f32::to_bits)
                && actual.step_count == source.step_count
                && actual.automatable == source.automatable
                && actual.read_only == source.read_only
                && actual.bypass == source.bypass
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        export_job::ExportControl,
        instrument_bounce::{
            BOUNCE_PREPARED_MAXIMUM,
            effect::{EffectCaptureSlotRequest, EffectPluginState},
        },
        plugins::plugin_runtime::SlotConfig,
    };
    use std::{collections::BTreeMap, time::Duration};

    fn parameter(id: u32) -> Parameter {
        Parameter {
            id,
            name: format!("Parameter {id}"),
            value: 0.123_456_789_123_456_78,
            min: 0.0,
            max: 1.0,
            default: 0.75,
            unit: "dB".into(),
            step_count: 0,
            can_automate: true,
            is_read_only: false,
            is_bypass: false,
            flags: 1,
        }
    }

    fn metadata() -> EffectSourceMetadata {
        EffectSourceMetadata {
            identity: EffectSourceIdentity {
                class_uid: "43545253454658440000000000000001".into(),
                vendor: "Citrus Studio Test Fixtures".into(),
                name: "Citrus Stereo Delay Fixture".into(),
                version: "1.0.0".into(),
                category: "Fx|Delay".into(),
            },
            path: PathBuf::from("source-only-effect.vst3"),
            requested_class_uid: None,
            sample_rate: 48_000.0,
            max_block_frames: BOUNCE_PREPARED_MAXIMUM,
            tempo: 128.0,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            input_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            event_input_buses: Vec::new(),
            event_output_buses: Vec::new(),
            latency_samples: 32,
            tail_samples: 160,
            async_started_revision: 17,
        }
    }

    #[test]
    fn effect_catalog_uses_the_same_bounded_projection_as_capture() {
        let mut long = parameter(9);
        long.name = "界".repeat(MAX_PLUGIN_PARAMETER_NAME_BYTES);
        long.unit = "界".repeat(MAX_PLUGIN_PARAMETER_UNIT_BYTES);
        long.step_count = -1;
        let value = long.value;
        let catalog = bounded_catalog(vec![long, parameter(3)]).unwrap();
        assert_eq!(catalog.iter().map(|p| p.id).collect::<Vec<_>>(), [3, 9]);
        assert_eq!(
            catalog[1].name,
            "界".repeat(MAX_PLUGIN_PARAMETER_NAME_BYTES / 3)
        );
        assert_eq!(
            catalog[1].unit,
            "界".repeat(MAX_PLUGIN_PARAMETER_UNIT_BYTES / 3)
        );
        assert_eq!(catalog[1].step_count, None);
        assert_eq!(catalog[1].default_normalized, Some(0.75));
        assert_eq!(
            catalog[1].current_normalized.to_bits(),
            (value as f32).to_bits()
        );
        assert_ne!(f64::from(value as f32).to_bits(), value.to_bits());
    }

    #[test]
    fn effect_schema_checks_every_captured_field_but_not_meter_currents() {
        let source = bounded_catalog(vec![parameter(1)]).unwrap();
        let mut current_changed = source.clone();
        current_changed[0].current_normalized = 0.9;
        assert!(catalog_schema_matches(&current_changed, &source));
        let changes: [fn(&mut PluginParameterDescriptor); 8] = [
            |p| p.id += 1,
            |p| p.name.push('x'),
            |p| p.unit.push('x'),
            |p| p.default_normalized = Some(0.5),
            |p| p.step_count = Some(1),
            |p| p.automatable = !p.automatable,
            |p| p.read_only = !p.read_only,
            |p| p.bypass = !p.bypass,
        ];
        for change in changes {
            let mut changed = source.clone();
            change(&mut changed[0]);
            assert!(!catalog_schema_matches(&changed, &source));
        }
        assert!(!catalog_schema_matches(&[], &source));
        let mut readonly = parameter(1000);
        readonly.is_read_only = true;
        let original = bounded_catalog(vec![readonly.clone()]).unwrap();
        readonly.value = 0.5;
        assert!(catalog_schema_matches(
            &bounded_catalog(vec![readonly]).unwrap(),
            &original
        ));
    }

    #[test]
    fn effect_catalog_rejects_duplicates_oversize_and_invalid_scalars() {
        assert!(bounded_catalog(vec![parameter(1), parameter(1)]).is_err());
        assert!(
            bounded_catalog(vec![parameter(0); MAX_PLUGIN_PARAMETER_CATALOG_ITEMS + 1]).is_err()
        );
        for invalid in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            let mut current = parameter(1);
            current.value = invalid;
            assert!(bounded_catalog(vec![current]).is_err());
            let mut default = parameter(1);
            default.default = invalid;
            assert!(bounded_catalog(vec![default]).is_err());
        }
    }

    #[test]
    fn effect_configuration_compares_all_source_fields_except_instance_local_revision() {
        let source = metadata();
        let mut clone = source.clone();
        clone.async_started_revision = 0;
        assert!(source_configuration_matches(&clone, &source));
        let changes: [fn(&mut EffectSourceMetadata); 19] = [
            |m| m.identity.class_uid.replace_range(..1, "0"),
            |m| m.identity.vendor.push('x'),
            |m| m.identity.name.push('x'),
            |m| m.identity.version.push('x'),
            |m| m.identity.category.push('x'),
            |m| m.path.push("other"),
            |m| m.requested_class_uid = Some(m.identity.class_uid.clone()),
            |m| m.sample_rate += 1.0,
            |m| m.max_block_frames += 1,
            |m| m.tempo += 1.0,
            |m| m.time_sig_numerator += 1,
            |m| m.time_sig_denominator *= 2,
            |m| m.input_buses[0].active = None,
            |m| m.output_buses[0].channel_count = 1,
            |m| m.event_input_buses.push((1, true, Some(true))),
            |m| m.event_output_buses.push((1, true, Some(true))),
            |m| m.latency_samples += 1,
            |m| m.tail_samples = u32::MAX,
            |m| {
                m.input_buses
                    .push(BounceAudioBus::new(1, false, Some(false)))
            },
        ];
        for change in changes {
            let mut changed = source.clone();
            change(&mut changed);
            assert!(!source_configuration_matches(&changed, &source));
        }
    }

    #[test]
    fn effect_preselection_checks_complete_descriptors_without_guessing_activation() {
        let source = [BounceAudioBus::new(2, true, Some(true))];
        let clone = [vst3_host::plugin::CheckedAudioBusConfig {
            channel_count: 2,
            is_main: true,
            active: None,
        }];
        assert!(audio_descriptors_match(&clone, &source));
        assert_ne!(audio_buses(&clone), source);
        assert!(!audio_descriptors_match(&[], &source));
        assert!(!audio_descriptors_match(
            &[clone[0].clone(), clone[0].clone()],
            &source
        ));
    }

    #[test]
    fn effect_local_prepare_refusal_is_one_shot_and_empty_cleanup_claims_no_reap() {
        let metadata = metadata();
        let captured = CapturedEffectSlot {
            request: EffectCaptureSlotRequest {
                instance_id: 1,
                config: SlotConfig::default(),
                path: metadata.path.clone(),
                requested_class_uid: None,
                parameter_ids: Vec::new(),
            },
            state: EffectPluginState {
                metadata,
                opaque_state: Vec::new(),
                catalog_revision: 1,
                catalog: Vec::new(),
                parameter_bases: BTreeMap::new(),
                native_generation: 0,
                native_dirty_revision: 0,
            },
        };
        let control = RenderControl {
            export: ExportControl::default(),
            deadline: Instant::now() + Duration::from_secs(300),
        };
        let mut backend = StrictEffectBackend::new(None);
        assert_eq!(backend.test_child_completion(), (0, None));
        assert!(backend.prepare(&captured, &control).is_err());
        assert!(backend.owner.prepare_attempted && backend.owner.terminal_exchange);
        assert!(backend.prepare(&captured, &control).is_err());
        let cleanup = backend.dispose(Instant::now());
        assert!(cleanup.terminated && cleanup.error.is_none());
        assert_eq!(backend.test_child_completion(), (0, None));
    }

    #[test]
    fn effect_local_process_refusal_seals_the_empty_owner() {
        let mut backend = StrictEffectBackend::new(None);
        assert!(backend.process(&[], PluginTransport::default()).is_err());
        assert!(backend.owner.terminal_exchange);
        assert_eq!(backend.test_child_completion(), (0, None));
    }
}
