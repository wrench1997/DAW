//! Persisted mixer-routing identities and deterministic graph compilation.
//!
//! Project identities are deliberately separate from callback resource slots:
//! reordering `Project::mixer_tracks` or `Project::mixer_routes` must not move a
//! live bus or delay line.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::Project;

pub type MixerTrackId = u64;
pub type MixerRouteId = u64;

pub const MASTER_MIXER_TRACK_ID: MixerTrackId = 0xffff_ffff;
pub const MIXER_GRAPH_MAX_NODES: usize = 32;
pub const MIXER_GRAPH_MAX_EDGES: usize = 128;
pub const MIXER_MASTER_RUNTIME_SLOT: u8 = 0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixerRouteTap {
    PreEffects,
    PostEffects,
    #[default]
    PostFader,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MixerRouteDestination {
    MainInput {
        mixer_track_id: MixerTrackId,
    },
    PluginSidechain {
        mixer_track_id: MixerTrackId,
        slot: u8,
        input_bus: u8,
    },
}

impl MixerRouteDestination {
    #[must_use]
    pub const fn mixer_track_id(self) -> MixerTrackId {
        match self {
            Self::MainInput { mixer_track_id } | Self::PluginSidechain { mixer_track_id, .. } => {
                mixer_track_id
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerRoute {
    pub id: MixerRouteId,
    pub runtime_slot: u8,
    pub source_mixer_track_id: MixerTrackId,
    pub destination: MixerRouteDestination,
    #[serde(default)]
    pub tap: MixerRouteTap,
    #[serde(default = "default_route_gain")]
    pub gain: f32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

const fn default_route_gain() -> f32 {
    1.0
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledMixerNode {
    pub id: MixerTrackId,
    pub runtime_slot: u8,
    pub project_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompiledMixerRoute {
    pub id: MixerRouteId,
    pub runtime_slot: u8,
    pub source_id: MixerTrackId,
    pub destination_id: MixerTrackId,
    pub source_dense: u8,
    pub destination_dense: u8,
    pub source_runtime_slot: u8,
    pub destination_runtime_slot: u8,
    pub tap: MixerRouteTap,
    pub gain: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledMixerGraph {
    nodes: Vec<CompiledMixerNode>,
    routes: Vec<CompiledMixerRoute>,
    topological_order: Vec<u8>,
    id_to_dense: BTreeMap<MixerTrackId, u8>,
    route_slot_to_edge: [Option<u8>; MIXER_GRAPH_MAX_EDGES],
    outgoing_route_slots: Vec<Vec<u8>>,
    master_dense_index: u8,
    fingerprint: u64,
}

impl CompiledMixerGraph {
    #[must_use]
    pub fn nodes(&self) -> &[CompiledMixerNode] {
        &self.nodes
    }

    #[must_use]
    pub fn routes(&self) -> &[CompiledMixerRoute] {
        &self.routes
    }

    #[must_use]
    pub fn topological_order(&self) -> &[u8] {
        &self.topological_order
    }

    #[must_use]
    pub fn dense_index(&self, id: MixerTrackId) -> Option<u8> {
        self.id_to_dense.get(&id).copied()
    }

    #[must_use]
    pub fn node_by_id(&self, id: MixerTrackId) -> Option<CompiledMixerNode> {
        self.dense_index(id)
            .and_then(|dense| self.nodes.get(usize::from(dense)).copied())
    }

    #[must_use]
    pub const fn master_dense_index(&self) -> u8 {
        self.master_dense_index
    }

    #[must_use]
    pub const fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    #[must_use]
    pub fn runtime_slot_for_id(&self, id: MixerTrackId) -> Option<u8> {
        self.node_by_id(id).map(|node| node.runtime_slot)
    }

    #[must_use]
    pub fn route_at_runtime_slot(&self, runtime_slot: u8) -> Option<CompiledMixerRoute> {
        self.route_slot_to_edge
            .get(usize::from(runtime_slot))
            .copied()
            .flatten()
            .and_then(|index| self.routes.get(usize::from(index)).copied())
    }

    #[must_use]
    pub fn outgoing_route_slots(&self, dense_index: u8) -> &[u8] {
        self.outgoing_route_slots
            .get(usize::from(dense_index))
            .map_or(&[], Vec::as_slice)
    }
}

/// Allocation-free callback candidate populated from a control-thread compiled
/// graph. `reset_from` only copies into fixed arrays; a later callback commit
/// can swap two layouts in O(1) without cloning project vectors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedMixerGraphLayout {
    fingerprint: u64,
    node_count: u8,
    route_count: u16,
    nodes_by_runtime_slot: [Option<CompiledMixerNode>; MIXER_GRAPH_MAX_NODES],
    routes_by_runtime_slot: [Option<CompiledMixerRoute>; MIXER_GRAPH_MAX_EDGES],
    topological_runtime_slots: [u8; MIXER_GRAPH_MAX_NODES],
    outgoing_route_slots: [[u8; MIXER_GRAPH_MAX_EDGES]; MIXER_GRAPH_MAX_NODES],
    outgoing_route_counts: [u16; MIXER_GRAPH_MAX_NODES],
}

impl Default for FixedMixerGraphLayout {
    fn default() -> Self {
        Self {
            fingerprint: 0,
            node_count: 0,
            route_count: 0,
            nodes_by_runtime_slot: [None; MIXER_GRAPH_MAX_NODES],
            routes_by_runtime_slot: [None; MIXER_GRAPH_MAX_EDGES],
            topological_runtime_slots: [0; MIXER_GRAPH_MAX_NODES],
            outgoing_route_slots: [[0; MIXER_GRAPH_MAX_EDGES]; MIXER_GRAPH_MAX_NODES],
            outgoing_route_counts: [0; MIXER_GRAPH_MAX_NODES],
        }
    }
}

impl FixedMixerGraphLayout {
    #[must_use]
    pub fn reset_from(&mut self, graph: &CompiledMixerGraph) -> bool {
        if graph.nodes.len() > MIXER_GRAPH_MAX_NODES || graph.routes.len() > MIXER_GRAPH_MAX_EDGES {
            return false;
        }
        *self = Self::default();
        self.fingerprint = graph.fingerprint;
        self.node_count = graph.nodes.len() as u8;
        self.route_count = graph.routes.len() as u16;
        for node in &graph.nodes {
            let slot = usize::from(node.runtime_slot);
            if slot >= MIXER_GRAPH_MAX_NODES || self.nodes_by_runtime_slot[slot].is_some() {
                return false;
            }
            self.nodes_by_runtime_slot[slot] = Some(*node);
        }
        for route in &graph.routes {
            let slot = usize::from(route.runtime_slot);
            if slot >= MIXER_GRAPH_MAX_EDGES || self.routes_by_runtime_slot[slot].is_some() {
                return false;
            }
            self.routes_by_runtime_slot[slot] = Some(*route);
        }
        for (topological_index, dense) in graph.topological_order.iter().copied().enumerate() {
            let Some(node) = graph.nodes.get(usize::from(dense)) else {
                return false;
            };
            self.topological_runtime_slots[topological_index] = node.runtime_slot;
        }
        for (dense, route_slots) in graph.outgoing_route_slots.iter().enumerate() {
            let Some(node) = graph.nodes.get(dense) else {
                return false;
            };
            let runtime_slot = usize::from(node.runtime_slot);
            if route_slots.len() > MIXER_GRAPH_MAX_EDGES {
                return false;
            }
            self.outgoing_route_counts[runtime_slot] = route_slots.len() as u16;
            self.outgoing_route_slots[runtime_slot][..route_slots.len()]
                .copy_from_slice(route_slots);
        }
        true
    }

    #[must_use]
    pub const fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    #[must_use]
    pub const fn node_count(&self) -> usize {
        self.node_count as usize
    }

    #[must_use]
    pub const fn route_count(&self) -> usize {
        self.route_count as usize
    }

    #[must_use]
    pub fn node_at_runtime_slot(&self, runtime_slot: u8) -> Option<CompiledMixerNode> {
        self.nodes_by_runtime_slot
            .get(usize::from(runtime_slot))
            .copied()
            .flatten()
    }

    #[must_use]
    pub fn route_at_runtime_slot(&self, runtime_slot: u8) -> Option<CompiledMixerRoute> {
        self.routes_by_runtime_slot
            .get(usize::from(runtime_slot))
            .copied()
            .flatten()
    }

    #[must_use]
    pub fn topological_runtime_slots(&self) -> &[u8] {
        &self.topological_runtime_slots[..self.node_count()]
    }

    #[must_use]
    pub fn outgoing_route_slots(&self, source_runtime_slot: u8) -> &[u8] {
        let index = usize::from(source_runtime_slot);
        let count = self
            .outgoing_route_counts
            .get(index)
            .copied()
            .map_or(0, usize::from);
        self.outgoing_route_slots
            .get(index)
            .map_or(&[], |slots| &slots[..count])
    }
}

#[derive(Clone, Debug, PartialEq, Error)]
pub enum MixerGraphCompileError {
    #[error("mixer graph contains {actual} tracks; maximum is {maximum}")]
    NodeLimit { actual: usize, maximum: usize },
    #[error("mixer graph contains {actual} routes; maximum is {maximum}")]
    EdgeLimit { actual: usize, maximum: usize },
    #[error("mixer track at project index {project_index} has reserved zero identity")]
    ZeroTrackId { project_index: usize },
    #[error("mixer track identity {id} is duplicated")]
    DuplicateTrackId { id: MixerTrackId },
    #[error("mixer graph has no MASTER track")]
    MissingMaster,
    #[error("mixer graph has {count} MASTER tracks")]
    MultipleMaster { count: usize },
    #[error("MASTER must own runtime slot 0, not {runtime_slot}")]
    InvalidMasterRuntimeSlot { runtime_slot: u8 },
    #[error("non-MASTER mixer track {id} cannot own runtime slot {runtime_slot}")]
    InvalidTrackRuntimeSlot { id: MixerTrackId, runtime_slot: u8 },
    #[error("mixer runtime slot {runtime_slot} is duplicated")]
    DuplicateTrackRuntimeSlot { runtime_slot: u8 },
    #[error("mixer route has reserved zero identity")]
    ZeroRouteId,
    #[error("mixer route identity {id} is duplicated")]
    DuplicateRouteId { id: MixerRouteId },
    #[error("mixer route runtime slot {runtime_slot} is outside 0..{maximum}")]
    InvalidRouteRuntimeSlot { runtime_slot: u8, maximum: usize },
    #[error("mixer route runtime slot {runtime_slot} is duplicated")]
    DuplicateRouteRuntimeSlot { runtime_slot: u8 },
    #[error("mixer route {route_id} has invalid gain {gain}")]
    InvalidGain { route_id: MixerRouteId, gain: f32 },
    #[error("mixer route {route_id} references missing source track {track_id}")]
    DanglingSource {
        route_id: MixerRouteId,
        track_id: MixerTrackId,
    },
    #[error("mixer route {route_id} references missing destination track {track_id}")]
    DanglingDestination {
        route_id: MixerRouteId,
        track_id: MixerTrackId,
    },
    #[error("mixer route {route_id} routes track {track_id} into itself")]
    SelfLoop {
        route_id: MixerRouteId,
        track_id: MixerTrackId,
    },
    #[error("enabled main-input route {route_id} cannot use MASTER as its source")]
    MasterCannotRoute { route_id: MixerRouteId },
    #[error("mixer routes {first_route_id} and {second_route_id} duplicate the same edge")]
    DuplicateEdge {
        first_route_id: MixerRouteId,
        second_route_id: MixerRouteId,
    },
    #[error("active plug-in sidechain route {route_id} is not supported by this graph kernel")]
    ActiveSidechainUnsupported { route_id: MixerRouteId },
    #[error("mixer main-input routes contain a cycle")]
    Cycle,
}

pub fn compile_mixer_graph(
    project: &Project,
) -> Result<CompiledMixerGraph, MixerGraphCompileError> {
    let track_count = project.mixer_tracks.len();
    if track_count > MIXER_GRAPH_MAX_NODES {
        return Err(MixerGraphCompileError::NodeLimit {
            actual: track_count,
            maximum: MIXER_GRAPH_MAX_NODES,
        });
    }
    if project.mixer_routes.len() > MIXER_GRAPH_MAX_EDGES {
        return Err(MixerGraphCompileError::EdgeLimit {
            actual: project.mixer_routes.len(),
            maximum: MIXER_GRAPH_MAX_EDGES,
        });
    }

    let master_count = project
        .mixer_tracks
        .iter()
        .filter(|track| track.id == MASTER_MIXER_TRACK_ID)
        .count();
    match master_count {
        0 => return Err(MixerGraphCompileError::MissingMaster),
        1 => {}
        count => return Err(MixerGraphCompileError::MultipleMaster { count }),
    }

    let mut tracks = project
        .mixer_tracks
        .iter()
        .enumerate()
        .map(|(project_index, track)| CompiledMixerNode {
            id: track.id,
            runtime_slot: track.runtime_slot,
            project_index,
        })
        .collect::<Vec<_>>();
    tracks.sort_by_key(|node| node.runtime_slot);

    let mut ids = BTreeSet::new();
    let mut runtime_slots = BTreeSet::new();
    for node in &tracks {
        if node.id == 0 {
            return Err(MixerGraphCompileError::ZeroTrackId {
                project_index: node.project_index,
            });
        }
        if !ids.insert(node.id) {
            return Err(MixerGraphCompileError::DuplicateTrackId { id: node.id });
        }
        if node.id == MASTER_MIXER_TRACK_ID {
            if node.runtime_slot != MIXER_MASTER_RUNTIME_SLOT {
                return Err(MixerGraphCompileError::InvalidMasterRuntimeSlot {
                    runtime_slot: node.runtime_slot,
                });
            }
        } else if node.runtime_slot == MIXER_MASTER_RUNTIME_SLOT
            || usize::from(node.runtime_slot) >= MIXER_GRAPH_MAX_NODES
        {
            return Err(MixerGraphCompileError::InvalidTrackRuntimeSlot {
                id: node.id,
                runtime_slot: node.runtime_slot,
            });
        }
        if !runtime_slots.insert(node.runtime_slot) {
            return Err(MixerGraphCompileError::DuplicateTrackRuntimeSlot {
                runtime_slot: node.runtime_slot,
            });
        }
    }

    let id_to_dense = tracks
        .iter()
        .enumerate()
        .map(|(dense, node)| (node.id, dense as u8))
        .collect::<BTreeMap<_, _>>();
    let master_dense_index = id_to_dense[&MASTER_MIXER_TRACK_ID];

    let mut route_ids = BTreeSet::new();
    let mut route_slots = BTreeSet::new();
    let mut edge_keys = BTreeMap::new();
    let mut routes = Vec::new();
    for route in &project.mixer_routes {
        if route.id == 0 {
            return Err(MixerGraphCompileError::ZeroRouteId);
        }
        if !route_ids.insert(route.id) {
            return Err(MixerGraphCompileError::DuplicateRouteId { id: route.id });
        }
        if usize::from(route.runtime_slot) >= MIXER_GRAPH_MAX_EDGES {
            return Err(MixerGraphCompileError::InvalidRouteRuntimeSlot {
                runtime_slot: route.runtime_slot,
                maximum: MIXER_GRAPH_MAX_EDGES,
            });
        }
        if !route_slots.insert(route.runtime_slot) {
            return Err(MixerGraphCompileError::DuplicateRouteRuntimeSlot {
                runtime_slot: route.runtime_slot,
            });
        }
        if !route.gain.is_finite() || !(0.0..=4.0).contains(&route.gain) {
            return Err(MixerGraphCompileError::InvalidGain {
                route_id: route.id,
                gain: route.gain,
            });
        }
        let source_dense = id_to_dense
            .get(&route.source_mixer_track_id)
            .copied()
            .ok_or(MixerGraphCompileError::DanglingSource {
                route_id: route.id,
                track_id: route.source_mixer_track_id,
            })?;
        let destination_id = route.destination.mixer_track_id();
        let destination_dense = id_to_dense.get(&destination_id).copied().ok_or(
            MixerGraphCompileError::DanglingDestination {
                route_id: route.id,
                track_id: destination_id,
            },
        )?;
        if source_dense == destination_dense {
            return Err(MixerGraphCompileError::SelfLoop {
                route_id: route.id,
                track_id: route.source_mixer_track_id,
            });
        }
        if matches!(
            route.destination,
            MixerRouteDestination::PluginSidechain { .. }
        ) {
            if route.enabled {
                return Err(MixerGraphCompileError::ActiveSidechainUnsupported {
                    route_id: route.id,
                });
            }
            continue;
        }
        if route.enabled && route.source_mixer_track_id == MASTER_MIXER_TRACK_ID {
            return Err(MixerGraphCompileError::MasterCannotRoute { route_id: route.id });
        }
        let key = (source_dense, destination_dense, route.tap);
        if let Some(first_route_id) = edge_keys.insert(key, route.id) {
            return Err(MixerGraphCompileError::DuplicateEdge {
                first_route_id,
                second_route_id: route.id,
            });
        }
        if !route.enabled {
            continue;
        }
        routes.push(CompiledMixerRoute {
            id: route.id,
            runtime_slot: route.runtime_slot,
            source_id: route.source_mixer_track_id,
            destination_id,
            source_dense,
            destination_dense,
            source_runtime_slot: tracks[usize::from(source_dense)].runtime_slot,
            destination_runtime_slot: tracks[usize::from(destination_dense)].runtime_slot,
            tap: route.tap,
            gain: route.gain,
        });
    }
    if routes.len() > MIXER_GRAPH_MAX_EDGES {
        return Err(MixerGraphCompileError::EdgeLimit {
            actual: routes.len(),
            maximum: MIXER_GRAPH_MAX_EDGES,
        });
    }
    routes.sort_by_key(|route| route.runtime_slot);

    let mut indegree = vec![0_u8; tracks.len()];
    let mut outgoing = vec![Vec::<u8>::new(); tracks.len()];
    let mut outgoing_route_slots = vec![Vec::<u8>::new(); tracks.len()];
    for route in &routes {
        indegree[usize::from(route.destination_dense)] += 1;
        outgoing[usize::from(route.source_dense)].push(route.destination_dense);
        outgoing_route_slots[usize::from(route.source_dense)].push(route.runtime_slot);
    }
    for destinations in &mut outgoing {
        destinations.sort_by_key(|dense| tracks[usize::from(*dense)].runtime_slot);
    }
    for slots in &mut outgoing_route_slots {
        slots.sort_unstable();
    }
    let mut ready = tracks
        .iter()
        .enumerate()
        .filter_map(|(dense, node)| {
            (indegree[dense] == 0).then_some((node.runtime_slot, dense as u8))
        })
        .collect::<BTreeSet<_>>();
    let mut topological_order = Vec::with_capacity(tracks.len());
    while let Some(&(runtime_slot, dense)) = ready.first() {
        ready.remove(&(runtime_slot, dense));
        topological_order.push(dense);
        for destination in &outgoing[usize::from(dense)] {
            let degree = &mut indegree[usize::from(*destination)];
            *degree -= 1;
            if *degree == 0 {
                ready.insert((tracks[usize::from(*destination)].runtime_slot, *destination));
            }
        }
    }
    if topological_order.len() != tracks.len() {
        return Err(MixerGraphCompileError::Cycle);
    }

    let mut route_slot_to_edge = [None; MIXER_GRAPH_MAX_EDGES];
    for (index, route) in routes.iter().enumerate() {
        route_slot_to_edge[usize::from(route.runtime_slot)] = Some(index as u8);
    }
    let fingerprint = mixer_graph_fingerprint(&tracks, &routes, &topological_order);
    Ok(CompiledMixerGraph {
        nodes: tracks,
        routes,
        topological_order,
        id_to_dense,
        route_slot_to_edge,
        outgoing_route_slots,
        master_dense_index,
        fingerprint,
    })
}

fn mixer_graph_fingerprint(
    nodes: &[CompiledMixerNode],
    routes: &[CompiledMixerRoute],
    topological_order: &[u8],
) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    fn mix(state: &mut u64, value: u64) {
        for byte in value.to_le_bytes() {
            *state ^= u64::from(byte);
            *state = state.wrapping_mul(FNV_PRIME);
        }
    }

    let mut state = FNV_OFFSET;
    mix(&mut state, nodes.len() as u64);
    for node in nodes {
        mix(&mut state, node.id);
        mix(&mut state, u64::from(node.runtime_slot));
    }
    mix(&mut state, routes.len() as u64);
    for route in routes {
        mix(&mut state, route.id);
        mix(&mut state, u64::from(route.runtime_slot));
        mix(&mut state, route.source_id);
        mix(&mut state, route.destination_id);
        mix(&mut state, u64::from(route.source_runtime_slot));
        mix(&mut state, u64::from(route.destination_runtime_slot));
        mix(
            &mut state,
            match route.tap {
                MixerRouteTap::PreEffects => 0,
                MixerRouteTap::PostEffects => 1,
                MixerRouteTap::PostFader => 2,
            },
        );
        mix(&mut state, u64::from(route.gain.to_bits()));
    }
    mix(&mut state, topological_order.len() as u64);
    for dense in topological_order {
        mix(
            &mut state,
            u64::from(nodes[usize::from(*dense)].runtime_slot),
        );
    }
    state.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(
        id: MixerRouteId,
        runtime_slot: u8,
        source: MixerTrackId,
        destination: MixerTrackId,
    ) -> MixerRoute {
        MixerRoute {
            id,
            runtime_slot,
            source_mixer_track_id: source,
            destination: MixerRouteDestination::MainInput {
                mixer_track_id: destination,
            },
            tap: MixerRouteTap::PostFader,
            gain: 1.0,
            enabled: true,
        }
    }

    #[test]
    fn default_graph_is_slot_addressed_and_master_is_the_final_sink() {
        let project = Project::blank();
        let graph = compile_mixer_graph(&project).unwrap();
        assert_eq!(graph.nodes().len(), 32);
        assert_eq!(graph.routes().len(), 31);
        assert_eq!(
            graph.runtime_slot_for_id(MASTER_MIXER_TRACK_ID),
            Some(MIXER_MASTER_RUNTIME_SLOT)
        );
        assert_eq!(
            graph.nodes()[usize::from(graph.master_dense_index())].id,
            MASTER_MIXER_TRACK_ID
        );
        let last = *graph.topological_order().last().unwrap();
        assert_eq!(last, graph.master_dense_index());
        let mut fixed = FixedMixerGraphLayout::default();
        assert!(fixed.reset_from(&graph));
        assert_eq!(fixed.fingerprint(), graph.fingerprint());
        assert_eq!(fixed.node_count(), 32);
        assert_eq!(fixed.route_count(), 31);
        assert_eq!(
            fixed.topological_runtime_slots().last(),
            Some(&MIXER_MASTER_RUNTIME_SLOT)
        );
        for runtime_slot in 0_u8..31 {
            let edge = graph.route_at_runtime_slot(runtime_slot).unwrap();
            assert_eq!(edge.runtime_slot, runtime_slot);
            assert_eq!(edge.destination_id, MASTER_MIXER_TRACK_ID);
        }
    }

    #[test]
    fn display_reorder_preserves_id_slot_mapping_routes_and_references() {
        let mut project = Project::blank();
        let channel_destination = project.channels[0].mixer_track;
        let route_endpoints = project
            .mixer_routes
            .iter()
            .map(|route| (route.id, route.source_mixer_track_id, route.destination))
            .collect::<Vec<_>>();
        let before = compile_mixer_graph(&project).unwrap();
        let before_topology = before
            .topological_order()
            .iter()
            .map(|dense| before.nodes()[usize::from(*dense)].runtime_slot)
            .collect::<Vec<_>>();

        project.mixer_tracks.reverse();
        let after = compile_mixer_graph(&project).unwrap();
        let after_topology = after
            .topological_order()
            .iter()
            .map(|dense| after.nodes()[usize::from(*dense)].runtime_slot)
            .collect::<Vec<_>>();

        assert_eq!(project.channels[0].mixer_track, channel_destination);
        assert_eq!(
            project
                .mixer_routes
                .iter()
                .map(|route| (route.id, route.source_mixer_track_id, route.destination))
                .collect::<Vec<_>>(),
            route_endpoints
        );
        assert_eq!(before_topology, after_topology);
        assert_eq!(before.fingerprint(), after.fingerprint());
        for id in project.mixer_tracks.iter().map(|track| track.id) {
            assert_eq!(
                before.runtime_slot_for_id(id),
                after.runtime_slot_for_id(id)
            );
        }
    }

    #[test]
    fn structural_identity_and_endpoint_errors_are_distinct() {
        let mut project = Project::blank();
        project.mixer_tracks[1].id = MASTER_MIXER_TRACK_ID;
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::MultipleMaster { count: 2 })
        ));

        let mut project = Project::blank();
        project.mixer_routes[1].id = project.mixer_routes[0].id;
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::DuplicateRouteId { .. })
        ));

        let mut project = Project::blank();
        project.mixer_routes = vec![route(1, 0, 1, 99_999)];
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::DanglingDestination { .. })
        ));

        let mut project = Project::blank();
        project.mixer_routes = vec![route(1, 0, 1, 1)];
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::SelfLoop { .. })
        ));
    }

    #[test]
    fn duplicate_edge_cycle_and_master_outgoing_are_rejected() {
        let mut project = Project::blank();
        project.mixer_routes = vec![route(1, 0, 1, 2), route(2, 1, 1, 2)];
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::DuplicateEdge { .. })
        ));

        project.mixer_routes = vec![route(1, 0, 1, 2), route(2, 1, 2, 1)];
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::Cycle)
        ));

        project.mixer_routes = vec![route(1, 0, MASTER_MIXER_TRACK_ID, 1)];
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::MasterCannotRoute { route_id: 1 })
        ));
    }

    #[test]
    fn disabled_routes_still_validate_structure_but_disabled_sidechain_roundtrips() {
        let mut project = Project::blank();
        let mut dangling = route(1, 0, 1, 99_999);
        dangling.enabled = false;
        project.mixer_routes = vec![dangling];
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::DanglingDestination { .. })
        ));

        let disabled_sidechain = MixerRoute {
            id: 2,
            runtime_slot: 1,
            source_mixer_track_id: 1,
            destination: MixerRouteDestination::PluginSidechain {
                mixer_track_id: 2,
                slot: 3,
                input_bus: 1,
            },
            tap: MixerRouteTap::PreEffects,
            gain: 0.5,
            enabled: false,
        };
        project.mixer_routes = vec![disabled_sidechain.clone()];
        let graph = compile_mixer_graph(&project).unwrap();
        assert!(graph.routes().is_empty());
        let encoded = serde_json::to_string(&disabled_sidechain).unwrap();
        assert_eq!(
            serde_json::from_str::<MixerRoute>(&encoded).unwrap(),
            disabled_sidechain
        );

        project.mixer_routes[0].enabled = true;
        assert!(matches!(
            compile_mixer_graph(&project),
            Err(MixerGraphCompileError::ActiveSidechainUnsupported { route_id: 2 })
        ));
    }
}
