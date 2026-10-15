//! Read-only, stopped real source capture. No normal save publication, editor Close or DSP.
use super::*;
use crate::instrument_bounce::effect::{
    EffectPluginState, EffectSourceIdentity, EffectSourceMetadata,
};
use BounceCaptureError::{BackendFailed, Rejected};

type EffectCatalogSnapshot = (
    u64,
    Vec<super::super::PluginParameterDescriptor>,
    BTreeMap<u32, f64>,
);

impl Vst3Backend {
    fn private_effect_metadata(
        &mut self,
        fence: &BounceCaptureFence,
    ) -> Result<EffectSourceMetadata, BounceCaptureError> {
        let before = capture_call(Some(fence), || {
            self.plugin
                .async_state_snapshot()
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        before.ensure_ready().map_err(|e| Rejected(e.to_string()))?;
        let audio = capture_call(Some(fence), || {
            self.plugin
                .checked_audio_bus_layout()
                .map_err(classify_metadata_error)
        })?;
        let events = capture_call(Some(fence), || {
            self.plugin
                .event_bus_layout()
                .map_err(classify_metadata_error)
        })?;
        let latency_samples = capture_call(Some(fence), || {
            self.plugin
                .try_latency_samples()
                .map_err(classify_metadata_error)
        })?;
        let tail_samples = capture_call(Some(fence), || {
            self.plugin
                .try_tail_samples()
                .map_err(classify_metadata_error)
        })?;
        let after = capture_call(Some(fence), || {
            self.plugin
                .async_state_snapshot()
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        after.ensure_ready().map_err(|e| Rejected(e.to_string()))?;
        if before.async_started_revision != after.async_started_revision {
            return Err(Rejected(
                "Effect source async restoration changed during capture".into(),
            ));
        }
        let info = self.plugin.info();
        if info.has_midi_input || info.has_midi_output {
            return Err(Rejected("Effect source declares MIDI capability".into()));
        }
        let metadata = EffectSourceMetadata {
            identity: EffectSourceIdentity {
                class_uid: info.uid.clone(),
                vendor: info.vendor.clone(),
                name: info.name.clone(),
                version: info.version.clone(),
                category: info.category.clone(),
            },
            path: self.loaded_path.clone(),
            requested_class_uid: self.requested_class_uid.clone(),
            sample_rate: self.plugin.sample_rate(),
            max_block_frames: self.max_block_frames,
            tempo: self.last_transport.tempo,
            time_sig_numerator: self.last_transport.time_sig_numerator,
            time_sig_denominator: self.last_transport.time_sig_denominator,
            input_buses: audio
                .inputs
                .iter()
                .map(|b| BounceAudioBus::new(b.channel_count, b.is_main, b.active))
                .collect(),
            output_buses: audio
                .outputs
                .iter()
                .map(|b| BounceAudioBus::new(b.channel_count, b.is_main, b.active))
                .collect(),
            event_input_buses: events
                .inputs
                .iter()
                .map(|b| (b.channel_count, b.is_main, b.active))
                .collect(),
            event_output_buses: events
                .outputs
                .iter()
                .map(|b| (b.channel_count, b.is_main, b.active))
                .collect(),
            latency_samples,
            tail_samples,
            async_started_revision: before.async_started_revision,
        };
        metadata.validate().map_err(Rejected)?;
        Ok(metadata)
    }

    fn private_effect_catalog(
        &mut self,
        fence: &BounceCaptureFence,
    ) -> Result<EffectCatalogSnapshot, BounceCaptureError> {
        let parameters = capture_call(Some(fence), || {
            self.plugin
                .get_parameters()
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        if parameters.len() > super::super::MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
            return Err(Rejected(
                "Effect source parameter catalog exceeds the bounded capture contract".into(),
            ));
        }
        let mut catalog = Vec::with_capacity(parameters.len());
        for parameter in parameters {
            if !parameter.value.is_finite()
                || !(0.0..=1.0).contains(&parameter.value)
                || !parameter.default.is_finite()
                || !(0.0..=1.0).contains(&parameter.default)
            {
                return Err(Rejected(
                    "Effect source catalog contains an invalid normalized value".into(),
                ));
            }
            let mut descriptor = super::super::PluginParameterDescriptor {
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
            super::super::bound_parameter_descriptor(&mut descriptor);
            catalog.push(descriptor);
        }
        catalog.sort_by_key(|p| p.id);
        if catalog.windows(2).any(|p| p[0].id >= p[1].id) {
            return Err(Rejected(
                "Effect source catalog has ambiguous parameter identities".into(),
            ));
        }
        let revision = super::super::parameter_catalog_revision(&catalog);
        let mut bases = BTreeMap::new();
        for parameter in catalog.iter().filter(|p| !p.read_only) {
            let value = capture_call(Some(fence), || {
                self.plugin
                    .get_parameter(parameter.id)
                    .map_err(|e| BackendFailed(e.to_string()))
            })?;
            if !value.is_finite()
                || !(0.0..=1.0).contains(&value)
                || value as f32 != parameter.current_normalized
            {
                return Err(Rejected(
                    "Effect source writable catalog/base readback changed or is invalid".into(),
                ));
            }
            bases.insert(parameter.id, value);
        }
        Ok((revision, catalog, bases))
    }

    pub(in crate::plugins::plugin_runtime) fn capture_private_effect_state(
        &mut self,
        required_ids: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<EffectPluginState, BounceCaptureError> {
        fence.checkpoint()?;
        if !self.prepared || self.last_transport.playing {
            return Err(Rejected(
                "Effect source has not reached stopped processing".into(),
            ));
        }
        let editor = capture_call(Some(fence), || {
            self.plugin
                .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        if editor.open {
            return Err(Rejected(
                "Close the source Effect native editor before capture".into(),
            ));
        }
        let revision = capture_call(Some(fence), || {
            self.plugin
                .native_dirty_revision()
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        let metadata = self.private_effect_metadata(fence)?;
        // The existing backend state call performs only its normal zero-frame flush. It never
        // enters capture_native_backend, closes the editor or publishes shared save state.
        let opaque_state = capture_call(Some(fence), || self.save_state().map_err(BackendFailed))?;
        let (catalog_revision, catalog, parameter_bases) = self.private_effect_catalog(fence)?;
        let state = EffectPluginState {
            metadata,
            opaque_state,
            catalog_revision,
            catalog,
            parameter_bases,
            native_generation: editor.generation,
            native_dirty_revision: revision,
        };
        state.validate(required_ids).map_err(Rejected)?;
        self.verify_private_effect_state(&state, fence)?;
        Ok(state)
    }

    pub(in crate::plugins::plugin_runtime) fn verify_private_effect_state(
        &mut self,
        state: &EffectPluginState,
        fence: &BounceCaptureFence,
    ) -> Result<(), BounceCaptureError> {
        fence.checkpoint()?;
        if !self.prepared || self.last_transport.playing {
            return Err(Rejected(
                "Effect source processing configuration changed".into(),
            ));
        }
        let before = capture_call(Some(fence), || {
            self.plugin
                .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        let revision = capture_call(Some(fence), || {
            self.plugin
                .native_dirty_revision()
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        let metadata = self.private_effect_metadata(fence)?;
        let (catalog_revision, catalog, bases) = self.private_effect_catalog(fence)?;
        let after_revision = capture_call(Some(fence), || {
            self.plugin
                .native_dirty_revision()
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        let after = capture_call(Some(fence), || {
            self.plugin
                .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        let async_after = capture_call(Some(fence), || {
            self.plugin
                .async_state_snapshot()
                .map_err(|e| BackendFailed(e.to_string()))
        })?;
        async_after
            .ensure_ready()
            .map_err(|e| Rejected(e.to_string()))?;
        if before.open
            || after.open
            || before.generation != state.native_generation
            || after.generation != state.native_generation
            || revision != state.native_dirty_revision
            || after_revision != state.native_dirty_revision
            || metadata != state.metadata
            || catalog_revision != state.catalog_revision
            || catalog != state.catalog
            || bases != state.parameter_bases
            || async_after.async_started_revision != state.metadata.async_started_revision
        {
            return Err(Rejected(
                "Effect source editor, revision, metadata, catalog or bases changed during capture"
                    .into(),
            ));
        }
        fence.checkpoint()
    }
}
