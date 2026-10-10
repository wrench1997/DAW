//! Thread-affine synchronous owner for one fresh isolated VST3 instrument.
//!
//! This is preparation/processing infrastructure only. No App action or WAV renderer calls it
//! yet. It deliberately bypasses the realtime bridge, never its backend/helper contracts.

use crate::instrument_bounce::{BouncePluginMetadata, CapturedInstrumentBounce};

#[cfg(all(test, feature = "vst3", unix))]
#[path = "offline_instrument_protocol_tests.rs"]
mod protocol_tests;

#[cfg(feature = "vst3")]
use {
    super::{PluginBackend, PluginPrepareConfig, Vst3Backend},
    crate::instrument_bounce::{
        BOUNCE_PREPARED_MAXIMUM, BounceCaptureError, BouncePluginState, MAX_BOUNCE_STATE_BYTES,
    },
    std::collections::BTreeMap,
};

/// Construct and use on the same background owner thread. The backend is deliberately !Send.
pub struct OfflineInstrument {
    #[cfg(feature = "vst3")]
    _backend: Vst3Backend,
    metadata: BouncePluginMetadata,
    _same_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl OfflineInstrument {
    /// A receipt is mandatory: stale Project.opaque_state or default-state fallback is impossible.
    pub fn from_capture(capture: &CapturedInstrumentBounce) -> Result<Self, String> {
        #[cfg(not(feature = "vst3"))]
        {
            let _ = capture;
            Err("This build does not include VST3 instrument bounce".into())
        }
        #[cfg(feature = "vst3")]
        {
            super::catch_backend(|| {
                Self::load_snapshot(
                    capture.render_spec(),
                    capture.plan().sample_rate(),
                    &capture.state,
                )
            })
        }
    }

    #[cfg(feature = "vst3")]
    fn load_snapshot(
        mut spec: super::PluginLoadSpec,
        sample_rate: u32,
        state: &BouncePluginState,
    ) -> Result<Self, String> {
        let ids: Vec<_> = state.parameter_bases.keys().copied().collect();
        state.validate(&ids, sample_rate)?;
        if spec.initial_state != state.opaque_state
            || spec.class_uid.as_deref() != Some(state.metadata.class_uid.as_str())
        {
            return Err(
                "Fresh bounce load differs from the authoritative captured state/class".into(),
            );
        }
        super::super::prepare_runtime_spec(&mut spec)?;
        let config = PluginPrepareConfig {
            sample_rate: f64::from(sample_rate),
            max_block_frames: BOUNCE_PREPARED_MAXIMUM,
        }
        .validate()?;
        let mut backend = Vst3Backend::load(spec, config)?;
        let identity = backend.plugin.info();
        if identity.uid != state.metadata.class_uid || identity.version != state.metadata.version {
            return Err(
                "The installed VST3 class/version differs from the authoritative captured source"
                    .into(),
            );
        }
        // Fresh load is inactive for processing. Set kOffline before restore/start, never
        // reconfigure the source instance or claim a successful realtime-mode bounce.
        backend
            .plugin
            .set_process_mode(vst3_host::ProcessMode::Offline)
            .map_err(|error| error.to_string())?;
        backend.prepare(config)?;
        if !ids.is_empty() {
            let (_, catalog) =
                super::validate_catalog_snapshot(backend.parameter_catalog_snapshot()?)?;
            for (&id, &value) in &state.parameter_bases {
                if !catalog
                    .iter()
                    .any(|parameter| parameter.id == id && !parameter.read_only)
                {
                    return Err(format!(
                        "Captured parameter base {id} is absent or read-only in the fresh instance"
                    ));
                }
                backend.set_parameter(id, value)?;
            }
        }
        let metadata = backend
            .bounce_metadata()
            .map_err(|error| error.to_string())?;
        metadata.validate(sample_rate)?;
        backend.input_channels = 0;
        backend.output_channels = metadata.output_buses[0].0;
        Ok(Self {
            _backend: backend,
            metadata,
            _same_thread: std::marker::PhantomData,
        })
    }

    pub fn metadata(&self) -> &BouncePluginMetadata {
        &self.metadata
    }
}

#[cfg(feature = "vst3")]
impl Vst3Backend {
    fn bounce_metadata(&self) -> vst3_host::Result<BouncePluginMetadata> {
        let layout = self.plugin.checked_audio_bus_layout()?;
        if layout.outputs.iter().any(|bus| !bus.is_main) {
            return Err(vst3_host::Error::MetadataQueryRejected(
                vst3_host::MetadataQueryRejection::Unsupported,
            ));
        }
        let events = self.plugin.event_bus_layout()?;
        let info = self.plugin.info();
        let latency_samples = self.plugin.try_latency_samples()?;
        let tail_samples = self.plugin.try_tail_samples()?;
        Ok(BouncePluginMetadata {
            class_uid: info.uid.clone(),
            version: info.version.clone(),
            name: info.name.clone(),
            category: info.category.clone(),
            event_input_buses: events
                .inputs
                .iter()
                .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                .collect(),
            event_output_buses: events
                .outputs
                .iter()
                .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                .collect(),
            input_buses: layout
                .inputs
                .iter()
                .map(|bus| (bus.channel_count, bus.active == Some(true)))
                .collect(),
            output_buses: layout
                .outputs
                .iter()
                .map(|bus| (bus.channel_count, bus.active == Some(true)))
                .collect(),
            latency_samples,
            tail_samples,
        })
    }

