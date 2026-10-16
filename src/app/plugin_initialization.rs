//! Persisted generic bases are ready only after their exact worker acknowledges them.
use super::*;
use crate::plugins::plugin_runtime::{PluginControlIdentity, PluginParameterCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct PluginInitializationKey {
    project_session: u64,
    endpoint: ParameterEndpoint,
    instance_id: u64,
    slot: usize,
}

pub(super) struct PendingPluginInitialization {
    owner: PluginControlIdentity,
    request_id: u64,
    bases: Vec<(u32, f32)>,
    next: usize,
    waiting: Option<(u32, f32)>,
    needs_initial_state: bool,
    last_progress: Instant,
    error: Option<String>,
    reported: bool,
    ready: bool,
}

impl PendingPluginInitialization {
    fn observe(&mut self, event: &RuntimeEvent) {
        if self.ready || self.error.is_some() {
            return;
        }
        match event {
            RuntimeEvent::ParameterSetAck {
                request_id,
                id,
                value,
                ..
            } if *request_id == self.request_id => {
                if self.waiting.is_some_and(|(expected_id, expected)| {
                    expected_id == *id && expected.to_bits() == value.to_bits()
                }) {
                    self.waiting = None;
                    self.next += 1;
                    self.last_progress = Instant::now();
                } else {
                    self.error = Some(
                        "the parameter restore acknowledgment did not match the outstanding base"
                            .into(),
                    );
                }
            }
            RuntimeEvent::ParameterCommandFailed {
                request_id,
                command: PluginParameterCommand::Set,
                message,
                ..
            } if *request_id == self.request_id => {
                self.error = Some(format!("parameter restore was rejected: {message}"));
            }
            RuntimeEvent::SlotFault { message, .. } => {
                self.error = Some(format!(
                    "the worker faulted during parameter restore: {message}"
                ));
            }
            _ => {}
        }
    }

    fn poll(&mut self, key: PluginInitializationKey, control: &PluginChainControl) -> bool {
        if self.error.is_some() {
            return false;
        }
        let stats = control.stats();
        if stats.stopped || stats.faults != 0 || stats.event_overflows != 0 {
            self.error = Some(
                "the parameter restore worker stopped, faulted or lost runtime receipts".into(),
            );
            return false;
        }
        // Use the existing restoration inactivity budget. A lost ACK is a failure,
        // never permission to resend a potentially applied base or declare Loaded.
        if self.last_progress.elapsed() >= AUDIO_RESTART_RESTORE_TIMEOUT {
            self.error = Some("parameter restore timed out without its exact worker acknowledgment; reload the plug-in to retry".into());
            return false;
        }
        if self.waiting.is_some() {
            return false;
        }
        if let Some(&(id, value)) = self.bases.get(self.next) {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                self.error = Some(format!(
                    "persisted parameter {id} is outside the normalized range"
                ));
            } else if control.set_parameter_tagged(key.slot, id, value, self.request_id) {
                self.waiting = Some((id, value));
            }
            // Bounded enqueue failure keeps this exact base for the next poll.
            return false;
        }
        if self.needs_initial_state && !control.request_state(key.slot) {
            return false;
        }
        true
    }
}

