//! Opt-in Linux/Windows adapter for one renderer-owned disposable VST3 clone.
//!
//! No source endpoint is retained and no App/UI entry selects this backend.
//! Construction is empty; spawn, every exchange, cleanup and final destruction
//! run on the renderer owner. Only closed cancellation authority can leave it.
//! No load/process replay, reset audio, warmup, editor or automatic recovery.

#[cfg(test)]
use crate::genuine_app_observer::{
    self as genuine_observer, Boundary, InstanceObserver, InstancePhase, Operation,
};
use crate::{
    export_job::StrictCancelRegistration,
    instrument_bounce::{
        BOUNCE_PREPARED_MAXIMUM, BOUNCE_QUANTUM, BounceAudioBus, BouncePluginMetadata,
        CapturedInstrumentBounce,
        render::{
            BackendFailure, BackendFailureKind, BlockReply, DisposalObservation, RenderBackend,
            RenderControl,
        },
    },
    plugins::plugin_runtime::{PluginLoadSpec, PluginTransport},
};
use std::{process::Command, time::Instant};
use vst3_host::{
    MidiChannel, MidiEvent, ProcessTransport,
    strict_offline::{self, CleanupState, Client, LoadedPlugin},
};

/// Closed deterministic owner-stage actions for mock qualification only.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestAction {
    CancelAfterSpawn,
    CancelAfterStop,
    PanicAfterDisposalOnce,
}

/// The owning Client makes even the empty backend !Send/!Sync. Create it inside
/// RenderAdmission's factory; never create a loaded instance before spawning.
pub(crate) struct StrictBackend {
    #[cfg(test)]
    observation: InstanceObserver,
    #[cfg(test)]
    test_action: Option<TestAction>,
    #[cfg(test)]
    successful_client_spawns: usize,
    #[cfg(test)]
    concrete_child_reap: Option<std::process::ExitStatus>,
    #[cfg(all(test, windows))]
    python_mock: bool,
    client: Option<Client>,
    registration: Option<StrictCancelRegistration>,
    control: Option<RenderControl>,
    loaded: Option<LoadedPlugin>,
    sample_rate: u32,
    input_channels: usize,
    output_channels: usize,
    prepare_attempted: bool,
    prepared: bool,
    started: bool,
    terminal_exchange: bool,
    stop_attempted: bool,
    disposal_begun: bool,
    terminated: bool,
    cleanup_error: Option<BackendFailure>,
    cleanup_diagnostics: Vec<String>,
    /// Preparation evidence only, never an edit-generation or metadata lease.
    prepared_dirty_revision: Option<u64>,
    prepared_async_revision: Option<u64>,
}