    pub(super) fn capture_bounce_state(
        &mut self,
        parameter_ids: &[u32],
    ) -> Result<BouncePluginState, BounceCaptureError> {
        use BounceCaptureError::{BackendFailed, Rejected};
        if !self.prepared || self.last_transport.playing {
            return Err(Rejected(
                "Instrument bounce source has not reached stopped processing".into(),
            ));
        }
        let before = self
            .plugin
            .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
            .map_err(|error| BackendFailed(error.to_string()))?;
        if before.open {
            return Err(Rejected(
                "Close the source native editor before instrument bounce".into(),
            ));
        }
        let revision = self
            .plugin
            .native_dirty_revision()
            .map_err(|error| BackendFailed(error.to_string()))?;
        let metadata = self.bounce_metadata().map_err(classify_metadata_error)?;
        metadata
            .validate(self.plugin.sample_rate() as u32)
            .map_err(Rejected)?;
        // Production SaveState has a zero-sample parameter flush. No Close, positive Process,
        // LoadState, transport reset or shared native publication is performed here.
        let opaque_state = self.save_state().map_err(BackendFailed)?;
        if opaque_state.is_empty() || opaque_state.len() > MAX_BOUNCE_STATE_BYTES {
            return Err(Rejected(
                "Instrument bounce requires nonempty authoritative state within 64 MiB".into(),
            ));
        }
        let mut parameter_bases = BTreeMap::new();
        if !parameter_ids.is_empty() {
            let (_, catalog) = super::validate_catalog_snapshot(
                self.parameter_catalog_snapshot().map_err(BackendFailed)?,
            )
            .map_err(Rejected)?;
            for &id in parameter_ids {
                if !catalog
                    .iter()
                    .any(|parameter| parameter.id == id && !parameter.read_only)
                {
                    return Err(Rejected(format!(
                        "Stored parameter base {id} is missing or read-only in current metadata"
                    )));
                }
                let value = self.get_parameter(id).map_err(BackendFailed)?;
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err(Rejected(format!(
                        "Stored parameter base {id} has an invalid current value"
                    )));
                }
                if parameter_bases.insert(id, value).is_some() {
                    return Err(Rejected("Duplicate captured parameter key".into()));
                }
            }
        }
        let after = self
            .plugin
            .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
            .map_err(|error| BackendFailed(error.to_string()))?;
        let after_revision = self
            .plugin
            .native_dirty_revision()
            .map_err(|error| BackendFailed(error.to_string()))?;
        if after.open
            || before.generation != after.generation
            || revision != after_revision
            || metadata != self.bounce_metadata().map_err(classify_metadata_error)?
        {
            return Err(Rejected("Source native generation, state revision or metadata changed during instrument bounce capture".into()));
        }
        Ok(BouncePluginState {
            metadata,
            opaque_state,
            parameter_bases,
            native_generation: before.generation,
            native_dirty_revision: revision,
        })
    }
}

/// Only explicit bounded admission refusal is harmless. Never infer it from remote text:
/// unknown/old helpers, malformed replies, SDK failures and poisoned state remain faults.
#[cfg(feature = "vst3")]
fn classify_metadata_error(error: vst3_host::Error) -> BounceCaptureError {
    match error {
        vst3_host::Error::MetadataQueryRejected(_) => {
            BounceCaptureError::Rejected(error.to_string())
        }
        _ => BounceCaptureError::BackendFailed(error.to_string()),
    }
}

#[cfg(test)]
mod affinity_tests {
    use super::OfflineInstrument;

    #[test]
    fn offline_owner_is_neither_send_nor_sync() {
        // One candidate impl exists only while the owner excludes each auto trait.
        // Adding Send/Sync makes type inference ambiguous and fails compilation.
        trait AmbiguousIfSend<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSend<()> for T {}
        impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}
        let _ = <OfflineInstrument as AmbiguousIfSend<_>>::check;
        trait AmbiguousIfSync<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSync<()> for T {}
        impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}
        let _ = <OfflineInstrument as AmbiguousIfSync<_>>::check;
    }
}