impl CitrusApp {
    /// Copy the retained result of the ordinary matched-ACK reducer, never a
    /// new initialization request or a fabricated Loaded status.
    #[cfg(all(
        test,
        any(target_os = "linux", windows),
        feature = "experimental-fresh-helper"
    ))]
    pub(in crate::app) fn completed_initialization_ack_for_sdk_test(
        &self,
        instance_id: u64,
    ) -> Option<(u64, u64, u64, usize)> {
        self.plugin_initializations
            .iter()
            .find_map(|(key, pending)| {
                (key.instance_id == instance_id
                    && pending.ready
                    && pending.error.is_none()
                    && pending.waiting.is_none()
                    && pending.request_id != 0
                    && pending.next != 0
                    && self
                        .plugin_initialization_control(*key)
                        .is_some_and(|control| pending.owner.matches(control)))
                .then_some((
                    key.project_session,
                    key.endpoint.id,
                    pending.request_id,
                    pending.next,
                ))
            })
    }

    fn plugin_initialization_control(
        &self,
        key: PluginInitializationKey,
    ) -> Option<&PluginChainControl> {
        if key.project_session != self.project_session {
            return None;
        }
        match key.endpoint.kind {
            ParameterEndpointKind::Insert => self
                .pending_insert_chains
                .values()
                .map(|chain| {
                    (
                        chain.project_session,
                        chain.endpoint_id,
                        &chain.instance_ids,
                        &chain.control,
                    )
                })
                .chain(self.running_insert_chains.values().map(|chain| {
                    (
                        chain.project_session,
                        chain.endpoint_id,
                        &chain.instance_ids,
                        &chain.control,
                    )
                }))
                .find_map(|(session, endpoint, ids, control)| {
                    (session == key.project_session
                        && endpoint == key.endpoint.id
                        && ids.get(key.slot) == Some(&key.instance_id))
                    .then_some(control)
                }),
            ParameterEndpointKind::Generator => self
                .pending_generator_chains
                .values()
                .map(|chain| {
                    (
                        chain.project_session,
                        chain.endpoint_id,
                        chain.instance_id,
                        &chain.control,
                    )
                })
                .chain(self.running_generator_chains.values().map(|chain| {
                    (
                        chain.project_session,
                        chain.endpoint_id,
                        chain.instance_id,
                        &chain.control,
                    )
                }))
                .find_map(|(session, endpoint, instance, control)| {
                    (session == key.project_session
                        && endpoint == key.endpoint.id
                        && instance == key.instance_id
                        && key.slot == 0)
                        .then_some(control)
                }),
        }
    }

    /// Returns true only when ordinary Ready is held behind nonempty base replay.
    pub(super) fn observe_plugin_initialization(
        &mut self,
        event_session: u64,
        endpoint: ParameterEndpoint,
        instance_ids: &[u64],
        event: &RuntimeEvent,
    ) -> bool {
        let Some(instance_id) = runtime_event_instance_id(instance_ids, event) else {
            return false;
        };
        let slot = instance_ids
            .iter()
            .position(|id| *id == instance_id)
            .unwrap();
        let key = PluginInitializationKey {
            project_session: event_session,
            endpoint,
            instance_id,
            slot,
        };
        if event_session != self.project_session
            || self.clip_render_restorations.contains_key(&instance_id)
        {
            return false;
        }
        if matches!(event, RuntimeEvent::SlotReady { .. }) {
            let bases = plugin_parameter_replay_values(&self.project, instance_id);
            let ready = bases.is_empty();
            let Some(control) = self.plugin_initialization_control(key) else {
                return !ready;
            };
            let owner = control.owner_identity();
            if self
                .plugin_initializations
                .get(&key)
                .is_some_and(|pending| pending.owner.matches(control))
            {
                return !ready;
            }
            let request_id = if ready {
                0
            } else {
                let request_id = self.next_plugin_parameter_request_id.max(1);
                self.next_plugin_parameter_request_id = request_id.wrapping_add(1).max(1);
                request_id
            };
            let needs_initial_state = !ready
                && (self.restart_initial_state_waiting.contains(&instance_id)
                    || self.project.plugin_instances.iter().any(|instance| {
                        instance.id == instance_id && instance.opaque_state.is_empty()
                    }));
            // A replacement owns a fresh replay. Retired endpoints cannot keep a
            // failure latch on the replacement or acknowledge any of its bases.
            self.plugin_initializations
                .retain(|old, _| old.instance_id != instance_id);
            self.plugin_initializations.insert(
                key,
                PendingPluginInitialization {
                    owner,
                    request_id,
                    bases,
                    next: 0,
                    waiting: None,
                    needs_initial_state,
                    last_progress: Instant::now(),
                    error: None,
                    reported: false,
                    ready,
                },
            );
            if !ready
                && let Some(instance) = self
                    .project
                    .plugin_instances
                    .iter_mut()
                    .find(|instance| instance.id == instance_id)
            {
                instance.runtime_status = PluginRuntimeStatus::Unloaded;
            }
            return !ready;
        }
        let same_owner = self
            .plugin_initialization_control(key)
            .is_some_and(|control| {
                self.plugin_initializations
                    .get(&key)
                    .is_some_and(|pending| pending.owner.matches(control))
            });
        if same_owner && let Some(pending) = self.plugin_initializations.get_mut(&key) {
            pending.observe(event);
        }
        false
    }

    pub(super) fn drive_plugin_initializations(&mut self, allow_commands: bool) {
        let pending = std::mem::take(&mut self.plugin_initializations);
        for (key, mut initialization) in pending {
            let Some(control) = self
                .plugin_initialization_control(key)
                .filter(|control| initialization.owner.matches(control))
            else {
                continue;
            };
            let complete =
                initialization.ready || (allow_commands && initialization.poll(key, control));
            if complete {
                if !initialization.ready
                    && let Some(instance) = self
                        .project
                        .plugin_instances
                        .iter_mut()
                        .find(|instance| instance.id == key.instance_id)
                {
                    instance.runtime_status = PluginRuntimeStatus::Loaded;
                }
                // Retain actual Ready/ACK evidence with its exact owner. A history
                // Project can contain an older display status, but cannot erase this
                // completed worker preparation or qualify a different allocation.
                initialization.ready = true;
                initialization.bases = Vec::new();
                self.plugin_initializations.insert(key, initialization);
                continue;
            }
            if let Some(error) = &initialization.error
                && !initialization.reported
            {
                let message = format!("Plug-in {} initialization failed: {error}", key.instance_id);
                if let Some(instance) = self
                    .project
                    .plugin_instances
                    .iter_mut()
                    .find(|instance| instance.id == key.instance_id)
                {
                    instance.runtime_status = PluginRuntimeStatus::Crashed;
                }
                self.fault_audio_restart_restore(message.clone());
                let recovery = if self.audio_restart_state.is_degraded() {
                    "Retry the audio device in Settings to restore this session"
                } else {
                    "Use RETRY RESTORE in PLUGINS"
                };
                self.notify(format!("{message}. {recovery}"));
                initialization.reported = true;
            }
            self.plugin_initializations.insert(key, initialization);
        }
    }

    pub(super) fn plugin_initializations_pending(&self) -> bool {
        self.plugin_initializations
            .values()
            .any(|pending| !pending.ready && pending.error.is_none())
            || self.pending_insert_chains.values().any(|chain| {
                chain
                    .instance_ids
                    .iter()
                    .enumerate()
                    .any(|(slot, &instance_id)| {
                        self.plugin_slot_awaits_initialization(
                            PluginInitializationKey {
                                project_session: chain.project_session,
                                endpoint: ParameterEndpoint {
                                    kind: ParameterEndpointKind::Insert,
                                    id: chain.endpoint_id,
                                },
                                instance_id,
                                slot,
                            },
                            &chain.control,
                        )
                    })
            })
            || self.running_insert_chains.values().any(|chain| {
                chain
                    .instance_ids
                    .iter()
                    .enumerate()
                    .any(|(slot, &instance_id)| {
                        self.plugin_slot_awaits_initialization(
                            PluginInitializationKey {
                                project_session: chain.project_session,
                                endpoint: ParameterEndpoint {
                                    kind: ParameterEndpointKind::Insert,
                                    id: chain.endpoint_id,
                                },
                                instance_id,
                                slot,
                            },
                            &chain.control,
                        )
                    })
            })
            || self.pending_generator_chains.values().any(|chain| {
                self.plugin_slot_awaits_initialization(
                    PluginInitializationKey {
                        project_session: chain.project_session,
                        endpoint: ParameterEndpoint {
                            kind: ParameterEndpointKind::Generator,
                            id: chain.endpoint_id,
                        },
                        instance_id: chain.instance_id,
                        slot: 0,
                    },
                    &chain.control,
                )
            })
            || self.running_generator_chains.values().any(|chain| {
                self.plugin_slot_awaits_initialization(
                    PluginInitializationKey {
                        project_session: chain.project_session,
                        endpoint: ParameterEndpoint {
                            kind: ParameterEndpointKind::Generator,
                            id: chain.endpoint_id,
                        },
                        instance_id: chain.instance_id,
                        slot: 0,
                    },
                    &chain.control,
                )
            })
    }

    fn plugin_slot_awaits_initialization(
        &self,
        key: PluginInitializationKey,
        control: &PluginChainControl,
    ) -> bool {
        if key.project_session != self.project_session
            || self.clip_render_restorations.contains_key(&key.instance_id)
        {
            return false;
        }
        if let Some(initialization) = self
            .plugin_initializations
            .get(&key)
            .filter(|initialization| initialization.owner.matches(control))
        {
            return !initialization.ready && initialization.error.is_none();
        }
        // Installed can precede the worker's first Ready event. Only an owner
        // without actual Ready evidence may use this pre-Ready fallback.
        self.project.plugin_instances.iter().any(|instance| {
            instance.id == key.instance_id
                && !instance.parameters.is_empty()
                && instance.runtime_status == PluginRuntimeStatus::Unloaded
        })
    }

    pub(super) fn plugin_initialization_blocks_control(
        &self,
        endpoint: ParameterEndpoint,
        instance_id: u64,
        slot: usize,
        control: &PluginChainControl,
    ) -> bool {
        let key = PluginInitializationKey {
            project_session: self.project_session,
            endpoint,
            instance_id,
            slot,
        };
        self.plugin_initializations
            .get(&key)
            .filter(|initialization| initialization.owner.matches(control))
            .map_or_else(
                || self.plugin_slot_awaits_initialization(key, control),
                |initialization| !initialization.ready,
            )
    }

    pub(super) fn plugin_initialization_failed(&self) -> bool {
        self.plugin_initializations
            .values()
            .any(|pending| pending.error.is_some())
    }

    pub(super) fn plugin_initialization_failed_for_instance(&self, instance_id: u64) -> bool {
        self.plugin_initializations
            .iter()
            .any(|(key, pending)| key.instance_id == instance_id && pending.error.is_some())
    }

    pub(super) fn plugin_initialization_status_ui(&mut self, ui: &mut egui::Ui) {
        let failures = self
            .plugin_initializations
            .iter()
            .filter_map(|(key, pending)| pending.error.as_ref().map(|error| (*key, error.clone())))
            .collect::<Vec<_>>();
        for (key, error) in failures {
            ui.colored_label(theme::RED, format!("Plug-in {}: {error}", key.instance_id));
            if self.audio_restart_state.is_degraded() {
                ui.label("Retry the audio device in Settings to restore this session");
                continue;
            }
            if ui
                .button(format!("RETRY RESTORE · {}", key.instance_id))
                .clicked()
            {
                match key.endpoint.kind {
                    ParameterEndpointKind::Generator => {
                        if let Some(channel_id) = self
                            .project
                            .channels
                            .iter()
                            .find(|channel| {
                                channel.instrument_plugin_instance_id == Some(key.instance_id)
                            })
                            .map(|channel| channel.id)
                        {
                            self.request_channel_generator_rebuild(channel_id);
                        }
                    }
                    ParameterEndpointKind::Insert => {
                        if let Some(track) = self
                            .project
                            .mixer_insert_slots
                            .iter()
                            .find(|slot| slot.plugin_instance_id == key.instance_id)
                            .and_then(|slot| self.project.mixer_runtime_slot(slot.track))
                        {
                            self.request_mixer_track_rebuild(track);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "plugin_initialization_tests.rs"]
mod tests;