impl StrictBackend {
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(test)]
            observation: InstanceObserver::new(InstancePhase::StrictClone),
            #[cfg(test)]
            test_action: None,
            #[cfg(test)]
            successful_client_spawns: 0,
            #[cfg(test)]
            concrete_child_reap: None,
            #[cfg(all(test, windows))]
            python_mock: false,
            client: None,
            registration: None,
            control: None,
            loaded: None,
            sample_rate: 0,
            input_channels: 0,
            output_channels: 0,
            prepare_attempted: false,
            prepared: false,
            started: false,
            terminal_exchange: false,
            stop_attempted: false,
            disposal_begun: false,
            terminated: false,
            cleanup_error: None,
            cleanup_diagnostics: Vec::new(),
            prepared_dirty_revision: None,
            prepared_async_revision: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn genuine_hooks_inactive(&self) -> bool {
        #[cfg(windows)]
        if self.python_mock {
            return false;
        }
        self.test_action.is_none()
    }
    #[cfg(all(test, windows))]
    pub(crate) fn with_python_mock(mut self) -> Self {
        self.python_mock = true;
        self
    }
    #[cfg(test)]
    pub(crate) fn with_test_action(action: TestAction) -> Self {
        let mut backend = Self::new();
        backend.test_action = Some(action);
        backend
    }
    /// The reap status comes only from the retained Client's concrete OS wait,
    /// never from a kill request, mock startup log, or empty-owner disposal.
    #[cfg(test)]
    pub(crate) fn test_child_completion(&self) -> (usize, Option<std::process::ExitStatus>) {
        (self.successful_client_spawns, self.concrete_child_reap)
    }
    #[cfg(test)]
    fn test_stage(&mut self, stage: TestAction) {
        if self.test_action != Some(stage) {
            return;
        }
        self.test_action = None;
        match stage {
            TestAction::CancelAfterSpawn | TestAction::CancelAfterStop => {
                self.control
                    .as_ref()
                    .expect("mock job control")
                    .export
                    .cancel();
            }
            TestAction::PanicAfterDisposalOnce => panic!("mock Rust panic after checked disposal"),
        }
    }
    fn admission(message: impl AsRef<str>) -> BackendFailure {
        BackendFailure::new(BackendFailureKind::Backend, message)
    }
    fn checkpoint(control: &RenderControl) -> Result<(), BackendFailure> {
        if control.export.check_cancelled().is_err() {
            return Err(BackendFailure::new(
                BackendFailureKind::Cancelled,
                "Instrument clone cancelled before admission",
            ));
        }
        if Instant::now() >= control.deadline {
            return Err(BackendFailure::new(
                BackendFailureKind::Deadline,
                "Instrument clone job deadline elapsed",
            ));
        }
        Ok(())
    }
    /// The canonical export CAS can win before its cancelling thread reaches
    /// the registered wake. Synchronize that sticky state on the owner before
    /// each new admission, without changing any already-returned outcome.
    fn sync_cancel(&self) {
        if self
            .control
            .as_ref()
            .is_some_and(|control| control.export.check_cancelled().is_err())
            && let Some(client) = self.client.as_ref()
        {
            client.cancellation().cancel();
        }
    }
    fn exchange_at<T>(
        &mut self,
        stage: &'static str,
        call: impl FnOnce(&mut Client, Instant) -> Result<T, strict_offline::Failure>,
    ) -> Result<T, BackendFailure> {
        if self.terminal_exchange || self.disposal_begun || self.terminated {
            return Err(Self::admission("Strict clone exchange admission is closed"));
        }
        let deadline = self
            .control
            .as_ref()
            .ok_or_else(|| Self::admission("Strict clone has no original job budget"))?
            .deadline;
        self.sync_cancel();
        let result = call(
            self.client
                .as_mut()
                .ok_or_else(|| Self::admission("Strict clone has no client"))?,
            deadline,
        );
        result.map_err(|error| {
            // Every failed closed-client exchange seals its candidate. Preserve
            // that first observed kind/delivery rather than issuing Stop later.
            self.terminal_exchange = true;
            BackendFailure::from(error).with_strict_context(stage)
        })
    }
    /// All polls consume the existing absolute job budget and sticky cancellation.
    /// The disposable clone owns this notification queue; the realtime instance is untouched.
    /// No LoadState, MIDI, Process, lifecycle call or failed exchange is replayed.
    fn wait_async_state_ready(&mut self, stage: &'static str) -> Result<u64, BackendFailure> {
        loop {
            Self::checkpoint(
                self.control
                    .as_ref()
                    .ok_or_else(|| Self::admission("Strict clone has no job budget"))?,
            )
            .map_err(|failure| failure.with_strict_context(stage))?;
            self.exchange_at(
                "prepare.discard_progress_notifications",
                Client::discard_progress_notifications,
            )?;
            let snapshot = self.exchange_at(stage, Client::async_state_snapshot)?;
            if snapshot.readiness_faulted || snapshot.active_other != 0 {
                return Err(Self::admission(
                    "Strict clone async-state readiness cannot be attested",
                ));
            }
            if snapshot.active_async == 0 {
                return Ok(snapshot.async_started_revision);
            }
            // The next bounded exchange pumps the existing helper owner; no fixed sleep,
            // new owner, extra timeout or mutation retry is introduced.
        }
    }
    fn verify_armed_async_state(&mut self) -> Result<(), BackendFailure> {
        let expected = self
            .prepared_async_revision
            .ok_or_else(|| Self::admission("Strict clone has no async-state readiness token"))?;
        let snapshot =
            self.exchange_at("render.async_state_snapshot", Client::async_state_snapshot)?;
        snapshot
            .ensure_ready()
            .map_err(|error| Self::admission(error.to_string()))?;
        if snapshot.async_started_revision != expected {
            return Err(Self::admission(
                "Strict clone async-state restoration changed after preparation",
            ));
        }
        Ok(())
    }

    fn ready(&self) -> Result<(), BackendFailure> {
        if self.prepared_async_revision.is_none()
            || !self.prepared
            || !self.started
            || self.disposal_begun
            || self.terminated
            || self.terminal_exchange
        {
            return Err(Self::admission("Strict clone is not ready for rendering"));
        }
        Ok(())
    }
    fn read_selected_metadata(
        &mut self,
        preparing: bool,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        use strict_offline::{MetadataGetter, SelectedMetadataAdmission, SelectedMetadataOutcome};
        let bundle_stage = if preparing {
            "prepare.final_metadata.bundle"
        } else {
            "render.metadata.bundle"
        };
        let outcome = self
            .exchange_at(bundle_stage, Client::selected_metadata_snapshot)
            .map_err(|mut failure| {
                failure.strict_stage = Some(match (preparing, failure.strict_metadata_getter) {
                    (true, Some(MetadataGetter::CheckedAudioBusLayout)) => {
                        "prepare.final_metadata.checked_audio_layout"
                    }
                    (true, Some(MetadataGetter::EventBusLayout)) => {
                        "prepare.final_metadata.event_layout"
                    }
                    (true, Some(MetadataGetter::LatencySamples)) => {
                        "prepare.final_metadata.latency_samples"
                    }
                    (true, Some(MetadataGetter::TailSamples)) => {
                        "prepare.final_metadata.tail_samples"
                    }
                    (false, Some(MetadataGetter::CheckedAudioBusLayout)) => {
                        "render.metadata.checked_audio_layout"
                    }
                    (false, Some(MetadataGetter::EventBusLayout)) => "render.metadata.event_layout",
                    (false, Some(MetadataGetter::LatencySamples)) => {
                        "render.metadata.latency_samples"
                    }
                    (false, Some(MetadataGetter::TailSamples)) => "render.metadata.tail_samples",
                    (_, None) => bundle_stage,
                });
                failure
            })?;
        let snapshot = match outcome {
            SelectedMetadataOutcome::Complete(snapshot) => snapshot,
            SelectedMetadataOutcome::Rejected(SelectedMetadataAdmission::FactoryIdentity) => {
                return Err(Self::admission(
                    "Strict selected metadata requires the actual reviewed Kontakt 8.4.1 factory",
                ));
            }
            SelectedMetadataOutcome::Rejected(SelectedMetadataAdmission::EventInput) => {
                return Err(Self::admission(
                    "Strict clone requires one known-active main event input and no event output",
                ));
            }
        };
        // The helper preserves the early input guard; independently recheck it
        // on the host before accepting any complete metadata value.
        if !snapshot.events.admits_selected_bounce_input() {
            return Err(Self::admission(
                "Strict clone requires one known-active main event input and no event output",
            ));
        }
        let input_buses: Vec<_> = snapshot
            .audio
            .inputs
            .iter()
            .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
            .collect();
        let output_buses: Vec<_> = snapshot
            .audio
            .outputs
            .iter()
            .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
            .collect();
        #[cfg(test)]
        {
            self.observation
                .audio_declaration(&input_buses, &output_buses);
            self.observation.event_declaration(
                snapshot
                    .events
                    .inputs
                    .iter()
                    .map(|bus| (bus.channel_count, bus.is_main, bus.active)),
                snapshot
                    .events
                    .outputs
                    .iter()
                    .map(|bus| (bus.channel_count, bus.is_main, bus.active)),
            );
        }
        let info = self
            .loaded
            .as_ref()
            .ok_or_else(|| Self::admission("Strict clone has no verified factory identity"))?;
        let metadata = BouncePluginMetadata {
            class_uid: info.uid.clone(),
            version: info.version.clone(),
            name: info.name.clone(),
            category: info.category.clone(),
            input_buses,
            output_buses,
            event_input_buses: snapshot
                .events
                .inputs
                .iter()
                .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                .collect(),
            event_output_buses: snapshot
                .events
                .outputs
                .iter()
                .map(|bus| (bus.channel_count, bus.is_main, bus.active))
                .collect(),
            latency_samples: snapshot.latency_samples,
            tail_samples: snapshot.tail_samples,
        };
        #[cfg(test)]
        self.observation
            .metadata("StrictBackend assembled actual metadata", Ok(&metadata));
        metadata
            .validate(self.sample_rate)
            .map_err(Self::admission)?;
        Ok(metadata)
    }
    fn read_metadata(&mut self, preparing: bool) -> Result<BouncePluginMetadata, BackendFailure> {
        let info = self
            .loaded
            .as_ref()
            .ok_or_else(|| Self::admission("Strict clone has no verified factory identity"))?;
        if crate::instrument_bounce::kontakt_selected_stereo_bounce_eligible(
            &info.uid,
            &info.version,
            &info.category,
        ) {
            return self.read_selected_metadata(preparing);
        }
        let [audio_stage, event_stage, latency_stage, tail_stage] = if preparing {
            [
                "prepare.final_metadata.checked_audio_layout",
                "prepare.final_metadata.event_layout",
                "prepare.final_metadata.latency_samples",
                "prepare.final_metadata.tail_samples",
            ]
        } else {
            [
                "render.metadata.checked_audio_layout",
                "render.metadata.event_layout",
                "render.metadata.latency_samples",
                "render.metadata.tail_samples",
            ]
        };
        let audio = self.exchange_at(audio_stage, Client::checked_audio_layout)?;
        let input_buses: Vec<_> = audio
            .inputs
            .iter()
            .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
            .collect();
        let output_buses: Vec<_> = audio
            .outputs
            .iter()
            .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
            .collect();
        #[cfg(test)]
        self.observation
            .audio_declaration(&input_buses, &output_buses);
        let info = self
            .loaded
            .as_ref()
            .ok_or_else(|| Self::admission("Strict clone has no verified factory identity"))?;
        let selected = crate::instrument_bounce::kontakt_selected_stereo_bounce_eligible(
            &info.uid,
            &info.version,
            &info.category,
        );
        if !selected {
            crate::instrument_bounce::validate_bounce_audio_topology(&input_buses, &output_buses)
                .map_err(Self::admission)?;
        }
        let events = self.exchange_at(event_stage, Client::event_layout)?;
        #[cfg(test)]
        self.observation.event_declaration(
            events
                .inputs
                .iter()
                .map(|bus| (bus.channel_count, bus.is_main, bus.active)),
            events
                .outputs
                .iter()
                .map(|bus| (bus.channel_count, bus.is_main, bus.active)),
        );
        if events.inputs.len() != 1
            || (!selected && !events.outputs.is_empty())
            || !events.inputs[0].is_main
            || events.inputs[0].active != Some(true)
            || !(1..=16).contains(&events.inputs[0].channel_count)
        {
            return Err(Self::admission(
                "Strict clone requires one known-active main event input and no event output",
            ));
        }
        let latency_samples = self.exchange_at(latency_stage, Client::latency_samples)?;
        let tail_samples = self.exchange_at(tail_stage, Client::tail_samples)?;
        let info = self
            .loaded
            .as_ref()
            .ok_or_else(|| Self::admission("Strict clone has no verified factory identity"))?;
        let metadata = BouncePluginMetadata {
            class_uid: info.uid.clone(),
            version: info.version.clone(),
            name: info.name.clone(),
            category: info.category.clone(),
            input_buses,
            output_buses,
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
            latency_samples,
            tail_samples,
        };
        #[cfg(test)]
        self.observation
            .metadata("StrictBackend assembled actual metadata", Ok(&metadata));
        metadata
            .validate(self.sample_rate)
            .map_err(Self::admission)?;
        Ok(metadata)
    }
    fn record_diagnostic(&mut self, text: impl Into<String>) {
        let mut text = text.into();
        let mut end = text.len().min(2048);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        if !self.cleanup_diagnostics.contains(&text) && self.cleanup_diagnostics.len() < 8 {
            self.cleanup_diagnostics.push(text);
        }
    }
    fn record_cleanup_error(&mut self, error: BackendFailure) {
        if self.cleanup_error.is_none() {
            self.cleanup_error = Some(error);
        } else {
            self.record_diagnostic(error.diagnostic().to_owned());
        }
    }
    fn observe_cleanup(&mut self, observation: strict_offline::CleanupObservation) {
        if let Some(error) = observation.shutdown_failure {
            self.record_cleanup_error(
                BackendFailure::from(error).with_strict_context("dispose.shutdown"),
            );
        }
        if let Some(errno) = observation.kill_error {
            self.record_diagnostic(format!("Strict clone kill failed with errno {errno}"));
            if self.cleanup_error.is_none() {
                self.cleanup_error = Some(Self::admission("Strict clone cleanup had a kill error"));
            }
        }
        if let Some(errno) = observation.wait_error {
            self.record_diagnostic(format!(
                "Strict clone exit observation failed with errno {errno}"
            ));
            if self.cleanup_error.is_none() {
                self.cleanup_error = Some(Self::admission("Strict clone cleanup had a wait error"));
            }
        }
        if let Some(code) = observation.pipe_error {
            self.record_diagnostic(format!(
                "Strict clone pipe cleanup failed with OS error {code}"
            ));
            if self.cleanup_error.is_none() {
                self.cleanup_error = Some(Self::admission("Strict clone cleanup had a pipe error"));
            }
        }
        if observation.kill_requested {
            self.record_diagnostic("Strict clone required forced termination");
            if self.cleanup_error.is_none() {
                self.cleanup_error = Some(
                    Self::admission("Strict clone did not terminate gracefully")
                        .with_strict_context("dispose.wait_for_exit"),
                );
            }
        }
        match observation.state {
            CleanupState::Reaped(status) => {
                #[cfg(test)]
                {
                    self.concrete_child_reap = Some(status);
                }
                #[cfg(test)]
                self.observation.lifecycle(if status.success() {
                    "strict Client Reaped success"
                } else {
                    "strict Client Reaped abnormal"
                });
                if !status.success() {
                    self.record_diagnostic(format!(
                        "Strict clone reaped with abnormal status {status}"
                    ));
                    if self.cleanup_error.is_none() {
                        self.cleanup_error =
                            Some(Self::admission("Strict clone exited unsuccessfully"));
                    }
                }
                // Concrete reap is the only point that releases this client.
                self.terminated = true;
                drop(self.client.take());
                drop(self.registration.take());
            }
            CleanupState::Pending => {}
            CleanupState::Empty | CleanupState::Busy => self.record_cleanup_error(Self::admission(
                "Owned strict clone cleanup returned no concrete child completion",
            )),
        }
    }
}

