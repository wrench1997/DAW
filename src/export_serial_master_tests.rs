//! Pure static-route regression fixtures. No helper, plug-in, audio device or file I/O.

use super::super::tests::test_plugin;
use super::*;
use crate::{
    automation::{AutomationLane, AutomationPoint, AutomationTarget},
    mixer_graph::{MixerRoute, MixerRouteDestination},
    model::{MixerTrack, PluginRole, ProjectAutomation},
};

const CHANNEL: u32 = 73;
const SOURCE: MixerTrackId = 10_001;
const BUS: MixerTrackId = 20_002;
const OTHER: MixerTrackId = 30_003;
const GENERATOR: u64 = 101;
const SOURCE_FIRST: u64 = 201;
const SOURCE_LAST: u64 = 203;
const BUS_EFFECT: u64 = 202;
const MASTER_EFFECT: u64 = 204;
type StagePlacements = (MixerTrackId, Vec<(usize, u64)>);

fn main_route(id: u64, slot: u8, source: MixerTrackId, destination: MixerTrackId) -> MixerRoute {
    MixerRoute {
        id,
        runtime_slot: slot,
        source_mixer_track_id: source,
        destination: MixerRouteDestination::MainInput {
            mixer_track_id: destination,
        },
        tap: MixerRouteTap::PostFader,
        gain: 1.0,
        enabled: true,
    }
}

fn track(project: &mut Project, id: MixerTrackId) -> &mut MixerTrack {
    project
        .mixer_tracks
        .iter_mut()
        .find(|track| track.id == id)
        .unwrap()
}

fn project() -> Project {
    let mut project = Project::blank();
    project.channels[0].id = CHANNEL;
    project.channels[0].mixer_track = SOURCE;
    project.channels[0].instrument_plugin_instance_id = Some(GENERATOR);
    project.channels[0].volume = 0.5;
    project.channels[0].pan = -0.5;
    project.mixer_tracks[1].id = SOURCE;
    project.mixer_tracks[2].id = BUS;
    project.mixer_tracks[3].id = OTHER;
    for track in &mut project.mixer_tracks {
        track.volume = 1.0;
        track.pan = 0.0;
    }
    track(&mut project, SOURCE).volume = 0.75;
    track(&mut project, SOURCE).pan = 0.5;
    track(&mut project, BUS).volume = 0.5;
    track(&mut project, BUS).pan = -0.25;
    track(&mut project, MASTER_MIXER_TRACK_ID).pan = 0.5;
    project.mixer_routes = vec![
        main_route(11, 5, SOURCE, BUS),
        main_route(12, 2, BUS, MASTER_MIXER_TRACK_ID),
    ];
    project.mixer_routes[0].gain = 0.5;
    project.mixer_routes[1].gain = 0.25;
    project.plugin_instances = [
        GENERATOR,
        SOURCE_FIRST,
        SOURCE_LAST,
        BUS_EFFECT,
        MASTER_EFFECT,
    ]
    .into_iter()
    .map(test_plugin)
    .collect();
    for instance in &mut project.plugin_instances {
        instance.role = if instance.id == GENERATOR {
            PluginRole::Instrument
        } else {
            PluginRole::Effect
        };
    }
    // Deliberately neither path order nor physical-slot order.
    project.mixer_insert_slots = vec![
        MixerInsertSlotRef {
            track: MASTER_MIXER_TRACK_ID,
            slot: 4,
            plugin_instance_id: MASTER_EFFECT,
        },
        MixerInsertSlotRef {
            track: SOURCE,
            slot: 7,
            plugin_instance_id: SOURCE_LAST,
        },
        MixerInsertSlotRef {
            track: BUS,
            slot: 2,
            plugin_instance_id: BUS_EFFECT,
        },
        MixerInsertSlotRef {
            track: SOURCE,
            slot: 1,
            plugin_instance_id: SOURCE_FIRST,
        },
    ];
    project
}

fn placements(plan: &StaticSerialMasterPlan) -> Vec<StagePlacements> {
    plan.nodes
        .iter()
        .map(|node| {
            (
                node.track_id,
                node.placements
                    .iter()
                    .map(|p| (p.slot, p.plugin_instance_id))
                    .collect(),
            )
        })
        .collect()
}

fn expected_placements() -> Vec<StagePlacements> {
    vec![
        (SOURCE, vec![(1, SOURCE_FIRST), (7, SOURCE_LAST)]),
        (BUS, vec![(2, BUS_EFFECT)]),
        (MASTER_MIXER_TRACK_ID, vec![(4, MASTER_EFFECT)]),
    ]
}

fn node_gains(plan: &StaticSerialMasterPlan) -> Vec<(f32, f32)> {
    plan.nodes
        .iter()
        .map(|node| {
            if node.audible {
                (node.volume * node.pan_left, node.volume * node.pan_right)
            } else {
                (0.0, 0.0)
            }
        })
        .collect()
}

fn channel_gains(plan: &StaticSerialMasterPlan) -> (f32, f32) {
    if plan.channel_audible {
        (
            plan.channel_volume * plan.channel_pan_left,
            plan.channel_volume * plan.channel_pan_right,
        )
    } else {
        (0.0, 0.0)
    }
}

