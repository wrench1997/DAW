//! Session-bound playback projection for a converted, deliberately retired generator.
//! Persisted state and ordinary disabled-generator semantics stay with the Project.
use super::*;

impl CitrusApp {
    pub(super) fn clip_dormant_channel_audition_refused(&self, channel_id: u32) -> bool {
        let Some(instance_id) = self
            .project
            .channels
            .iter()
            .find(|channel| channel.id == channel_id)
            .and_then(|channel| channel.instrument_plugin_instance_id)
        else {
            return false;
        };
        let Some(binding) = self.clip_render_restorations.get(&instance_id) else {
            return false;
        };
        let mut instances = self
            .project
            .plugin_instances
            .iter()
            .filter(|p| p.id == instance_id);
        let Some(instance) = instances.next() else {
            return true;
        };
        instances.next().is_some()
            || !instance.enabled
            || !binding.matches_playback_instance(self.project_session, instance)
    }
    pub(super) fn clip_timeline_fingerprint(&self, sample_rate: u32) -> u64 {
        let base = timeline_playback_fingerprint(&self.project, self.transport_mode, sample_rate);
        if self.clip_render_restorations.is_empty() {
            return base;
        }
        let mut hash = DefaultHasher::new();
        "converted-generator-playback-v1".hash(&mut hash);
        base.hash(&mut hash);
        self.project_session.hash(&mut hash);
        self.clip_timeline_projection_revision.hash(&mut hash);
        // Descriptor identity/enabled state are not all part of the ordinary
        // musical fingerprint. A mismatch must force the fallible projector.
        for instance in &self.project.plugin_instances {
            if self.clip_render_restorations.contains_key(&instance.id) {
                instance.id.hash(&mut hash);
                instance.uid.hash(&mut hash);
                instance.path.hash(&mut hash);
                instance.enabled.hash(&mut hash);
                std::mem::discriminant(&instance.format).hash(&mut hash);
                std::mem::discriminant(&instance.role).hash(&mut hash);
            }
        }
        hash.finish()
    }

    pub(super) fn clip_timeline_playback_project(&self) -> Result<Project, String> {
        let mut dormant = Vec::new();
        dormant
            .try_reserve_exact(self.clip_render_restorations.len())
            .map_err(|_| {
                "Converted playback projection could not reserve bounded source identities"
                    .to_owned()
            })?;
        for (&instance_id, binding) in &self.clip_render_restorations {
            if binding.project_session() != self.project_session {
                return Err("Converted playback projection has a stale session binding".into());
            }
            let mut instances = self
                .project
                .plugin_instances
                .iter()
                .filter(|p| p.id == instance_id);
            let Some(instance) = instances.next() else {
                // History can retain a binding absent from the current Project.
                // Any dangling persisted references retain compiler diagnostics.
                continue;
            };
            if instances.next().is_some() || instance_id == 0 {
                return Err(
                    "Converted playback projection has ambiguous instance definitions".into(),
                );
            }
            if !binding.matches_playback_instance(self.project_session, instance) {
                return Err(
                    "Converted playback projection identity/path does not match its source".into(),
                );
            }
            if instance.enabled {
                continue;
            }
            let mut channels = self
                .project
                .channels
                .iter()
                .enumerate()
                .filter(|(_, channel)| channel.instrument_plugin_instance_id == Some(instance_id));
            let Some((index, channel)) = channels.next() else {
                continue;
            };
            if channels.next().is_some()
                || self
                    .project
                    .channels
                    .iter()
                    .filter(|other| other.id == channel.id)
                    .count()
                    != 1
                || self
                    .project
                    .mixer_insert_slots
                    .iter()
                    .any(|slot| slot.plugin_instance_id == instance_id)
            {
                return Err("Converted playback projection has ambiguous source ownership".into());
            }
            dormant.push((instance_id, index, channel.id));
        }
        // Mode-derived duration and synthetic placement are established before
        // suppression. No note extent, row, or note identity is removed.
        let mut playback = transport_playback_project(&self.project, self.transport_mode);
        for (instance_id, index, channel_id) in dormant {
            let channel = &mut playback.channels[index];
            channel.instrument_plugin_instance_id = None;
            channel.muted = true;
            channel.steps.fill(false);
            for pattern in &mut playback.patterns {
                if let Some(steps) = pattern.channel_steps.get_mut(index) {
                    steps.fill(false);
                }
                for note in &mut pattern.notes {
                    if note.channel_id == Some(channel_id) {
                        note.muted = true;
                    }
                }
            }
            for note in &mut playback.piano_notes {
                if note.channel_id == Some(channel_id) {
                    note.muted = true;
                }
            }
            for automation in &mut playback.automation_lanes {
                let suppress = match automation.lane.target() {
                    AutomationTarget::PluginParameter { instance, .. } => *instance == instance_id,
                    AutomationTarget::ChannelVolume { channel }
                    | AutomationTarget::ChannelPan { channel }
                    | AutomationTarget::ChannelMute { channel } => *channel == channel_id,
                    _ => false,
                };
                if suppress {
                    automation.lane.set_enabled(false);
                }
            }
        }
        Ok(playback)
    }
}

#[cfg(test)]
#[path = "clip_render_playback_tests.rs"]
mod tests;