impl RenderBackend for StrictBackend {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        #[cfg(test)]
        let observation = self.observation.clone();
        #[cfg(test)]
        {
            genuine_observer::record_hooks(
                "StrictBackend actual prepare",
                self.genuine_hooks_inactive(),
            );
            observation.capture_identity(capture.identity());
            observation.call(
                Operation::StrictPrepare,
                Boundary::Attempt,
                None,
                None,
                None,
            );
        }
        let result = (|| {
            Self::checkpoint(control)?;
            if self.prepare_attempted {
                return Err(Self::admission("Strict clone preparation is one-shot"));
            }
            self.prepare_attempted = true;
            self.control = Some(control.clone());
            self.sample_rate = capture.plan().sample_rate();
            let (captured_spec, requested_ids) = capture.strict_load_inputs();
            capture
                .state
                .validate(requested_ids, self.sample_rate)
                .map_err(Self::admission)?;
            if !(8000..=192000).contains(&self.sample_rate)
                || captured_spec.descriptor.format != super::PluginFormat::Vst3
                || !captured_spec.enabled
                || captured_spec.bypassed
                || captured_spec.wet != 1.0
            {
                return Err(Self::admission(
                    "Captured strict clone configuration is unsupported",
                ));
            }
            // Resolve the existing installed/explicit helper policy without copying
            // Project or captured opaque state into a second large Vec.
            let mut spec = PluginLoadSpec {
                descriptor: captured_spec.descriptor.clone(),
                class_uid: Some(capture.metadata().class_uid.clone()),
                vst3_helper_path: captured_spec.vst3_helper_path.clone(),
                initial_state: Vec::new(),
                enabled: captured_spec.enabled,
                bypassed: captured_spec.bypassed,
                wet: captured_spec.wet,
            };
            super::prepare_runtime_spec(&mut spec).map_err(Self::admission)?;
            let helper = spec
                .vst3_helper_path
                .as_ref()
                .filter(|p| p.is_file())
                .ok_or_else(|| Self::admission("Strict clone helper is absent"))?;
            let path = spec
                .descriptor
                .path
                .to_str()
                .ok_or_else(|| Self::admission("Strict clone plugin path is not UTF-8"))?;
            let tempo = capture.plan().timeline().plugin_transport_tempo();
            Self::checkpoint(control)?;
            let mut command = Command::new(helper);
            #[cfg(all(test, windows))]
            if self.python_mock {
                // The actual direct child is Python, never a cmd/shebang wrapper.
                // This test-only override cannot be selected in a shipped App.
                command = Command::new(
                    std::env::var_os("CITRUS_STRICT_TEST_PYTHON")
                        .unwrap_or_else(|| "python".into()),
                );
                command.arg("-u").arg(helper);
            }
            match Client::spawn(&mut command, control.deadline) {
                Ok(client) => {
                    #[cfg(test)]
                    {
                        self.successful_client_spawns += 1;
                    }
                    self.client = Some(client);
                }
                Err(failure) => {
                    // Retain a post-spawn setup-failed owner BEFORE any error conversion.
                    self.client = failure.cleanup.map(|client| *client);
                    self.terminal_exchange = true;
                    return Err(
                        BackendFailure::from(failure.failure).with_strict_context("prepare.spawn")
                    );
                }
            }
            #[cfg(test)]
            self.test_stage(TestAction::CancelAfterSpawn);
            let cancel = self
                .client
                .as_ref()
                .expect("stored spawned client")
                .cancellation();
            self.registration = Some(control.export.register_strict_cancel(cancel));
            // Registration rereads sticky cancellation; the client budget rejects
            // before LoadPlugin if cancellation/spawn consumed the original budget.
            let sample_rate = f64::from(self.sample_rate);
            let loaded = self.exchange_at("prepare.load_plugin", |client, deadline| {
                client.load_plugin(
                    strict_offline::LoadSpec {
                        path,
                        sample_rate,
                        tempo,
                        time_sig_numerator: 4,
                        time_sig_denominator: 4,
                        class_id: &capture.metadata().class_uid,
                    },
                    deadline,
                )
            })?;
            let matches = loaded.uid == capture.metadata().class_uid
                && loaded.version == capture.metadata().version;
            self.loaded = Some(loaded);
            if !matches {
                return Err(Self::admission(
                    "Fresh clone factory class/version differs from captured identity",
                ));
            }
            self.exchange_at("prepare.set_offline", Client::set_offline)?;
            let support =
                self.exchange_at("prepare.reset_origin_support", Client::reset_origin_support)?;
            if support.contract_version != 1
                || support.max_block_frames as usize != BOUNCE_PREPARED_MAXIMUM
                || support.processing
            {
                return Err(Self::admission(
                    "Fresh clone reset contract/configured maximum/inactive status differs",
                ));
            }
            let state_loaded = self.exchange_at("prepare.load_state", |client, deadline| {
                client.load_state(&capture.state.opaque_state, deadline)
            });
            #[cfg(test)]
            observation.strict_state_load(&capture.state.opaque_state, &state_loaded);
            state_loaded?;
            self.wait_async_state_ready("prepare.post_state_async_snapshot")?;
            if capture.metadata().uses_selected_stereo_bounce() {
                let declared = self.exchange_at(
                    "prepare.post_state_audio_layout",
                    Client::checked_audio_layout,
                )?;
                let descriptors_match =
                    |clone: &[vst3_host::plugin::CheckedAudioBusConfig],
                     source: &[BounceAudioBus]| {
                        clone.len() == source.len()
                            && clone.iter().zip(source).all(|(clone, source)| {
                                clone.channel_count == source.channel_count
                                    && clone.is_main == source.is_main
                            })
                    };
                if !descriptors_match(&declared.inputs, &capture.metadata().input_buses)
                    || !descriptors_match(&declared.outputs, &capture.metadata().output_buses)
                {
                    return Err(Self::admission(
                        "Fresh selected clone complete descriptors differ from captured source",
                    ));
                }
                let selected = self
                    .exchange_at("prepare.select_main_stereo_output", |client, deadline| {
                        client.select_main_stereo_output(&declared, deadline)
                    })?;
                let participation_matches =
                    |clone: &[vst3_host::plugin::CheckedAudioBusConfig],
                     source: &[BounceAudioBus]| {
                        clone.len() == source.len()
                            && clone.iter().zip(source).all(|(clone, source)| {
                                clone.channel_count == source.channel_count
                                    && clone.is_main == source.is_main
                                    && clone.active == source.active
                            })
                    };
                if !participation_matches(&selected.inputs, &capture.metadata().input_buses)
                    || !participation_matches(&selected.outputs, &capture.metadata().output_buses)
                {
                    return Err(Self::admission(
                        "Fresh selected clone acknowledged participation differs from captured source",
                    ));
                }
            }
            self.exchange_at("prepare.start_processing", Client::start_processing)?;
            self.started = true;
            let async_revision =
                self.wait_async_state_ready("prepare.post_start_async_snapshot")?;
            self.prepared_dirty_revision = Some(self.exchange_at(
                "prepare.native_dirty_revision",
                Client::native_dirty_revision,
            )?);
            // state.validate already proved that this is exactly the requested
            // generic base set. An empty set needs no unused controller catalog;
            // opaque state and all identity/topology/dirty/metadata checks remain.
            // This does not attest every parameter internal to the instrument.
            if !capture.state.parameter_bases.is_empty() {
                let catalog = self.exchange_at("prepare.parameters", Client::parameters)?;
                // Validate the entire captured base set before the first mutation.
                for id in capture.state.parameter_bases.keys() {
                    if !catalog
                        .iter()
                        .any(|parameter| parameter.id == *id && !parameter.is_read_only)
                    {
                        return Err(Self::admission(format!(
                            "Captured base {id} is absent or read-only"
                        )));
                    }
                }
            }
            for (&id, &value) in &capture.state.parameter_bases {
                self.exchange_at("prepare.set_parameter", |client, deadline| {
                    client.set_parameter(id, value, deadline)
                })?;
            }
            for (&id, &captured) in &capture.state.parameter_bases {
                let readback = self
                    .exchange_at("prepare.parameter_readback", |client, deadline| {
                        client.parameter(id, deadline)
                    });
                #[cfg(test)]
                observation.strict_base_read("StrictBackend prepare readback", id, &readback);
                let raw = readback?;
                if !strict_offline::captured_base_matches(raw, captured) {
                    return Err(Self::admission(format!(
                        "Captured base {id} readback mismatch: raw={raw:?}, projected={:?}, captured={captured:?}",
                        raw as f32
                    )));
                }
            }
            let metadata = self.read_metadata(true)?;
            if &metadata != capture.metadata() {
                return Err(Self::admission(
                    "Fresh strict clone metadata differs from authoritative capture",
                ));
            }
            self.input_channels = metadata.participating_input_channels();
            self.output_channels = metadata.output_buses[0].channel_count;
            self.exchange_at("prepare.arm_async_state_readiness", |client, deadline| {
                client.arm_offline_state_readiness(async_revision, deadline)
            })?;
            self.prepared_async_revision = Some(async_revision);
            self.prepared = true;
            Ok(metadata)
        })();
        #[cfg(test)]
        {
            observation.strict_result(Operation::StrictPrepare, None, None, &result);
            observation.metadata(
                "StrictBackend prepare result",
                result.as_ref().map_err(|e| e.diagnostic()),
            );
        }
        result
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        #[cfg(test)]
        let observation = self.observation.clone();
        #[cfg(test)]
        observation.call(
            Operation::StrictMetadata,
            Boundary::Attempt,
            None,
            None,
            None,
        );
        let result = (|| {
            self.ready()?;
            self.verify_armed_async_state()?;
            let metadata = self.read_metadata(false)?;
            self.verify_armed_async_state()?;
            Ok(metadata)
        })();
        #[cfg(test)]
        {
            observation.strict_result(Operation::StrictMetadata, None, None, &result);
            observation.metadata(
                "StrictBackend metadata result",
                result.as_ref().map_err(|e| e.diagnostic()),
            );
        }
        result
    }
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        #[cfg(test)]
        let observation = self.observation.clone();
        #[cfg(test)]
        {
            observation.midi(midi, offset);
            observation.call(Operation::StrictMidi, Boundary::Attempt, None, None, None);
        }
        let result = (|| {
            self.ready()?;
            if midi[1] > 127 || midi[2] > 127 || offset as usize >= BOUNCE_QUANTUM {
                return Err(Self::admission(
                    "Invalid strict ordinary-note scalar or offset",
                ));
            }
            let channel = MidiChannel::from_index(midi[0] & 0x0f)
                .ok_or_else(|| Self::admission("Invalid strict MIDI channel"))?;
            let event = match midi[0] & 0xf0 {
                0x90 if midi[2] > 0 => MidiEvent::NoteOn {
                    channel,
                    note: midi[1],
                    velocity: midi[2],
                },
                0x80 | 0x90 => MidiEvent::NoteOff {
                    channel,
                    note: midi[1],
                    velocity: midi[2],
                },
                _ => {
                    return Err(Self::admission(
                        "Strict clone accepts only ordinary NoteOn/NoteOff",
                    ));
                }
            };
            self.exchange_at("render.send_midi", |client, deadline| {
                client.send_note(event, i32::from(offset), deadline)
            })
        })();
        #[cfg(test)]
        observation.strict_result(Operation::StrictMidi, None, None, &result);
        result
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        #[cfg(test)]
        let observation = self.observation.clone();
        #[cfg(test)]
        observation.call(
            Operation::StrictProcess,
            Boundary::Attempt,
            Some(frames),
            Some(transport),
            None,
        );
        let result = (|| {
            self.ready()?;
            if frames != BOUNCE_QUANTUM {
                return Err(Self::admission(
                    "Strict clone requires exact Q128 processing",
                ));
            }
            let input_channels = self.input_channels;
            let channels = self.output_channels;
            let transport = ProcessTransport {
                sample_position: transport.sample_position,
                quarter_note_position: transport.quarter_note_position,
                tempo: transport.tempo,
                playing: transport.playing,
                time_sig_numerator: transport.time_sig_numerator,
                time_sig_denominator: transport.time_sig_denominator,
            };
            let audio = self.exchange_at("render.process", |client, deadline| {
                client.process_silent_generator(input_channels, channels, transport, deadline)
            })?;
            Ok(BlockReply {
                channels: audio.outputs,
                transport_applied: audio.transport_applied,
                output_events: audio.output_event_count,
                output_events_lost: audio.output_events_lost,
            })
        })();
        #[cfg(test)]
        {
            observation.strict_result(
                Operation::StrictProcess,
                Some(frames),
                Some(transport),
                &result,
            );
            if let Ok(reply) = &result {
                observation.pcm(frames, transport, reply);
            }
        }
        result
    }
    fn dispose(&mut self, cleanup_deadline: Instant) -> DisposalObservation {
        #[cfg(test)]
        let observation = self.observation.clone();
        #[cfg(test)]
        observation.call(
            Operation::StrictDispose,
            Boundary::Attempt,
            None,
            None,
            None,
        );
        let result = {
            if self.client.is_none() {
                self.terminated = true;
                drop(self.registration.take());
            }
            if !self.terminated {
                let protocol_deadline = self.control.as_ref().map_or(cleanup_deadline, |control| {
                    control.deadline.min(cleanup_deadline)
                });
                if !self.disposal_begun {
                    if self.started && !self.stop_attempted && !self.terminal_exchange {
                        self.stop_attempted = true;
                        self.sync_cancel();
                        let result = self
                            .client
                            .as_mut()
                            .expect("retained client")
                            .stop_processing(protocol_deadline);
                        match result {
                            Ok(()) => {
                                self.started = false;
                                #[cfg(test)]
                                self.test_stage(TestAction::CancelAfterStop);
                            }
                            Err(error) => {
                                self.terminal_exchange = true;
                                self.record_cleanup_error(
                                    BackendFailure::from(error)
                                        .with_strict_context("dispose.stop_processing"),
                                );
                            }
                        }
                    }
                    self.sync_cancel();
                    let observation = self
                        .client
                        .as_mut()
                        .expect("retained client")
                        .dispose(protocol_deadline, cleanup_deadline);
                    #[cfg(test)]
                    self.test_stage(TestAction::PanicAfterDisposalOnce);
                    self.disposal_begun = true;
                    self.observe_cleanup(observation);
                } else {
                    match self
                        .client
                        .as_mut()
                        .expect("retained client")
                        .continue_cleanup()
                    {
                        Ok(observation) => self.observe_cleanup(observation),
                        Err(error) => self.record_cleanup_error(error.into()),
                    }
                }
            }
            DisposalObservation {
                terminated: self.terminated,
                error: self.cleanup_error.clone(),
                diagnostics: self.cleanup_diagnostics.clone(),
            }
        };
        #[cfg(test)]
        observation.disposal(&result);
        result
    }
}