fn assert_refused(project: &Project, expected: &str) {
    let error = StaticSerialMasterPlan::prepare(project, CHANNEL)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(expected),
        "expected {expected:?}, got {error:?}"
    );
}

#[test]
fn serial_master_plan_orders_source_bus_master_and_physical_slots_by_stable_identity() {
    let mut project = project();
    project.mixer_tracks.reverse();
    project.mixer_routes.reverse();
    project.plugin_instances.reverse();
    let plan = StaticSerialMasterPlan::prepare(&project, CHANNEL).unwrap();
    assert_eq!(placements(&plan), expected_placements());
    assert_eq!(
        plan.nodes
            .iter()
            .map(|node| node.runtime_track)
            .collect::<Vec<_>>(),
        [1, 2, 0]
    );
    assert_eq!(
        plan.nodes
            .iter()
            .map(|node| node.is_master)
            .collect::<Vec<_>>(),
        [false, false, true]
    );
    assert_eq!(
        plan.nodes
            .iter()
            .map(|node| node.outgoing_gain)
            .collect::<Vec<_>>(),
        [0.5, 0.25, 1.0]
    );
    assert_eq!(channel_gains(&plan), (0.5, 0.25));
    assert_eq!(
        (
            plan.channel_volume,
            plan.channel_pan_left,
            plan.channel_pan_right,
            plan.channel_audible,
        ),
        (0.5, 1.0, 0.5, true)
    );
    assert_eq!(node_gains(&plan), [(0.375, 0.75), (0.5, 0.375), (0.5, 1.0)]);
}

#[test]
fn serial_master_plan_rejects_active_partial_wet_and_retains_inactive_endpoints() {
    for wet in [0.0, 0.25, 0.999] {
        let mut project = project();
        project
            .plugin_instances
            .iter_mut()
            .find(|p| p.id == SOURCE_FIRST)
            .unwrap()
            .wet = wet;
        assert_refused(&project, "fully wet active Inserts");
        for bypassed in [false, true] {
            let mut inactive = project.clone();
            for plugin in inactive
                .plugin_instances
                .iter_mut()
                .filter(|p| p.id != GENERATOR)
            {
                plugin.enabled = bypassed;
                plugin.bypass = bypassed;
                plugin.wet = wet;
            }
            let plan = StaticSerialMasterPlan::prepare(&inactive, CHANNEL).unwrap();
            assert_eq!(placements(&plan), expected_placements());
            assert_eq!(
                plan.nodes.len(),
                3,
                "bypassed/disabled endpoints remain real stages"
            );
            assert_eq!(node_gains(&plan), [(0.375, 0.75), (0.5, 0.375), (0.5, 1.0)]);
        }
    }
}

#[test]
fn serial_master_plan_rejects_invalid_wet_even_on_inactive_physical_slots() {
    for wet in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
        for enabled in [false, true] {
            let mut project = project();
            let plugin = project
                .plugin_instances
                .iter_mut()
                .find(|p| p.id == SOURCE_FIRST)
                .unwrap();
            plugin.enabled = enabled;
            plugin.bypass = true;
            plugin.wet = wet;
            assert_refused(&project, "wet value is invalid");
        }
    }
}

#[test]
fn serial_master_plan_rejects_off_path_fx_even_when_disabled_or_bypassed() {
    for (enabled, bypass) in [(true, false), (false, false), (true, true)] {
        let mut project = project();
        let mut off_path = test_plugin(300);
        off_path.enabled = enabled;
        off_path.bypass = bypass;
        project.plugin_instances.push(off_path);
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: OTHER,
            slot: 0,
            plugin_instance_id: 300,
        });
        assert_refused(&project, "off-path Inserts");
    }
}

#[test]
fn serial_master_plan_rejects_branches_sends_and_a_missing_main_route() {
    let mut branch = project();
    branch
        .mixer_routes
        .push(main_route(13, 7, SOURCE, MASTER_MIXER_TRACK_ID));
    assert_refused(&branch, "branches and sends are unsupported");
    for tap in [MixerRouteTap::PreEffects, MixerRouteTap::PostEffects] {
        let mut send = project();
        send.mixer_routes[0].tap = tap;
        assert_refused(&send, "one post-fader main route");
    }
    let mut missing = project();
    missing.mixer_routes.remove(1);
    assert_refused(&missing, "one post-fader main route");
    let mut master_send = project();
    master_send
        .mixer_routes
        .push(main_route(13, 7, MASTER_MIXER_TRACK_ID, OTHER));
    assert_refused(&master_send, "cannot use MASTER as its source");
}

#[test]
fn serial_master_plan_rejects_active_sidechain_routes() {
    let mut project = project();
    let mut sidechain = main_route(13, 7, OTHER, BUS);
    sidechain.destination = MixerRouteDestination::PluginSidechain {
        mixer_track_id: BUS,
        slot: 2,
        input_bus: 1,
    };
    project.mixer_routes.push(sidechain);
    assert_refused(&project, "active plug-in sidechain route");
}

