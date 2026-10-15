//! Actual Ready identity retained by its exact generator control owner, never persisted.
use super::*;
use crate::plugins::plugin_runtime::{LoadedVst3Identity, PluginControlIdentity};

#[derive(Clone)]
pub(in crate::app) struct LoadedGeneratorIdentity {
    project_session: u64,
    channel_id: u32,
    endpoint_id: u64,
    instance_id: u64,
    slot: usize,
    control: PluginControlIdentity,
    identity: LoadedVst3Identity,
}

#[derive(Default)]
pub(in crate::app) enum GeneratorReadiness {
    #[default]
    Awaiting,
    Ready(Option<Box<LoadedGeneratorIdentity>>, Option<String>),
    Faulted,
}

impl GeneratorReadiness {
    #[allow(clippy::too_many_arguments)] // one exact owner/event binding at the queue boundary
    pub(in crate::app) fn observe(
        &mut self,
        project: &Project,
        current_session: u64,
        owner_session: u64,
        channel_id: u32,
        endpoint_id: u64,
        instance_id: u64,
        control: &PluginChainControl,
        event: &RuntimeEvent,
    ) {
        let stats = control.stats();
        match event {
            RuntimeEvent::SlotFault { slot: 0, .. } | RuntimeEvent::ShutdownComplete => {
                *self = Self::Faulted;
            }
            RuntimeEvent::SlotReady {
                slot: 0,
                loaded_vst3_identity,
                loaded_vst3_identity_error,
                ..
            } if matches!(self, Self::Awaiting) => {
                let loaded = loaded_vst3_identity
                    .as_ref()
                    .filter(|identity| {
                        current_session == owner_session
                            && project_assignment_matches(
                                project,
                                channel_id,
                                instance_id,
                                identity,
                            )
                            && control.endpoint_manifest().slot_count == 1
                            && control.endpoint_manifest().instance_id(0) == Some(instance_id)
                            && stats.faults == 0
                            && !stats.stopped
                    })
                    .map(|identity| {
                        Box::new(LoadedGeneratorIdentity {
                            project_session: owner_session,
                            channel_id,
                            endpoint_id,
                            instance_id,
                            slot: 0,
                            control: control.owner_identity(),
                            identity: identity.clone(),
                        })
                    });
                let reason = loaded_vst3_identity_error.clone().or_else(|| {
                    (loaded_vst3_identity.is_some() && loaded.is_none()).then(|| "Actual VST3 Ready identity no longer matches its source assignment or owner".into())
                });
                *self = Self::Ready(loaded, reason);
            }
            _ => {}
        }
    }
}

fn project_assignment_matches(
    project: &Project,
    channel_id: u32,
    instance_id: u64,
    identity: &LoadedVst3Identity,
) -> bool {
    let mut channels = project
        .channels
        .iter()
        .filter(|channel| channel.id == channel_id);
    let Some(channel) = channels.next() else {
        return false;
    };
    if channels.next().is_some() || channel.instrument_plugin_instance_id != Some(instance_id) {
        return false;
    }
    let mut instances = project
        .plugin_instances
        .iter()
        .filter(|instance| instance.id == instance_id);
    let Some(instance) = instances.next() else {
        return false;
    };
    instances.next().is_none()
        && instance.format == ProjectPluginFormat::Vst3
        && instance.path == identity.path()
}

impl LoadedGeneratorIdentity {
    pub(in crate::app) fn identity(&self) -> &LoadedVst3Identity {
        &self.identity
    }

    pub(in crate::app) fn matches_current(&self, app: &CitrusApp) -> bool {
        let Some(chain) = app.running_generator_chains.get(&self.channel_id) else {
            return false;
        };
        let GeneratorReadiness::Ready(Some(current), None) = &chain.readiness else {
            return false;
        };
        let stats = chain.control.stats();
        self.project_session == app.project_session
            && chain.project_session == self.project_session
            && chain.endpoint_id == self.endpoint_id
            && chain.instance_id == self.instance_id
            && self.slot == 0
            && self.control.matches(&chain.control)
            && current.control.matches(&chain.control)
            && current.project_session == self.project_session
            && current.channel_id == self.channel_id
            && current.endpoint_id == self.endpoint_id
            && current.instance_id == self.instance_id
            && current.slot == self.slot
            && current.identity == self.identity
            && stats.faults == 0
            && !stats.stopped
            && project_assignment_matches(
                &app.project,
                self.channel_id,
                self.instance_id,
                &self.identity,
            )
            && !app.pending_generator_chains.contains_key(&self.channel_id)
            && !app
                .pending_generator_removals
                .contains_key(&self.channel_id)
    }
}

impl CitrusApp {
    pub(in crate::app) fn loaded_generator_identity(
        &self,
        channel_id: u32,
    ) -> Option<LoadedGeneratorIdentity> {
        let chain = self.running_generator_chains.get(&channel_id)?;
        let GeneratorReadiness::Ready(Some(identity), None) = &chain.readiness else {
            return None;
        };
        identity
            .matches_current(self)
            .then(|| identity.as_ref().clone())
    }

    pub(in crate::app) fn loaded_generator_identity_unavailable_reason(
        &self,
        channel_id: u32,
    ) -> String {
        match self.running_generator_chains.get(&channel_id).map(|chain| &chain.readiness) {
            Some(GeneratorReadiness::Ready(_, Some(reason))) => format!("Actual VST3 identity unavailable: {reason}"),
            Some(GeneratorReadiness::Faulted) => "The actual generator faulted; reload it explicitly before export".into(),
            _ => "Unknown role requires the actual current generator's prepared VST3 Instrument and active MIDI-input identity".into(),
        }
    }
}

#[cfg(test)]
#[path = "generator_loaded_identity_tests.rs"]
mod tests;