#[cfg(test)]
impl Drop for StrictBackend {
    fn drop(&mut self) {
        self.observation
            .lifecycle("StrictBackend destructor entered; fields still owned");
    }
}

#[cfg(test)]
mod cleanup_context_tests {
    use super::*;

    #[test]
    fn strict_cleanup_shutdown_context_retains_original_failure_across_forced_exit_observation() {
        let mut backend = StrictBackend::new();
        backend.observe_cleanup(strict_offline::CleanupObservation {
            state: CleanupState::Pending,
            shutdown_failure: Some(strict_offline::Failure {
                kind: strict_offline::FailureKind::Deadline,
                delivery: strict_offline::Delivery::MayHaveApplied,
                diagnostic: None,
                diagnostic_unavailable: None,
                request_context: None,
                metadata_getter: None,
                limit_evidence: None,
            }),
            kill_error: None,
            wait_error: None,
            pipe_error: None,
            kill_requested: false,
        });
        let first = backend.cleanup_error.clone().unwrap();
        assert_eq!(first.strict_stage, Some("dispose.shutdown"));
        assert_eq!(
            first.strict_kind,
            Some(strict_offline::FailureKind::Deadline)
        );
        assert_eq!(
            first.delivery,
            Some(crate::instrument_bounce::render::BackendDelivery::MayHaveApplied)
        );
        assert!(first.last_wire_request.is_none());
        backend.observe_cleanup(strict_offline::CleanupObservation {
            state: CleanupState::Pending,
            shutdown_failure: None,
            kill_error: None,
            wait_error: None,
            pipe_error: None,
            kill_requested: true,
        });
        assert_eq!(backend.cleanup_error.as_ref(), Some(&first));
        assert!(!backend.terminated);
        assert_eq!(
            backend.cleanup_diagnostics,
            ["Strict clone required forced termination"]
        );
    }
}