#[test]
fn serial_master_plan_rejects_active_automation_but_allows_empty_or_disabled_lanes() {
    for target in [
        AutomationTarget::Tempo,
        AutomationTarget::MasterVolume,
        AutomationTarget::MasterPan,
        AutomationTarget::MixerMute {
            track: MASTER_MIXER_TRACK_ID,
        },
        AutomationTarget::MixerVolume { track: BUS },
        AutomationTarget::ChannelPan { channel: CHANNEL },
        AutomationTarget::PluginParameter {
            instance: SOURCE_FIRST,
            parameter: 0,
        },
    ] {
        let mut project = project();
        let mut lane = AutomationLane::new(target);
        lane.replace_points([AutomationPoint::new(0.0, 0.5)]);
        project.automation_lanes.push(ProjectAutomation {
            id: 1,
            name: "Active static-slice refusal".into(),
            lane,
        });
        assert_refused(&project, "does not yet support active automation");
        project.automation_lanes[0].lane.set_enabled(false);
        assert_eq!(
            placements(&StaticSerialMasterPlan::prepare(&project, CHANNEL).unwrap()),
            expected_placements()
        );
        project.automation_lanes[0].lane.set_enabled(true);
        project.automation_lanes[0].lane.replace_points([]);
        assert_eq!(
            placements(&StaticSerialMasterPlan::prepare(&project, CHANNEL).unwrap()),
            expected_placements()
        );
    }
}

#[test]
fn serial_master_plan_master_solo_never_seeds_nonmaster_audibility() {
    let mut project = project();
    track(&mut project, MASTER_MIXER_TRACK_ID).solo = true;
    let master_only = StaticSerialMasterPlan::prepare(&project, CHANNEL).unwrap();
    assert_eq!(
        node_gains(&master_only),
        [(0.375, 0.75), (0.5, 0.375), (0.5, 1.0)]
    );

    // If Master were a seed, its ancestors would incorrectly reopen this path
    // while an unrelated nonmaster solo is active.
    project
        .mixer_routes
        .push(main_route(13, 7, OTHER, MASTER_MIXER_TRACK_ID));
    track(&mut project, OTHER).solo = true;
    let unrelated = StaticSerialMasterPlan::prepare(&project, CHANNEL).unwrap();
    assert_eq!(node_gains(&unrelated), [(0.0, 0.0), (0.0, 0.0), (0.5, 1.0)]);
    assert_eq!(placements(&unrelated), expected_placements());
}

#[test]
fn serial_master_plan_source_and_bus_solo_keep_their_ancestor_descendant_path() {
    for solo in [SOURCE, BUS] {
        let mut project = project();
        track(&mut project, solo).solo = true;
        let plan = StaticSerialMasterPlan::prepare(&project, CHANNEL).unwrap();
        assert_eq!(node_gains(&plan), [(0.375, 0.75), (0.5, 0.375), (0.5, 1.0)]);
        assert_eq!(placements(&plan), expected_placements());
    }
}

#[test]
fn serial_master_plan_mute_gates_only_its_control_and_keeps_every_fx_stage() {
    for muted in [None, Some(SOURCE), Some(BUS), Some(MASTER_MIXER_TRACK_ID)] {
        let mut project = project();
        match muted {
            Some(id) => track(&mut project, id).muted = true,
            None => project.channels[0].muted = true,
        }
        let plan = StaticSerialMasterPlan::prepare(&project, CHANNEL).unwrap();
        assert_eq!(placements(&plan), expected_placements());
        assert_eq!(
            plan.nodes
                .iter()
                .map(|node| node.outgoing_gain)
                .collect::<Vec<_>>(),
            [0.5, 0.25, 1.0]
        );
        assert_eq!(
            channel_gains(&plan),
            if muted.is_none() {
                (0.0, 0.0)
            } else {
                (0.5, 0.25)
            }
        );
        for (index, expected) in [(0.375, 0.75), (0.5, 0.375), (0.5, 1.0)]
            .into_iter()
            .enumerate()
        {
            let node = &plan.nodes[index];
            assert_eq!(
                node_gains(&plan)[index],
                if muted == Some(node.track_id) {
                    (0.0, 0.0)
                } else {
                    expected
                }
            );
        }
    }
}

#[test]
fn serial_master_plan_rejects_ambiguous_or_missing_physical_insert_bindings() {
    let mut duplicate_slot = project();
    duplicate_slot.mixer_insert_slots[1].slot = 1;
    assert_refused(&duplicate_slot, "duplicate Insert slot placements");

    let mut duplicate_instance = project();
    duplicate_instance.mixer_insert_slots[1].plugin_instance_id = SOURCE_FIRST;
    assert_refused(
        &duplicate_instance,
        "unique physical Insert instance identities",
    );

    let mut missing_instance = project();
    missing_instance
        .plugin_instances
        .retain(|p| p.id != SOURCE_FIRST);
    assert_refused(&missing_instance, "Insert instance is missing");

    let mut generator_alias = project();
    generator_alias.mixer_insert_slots[3].plugin_instance_id = GENERATOR;
    assert_refused(&generator_alias, "Generator cannot also be an Insert");
}
