//! Thread-affine synchronous owner for one fresh isolated VST3 instrument.
//!
//! This is preparation/processing infrastructure only. No App action or WAV renderer calls it
//! yet. It deliberately bypasses the realtime bridge, never its backend/helper contracts.

#[cfg(feature = "vst3")]
use crate::instrument_bounce::BounceAudioBus;
use crate::instrument_bounce::{BouncePluginMetadata, CapturedInstrumentBounce};

#[cfg(all(test, feature = "vst3", unix))]
#[path = "offline_instrument_protocol_tests.rs"]
mod protocol_tests;

#[cfg(feature = "vst3")]
use {
    super::{BounceCaptureFence, PluginBackend, PluginPrepareConfig, Vst3Backend},
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
            .bounce_metadata(None)
            .map_err(|error| error.to_string())?;
        metadata.validate(sample_rate)?;
        if metadata != state.metadata {
            return Err(
                "Fresh bounce instance metadata differs from the authoritative captured source"
                    .into(),
            );
        }
        backend.input_channels = metadata.participating_input_channels();
        backend.output_channels = metadata.output_buses[0].channel_count;
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
    /// Only the new owner's first prepare may call this, after restore and before Start.
    /// Preflight the entire declaration before the first acknowledgment, then check that
    /// every intermediate operation preserved all descriptors and prepared participation.
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub(super) fn qualify_fresh_bounce_source(
        &mut self,
        sample_rate: u32,
    ) -> Result<BouncePluginMetadata, String> {
        use vst3_host::audio::{BusDirection, MediaType};
        if self.prepared || self.plugin.is_processing() {
            return Err(
                "Fresh bounce source setup requires an unpublished inactive candidate".into(),
            );
        }
        let mut actual = self
            .bounce_metadata(None)
            .map_err(|error| error.to_string())?;
        let prepared = self
            .plugin
            .audio_bus_layout()
            .map_err(|error| error.to_string())?;
        let mut proposed = actual.clone();
        let mut acknowledge = Vec::new();
        for (direction, declared, projected, participating) in [
            (
                BusDirection::Input,
                &actual.input_buses,
                &mut proposed.input_buses,
                &prepared.inputs,
            ),
            (
                BusDirection::Output,
                &actual.output_buses,
                &mut proposed.output_buses,
                &prepared.outputs,
            ),
        ] {
            if declared.len() != participating.len() {
                return Err("Fresh bounce source actual/prepared bus count differs".into());
            }
            for (index, ((bus, proposal), storage)) in declared
                .iter()
                .zip(projected.iter_mut())
                .zip(participating)
                .enumerate()
            {
                if bus.channel_count != storage.channel_count
                    || bus.active.is_some_and(|active| active != storage.active)
                {
                    return Err("Fresh bounce source actual/prepared participation differs".into());
                }
                if bus.active.is_none() && !storage.active {
                    // This is a preflight proposal, never a source metadata receipt. Only
                    // a successful owner call plus a checked reread can attest false.
                    proposal.active = Some(false);
                    acknowledge.push((direction, index));
                }
            }
        }
        // Includes actual factory Instrument identity, ALL later buses, event policy and
        // latency. In particular an active aux is rejected rather than disabled to fit.
        proposed.validate(sample_rate)?;
        for (direction, index) in acknowledge {
            self.plugin
                .set_bus_active(MediaType::Audio, direction, index as i32, false)
                .map_err(|error| error.to_string())?;
            match direction {
                BusDirection::Input => actual.input_buses[index].active = Some(false),
                BusDirection::Output => actual.output_buses[index].active = Some(false),
            }
            let checked = self
                .bounce_metadata(None)
                .map_err(|error| error.to_string())?;
            let storage = self
                .plugin
                .audio_bus_layout()
                .map_err(|error| error.to_string())?;
            if checked != actual || storage != prepared {
                return Err(
                    "Fresh bounce source topology changed during inactive-bus acknowledgment"
                        .into(),
                );
            }
        }
        let checked = self
            .bounce_metadata(None)
            .map_err(|error| error.to_string())?;
        checked.validate(sample_rate)?;
        if checked != proposed {
            return Err("Fresh bounce source topology changed before admission".into());
        }
        self.input_channels = checked.participating_input_channels();
        self.output_channels = checked.output_buses[0].channel_count;
        Ok(checked)
    }

    pub(super) fn bounce_metadata(
        &self,
        fence: Option<&BounceCaptureFence>,
    ) -> Result<BouncePluginMetadata, BounceCaptureError> {
        let layout = capture_call(fence, || {
            self.plugin
                .checked_audio_bus_layout()
                .map_err(classify_metadata_error)
        })?;
        let events = capture_call(fence, || {
            self.plugin
                .event_bus_layout()
                .map_err(classify_metadata_error)
        })?;
        let info = self.plugin.info();
        let latency_samples = capture_call(fence, || {
            self.plugin
                .try_latency_samples()
                .map_err(classify_metadata_error)
        })?;
        let tail_samples = capture_call(fence, || {
            self.plugin
                .try_tail_samples()
                .map_err(classify_metadata_error)
        })?;
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
                .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
                .collect(),
            output_buses: layout
                .outputs
                .iter()
                .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
                .collect(),
            latency_samples,
            tail_samples,
        })
    }

    pub(super) fn capture_bounce_state(
        &mut self,
        parameter_ids: &[u32],
        fence: &BounceCaptureFence,
    ) -> Result<BouncePluginState, BounceCaptureError> {
        use BounceCaptureError::{BackendFailed, Rejected};
        fence.checkpoint()?;
        if !self.prepared || self.last_transport.playing {
            return Err(Rejected(
                "Instrument bounce source has not reached stopped processing".into(),
            ));
        }
        let before = capture_call(Some(fence), || {
            self.plugin
                .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
                .map_err(|error| BackendFailed(error.to_string()))
        })?;
        if before.open {
            return Err(Rejected(
                "Close the source native editor before instrument bounce".into(),
            ));
        }
        let revision = capture_call(Some(fence), || {
            self.plugin
                .native_dirty_revision()
                .map_err(|error| BackendFailed(error.to_string()))
        })?;
        let metadata = self.bounce_metadata(Some(fence))?;
        metadata
            .validate(self.plugin.sample_rate() as u32)
            .map_err(Rejected)?;
        // Production SaveState has a zero-sample parameter flush. No Close, positive Process,
        // LoadState, transport reset or shared native publication is performed here.
        let opaque_state = capture_call(Some(fence), || self.save_state().map_err(BackendFailed))?;
        if opaque_state.is_empty() || opaque_state.len() > MAX_BOUNCE_STATE_BYTES {
            return Err(Rejected(
                "Instrument bounce requires nonempty authoritative state within 64 MiB".into(),
            ));
        }
        let mut parameter_bases = BTreeMap::new();
        if !parameter_ids.is_empty() {
            let (_, catalog) = super::validate_catalog_snapshot(capture_call(Some(fence), || {
                self.parameter_catalog_snapshot().map_err(BackendFailed)
            })?)
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
                let value = capture_call(Some(fence), || {
                    self.get_parameter(id).map_err(BackendFailed)
                })?;
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
        let after = capture_call(Some(fence), || {
            self.plugin
                .isolated_editor(vst3_host::IsolatedEditorCommand::Query)
                .map_err(|error| BackendFailed(error.to_string()))
        })?;
        let after_revision = capture_call(Some(fence), || {
            self.plugin
                .native_dirty_revision()
                .map_err(|error| BackendFailed(error.to_string()))
        })?;
        if after.open
            || before.generation != after.generation
            || revision != after_revision
            || metadata != self.bounce_metadata(Some(fence))?
        {
            return Err(Rejected("Source native generation, state revision or metadata changed during instrument bounce capture".into()));
        }
        fence.checkpoint()?;
        Ok(BouncePluginState {
            metadata,
            opaque_state,
            parameter_bases,
            native_generation: before.generation,
            native_dirty_revision: revision,
        })
    }
}

/// Preserve the operation's failure before examining post-call cancellation. Once dispatched,
/// IPC is neither retried nor reclassified as harmless merely because its owner cancelled.
#[cfg(feature = "vst3")]
fn capture_call<T>(
    fence: Option<&BounceCaptureFence>,
    operation: impl FnOnce() -> Result<T, BounceCaptureError>,
) -> Result<T, BounceCaptureError> {
    if let Some(fence) = fence {
        fence.checkpoint()?;
    }
    let value = operation()?;
    if let Some(fence) = fence {
        fence.checkpoint()?;
    }
    Ok(value)
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
