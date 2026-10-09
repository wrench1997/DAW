//! Bounded, single-hop Generator MIDI-port routing. Control compilation rejects ambiguous
//! ownership instead of allowing note-off from one source to cut another source's same note.
use crate::{automation::AutomationTarget, model::Project};
use serde::{Deserialize, Serialize};

pub const MAX_MIDI_PORT_ROUTES: usize = 64;
/// Sixteen async Q128 turns plus one Q128 accumulation pre-roll. Fixed, independent
/// of callback partitioning, and conservatively safe for callbacks up to 2048 frames.
pub const MIDI_ROUTE_BRIDGE_FRAMES: u32 = 128 * (16 + 1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginMidiPorts {
    pub input: Option<u8>,
    pub output: Option<u8>,
    /// Silence this device's audio without bypassing its event processor.
    pub audio_monitor_muted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledMidiPortRoute {
    pub source_instance: u64,
    pub destination_instance: u64,
    pub source_channel: u32,
    pub destination_channel: u32,
    pub port: u8,
}

/// Channel placement, not discovery-role heuristics, owns the graph. Actual event-bus
/// capabilities are attested by the running endpoint before any edge activates.
pub fn compile_midi_port_routes(project: &Project) -> Result<Vec<CompiledMidiPortRoute>, String> {
    let devices = project
        .channels
        .iter()
        .filter_map(|channel| {
            let id = channel.instrument_plugin_instance_id?;
            let plugin = project
                .plugin_instances
                .iter()
                .find(|plugin| plugin.id == id)?;
            Some((channel.id, plugin))
        })
        .collect::<Vec<_>>();
    if devices
        .iter()
        .any(|(_, plugin)| plugin.midi_ports.input.is_some() && plugin.midi_ports.output.is_some())
    {
        return Err("MIDI processor chains and cycles are not supported yet. A device can have an input port or an output port, not both.".into());
    }
    let mut routes = Vec::new();
    for (destination_channel, destination) in &devices {
        let Some(port) = destination.midi_ports.input else {
            continue;
        };
        let mut sources = devices
            .iter()
            .filter(|(_, source)| source.midi_ports.output == Some(port));
        let Some((source_channel, source)) = sources.next() else {
            return Err(format!(
                "MIDI input port {port} has no Generator output. Set the producer's output port first, or choose Off."
            ));
        };
        if sources.next().is_some() {
            return Err(format!(
                "MIDI port {port} has multiple producers. Same-note fan-in is not supported; choose distinct ports."
            ));
        }
        if source.id == destination.id {
            return Err("A MIDI device cannot route to itself.".into());
        }
        if routes.len() == MAX_MIDI_PORT_ROUTES {
            return Err("Too many MIDI port destinations (maximum 64).".into());
        }
        routes.push(CompiledMidiPortRoute {
            source_instance: source.id,
            destination_instance: destination.id,
            source_channel: *source_channel,
            destination_channel: *destination_channel,
            port,
        });
    }
    if !routes.is_empty()
        && project.automation_lanes.iter().any(|lane| {
            lane.lane.is_enabled() && matches!(lane.lane.target(), AutomationTarget::Tempo)
        })
    {
        return Err("MIDI port routing currently requires constant project tempo. Turn off Tempo automation before connecting ports.".into());
    }
    if project.automation_lanes.iter().any(|lane| lane.lane.is_enabled()
        && matches!(lane.lane.target(), AutomationTarget::PluginParameter { instance, .. } if routes.iter().any(|route| route.destination_instance == *instance))) {
        return Err("MIDI-routed instrument parameter automation is not yet supported. Turn its parameter lanes off before connecting its input port.".into());
    }
    routes.sort_by_key(|route| (route.source_instance, route.destination_instance));
    Ok(routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn port_zero_is_distinct_from_off_and_old_settings_default_off() {
        let off: PluginMidiPorts = serde_json::from_str("{}").unwrap();
        let zero = PluginMidiPorts {
            input: Some(0),
            ..off
        };
        assert_ne!(off, zero);
        assert_eq!(
            serde_json::from_str::<PluginMidiPorts>(&serde_json::to_string(&zero).unwrap())
                .unwrap(),
            zero
        );
    }
    fn routed_project() -> Project {
        let mut project = Project::blank();
        let mut second = project.channels[0].clone();
        second.id = 2;
        second.name = "Synth".into();
        project.channels.push(second);
        for (index, id) in [101, 202].into_iter().enumerate() {
            project.plugin_instances.push(serde_json::from_value(serde_json::json!({"id":id,"format":"vst3","path":"test.vst3","uid":format!("test-{id}")})).unwrap());
            project.channels[index].instrument_plugin_instance_id = Some(id);
        }
        project.plugin_instances[0].midi_ports.output = Some(0);
        project.plugin_instances[1].midi_ports.input = Some(0);
        project
    }

    #[test]
    fn single_producer_fanout_and_reverse_channel_order_keep_stable_identity() {
        let mut project = routed_project();
        let mut third = project.channels[1].clone();
        third.id = 3;
        third.instrument_plugin_instance_id = Some(303);
        project.channels.push(third);
        let mut plugin = project.plugin_instances[1].clone();
        plugin.id = 303;
        project.plugin_instances.push(plugin);
        let expected = compile_midi_port_routes(&project).unwrap();
        assert_eq!(expected.len(), 2);
        assert_eq!(expected[0].port, 0);
        assert_eq!(expected[0].source_instance, 101);
        project.channels.reverse();
        project.plugin_instances.reverse();
        assert_eq!(compile_midi_port_routes(&project).unwrap(), expected);
    }

    #[test]
    fn reject_cycles_chains_missing_producer_and_same_note_fan_in() {
        let project = routed_project();
        let mut cycle = project.clone();
        cycle.plugin_instances[0].midi_ports.input = Some(0);
        assert!(
            compile_midi_port_routes(&cycle)
                .unwrap_err()
                .contains("cycles")
        );
        let mut chain = project.clone();
        chain.plugin_instances[1].midi_ports.output = Some(1);
        assert!(
            compile_midi_port_routes(&chain)
                .unwrap_err()
                .contains("chains")
        );
        let mut missing = project.clone();
        missing.plugin_instances[0].midi_ports.output = None;
        assert!(
            compile_midi_port_routes(&missing)
                .unwrap_err()
                .contains("no Generator")
        );
        let mut fan_in = project.clone();
        let mut channel = fan_in.channels[0].clone();
        channel.id = 3;
        channel.instrument_plugin_instance_id = Some(303);
        fan_in.channels.push(channel);
        let mut plugin = fan_in.plugin_instances[0].clone();
        plugin.id = 303;
        fan_in.plugin_instances.push(plugin);
        assert!(
            compile_midi_port_routes(&fan_in)
                .unwrap_err()
                .contains("multiple producers")
        );
        assert_eq!(compile_midi_port_routes(&project).unwrap().len(), 1);
    }

    #[test]
    fn reject_variable_tempo_or_routed_sink_parameter_automation() {
        use crate::automation::{AutomationLane, AutomationPoint};
        for target in [
            AutomationTarget::Tempo,
            AutomationTarget::PluginParameter {
                instance: 202,
                parameter: 7,
            },
        ] {
            let mut project = routed_project();
            let mut lane = AutomationLane::new(target);
            lane.replace_points([AutomationPoint::new(0.0, 0.5)]);
            project
                .automation_lanes
                .push(crate::model::ProjectAutomation {
                    id: 1,
                    name: "Unsupported routing automation".into(),
                    lane,
                });
            assert!(compile_midi_port_routes(&project).is_err());
            project.automation_lanes[0].lane.replace_points([]);
            assert!(
                compile_midi_port_routes(&project).is_err(),
                "enabled empty lane could later acquire points"
            );
            project.automation_lanes[0].lane.set_enabled(false);
            assert!(compile_midi_port_routes(&project).is_ok());
        }
    }

    #[test]
    fn persisted_ports_and_monitor_mute_round_trip_and_v11_defaults_off() {
        let mut project = routed_project();
        project.plugin_instances[0].midi_ports.audio_monitor_muted = true;
        let json = serde_json::to_string(&project).unwrap();
        let restored: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.plugin_instances[0].midi_ports,
            project.plugin_instances[0].midi_ports
        );
        assert_eq!(
            compile_midi_port_routes(&restored).unwrap(),
            compile_midi_port_routes(&project).unwrap()
        );
        let mut legacy = serde_json::to_value(&project).unwrap();
        legacy["format_version"] = 11.into();
        for plugin in legacy["plugin_instances"].as_array_mut().unwrap() {
            plugin.as_object_mut().unwrap().remove("midi_ports");
        }
        let legacy: Project = serde_json::from_value(legacy).unwrap();
        assert!(
            legacy
                .plugin_instances
                .iter()
                .all(|plugin| plugin.midi_ports == PluginMidiPorts::default())
        );
        assert!(compile_midi_port_routes(&legacy).unwrap().is_empty());
    }
}
