//! Disabled Linux adapter for one renderer-owned disposable VST3 clone.
//!
//! No source endpoint is retained and no App/UI entry selects this backend.
//! Construction is empty; spawn, every exchange, cleanup and final destruction
//! run on the renderer owner. Only closed cancellation authority can leave it.
//! No load/process replay, reset audio, warmup, editor or automatic recovery.

use crate::{
    export_job::StrictCancelRegistration,
    instrument_bounce::{
        BOUNCE_PREPARED_MAXIMUM, BOUNCE_QUANTUM, BouncePluginMetadata, CapturedInstrumentBounce,
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
    test_action: Option<TestAction>,
    client: Option<Client>,
    registration: Option<StrictCancelRegistration>,
    control: Option<RenderControl>,
    loaded: Option<LoadedPlugin>,
    sample_rate: u32,
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
}

impl StrictBackend {
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(test)]
            test_action: None,
            client: None,
            registration: None,
            control: None,
            loaded: None,
            sample_rate: 0,
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
        }
    }
    #[cfg(test)]
    pub(crate) fn with_test_action(action: TestAction) -> Self {
        let mut backend = Self::new();
        backend.test_action = Some(action);
        backend
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
    fn exchange<T>(
        &mut self,
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
            BackendFailure::from(error)
        })
    }
    fn ready(&self) -> Result<(), BackendFailure> {
        if !self.prepared
            || !self.started
            || self.disposal_begun
            || self.terminated
            || self.terminal_exchange
        {
            return Err(Self::admission("Strict clone is not ready for rendering"));
        }
        Ok(())
    }
    fn read_metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        let audio = self.exchange(Client::checked_audio_layout)?;
        // Validate raw information BEFORE the lossy legacy metadata projection.
        if !audio.inputs.is_empty()
            || audio.outputs.len() != 1
            || !audio.outputs[0].is_main
            || audio.outputs[0].active != Some(true)
            || !matches!(audio.outputs[0].channel_count, 1 | 2)
        {
            return Err(Self::admission(
                "Strict clone requires no audio input and one known-active main mono/stereo output",
            ));
        }
        let events = self.exchange(Client::event_layout)?;
        if events.inputs.len() != 1
            || !events.outputs.is_empty()
            || !events.inputs[0].is_main
            || events.inputs[0].active != Some(true)
            || !(1..=16).contains(&events.inputs[0].channel_count)
        {
            return Err(Self::admission(
                "Strict clone requires one known-active main event input and no event output",
            ));
        }
        let latency_samples = self.exchange(Client::latency_samples)?;
        let tail_samples = self.exchange(Client::tail_samples)?;
        let info = self
            .loaded
            .as_ref()
            .ok_or_else(|| Self::admission("Strict clone has no verified factory identity"))?;
        let metadata = BouncePluginMetadata {
            class_uid: info.uid.clone(),
            version: info.version.clone(),
            name: info.name.clone(),
            category: info.category.clone(),
            input_buses: Vec::new(),
            output_buses: vec![(audio.outputs[0].channel_count, true)],
            event_input_buses: vec![(events.inputs[0].channel_count, true, Some(true))],
            event_output_buses: Vec::new(),
            latency_samples,
            tail_samples,
        };
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
            self.record_cleanup_error(error.into());
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
        if observation.kill_requested {
            self.record_diagnostic("Strict clone required forced termination");
            if self.cleanup_error.is_none() {
                self.cleanup_error =
                    Some(Self::admission("Strict clone did not terminate gracefully"));
            }
        }
        match observation.state {
            CleanupState::Reaped(status) => {
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
        match Client::spawn(&mut command, control.deadline) {
            Ok(client) => self.client = Some(client),
            Err(failure) => {
                // Retain a post-spawn setup-failed owner BEFORE any error conversion.
                self.client = failure.cleanup.map(|client| *client);
                self.terminal_exchange = true;
                return Err(failure.failure.into());
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
        let loaded = self.exchange(|client, deadline| {
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
        self.exchange(Client::set_offline)?;
        let support = self.exchange(Client::reset_origin_support)?;
        if support.contract_version != 1
            || support.max_block_frames as usize != BOUNCE_PREPARED_MAXIMUM
            || support.processing
        {
            return Err(Self::admission(
                "Fresh clone reset contract/configured maximum/inactive status differs",
            ));
        }
        self.exchange(|client, deadline| client.load_state(&capture.state.opaque_state, deadline))?;
        self.exchange(Client::start_processing)?;
        self.started = true;
        self.prepared_dirty_revision = Some(self.exchange(Client::native_dirty_revision)?);
        let catalog = self.exchange(Client::parameters)?;
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
        for (&id, &value) in &capture.state.parameter_bases {
            self.exchange(|client, deadline| client.set_parameter(id, value, deadline))?;
        }
        for (&id, &captured) in &capture.state.parameter_bases {
            let raw = self.exchange(|client, deadline| client.parameter(id, deadline))?;
            if !strict_offline::captured_base_matches(raw, captured) {
                return Err(Self::admission(format!(
                    "Captured base {id} readback mismatch: raw={raw:?}, projected={:?}, captured={captured:?}",
                    raw as f32
                )));
            }
        }
        let metadata = self.read_metadata()?;
        if &metadata != capture.metadata() {
            return Err(Self::admission(
                "Fresh strict clone metadata differs from authoritative capture",
            ));
        }
        self.output_channels = metadata.output_buses[0].0;
        self.prepared = true;
        Ok(metadata)
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        self.ready()?;
        self.read_metadata()
    }
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
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
        self.exchange(|client, deadline| client.send_note(event, i32::from(offset), deadline))
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        self.ready()?;
        if frames != BOUNCE_QUANTUM {
            return Err(Self::admission(
                "Strict clone requires exact Q128 processing",
            ));
        }
        let channels = self.output_channels;
        let transport = ProcessTransport {
            sample_position: transport.sample_position,
            quarter_note_position: transport.quarter_note_position,
            tempo: transport.tempo,
            playing: transport.playing,
            time_sig_numerator: transport.time_sig_numerator,
            time_sig_denominator: transport.time_sig_denominator,
        };
        let audio =
            self.exchange(|client, deadline| client.process(channels, transport, deadline))?;
        Ok(BlockReply {
            channels: audio.outputs,
            transport_applied: audio.transport_applied,
            output_events: audio.output_event_count,
            output_events_lost: audio.output_events_lost,
        })
    }
    fn dispose(&mut self, cleanup_deadline: Instant) -> DisposalObservation {
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
                            self.record_cleanup_error(error.into());
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
    }
}
