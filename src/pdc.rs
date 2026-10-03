//! Plug-in delay compensation planning and realtime-safe stereo delay primitives.
//!
//! The legacy plan models generators feeding one mixer insert. The graph plan
//! additionally handles bounded multi-edge DAGs; sidechains remain a separate
//! signal domain and never participate in main-path compensation.

use thiserror::Error;

use crate::mixer_graph::{
    CompiledMixerGraph, MIXER_GRAPH_MAX_EDGES, MIXER_GRAPH_MAX_NODES, MixerRouteId, MixerRouteTap,
    MixerTrackId,
};

pub const PDC_TRACK_COUNT: usize = 32;
pub const PDC_MAX_GENERATORS: usize = 64;
pub const PDC_GRAPH_MAX_NODES: usize = MIXER_GRAPH_MAX_NODES;
pub const PDC_GRAPH_MAX_MAIN_INPUTS: usize = MIXER_GRAPH_MAX_EDGES;
pub const PDC_DEFAULT_MAX_DELAY_SAMPLES: u32 = 262_144;
pub const PDC_HARD_MAX_DELAY_SAMPLES: u32 = 1_048_576;
pub const Q128_CONTROL_QUANTUM_FRAMES: u64 = 128;

pub type GraphPdcNodeId = MixerTrackId;
pub type GraphPdcRouteId = MixerRouteId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratorPathLatency {
    pub endpoint_id: u64,
    pub channel_id: u32,
    pub mixer_track: usize,
    pub latency_samples: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompensationDelay {
    requested_samples: u64,
    applied_samples: u32,
}

impl CompensationDelay {
    fn new(requested_samples: u64, maximum_samples: u32) -> Self {
        Self {
            requested_samples,
            applied_samples: requested_samples.min(u64::from(maximum_samples)) as u32,
        }
    }

    pub fn requested_samples(self) -> u64 {
        self.requested_samples
    }

    pub fn applied_samples(self) -> u32 {
        self.applied_samples
    }

    pub fn is_clamped(self) -> bool {
        self.requested_samples > u64::from(self.applied_samples)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneratorCompensation {
    pub endpoint_id: u64,
    pub channel_id: u32,
    pub mixer_track: usize,
    pub path_latency_samples: u64,
    pub delay: CompensationDelay,
}

/// Immutable control-thread result consumed by the audio graph.
///
/// Mixer insert zero is the master endpoint. Its latency is common to every
/// source, so it contributes to the reported output latency but never to a
/// relative delay. Track zero therefore behaves as a direct-master source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdcPlan {
    reference_latency_samples: u64,
    master_latency_samples: u32,
    maximum_delay_samples: u32,
    raw_track_delays: [CompensationDelay; PDC_TRACK_COUNT],
    generator_delays: [Option<GeneratorCompensation>; PDC_MAX_GENERATORS],
    generator_count: usize,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PdcPlanError {
    #[error("at most {PDC_MAX_GENERATORS} generator paths are supported, got {0}")]
    TooManyGenerators(usize),
    #[error("generator channel {channel_id} routes to invalid mixer track {mixer_track}")]
    InvalidMixerTrack { channel_id: u32, mixer_track: usize },
    #[error(
        "PDC delay capacity {0} exceeds the hard limit of {PDC_HARD_MAX_DELAY_SAMPLES} samples"
    )]
    DelayCapacityTooLarge(u32),
}

impl PdcPlan {
    pub fn build(
        insert_latencies: [u32; PDC_TRACK_COUNT],
        master_latency_samples: u32,
        generators: &[GeneratorPathLatency],
        maximum_delay_samples: u32,
    ) -> Result<Self, PdcPlanError> {
        if generators.len() > PDC_MAX_GENERATORS {
            return Err(PdcPlanError::TooManyGenerators(generators.len()));
        }
        if maximum_delay_samples > PDC_HARD_MAX_DELAY_SAMPLES {
            return Err(PdcPlanError::DelayCapacityTooLarge(maximum_delay_samples));
        }
        for generator in generators {
            if generator.mixer_track >= PDC_TRACK_COUNT {
                return Err(PdcPlanError::InvalidMixerTrack {
                    channel_id: generator.channel_id,
                    mixer_track: generator.mixer_track,
                });
            }
        }

        // Insert zero is Master and is intentionally excluded from the source
        // reference. Every other insert latency is a complete serial chain.
        let mut reference_latency_samples = insert_latencies[1..]
            .iter()
            .copied()
            .map(u64::from)
            .max()
            .unwrap_or(0);
        for generator in generators {
            let insert_latency = if generator.mixer_track == 0 {
                0
            } else {
                insert_latencies[generator.mixer_track]
            };
            reference_latency_samples = reference_latency_samples
                .max(u64::from(generator.latency_samples) + u64::from(insert_latency));
        }

        let raw_track_delays = std::array::from_fn(|track| {
            let path_latency = if track == 0 {
                0
            } else {
                u64::from(insert_latencies[track])
            };
            CompensationDelay::new(
                reference_latency_samples.saturating_sub(path_latency),
                maximum_delay_samples,
            )
        });
        let mut generator_delays = [None; PDC_MAX_GENERATORS];
        for (index, generator) in generators.iter().copied().enumerate() {
            let insert_latency = if generator.mixer_track == 0 {
                0
            } else {
                insert_latencies[generator.mixer_track]
            };
            let path_latency_samples =
                u64::from(generator.latency_samples) + u64::from(insert_latency);
            generator_delays[index] = Some(GeneratorCompensation {
                endpoint_id: generator.endpoint_id,
                channel_id: generator.channel_id,
                mixer_track: generator.mixer_track,
                path_latency_samples,
                delay: CompensationDelay::new(
                    reference_latency_samples.saturating_sub(path_latency_samples),
                    maximum_delay_samples,
                ),
            });
        }

        Ok(Self {
            reference_latency_samples,
            master_latency_samples,
            maximum_delay_samples,
            raw_track_delays,
            generator_delays,
            generator_count: generators.len(),
        })
    }

    pub fn reference_latency_samples(&self) -> u64 {
        self.reference_latency_samples
    }

    pub fn master_latency_samples(&self) -> u32 {
        self.master_latency_samples
    }

    pub fn output_latency_samples(&self) -> u64 {
        self.reference_latency_samples
            .saturating_add(u64::from(self.master_latency_samples))
    }

    pub fn maximum_delay_samples(&self) -> u32 {
        self.maximum_delay_samples
    }

    pub fn raw_track_delay(&self, mixer_track: usize) -> Option<CompensationDelay> {
        self.raw_track_delays.get(mixer_track).copied()
    }

    pub fn generator_delays(&self) -> impl Iterator<Item = &GeneratorCompensation> {
        self.generator_delays[..self.generator_count]
            .iter()
            .flatten()
    }

    pub fn has_clamped_delays(&self) -> bool {
        self.raw_track_delays.iter().any(|delay| delay.is_clamped())
            || self
                .generator_delays()
                .any(|generator| generator.delay.is_clamped())
    }
}

/// One mixer processing stage in a compiled acyclic graph.
///
/// `runtime_slot` is the bounded index used by the callback. Node identifiers
/// are control-thread identities and need not be dense.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphPdcNode {
    pub node_id: GraphPdcNodeId,
    pub runtime_slot: usize,
    pub stage_latency_samples: u64,
}

/// A main-audio edge. Sidechains deliberately have no representation here, so
/// an active sidechain can never accidentally participate in main-path PDC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphPdcMainInput {
    pub route_id: GraphPdcRouteId,
    pub runtime_slot: usize,
    pub source_id: GraphPdcNodeId,
    pub destination_id: GraphPdcNodeId,
    pub tap: MixerRouteTap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphPdcGenerator {
    pub endpoint_id: u64,
    pub channel_id: u32,
    pub destination_id: GraphPdcNodeId,
    pub latency_samples: u64,
}

/// Borrowed control-thread input for the fixed-capacity graph planner.
#[derive(Clone, Copy, Debug)]
pub struct GraphPdcInput<'a> {
    pub nodes: &'a [GraphPdcNode],
    /// Node identifiers in strict source-to-master order.
    pub topological_order: &'a [GraphPdcNodeId],
    pub main_inputs: &'a [GraphPdcMainInput],
    pub generators: &'a [GraphPdcGenerator],
    pub master_node_id: GraphPdcNodeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphPdcNodeCompensation {
    pub node_id: GraphPdcNodeId,
    pub runtime_slot: usize,
    pub stage_latency_samples: u64,
    pub join_latency_samples: u64,
    pub raw_source_delay: CompensationDelay,
    pub output_latency_samples: u64,
    pub output_latency_overflowed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphPdcMainInputCompensation {
    pub route_id: GraphPdcRouteId,
    pub runtime_slot: usize,
    pub source_id: GraphPdcNodeId,
    pub destination_id: GraphPdcNodeId,
    pub tap: MixerRouteTap,
    pub source_latency_samples: u64,
    pub destination_join_latency_samples: u64,
    pub delay: CompensationDelay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphPdcGeneratorCompensation {
    pub endpoint_id: u64,
    pub channel_id: u32,
    pub destination_id: GraphPdcNodeId,
    pub path_latency_samples: u64,
    pub destination_join_latency_samples: u64,
    pub delay: CompensationDelay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphPdcPathIdentity {
    RawSource {
        node_id: GraphPdcNodeId,
    },
    Generator {
        endpoint_id: u64,
        channel_id: u32,
        destination_id: GraphPdcNodeId,
    },
    MainInput {
        route_id: GraphPdcRouteId,
        source_id: GraphPdcNodeId,
        destination_id: GraphPdcNodeId,
        tap: MixerRouteTap,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphPdcPathCompensation {
    pub identity: GraphPdcPathIdentity,
    pub arrival_latency_samples: u64,
    pub join_latency_samples: u64,
    pub delay: CompensationDelay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphPdcDiagnostics {
    pub arithmetic_overflows: u16,
    pub clamped_raw_sources: u16,
    pub clamped_generators: u16,
    pub clamped_main_inputs: u16,
}

impl GraphPdcDiagnostics {
    pub fn has_arithmetic_overflow(self) -> bool {
        self.arithmetic_overflows != 0
    }

    pub fn has_clamped_delays(self) -> bool {
        self.clamped_raw_sources != 0
            || self.clamped_generators != 0
            || self.clamped_main_inputs != 0
    }

    pub fn total_clamped_delays(self) -> u16 {
        self.clamped_raw_sources
            .saturating_add(self.clamped_generators)
            .saturating_add(self.clamped_main_inputs)
    }
}

/// Fixed-capacity graph PDC result. Building is control-thread work; all
/// callback-facing lookups and iterators are bounded and allocation-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphPdcPlan {
    maximum_delay_samples: u32,
    graph_fingerprint: Option<u64>,
    master_node_id: GraphPdcNodeId,
    master_output_latency_samples: u64,
    nodes_by_runtime_slot: [Option<GraphPdcNodeCompensation>; PDC_GRAPH_MAX_NODES],
    topological_runtime_slots: [usize; PDC_GRAPH_MAX_NODES],
    node_count: usize,
    main_inputs: [Option<GraphPdcMainInputCompensation>; PDC_GRAPH_MAX_MAIN_INPUTS],
    main_input_count: usize,
    generators: [Option<GraphPdcGeneratorCompensation>; PDC_MAX_GENERATORS],
    generator_count: usize,
    diagnostics: GraphPdcDiagnostics,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum GraphPdcPlanError {
    #[error("at most {PDC_GRAPH_MAX_NODES} graph nodes are supported, got {0}")]
    TooManyNodes(usize),
    #[error("at most {PDC_GRAPH_MAX_MAIN_INPUTS} main-input edges are supported, got {0}")]
    TooManyMainInputs(usize),
    #[error("at most {PDC_MAX_GENERATORS} generator paths are supported, got {0}")]
    TooManyGenerators(usize),
    #[error(
        "PDC delay capacity {0} exceeds the hard limit of {PDC_HARD_MAX_DELAY_SAMPLES} samples"
    )]
    DelayCapacityTooLarge(u32),
    #[error("node {node_id} has invalid runtime slot {runtime_slot}")]
    InvalidRuntimeSlot {
        node_id: GraphPdcNodeId,
        runtime_slot: usize,
    },
    #[error("mixer node identity zero is reserved")]
    ZeroNodeId,
    #[error("node identifier {0} occurs more than once")]
    DuplicateNode(GraphPdcNodeId),
    #[error("runtime slot {0} occurs more than once")]
    DuplicateRuntimeSlot(usize),
    #[error("topological order has {actual} nodes; expected {expected}")]
    TopologicalNodeCount { expected: usize, actual: usize },
    #[error("topological order contains unknown node {0}")]
    UnknownTopologicalNode(GraphPdcNodeId),
    #[error("topological order contains node {0} more than once")]
    DuplicateTopologicalNode(GraphPdcNodeId),
    #[error("master node {0} is absent")]
    UnknownMasterNode(GraphPdcNodeId),
    #[error("main-input route identity zero is reserved")]
    ZeroMainInputId,
    #[error("main-input route {route_id} references unknown node {node_id}")]
    UnknownMainInputNode {
        route_id: GraphPdcRouteId,
        node_id: GraphPdcNodeId,
    },
    #[error("main-input route identifier {0} occurs more than once")]
    DuplicateMainInput(GraphPdcRouteId),
    #[error("main-input route {route_id} has invalid runtime slot {runtime_slot}")]
    InvalidMainInputRuntimeSlot {
        route_id: GraphPdcRouteId,
        runtime_slot: usize,
    },
    #[error("main-input runtime slot {0} occurs more than once")]
    DuplicateMainInputRuntimeSlot(usize),
    #[error("main-input route {route_id} violates topological order")]
    MainInputNotTopological { route_id: GraphPdcRouteId },
    #[error("generator endpoint identity zero is reserved")]
    ZeroGeneratorEndpoint,
    #[error(
        "generator endpoint {endpoint_id} channel {channel_id} targets unknown node {destination_id}"
    )]
    UnknownGeneratorDestination {
        endpoint_id: u64,
        channel_id: u32,
        destination_id: GraphPdcNodeId,
    },
    #[error("generator endpoint {endpoint_id} channel {channel_id} occurs more than once")]
    DuplicateGenerator { endpoint_id: u64, channel_id: u32 },
}

impl GraphPdcPlan {
    /// Builds directly from a validated mixer graph without allocating an
    /// intermediate graph description. Stage latency is indexed by callback
    /// runtime slot; unused slots are ignored.
    pub fn build_for_mixer_graph(
        graph: &CompiledMixerGraph,
        stage_latency_by_runtime_slot: &[u64; PDC_GRAPH_MAX_NODES],
        generators: &[GraphPdcGenerator],
        maximum_delay_samples: u32,
    ) -> Result<Self, GraphPdcPlanError> {
        let mut nodes = [GraphPdcNode {
            node_id: 0,
            runtime_slot: 0,
            stage_latency_samples: 0,
        }; PDC_GRAPH_MAX_NODES];
        for (index, compiled) in graph.nodes().iter().copied().enumerate() {
            let runtime_slot = usize::from(compiled.runtime_slot);
            nodes[index] = GraphPdcNode {
                node_id: compiled.id,
                runtime_slot,
                stage_latency_samples: stage_latency_by_runtime_slot[runtime_slot],
            };
        }

        let mut topological_order = [0; PDC_GRAPH_MAX_NODES];
        for (position, dense) in graph.topological_order().iter().copied().enumerate() {
            topological_order[position] = graph.nodes()[usize::from(dense)].id;
        }

        let mut main_inputs = [GraphPdcMainInput {
            route_id: 0,
            runtime_slot: 0,
            source_id: 0,
            destination_id: 0,
            tap: MixerRouteTap::PostFader,
        }; PDC_GRAPH_MAX_MAIN_INPUTS];
        for (index, compiled) in graph.routes().iter().copied().enumerate() {
            main_inputs[index] = GraphPdcMainInput {
                route_id: compiled.id,
                runtime_slot: usize::from(compiled.runtime_slot),
                source_id: compiled.source_id,
                destination_id: compiled.destination_id,
                tap: compiled.tap,
            };
        }

        let node_count = graph.nodes().len();
        let main_input_count = graph.routes().len();
        let master_node_id = graph.nodes()[usize::from(graph.master_dense_index())].id;
        let mut plan = Self::build(
            GraphPdcInput {
                nodes: &nodes[..node_count],
                topological_order: &topological_order[..node_count],
                main_inputs: &main_inputs[..main_input_count],
                generators,
                master_node_id,
            },
            maximum_delay_samples,
        )?;
        plan.graph_fingerprint = Some(graph.fingerprint());
        Ok(plan)
    }

    pub fn build(
        input: GraphPdcInput<'_>,
        maximum_delay_samples: u32,
    ) -> Result<Self, GraphPdcPlanError> {
        Self::validate_input(input, maximum_delay_samples)?;

        let mut plan = Self {
            maximum_delay_samples,
            graph_fingerprint: None,
            master_node_id: input.master_node_id,
            master_output_latency_samples: 0,
            nodes_by_runtime_slot: [None; PDC_GRAPH_MAX_NODES],
            topological_runtime_slots: [0; PDC_GRAPH_MAX_NODES],
            node_count: input.nodes.len(),
            main_inputs: [None; PDC_GRAPH_MAX_MAIN_INPUTS],
            main_input_count: input.main_inputs.len(),
            generators: [None; PDC_MAX_GENERATORS],
            generator_count: input.generators.len(),
            diagnostics: GraphPdcDiagnostics::default(),
        };

        for (topological_index, node_id) in input.topological_order.iter().copied().enumerate() {
            let node = *input
                .nodes
                .iter()
                .find(|node| node.node_id == node_id)
                .expect("validated topological node");
            plan.topological_runtime_slots[topological_index] = node.runtime_slot;

            let mut join_latency_samples = 0_u64;
            for generator in input
                .generators
                .iter()
                .filter(|generator| generator.destination_id == node_id)
            {
                join_latency_samples = join_latency_samples.max(generator.latency_samples);
            }
            for edge in input
                .main_inputs
                .iter()
                .filter(|edge| edge.destination_id == node_id)
            {
                let source_slot = Self::input_runtime_slot(input.nodes, edge.source_id)
                    .expect("validated main-input source");
                let source = plan.nodes_by_runtime_slot[source_slot]
                    .expect("validated topological source precedes destination");
                join_latency_samples =
                    join_latency_samples.max(Self::source_latency_for_tap(source, edge.tap));
            }

            let raw_source_delay =
                CompensationDelay::new(join_latency_samples, maximum_delay_samples);
            plan.diagnostics.clamped_raw_sources += u16::from(raw_source_delay.is_clamped());

            for (generator_index, generator) in input.generators.iter().copied().enumerate() {
                if generator.destination_id != node_id {
                    continue;
                }
                let delay = CompensationDelay::new(
                    join_latency_samples.saturating_sub(generator.latency_samples),
                    maximum_delay_samples,
                );
                plan.diagnostics.clamped_generators += u16::from(delay.is_clamped());
                plan.generators[generator_index] = Some(GraphPdcGeneratorCompensation {
                    endpoint_id: generator.endpoint_id,
                    channel_id: generator.channel_id,
                    destination_id: generator.destination_id,
                    path_latency_samples: generator.latency_samples,
                    destination_join_latency_samples: join_latency_samples,
                    delay,
                });
            }

            for edge in input.main_inputs.iter().copied() {
                if edge.destination_id != node_id {
                    continue;
                }
                let source_slot = Self::input_runtime_slot(input.nodes, edge.source_id)
                    .expect("validated main-input source");
                let source = plan.nodes_by_runtime_slot[source_slot]
                    .expect("validated topological source precedes destination");
                let source_latency_samples = Self::source_latency_for_tap(source, edge.tap);
                let delay = CompensationDelay::new(
                    join_latency_samples.saturating_sub(source_latency_samples),
                    maximum_delay_samples,
                );
                plan.diagnostics.clamped_main_inputs += u16::from(delay.is_clamped());
                plan.main_inputs[edge.runtime_slot] = Some(GraphPdcMainInputCompensation {
                    route_id: edge.route_id,
                    runtime_slot: edge.runtime_slot,
                    source_id: edge.source_id,
                    destination_id: edge.destination_id,
                    tap: edge.tap,
                    source_latency_samples,
                    destination_join_latency_samples: join_latency_samples,
                    delay,
                });
            }

            let (output_latency_samples, output_latency_overflowed) =
                match join_latency_samples.checked_add(node.stage_latency_samples) {
                    Some(output) => (output, false),
                    None => {
                        plan.diagnostics.arithmetic_overflows =
                            plan.diagnostics.arithmetic_overflows.saturating_add(1);
                        (u64::MAX, true)
                    }
                };
            plan.nodes_by_runtime_slot[node.runtime_slot] = Some(GraphPdcNodeCompensation {
                node_id,
                runtime_slot: node.runtime_slot,
                stage_latency_samples: node.stage_latency_samples,
                join_latency_samples,
                raw_source_delay,
                output_latency_samples,
                output_latency_overflowed,
            });
        }

        plan.master_output_latency_samples = plan
            .node(input.master_node_id)
            .expect("validated master node")
            .output_latency_samples;
        Ok(plan)
    }

    fn validate_input(
        input: GraphPdcInput<'_>,
        maximum_delay_samples: u32,
    ) -> Result<(), GraphPdcPlanError> {
        if input.nodes.len() > PDC_GRAPH_MAX_NODES {
            return Err(GraphPdcPlanError::TooManyNodes(input.nodes.len()));
        }
        if input.main_inputs.len() > PDC_GRAPH_MAX_MAIN_INPUTS {
            return Err(GraphPdcPlanError::TooManyMainInputs(
                input.main_inputs.len(),
            ));
        }
        if input.generators.len() > PDC_MAX_GENERATORS {
            return Err(GraphPdcPlanError::TooManyGenerators(input.generators.len()));
        }
        if maximum_delay_samples > PDC_HARD_MAX_DELAY_SAMPLES {
            return Err(GraphPdcPlanError::DelayCapacityTooLarge(
                maximum_delay_samples,
            ));
        }
        if input.topological_order.len() != input.nodes.len() {
            return Err(GraphPdcPlanError::TopologicalNodeCount {
                expected: input.nodes.len(),
                actual: input.topological_order.len(),
            });
        }

        for (index, node) in input.nodes.iter().enumerate() {
            if node.node_id == 0 {
                return Err(GraphPdcPlanError::ZeroNodeId);
            }
            if node.runtime_slot >= PDC_GRAPH_MAX_NODES {
                return Err(GraphPdcPlanError::InvalidRuntimeSlot {
                    node_id: node.node_id,
                    runtime_slot: node.runtime_slot,
                });
            }
            if input.nodes[..index]
                .iter()
                .any(|previous| previous.node_id == node.node_id)
            {
                return Err(GraphPdcPlanError::DuplicateNode(node.node_id));
            }
            if input.nodes[..index]
                .iter()
                .any(|previous| previous.runtime_slot == node.runtime_slot)
            {
                return Err(GraphPdcPlanError::DuplicateRuntimeSlot(node.runtime_slot));
            }
        }
        if !input
            .nodes
            .iter()
            .any(|node| node.node_id == input.master_node_id)
        {
            return Err(GraphPdcPlanError::UnknownMasterNode(input.master_node_id));
        }

        let mut topological_positions = [usize::MAX; PDC_GRAPH_MAX_NODES];
        for (position, node_id) in input.topological_order.iter().copied().enumerate() {
            let runtime_slot = Self::input_runtime_slot(input.nodes, node_id)
                .ok_or(GraphPdcPlanError::UnknownTopologicalNode(node_id))?;
            if topological_positions[runtime_slot] != usize::MAX {
                return Err(GraphPdcPlanError::DuplicateTopologicalNode(node_id));
            }
            topological_positions[runtime_slot] = position;
        }

        for (index, edge) in input.main_inputs.iter().enumerate() {
            if edge.route_id == 0 {
                return Err(GraphPdcPlanError::ZeroMainInputId);
            }
            if edge.runtime_slot >= PDC_GRAPH_MAX_MAIN_INPUTS {
                return Err(GraphPdcPlanError::InvalidMainInputRuntimeSlot {
                    route_id: edge.route_id,
                    runtime_slot: edge.runtime_slot,
                });
            }
            if input.main_inputs[..index]
                .iter()
                .any(|previous| previous.route_id == edge.route_id)
            {
                return Err(GraphPdcPlanError::DuplicateMainInput(edge.route_id));
            }
            if input.main_inputs[..index]
                .iter()
                .any(|previous| previous.runtime_slot == edge.runtime_slot)
            {
                return Err(GraphPdcPlanError::DuplicateMainInputRuntimeSlot(
                    edge.runtime_slot,
                ));
            }
            let source_slot = Self::input_runtime_slot(input.nodes, edge.source_id).ok_or(
                GraphPdcPlanError::UnknownMainInputNode {
                    route_id: edge.route_id,
                    node_id: edge.source_id,
                },
            )?;
            let destination_slot = Self::input_runtime_slot(input.nodes, edge.destination_id)
                .ok_or(GraphPdcPlanError::UnknownMainInputNode {
                    route_id: edge.route_id,
                    node_id: edge.destination_id,
                })?;
            if topological_positions[source_slot] >= topological_positions[destination_slot] {
                return Err(GraphPdcPlanError::MainInputNotTopological {
                    route_id: edge.route_id,
                });
            }
        }
        for (index, generator) in input.generators.iter().enumerate() {
            if generator.endpoint_id == 0 {
                return Err(GraphPdcPlanError::ZeroGeneratorEndpoint);
            }
            if Self::input_runtime_slot(input.nodes, generator.destination_id).is_none() {
                return Err(GraphPdcPlanError::UnknownGeneratorDestination {
                    endpoint_id: generator.endpoint_id,
                    channel_id: generator.channel_id,
                    destination_id: generator.destination_id,
                });
            }
            if input.generators[..index].iter().any(|previous| {
                previous.endpoint_id == generator.endpoint_id
                    && previous.channel_id == generator.channel_id
            }) {
                return Err(GraphPdcPlanError::DuplicateGenerator {
                    endpoint_id: generator.endpoint_id,
                    channel_id: generator.channel_id,
                });
            }
        }
        Ok(())
    }

    fn input_runtime_slot(nodes: &[GraphPdcNode], node_id: GraphPdcNodeId) -> Option<usize> {
        nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| node.runtime_slot)
    }

    fn source_latency_for_tap(source: GraphPdcNodeCompensation, tap: MixerRouteTap) -> u64 {
        match tap {
            MixerRouteTap::PreEffects => source.join_latency_samples,
            MixerRouteTap::PostEffects | MixerRouteTap::PostFader => source.output_latency_samples,
        }
    }

    pub fn maximum_delay_samples(&self) -> u32 {
        self.maximum_delay_samples
    }

    pub fn graph_fingerprint(&self) -> Option<u64> {
        self.graph_fingerprint
    }

    pub fn master_node_id(&self) -> GraphPdcNodeId {
        self.master_node_id
    }

    pub fn master_output_latency_samples(&self) -> u64 {
        self.master_output_latency_samples
    }

    pub fn diagnostics(&self) -> GraphPdcDiagnostics {
        self.diagnostics
    }

    pub fn has_clamped_delays(&self) -> bool {
        self.diagnostics.has_clamped_delays()
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    pub fn node(&self, node_id: GraphPdcNodeId) -> Option<&GraphPdcNodeCompensation> {
        self.nodes_by_runtime_slot
            .iter()
            .flatten()
            .find(|node| node.node_id == node_id)
    }

    pub fn node_for_runtime_slot(&self, runtime_slot: usize) -> Option<&GraphPdcNodeCompensation> {
        self.nodes_by_runtime_slot.get(runtime_slot)?.as_ref()
    }

    pub fn raw_source_delay_for_runtime_slot(
        &self,
        runtime_slot: usize,
    ) -> Option<CompensationDelay> {
        self.node_for_runtime_slot(runtime_slot)
            .map(|node| node.raw_source_delay)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &GraphPdcNodeCompensation> {
        self.topological_runtime_slots[..self.node_count]
            .iter()
            .filter_map(|runtime_slot| self.nodes_by_runtime_slot[*runtime_slot].as_ref())
    }

    pub fn main_inputs(&self) -> impl Iterator<Item = &GraphPdcMainInputCompensation> {
        self.main_inputs.iter().flatten()
    }

    pub fn main_input_count(&self) -> usize {
        self.main_input_count
    }

    pub fn main_input(&self, route_id: GraphPdcRouteId) -> Option<&GraphPdcMainInputCompensation> {
        self.main_inputs()
            .find(|main_input| main_input.route_id == route_id)
    }

    pub fn main_input_for_runtime_slot(
        &self,
        runtime_slot: usize,
    ) -> Option<&GraphPdcMainInputCompensation> {
        self.main_inputs.get(runtime_slot)?.as_ref()
    }

    pub fn generators(&self) -> impl Iterator<Item = &GraphPdcGeneratorCompensation> {
        self.generators[..self.generator_count].iter().flatten()
    }

    pub fn generator_count(&self) -> usize {
        self.generator_count
    }

    pub fn generator(
        &self,
        endpoint_id: u64,
        channel_id: u32,
    ) -> Option<&GraphPdcGeneratorCompensation> {
        self.generators().find(|generator| {
            generator.endpoint_id == endpoint_id && generator.channel_id == channel_id
        })
    }

    /// Enumerates every independently compensated main-domain path in stable
    /// raw/topology, generator-input, then route-slot order.
    pub fn paths(&self) -> impl Iterator<Item = GraphPdcPathCompensation> + '_ {
        let raw_sources = self.nodes().map(|node| GraphPdcPathCompensation {
            identity: GraphPdcPathIdentity::RawSource {
                node_id: node.node_id,
            },
            arrival_latency_samples: 0,
            join_latency_samples: node.join_latency_samples,
            delay: node.raw_source_delay,
        });
        let generators = self.generators().map(|generator| GraphPdcPathCompensation {
            identity: GraphPdcPathIdentity::Generator {
                endpoint_id: generator.endpoint_id,
                channel_id: generator.channel_id,
                destination_id: generator.destination_id,
            },
            arrival_latency_samples: generator.path_latency_samples,
            join_latency_samples: generator.destination_join_latency_samples,
            delay: generator.delay,
        });
        let main_inputs = self
            .main_inputs()
            .map(|main_input| GraphPdcPathCompensation {
                identity: GraphPdcPathIdentity::MainInput {
                    route_id: main_input.route_id,
                    source_id: main_input.source_id,
                    destination_id: main_input.destination_id,
                    tap: main_input.tap,
                },
                arrival_latency_samples: main_input.source_latency_samples,
                join_latency_samples: main_input.destination_join_latency_samples,
                delay: main_input.delay,
            });
        raw_sources.chain(generators).chain(main_inputs)
    }

    pub fn path_count(&self) -> usize {
        self.node_count + self.generator_count + self.main_input_count
    }
}

/// Preallocated stereo delay with virtual-zero reset and click-reducing tap changes.
///
/// Construction is control-thread work. `process_sample`, `request_delay`, and
/// `reset` perform no allocation, locking, I/O, or buffer clearing.
#[derive(Debug)]
pub struct StereoDelayLine {
    samples: Box<[[f32; 2]]>,
    maximum_delay_samples: u32,
    write_index: usize,
    frames_since_reset: u64,
    current_delay: u32,
    target_delay: u32,
    fade_total: u32,
    fade_remaining: u32,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum StereoDelayError {
    #[error("delay capacity {0} exceeds the hard limit of {PDC_HARD_MAX_DELAY_SAMPLES} samples")]
    CapacityTooLarge(u32),
    #[error("unable to allocate a stereo PDC line for {0} samples")]
    AllocationFailed(usize),
    #[error("requested delay {requested} exceeds this line's capacity {maximum}")]
    DelayOutOfRange { requested: u32, maximum: u32 },
}

/// Sparse, control-thread-prepared route delay storage installed with one
/// compiled mixer-graph revision.
///
/// The callback-visible metadata is fixed-size, while backing sample storage
/// exists only for active main-input route slots. Construction and destruction
/// are control-thread operations; target requests, reset, and sample processing
/// allocate nothing and acquire no locks.
#[derive(Debug)]
pub struct PreparedMixerGraphDelayBank {
    graph_fingerprint: u64,
    maximum_delay_samples: u32,
    route_ids: [Option<GraphPdcRouteId>; PDC_GRAPH_MAX_MAIN_INPUTS],
    lines: [Option<StereoDelayLine>; PDC_GRAPH_MAX_MAIN_INPUTS],
    route_count: usize,
    allocated_samples: u64,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PreparedMixerGraphDelayBankError {
    #[error(
        "mixer graph delay capacity {0} exceeds the hard limit of {PDC_HARD_MAX_DELAY_SAMPLES} samples"
    )]
    DelayCapacityTooLarge(u32),
    #[error("mixer route {route_id} has reserved zero identity")]
    ZeroRouteId { route_id: GraphPdcRouteId },
    #[error("mixer route {route_id} has invalid runtime slot {runtime_slot}")]
    InvalidRouteRuntimeSlot {
        route_id: GraphPdcRouteId,
        runtime_slot: usize,
    },
    #[error("mixer route runtime slot {0} occurs more than once")]
    DuplicateRouteRuntimeSlot(usize),
    #[error(
        "unable to allocate {sample_frames} stereo delay frames for mixer route {route_id} at runtime slot {runtime_slot}"
    )]
    AllocationFailed {
        route_id: GraphPdcRouteId,
        runtime_slot: usize,
        sample_frames: usize,
    },
    #[error("PDC plan graph fingerprint {actual} does not match delay bank fingerprint {expected}")]
    GraphFingerprintMismatch { expected: u64, actual: u64 },
    #[error("PDC plan is missing mixer route {route_id} at runtime slot {runtime_slot}")]
    MissingPlanRoute {
        route_id: GraphPdcRouteId,
        runtime_slot: usize,
    },
    #[error("PDC plan contains unexpected mixer route {route_id} at runtime slot {runtime_slot}")]
    UnexpectedPlanRoute {
        route_id: GraphPdcRouteId,
        runtime_slot: usize,
    },
    #[error(
        "PDC plan mixer route {actual_route_id} does not match route {expected_route_id} at runtime slot {runtime_slot}"
    )]
    RouteIdentityMismatch {
        runtime_slot: usize,
        expected_route_id: GraphPdcRouteId,
        actual_route_id: GraphPdcRouteId,
    },
    #[error(
        "PDC plan requests {requested} samples for mixer route {route_id} at runtime slot {runtime_slot}; bank capacity is {maximum}"
    )]
    PlanDelayOutOfRange {
        route_id: GraphPdcRouteId,
        runtime_slot: usize,
        requested: u32,
        maximum: u32,
    },
}

impl PreparedMixerGraphDelayBank {
    pub fn new(
        graph: &CompiledMixerGraph,
        maximum_delay_samples: u32,
    ) -> Result<Self, PreparedMixerGraphDelayBankError> {
        if maximum_delay_samples > PDC_HARD_MAX_DELAY_SAMPLES {
            return Err(PreparedMixerGraphDelayBankError::DelayCapacityTooLarge(
                maximum_delay_samples,
            ));
        }
        let mut bank = Self {
            graph_fingerprint: graph.fingerprint(),
            maximum_delay_samples,
            route_ids: [None; PDC_GRAPH_MAX_MAIN_INPUTS],
            lines: std::array::from_fn(|_| None),
            route_count: 0,
            allocated_samples: 0,
        };
        let sample_frames = maximum_delay_samples as usize + 1;
        let scalar_samples_per_route = (u64::from(maximum_delay_samples) + 1).saturating_mul(2);
        for route in graph.routes().iter().copied() {
            let runtime_slot = usize::from(route.runtime_slot);
            if route.id == 0 {
                return Err(PreparedMixerGraphDelayBankError::ZeroRouteId { route_id: route.id });
            }
            if runtime_slot >= PDC_GRAPH_MAX_MAIN_INPUTS {
                return Err(PreparedMixerGraphDelayBankError::InvalidRouteRuntimeSlot {
                    route_id: route.id,
                    runtime_slot,
                });
            }
            if bank.route_ids[runtime_slot].is_some() {
                return Err(PreparedMixerGraphDelayBankError::DuplicateRouteRuntimeSlot(
                    runtime_slot,
                ));
            }
            let line = match StereoDelayLine::new(maximum_delay_samples) {
                Ok(line) => line,
                Err(StereoDelayError::AllocationFailed(_)) => {
                    return Err(PreparedMixerGraphDelayBankError::AllocationFailed {
                        route_id: route.id,
                        runtime_slot,
                        sample_frames,
                    });
                }
                Err(StereoDelayError::CapacityTooLarge(capacity)) => {
                    return Err(PreparedMixerGraphDelayBankError::DelayCapacityTooLarge(
                        capacity,
                    ));
                }
                Err(StereoDelayError::DelayOutOfRange { .. }) => {
                    unreachable!("constructing a delay line never requests a tap")
                }
            };
            bank.route_ids[runtime_slot] = Some(route.id);
            bank.lines[runtime_slot] = Some(line);
            bank.route_count += 1;
            bank.allocated_samples = bank
                .allocated_samples
                .saturating_add(scalar_samples_per_route);
        }
        Ok(bank)
    }

    pub fn graph_fingerprint(&self) -> u64 {
        self.graph_fingerprint
    }

    pub fn maximum_delay_samples(&self) -> u32 {
        self.maximum_delay_samples
    }

    pub fn route_count(&self) -> usize {
        self.route_count
    }

    /// Number of allocated scalar `f32` samples, counting left and right
    /// channels separately.
    pub fn allocated_samples(&self) -> u64 {
        self.allocated_samples
    }

    pub fn route_id_for_runtime_slot(&self, runtime_slot: usize) -> Option<GraphPdcRouteId> {
        self.route_ids.get(runtime_slot).copied().flatten()
    }

    pub fn current_delay_samples(&self, runtime_slot: usize) -> Option<u32> {
        self.lines
            .get(runtime_slot)?
            .as_ref()
            .map(StereoDelayLine::current_delay_samples)
    }

    pub fn target_delay_samples(&self, runtime_slot: usize) -> Option<u32> {
        self.lines
            .get(runtime_slot)?
            .as_ref()
            .map(StereoDelayLine::target_delay_samples)
    }

    /// Validates every route and target without mutating a delay line.
    pub fn preflight_plan(
        &self,
        plan: &GraphPdcPlan,
    ) -> Result<(), PreparedMixerGraphDelayBankError> {
        if let Some(actual) = plan.graph_fingerprint()
            && actual != self.graph_fingerprint
        {
            return Err(PreparedMixerGraphDelayBankError::GraphFingerprintMismatch {
                expected: self.graph_fingerprint,
                actual,
            });
        }
        for runtime_slot in 0..PDC_GRAPH_MAX_MAIN_INPUTS {
            let expected_route_id = self.route_ids[runtime_slot];
            let target = plan.main_input_for_runtime_slot(runtime_slot);
            match (expected_route_id, target) {
                (Some(route_id), None) => {
                    return Err(PreparedMixerGraphDelayBankError::MissingPlanRoute {
                        route_id,
                        runtime_slot,
                    });
                }
                (None, Some(target)) => {
                    return Err(PreparedMixerGraphDelayBankError::UnexpectedPlanRoute {
                        route_id: target.route_id,
                        runtime_slot,
                    });
                }
                (Some(expected_route_id), Some(target)) if expected_route_id != target.route_id => {
                    return Err(PreparedMixerGraphDelayBankError::RouteIdentityMismatch {
                        runtime_slot,
                        expected_route_id,
                        actual_route_id: target.route_id,
                    });
                }
                (Some(route_id), Some(target))
                    if target.delay.applied_samples() > self.maximum_delay_samples =>
                {
                    return Err(PreparedMixerGraphDelayBankError::PlanDelayOutOfRange {
                        route_id,
                        runtime_slot,
                        requested: target.delay.applied_samples(),
                        maximum: self.maximum_delay_samples,
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Atomically validates all targets before changing any candidate tap.
    /// No sample history is advanced by this operation.
    pub fn request_plan(
        &mut self,
        plan: &GraphPdcPlan,
        crossfade_frames: u32,
    ) -> Result<(), PreparedMixerGraphDelayBankError> {
        self.preflight_plan(plan)?;
        for target in plan.main_inputs() {
            let runtime_slot = target.runtime_slot;
            let line = self.lines[runtime_slot]
                .as_mut()
                .expect("preflight proved the route slot exists");
            match line.request_delay(target.delay.applied_samples(), crossfade_frames) {
                Ok(()) => {}
                Err(StereoDelayError::DelayOutOfRange { requested, maximum }) => {
                    return Err(PreparedMixerGraphDelayBankError::PlanDelayOutOfRange {
                        route_id: target.route_id,
                        runtime_slot,
                        requested,
                        maximum,
                    });
                }
                Err(
                    StereoDelayError::CapacityTooLarge(_) | StereoDelayError::AllocationFailed(_),
                ) => unreachable!("requesting a prepared delay tap never allocates"),
            }
        }
        Ok(())
    }

    /// Starts a new callback generation without clearing backing allocations.
    /// Configured targets and graph identity are retained.
    pub fn reset(&mut self) {
        for line in self.lines.iter_mut().flatten() {
            line.reset();
        }
    }

    /// Allocation-free route processing. `None` is a fail-closed indication
    /// that the installed graph and delay bank disagree about the route slot.
    pub fn process_sample(&mut self, runtime_slot: usize, input: [f32; 2]) -> Option<[f32; 2]> {
        self.lines
            .get_mut(runtime_slot)?
            .as_mut()
            .map(|line| line.process_sample(input))
    }
}

/// Preallocated scalar delay for sample-accurate control streams.
///
/// Audio and the control value applied to it must cross the same time domain.
/// This line mirrors [`StereoDelayLine`] without forcing callers to duplicate a
/// scalar into two audio channels. Construction is control-thread work;
/// [`Self::process_sample`], [`Self::request_delay`], and [`Self::reset`] are
/// bounded and allocation-free.
#[derive(Debug)]
pub struct ControlDelayLine {
    samples: Box<[f32]>,
    maximum_delay_samples: u32,
    write_index: usize,
    frames_since_reset: u64,
    initial_value: f32,
    current_delay: u32,
    target_delay: u32,
    fade_total: u32,
    fade_remaining: u32,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ControlDelayError {
    #[error("delay capacity {0} exceeds the hard limit of {PDC_HARD_MAX_DELAY_SAMPLES} samples")]
    CapacityTooLarge(u32),
    #[error("unable to allocate a control PDC line for {0} samples")]
    AllocationFailed(usize),
    #[error("requested delay {requested} exceeds this line's capacity {maximum}")]
    DelayOutOfRange { requested: u32, maximum: u32 },
    #[error("control delay values must be finite")]
    NonFiniteValue,
}

/// Transactional history for control values sampled at exact Q128 delay phases.
///
/// Plug-in parameters are delivered only at Q128 target boundaries, but the value must come from
/// the exact source frame `target - delay`; it must not be quantized to another Q128 boundary.
/// For a fixed delay every such source frame has the same phase modulo 128, so this ring stores
/// only that one phase sample per quantum. Construction allocates on the control thread; reset,
/// block queries and commit are bounded, allocation-free callback operations.
#[derive(Debug)]
pub struct Q128ControlHistory {
    phase_samples: Box<[f32]>,
    maximum_delay_samples: u32,
    epoch: u64,
    initial_value: f32,
    delay_samples: u32,
    source_phase: u32,
    next_frame: u64,
    committed_phase_samples: u64,
    write_index: usize,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum Q128ControlHistoryError {
    #[error(
        "Q128 control history capacity {0} exceeds the hard limit of {PDC_HARD_MAX_DELAY_SAMPLES} samples"
    )]
    CapacityTooLarge(u32),
    #[error("unable to allocate a Q128 control history with {0} phase samples")]
    AllocationFailed(usize),
    #[error("Q128 control history epochs must be nonzero")]
    EpochZero,
    #[error("Q128 control history has not been reset to an epoch")]
    NotReset,
    #[error("requested delay {requested} exceeds this history's capacity {maximum}")]
    DelayOutOfRange { requested: u32, maximum: u32 },
    #[error("Q128 control values must be finite")]
    NonFiniteInitialValue,
    #[error("Q128 control block for epoch {actual} does not match active epoch {expected}")]
    EpochMismatch { expected: u64, actual: u64 },
    #[error("Q128 control blocks must contain at least one frame")]
    EmptyBlock,
    #[error("Q128 control block frame range overflows u64")]
    FrameRangeOverflow,
    #[error("Q128 control block starts at frame {actual}, expected {expected}")]
    FrameDiscontinuity { expected: u64, actual: u64 },
    #[error("Q128 control block contains a non-finite value at sample offset {sample_offset}")]
    NonFiniteBlockValue { sample_offset: usize },
}

impl Q128ControlHistory {
    /// Allocate enough phase samples for every delay through `maximum_delay_samples`.
    pub fn new(maximum_delay_samples: u32) -> Result<Self, Q128ControlHistoryError> {
        if maximum_delay_samples > PDC_HARD_MAX_DELAY_SAMPLES {
            return Err(Q128ControlHistoryError::CapacityTooLarge(
                maximum_delay_samples,
            ));
        }
        let phase_sample_count = maximum_delay_samples
            .div_ceil(Q128_CONTROL_QUANTUM_FRAMES as u32)
            .max(1) as usize;
        let mut phase_samples = Vec::new();
        phase_samples
            .try_reserve_exact(phase_sample_count)
            .map_err(|_| Q128ControlHistoryError::AllocationFailed(phase_sample_count))?;
        phase_samples.resize(phase_sample_count, 0.0);
        Ok(Self {
            phase_samples: phase_samples.into_boxed_slice(),
            maximum_delay_samples,
            epoch: 0,
            initial_value: 0.0,
            delay_samples: 0,
            source_phase: 0,
            next_frame: 0,
            committed_phase_samples: 0,
            write_index: 0,
        })
    }

    #[must_use]
    pub const fn maximum_delay_samples(&self) -> u32 {
        self.maximum_delay_samples
    }

    /// Number of scalar values in the preallocated ring.
    #[must_use]
    pub fn phase_sample_capacity(&self) -> usize {
        self.phase_samples.len()
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    #[must_use]
    pub const fn delay_samples(&self) -> u32 {
        self.delay_samples
    }

    /// Exact source-frame phase retained for this delay, in `0..128`.
    #[must_use]
    pub const fn source_phase(&self) -> u32 {
        self.source_phase
    }

    #[must_use]
    pub const fn next_frame(&self) -> u64 {
        self.next_frame
    }

    /// Start an activation or loop epoch without clearing the preallocated history.
    pub fn reset(
        &mut self,
        epoch: u64,
        initial_value: f32,
        delay_samples: u32,
    ) -> Result<(), Q128ControlHistoryError> {
        if epoch == 0 {
            return Err(Q128ControlHistoryError::EpochZero);
        }
        if !initial_value.is_finite() {
            return Err(Q128ControlHistoryError::NonFiniteInitialValue);
        }
        if delay_samples > self.maximum_delay_samples {
            return Err(Q128ControlHistoryError::DelayOutOfRange {
                requested: delay_samples,
                maximum: self.maximum_delay_samples,
            });
        }

        self.epoch = epoch;
        self.initial_value = initial_value;
        self.delay_samples = delay_samples;
        self.source_phase = (Q128_CONTROL_QUANTUM_FRAMES as u32 - delay_samples % 128) % 128;
        self.next_frame = 0;
        self.committed_phase_samples = 0;
        self.write_index = 0;
        Ok(())
    }

    /// Validate a continuous candidate callback without mutating committed history.
    ///
    /// `per_sample_values` is the undelayed, sample-accurate automation stream for the half-open
    /// interval beginning at `start_frame`. Dropping or aborting the returned transaction leaves
    /// all history metadata and samples unchanged.
    pub fn begin_block<'history, 'values>(
        &'history mut self,
        epoch: u64,
        start_frame: u64,
        per_sample_values: &'values [f32],
    ) -> Result<Q128ControlBlockTxn<'history, 'values>, Q128ControlHistoryError> {
        if epoch == 0 {
            return Err(Q128ControlHistoryError::EpochZero);
        }
        if self.epoch == 0 {
            return Err(Q128ControlHistoryError::NotReset);
        }
        if epoch != self.epoch {
            return Err(Q128ControlHistoryError::EpochMismatch {
                expected: self.epoch,
                actual: epoch,
            });
        }
        if per_sample_values.is_empty() {
            return Err(Q128ControlHistoryError::EmptyBlock);
        }
        if start_frame != self.next_frame {
            return Err(Q128ControlHistoryError::FrameDiscontinuity {
                expected: self.next_frame,
                actual: start_frame,
            });
        }
        let frame_count = u64::try_from(per_sample_values.len())
            .map_err(|_| Q128ControlHistoryError::FrameRangeOverflow)?;
        let end_frame = start_frame
            .checked_add(frame_count)
            .ok_or(Q128ControlHistoryError::FrameRangeOverflow)?;
        if let Some(sample_offset) = per_sample_values
            .iter()
            .position(|value| !value.is_finite())
        {
            return Err(Q128ControlHistoryError::NonFiniteBlockValue { sample_offset });
        }

        let first_boundary_frame =
            first_frame_at_phase(start_frame, 0).filter(|first| *first < end_frame);
        let boundary_count = phase_frame_count(first_boundary_frame, end_frame)?;
        let first_source_phase_frame =
            first_frame_at_phase(start_frame, self.source_phase).filter(|first| *first < end_frame);
        let source_phase_count = phase_frame_count(first_source_phase_frame, end_frame)?;

        Ok(Q128ControlBlockTxn {
            history: self,
            start_frame,
            end_frame,
            per_sample_values,
            first_boundary_frame,
            boundary_count,
            first_source_phase_frame,
            source_phase_count,
        })
    }

    fn committed_value_at(&self, source_frame: u64) -> f32 {
        let phase = u64::from(self.source_phase);
        if source_frame % Q128_CONTROL_QUANTUM_FRAMES != phase {
            return self.initial_value;
        }
        let phase_index = (source_frame - phase) / Q128_CONTROL_QUANTUM_FRAMES;
        if phase_index >= self.committed_phase_samples {
            return self.initial_value;
        }
        let age = self.committed_phase_samples - phase_index;
        if age > self.phase_samples.len() as u64 {
            return self.initial_value;
        }
        let ring_index = (phase_index % self.phase_samples.len() as u64) as usize;
        self.phase_samples[ring_index]
    }

    fn push_phase_sample(&mut self, value: f32) {
        self.phase_samples[self.write_index] = value;
        self.write_index += 1;
        if self.write_index == self.phase_samples.len() {
            self.write_index = 0;
        }
        self.committed_phase_samples += 1;
    }
}

/// Read-only candidate block which becomes committed only through [`Self::commit_block`].
#[derive(Debug)]
pub struct Q128ControlBlockTxn<'history, 'values> {
    history: &'history mut Q128ControlHistory,
    start_frame: u64,
    end_frame: u64,
    per_sample_values: &'values [f32],
    first_boundary_frame: Option<u64>,
    boundary_count: usize,
    first_source_phase_frame: Option<u64>,
    source_phase_count: usize,
}

impl Q128ControlBlockTxn<'_, '_> {
    #[must_use]
    pub const fn start_frame(&self) -> u64 {
        self.start_frame
    }

    #[must_use]
    pub const fn end_frame(&self) -> u64 {
        self.end_frame
    }

    #[must_use]
    pub const fn boundary_count(&self) -> usize {
        self.boundary_count
    }

    #[must_use]
    pub fn boundary_frame(&self, index: usize) -> Option<u64> {
        if index >= self.boundary_count {
            return None;
        }
        self.first_boundary_frame?
            .checked_add(index as u64 * Q128_CONTROL_QUANTUM_FRAMES)
    }

    #[must_use]
    pub fn input_value(&self, index: usize) -> Option<f32> {
        let frame = self.boundary_frame(index)?;
        let offset = usize::try_from(frame - self.start_frame).ok()?;
        self.per_sample_values.get(offset).copied()
    }

    /// Value of the raw automation stream at the exact frame `T - delay`.
    #[must_use]
    pub fn delayed_value(&self, index: usize) -> Option<f32> {
        let target_frame = self.boundary_frame(index)?;
        let delay = u64::from(self.history.delay_samples);
        if target_frame < delay {
            return Some(self.history.initial_value);
        }
        let source_frame = target_frame - delay;
        if source_frame >= self.start_frame {
            let offset = usize::try_from(source_frame - self.start_frame).ok()?;
            return self.per_sample_values.get(offset).copied();
        }
        Some(self.history.committed_value_at(source_frame))
    }

    /// Explicit no-op abort; dropping the transaction has identical semantics.
    pub fn abort(self) {}

    /// Commit the preflighted block. This operation is infallible and allocation-free.
    pub fn commit_block(self) {
        let Self {
            history,
            start_frame,
            end_frame,
            per_sample_values,
            first_source_phase_frame,
            source_phase_count,
            ..
        } = self;
        if let Some(first_frame) = first_source_phase_frame {
            let first_offset = (first_frame - start_frame) as usize;
            for phase_index in 0..source_phase_count {
                let offset = first_offset + phase_index * Q128_CONTROL_QUANTUM_FRAMES as usize;
                history.push_phase_sample(per_sample_values[offset]);
            }
        }
        history.next_frame = end_frame;
    }
}

fn first_frame_at_phase(frame: u64, phase: u32) -> Option<u64> {
    debug_assert!(u64::from(phase) < Q128_CONTROL_QUANTUM_FRAMES);
    let current_phase = frame % Q128_CONTROL_QUANTUM_FRAMES;
    let advance = (u64::from(phase) + Q128_CONTROL_QUANTUM_FRAMES - current_phase)
        % Q128_CONTROL_QUANTUM_FRAMES;
    frame.checked_add(advance)
}

fn phase_frame_count(
    first_frame: Option<u64>,
    end_frame: u64,
) -> Result<usize, Q128ControlHistoryError> {
    let Some(first_frame) = first_frame else {
        return Ok(0);
    };
    debug_assert!(first_frame < end_frame);
    let count = (end_frame - 1 - first_frame) / Q128_CONTROL_QUANTUM_FRAMES + 1;
    usize::try_from(count).map_err(|_| Q128ControlHistoryError::FrameRangeOverflow)
}

impl ControlDelayLine {
    pub fn new(maximum_delay_samples: u32, initial_value: f32) -> Result<Self, ControlDelayError> {
        if maximum_delay_samples > PDC_HARD_MAX_DELAY_SAMPLES {
            return Err(ControlDelayError::CapacityTooLarge(maximum_delay_samples));
        }
        if !initial_value.is_finite() {
            return Err(ControlDelayError::NonFiniteValue);
        }
        let sample_count = maximum_delay_samples as usize + 1;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(sample_count)
            .map_err(|_| ControlDelayError::AllocationFailed(sample_count))?;
        samples.resize(sample_count, initial_value);
        Ok(Self {
            samples: samples.into_boxed_slice(),
            maximum_delay_samples,
            write_index: 0,
            frames_since_reset: 0,
            initial_value,
            current_delay: 0,
            target_delay: 0,
            fade_total: 0,
            fade_remaining: 0,
        })
    }

    pub fn maximum_delay_samples(&self) -> u32 {
        self.maximum_delay_samples
    }

    pub fn current_delay_samples(&self) -> u32 {
        self.current_delay
    }

    pub fn target_delay_samples(&self) -> u32 {
        self.target_delay
    }

    pub fn request_delay(
        &mut self,
        delay_samples: u32,
        crossfade_frames: u32,
    ) -> Result<(), ControlDelayError> {
        if delay_samples > self.maximum_delay_samples {
            return Err(ControlDelayError::DelayOutOfRange {
                requested: delay_samples,
                maximum: self.maximum_delay_samples,
            });
        }
        if delay_samples == self.target_delay {
            return Ok(());
        }
        if self.fade_remaining != 0 && self.fade_remaining.saturating_mul(2) <= self.fade_total {
            self.current_delay = self.target_delay;
        }
        self.target_delay = delay_samples;
        if crossfade_frames == 0 || self.current_delay == self.target_delay {
            self.current_delay = self.target_delay;
            self.fade_total = 0;
            self.fade_remaining = 0;
        } else {
            self.fade_total = crossfade_frames;
            self.fade_remaining = crossfade_frames;
        }
        Ok(())
    }

    /// Starts a new transport/PDC generation without clearing the backing
    /// allocation. Until enough new samples have arrived, delayed taps resolve
    /// to the chased value supplied here rather than leaking a prior epoch.
    pub fn reset(&mut self, initial_value: f32) -> Result<(), ControlDelayError> {
        if !initial_value.is_finite() {
            return Err(ControlDelayError::NonFiniteValue);
        }
        self.write_index = 0;
        self.frames_since_reset = 0;
        self.initial_value = initial_value;
        self.current_delay = self.target_delay;
        self.fade_total = 0;
        self.fade_remaining = 0;
        Ok(())
    }

    pub fn process_sample(&mut self, input: f32) -> f32 {
        let input = if input.is_finite() {
            input
        } else {
            self.initial_value
        };
        self.samples[self.write_index] = input;
        let current = self.read_tap(self.current_delay, input);
        let output = if self.fade_remaining == 0 {
            current
        } else {
            let target = self.read_tap(self.target_delay, input);
            let progressed = self.fade_total - self.fade_remaining + 1;
            let mix = progressed as f32 / self.fade_total as f32;
            let output = current + (target - current) * mix;
            self.fade_remaining -= 1;
            if self.fade_remaining == 0 {
                self.current_delay = self.target_delay;
                self.fade_total = 0;
            }
            output
        };
        self.write_index += 1;
        if self.write_index == self.samples.len() {
            self.write_index = 0;
        }
        self.frames_since_reset = self.frames_since_reset.saturating_add(1);
        output
    }

    fn read_tap(&self, delay_samples: u32, current_input: f32) -> f32 {
        if delay_samples == 0 {
            return current_input;
        }
        if self.frames_since_reset < u64::from(delay_samples) {
            return self.initial_value;
        }
        let delay = delay_samples as usize;
        let index = (self.write_index + self.samples.len() - delay) % self.samples.len();
        self.samples[index]
    }
}

impl StereoDelayLine {
    pub fn new(maximum_delay_samples: u32) -> Result<Self, StereoDelayError> {
        if maximum_delay_samples > PDC_HARD_MAX_DELAY_SAMPLES {
            return Err(StereoDelayError::CapacityTooLarge(maximum_delay_samples));
        }
        // The extra cell prevents the current write from overwriting the oldest
        // readable sample when delay == maximum_delay_samples.
        let sample_count = maximum_delay_samples as usize + 1;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(sample_count)
            .map_err(|_| StereoDelayError::AllocationFailed(sample_count))?;
        samples.resize(sample_count, [0.0; 2]);
        Ok(Self {
            samples: samples.into_boxed_slice(),
            maximum_delay_samples,
            write_index: 0,
            frames_since_reset: 0,
            current_delay: 0,
            target_delay: 0,
            fade_total: 0,
            fade_remaining: 0,
        })
    }

    pub fn maximum_delay_samples(&self) -> u32 {
        self.maximum_delay_samples
    }

    pub fn current_delay_samples(&self) -> u32 {
        self.current_delay
    }

    pub fn target_delay_samples(&self) -> u32 {
        self.target_delay
    }

    pub fn request_delay(
        &mut self,
        delay_samples: u32,
        crossfade_frames: u32,
    ) -> Result<(), StereoDelayError> {
        if delay_samples > self.maximum_delay_samples {
            return Err(StereoDelayError::DelayOutOfRange {
                requested: delay_samples,
                maximum: self.maximum_delay_samples,
            });
        }
        if delay_samples == self.target_delay {
            return Ok(());
        }

        if self.fade_remaining != 0 {
            // A fixed two-tap line cannot preserve an in-flight blend while
            // starting a third tap. Continue from the currently dominant tap.
            if self.fade_remaining.saturating_mul(2) <= self.fade_total {
                self.current_delay = self.target_delay;
            }
        }
        self.target_delay = delay_samples;
        if crossfade_frames == 0 || self.current_delay == self.target_delay {
            self.current_delay = self.target_delay;
            self.fade_total = 0;
            self.fade_remaining = 0;
        } else {
            self.fade_total = crossfade_frames;
            self.fade_remaining = crossfade_frames;
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        self.write_index = 0;
        self.frames_since_reset = 0;
        self.current_delay = self.target_delay;
        self.fade_total = 0;
        self.fade_remaining = 0;
    }

    pub fn process_sample(&mut self, input: [f32; 2]) -> [f32; 2] {
        self.samples[self.write_index] = input;
        let current = self.read_tap(self.current_delay, input);
        let output = if self.fade_remaining == 0 {
            current
        } else {
            let target = self.read_tap(self.target_delay, input);
            let progressed = self.fade_total - self.fade_remaining + 1;
            let mix = progressed as f32 / self.fade_total as f32;
            let output = [
                current[0] + (target[0] - current[0]) * mix,
                current[1] + (target[1] - current[1]) * mix,
            ];
            self.fade_remaining -= 1;
            if self.fade_remaining == 0 {
                self.current_delay = self.target_delay;
                self.fade_total = 0;
            }
            output
        };

        self.write_index += 1;
        if self.write_index == self.samples.len() {
            self.write_index = 0;
        }
        self.frames_since_reset = self.frames_since_reset.saturating_add(1);
        output
    }

    fn read_tap(&self, delay_samples: u32, current_input: [f32; 2]) -> [f32; 2] {
        if delay_samples == 0 {
            return current_input;
        }
        if self.frames_since_reset < u64::from(delay_samples) {
            return [0.0; 2];
        }
        let delay = delay_samples as usize;
        let index = (self.write_index + self.samples.len() - delay) % self.samples.len();
        self.samples[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_aligns_raw_generator_and_master_paths() {
        let mut inserts = [0; PDC_TRACK_COUNT];
        inserts[1] = 100;
        inserts[2] = 20;
        let generators = [
            GeneratorPathLatency {
                endpoint_id: 10,
                channel_id: 7,
                mixer_track: 1,
                latency_samples: 50,
            },
            GeneratorPathLatency {
                endpoint_id: 11,
                channel_id: 8,
                mixer_track: 2,
                latency_samples: 200,
            },
        ];
        let plan = PdcPlan::build(inserts, 30, &generators, 1_000).unwrap();
        assert_eq!(plan.reference_latency_samples(), 220);
        assert_eq!(plan.output_latency_samples(), 250);
        assert_eq!(plan.raw_track_delay(0).unwrap().applied_samples(), 220);
        assert_eq!(plan.raw_track_delay(1).unwrap().applied_samples(), 120);
        assert_eq!(plan.raw_track_delay(2).unwrap().applied_samples(), 200);
        let generators = plan.generator_delays().collect::<Vec<_>>();
        assert_eq!(generators[0].path_latency_samples, 150);
        assert_eq!(generators[0].delay.applied_samples(), 70);
        assert_eq!(generators[1].path_latency_samples, 220);
        assert_eq!(generators[1].delay.applied_samples(), 0);
        assert!(!plan.has_clamped_delays());
    }

    #[test]
    fn direct_master_generator_excludes_master_endpoint_from_relative_math() {
        let mut inserts = [0; PDC_TRACK_COUNT];
        inserts[0] = 9_999;
        inserts[3] = 64;
        let generator = GeneratorPathLatency {
            endpoint_id: 1,
            channel_id: 2,
            mixer_track: 0,
            latency_samples: 16,
        };
        let plan = PdcPlan::build(inserts, 400, &[generator], 1_000).unwrap();
        assert_eq!(plan.reference_latency_samples(), 64);
        assert_eq!(plan.output_latency_samples(), 464);
        assert_eq!(
            plan.generator_delays()
                .next()
                .unwrap()
                .delay
                .applied_samples(),
            48
        );
    }

    #[test]
    fn capacity_clamps_are_explicit_instead_of_wrapping() {
        let mut inserts = [0; PDC_TRACK_COUNT];
        inserts[4] = 10_000;
        let plan = PdcPlan::build(inserts, u32::MAX, &[], 128).unwrap();
        assert_eq!(plan.reference_latency_samples(), 10_000);
        assert_eq!(plan.output_latency_samples(), 10_000 + u64::from(u32::MAX));
        let direct = plan.raw_track_delay(0).unwrap();
        assert_eq!(direct.requested_samples(), 10_000);
        assert_eq!(direct.applied_samples(), 128);
        assert!(direct.is_clamped());
        assert!(plan.has_clamped_delays());
    }

    #[test]
    fn plan_rejects_bad_routes_counts_and_capacity() {
        let bad = GeneratorPathLatency {
            endpoint_id: 1,
            channel_id: 99,
            mixer_track: PDC_TRACK_COUNT,
            latency_samples: 0,
        };
        assert!(matches!(
            PdcPlan::build([0; PDC_TRACK_COUNT], 0, &[bad], 1),
            Err(PdcPlanError::InvalidMixerTrack { channel_id: 99, .. })
        ));
        let too_many = vec![bad; PDC_MAX_GENERATORS + 1];
        assert_eq!(
            PdcPlan::build([0; PDC_TRACK_COUNT], 0, &too_many, 1).unwrap_err(),
            PdcPlanError::TooManyGenerators(PDC_MAX_GENERATORS + 1)
        );
        assert_eq!(
            PdcPlan::build([0; PDC_TRACK_COUNT], 0, &[], PDC_HARD_MAX_DELAY_SAMPLES + 1)
                .unwrap_err(),
            PdcPlanError::DelayCapacityTooLarge(PDC_HARD_MAX_DELAY_SAMPLES + 1)
        );
    }

    fn graph_input<'a>(
        nodes: &'a [GraphPdcNode],
        topological_order: &'a [GraphPdcNodeId],
        main_inputs: &'a [GraphPdcMainInput],
        generators: &'a [GraphPdcGenerator],
        master_node_id: GraphPdcNodeId,
    ) -> GraphPdcInput<'a> {
        GraphPdcInput {
            nodes,
            topological_order,
            main_inputs,
            generators,
            master_node_id,
        }
    }

    #[test]
    fn graph_plan_aligns_a_diamond_at_each_join() {
        let nodes = [
            GraphPdcNode {
                node_id: 1,
                runtime_slot: 3,
                stage_latency_samples: 10,
            },
            GraphPdcNode {
                node_id: 2,
                runtime_slot: 0,
                stage_latency_samples: 30,
            },
            GraphPdcNode {
                node_id: 3,
                runtime_slot: 2,
                stage_latency_samples: 5,
            },
            GraphPdcNode {
                node_id: 99,
                runtime_slot: 1,
                stage_latency_samples: 2,
            },
        ];
        let edges = [
            GraphPdcMainInput {
                route_id: 10,
                runtime_slot: 0,
                source_id: 1,
                destination_id: 2,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 11,
                runtime_slot: 1,
                source_id: 1,
                destination_id: 3,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 12,
                runtime_slot: 2,
                source_id: 2,
                destination_id: 99,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 13,
                runtime_slot: 3,
                source_id: 3,
                destination_id: 99,
                tap: MixerRouteTap::PostFader,
            },
        ];
        let plan = GraphPdcPlan::build(graph_input(&nodes, &[1, 2, 3, 99], &edges, &[], 99), 1_000)
            .unwrap();

        assert_eq!(plan.node(2).unwrap().join_latency_samples, 10);
        assert_eq!(plan.node(2).unwrap().output_latency_samples, 40);
        assert_eq!(plan.node(3).unwrap().output_latency_samples, 15);
        assert_eq!(plan.node(99).unwrap().join_latency_samples, 40);
        assert_eq!(plan.main_input(12).unwrap().delay.applied_samples(), 0);
        assert_eq!(plan.main_input(13).unwrap().delay.applied_samples(), 25);
        assert_eq!(
            plan.node(99).unwrap().raw_source_delay.applied_samples(),
            40
        );
        assert_eq!(plan.master_output_latency_samples(), 42);
        assert_eq!(
            plan.nodes().map(|node| node.node_id).collect::<Vec<_>>(),
            [1, 2, 3, 99]
        );
    }

    #[test]
    fn graph_plan_fanout_compensates_each_destination_independently() {
        let nodes = [
            GraphPdcNode {
                node_id: 1,
                runtime_slot: 0,
                stage_latency_samples: 10,
            },
            GraphPdcNode {
                node_id: 2,
                runtime_slot: 1,
                stage_latency_samples: 3,
            },
            GraphPdcNode {
                node_id: 3,
                runtime_slot: 2,
                stage_latency_samples: 7,
            },
            GraphPdcNode {
                node_id: 9,
                runtime_slot: 3,
                stage_latency_samples: 0,
            },
        ];
        let edges = [
            GraphPdcMainInput {
                route_id: 20,
                runtime_slot: 0,
                source_id: 1,
                destination_id: 2,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 21,
                runtime_slot: 1,
                source_id: 1,
                destination_id: 3,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 22,
                runtime_slot: 2,
                source_id: 2,
                destination_id: 9,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 23,
                runtime_slot: 3,
                source_id: 3,
                destination_id: 9,
                tap: MixerRouteTap::PostFader,
            },
        ];
        let generators = [
            GraphPdcGenerator {
                endpoint_id: 100,
                channel_id: 4,
                destination_id: 2,
                latency_samples: 30,
            },
            GraphPdcGenerator {
                endpoint_id: 101,
                channel_id: 5,
                destination_id: 3,
                latency_samples: 50,
            },
        ];
        let plan = GraphPdcPlan::build(
            graph_input(&nodes, &[1, 2, 3, 9], &edges, &generators, 9),
            1_000,
        )
        .unwrap();

        assert_eq!(plan.main_input(20).unwrap().delay.applied_samples(), 20);
        assert_eq!(plan.main_input(21).unwrap().delay.applied_samples(), 40);
        assert_eq!(plan.node(2).unwrap().output_latency_samples, 33);
        assert_eq!(plan.node(3).unwrap().output_latency_samples, 57);
        assert_eq!(plan.main_input(22).unwrap().delay.applied_samples(), 24);
        assert_eq!(plan.main_input(23).unwrap().delay.applied_samples(), 0);
        assert_eq!(plan.master_output_latency_samples(), 57);
    }

    #[test]
    fn graph_plan_saturates_arithmetic_and_reports_every_clamped_path_kind() {
        let nodes = [
            GraphPdcNode {
                node_id: 1,
                runtime_slot: 0,
                stage_latency_samples: u64::MAX,
            },
            GraphPdcNode {
                node_id: 2,
                runtime_slot: 1,
                stage_latency_samples: 10,
            },
            GraphPdcNode {
                node_id: 9,
                runtime_slot: 2,
                stage_latency_samples: 1,
            },
        ];
        let edges = [
            GraphPdcMainInput {
                route_id: 30,
                runtime_slot: 0,
                source_id: 1,
                destination_id: 9,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 31,
                runtime_slot: 1,
                source_id: 2,
                destination_id: 9,
                tap: MixerRouteTap::PostFader,
            },
        ];
        let generators = [GraphPdcGenerator {
            endpoint_id: 200,
            channel_id: 8,
            destination_id: 9,
            latency_samples: 20,
        }];
        let plan =
            GraphPdcPlan::build(graph_input(&nodes, &[1, 2, 9], &edges, &generators, 9), 128)
                .unwrap();

        assert_eq!(plan.master_output_latency_samples(), u64::MAX);
        assert!(plan.node(9).unwrap().output_latency_overflowed);
        assert!(!plan.node(1).unwrap().output_latency_overflowed);
        assert_eq!(plan.diagnostics().arithmetic_overflows, 1);
        assert_eq!(plan.diagnostics().clamped_raw_sources, 1);
        assert_eq!(plan.diagnostics().clamped_generators, 1);
        assert_eq!(plan.diagnostics().clamped_main_inputs, 1);
        assert!(plan.node(9).unwrap().raw_source_delay.is_clamped());
        assert!(plan.main_input(31).unwrap().delay.is_clamped());
        assert!(plan.generator(200, 8).unwrap().delay.is_clamped());
    }

    #[test]
    fn graph_plan_distinguishes_pre_and_post_effects_taps() {
        let nodes = [
            GraphPdcNode {
                node_id: 1,
                runtime_slot: 1,
                stage_latency_samples: 100,
            },
            GraphPdcNode {
                node_id: 9,
                runtime_slot: 0,
                stage_latency_samples: 5,
            },
        ];
        let edges = [
            GraphPdcMainInput {
                route_id: 40,
                runtime_slot: 7,
                source_id: 1,
                destination_id: 9,
                tap: MixerRouteTap::PreEffects,
            },
            GraphPdcMainInput {
                route_id: 41,
                runtime_slot: 3,
                source_id: 1,
                destination_id: 9,
                tap: MixerRouteTap::PostEffects,
            },
            GraphPdcMainInput {
                route_id: 42,
                runtime_slot: 5,
                source_id: 1,
                destination_id: 9,
                tap: MixerRouteTap::PostFader,
            },
        ];
        let plan =
            GraphPdcPlan::build(graph_input(&nodes, &[1, 9], &edges, &[], 9), 1_000).unwrap();

        assert_eq!(plan.main_input(40).unwrap().source_latency_samples, 0);
        assert_eq!(plan.main_input(40).unwrap().delay.applied_samples(), 100);
        assert_eq!(plan.main_input(41).unwrap().source_latency_samples, 100);
        assert_eq!(plan.main_input(41).unwrap().delay.applied_samples(), 0);
        assert_eq!(plan.main_input(42).unwrap().source_latency_samples, 100);
        assert_eq!(plan.main_input_for_runtime_slot(3).unwrap().route_id, 41);
        assert!(plan.main_input_for_runtime_slot(4).is_none());
        assert_eq!(plan.master_output_latency_samples(), 105);
    }

    #[test]
    fn graph_plan_handles_multiple_serial_joins_and_arbitrary_generators() {
        let nodes = [
            GraphPdcNode {
                node_id: 10,
                runtime_slot: 4,
                stage_latency_samples: 5,
            },
            GraphPdcNode {
                node_id: 20,
                runtime_slot: 3,
                stage_latency_samples: 20,
            },
            GraphPdcNode {
                node_id: 30,
                runtime_slot: 2,
                stage_latency_samples: 7,
            },
            GraphPdcNode {
                node_id: 40,
                runtime_slot: 1,
                stage_latency_samples: 50,
            },
            GraphPdcNode {
                node_id: 999_999_999,
                runtime_slot: 0,
                stage_latency_samples: 3,
            },
        ];
        let edges = [
            GraphPdcMainInput {
                route_id: 50,
                runtime_slot: 4,
                source_id: 10,
                destination_id: 30,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 51,
                runtime_slot: 2,
                source_id: 20,
                destination_id: 30,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 52,
                runtime_slot: 8,
                source_id: 30,
                destination_id: 999_999_999,
                tap: MixerRouteTap::PostFader,
            },
            GraphPdcMainInput {
                route_id: 53,
                runtime_slot: 1,
                source_id: 40,
                destination_id: 999_999_999,
                tap: MixerRouteTap::PostFader,
            },
        ];
        let generators = [
            GraphPdcGenerator {
                endpoint_id: u64::MAX - 1,
                channel_id: 70,
                destination_id: 30,
                latency_samples: 30,
            },
            GraphPdcGenerator {
                endpoint_id: u64::MAX,
                channel_id: 71,
                destination_id: 999_999_999,
                latency_samples: 60,
            },
        ];
        let plan = GraphPdcPlan::build(
            graph_input(
                &nodes,
                &[10, 20, 30, 40, 999_999_999],
                &edges,
                &generators,
                999_999_999,
            ),
            1_000,
        )
        .unwrap();

        assert_eq!(plan.node(30).unwrap().join_latency_samples, 30);
        assert_eq!(plan.main_input(50).unwrap().delay.applied_samples(), 25);
        assert_eq!(plan.main_input(51).unwrap().delay.applied_samples(), 10);
        assert_eq!(plan.node(30).unwrap().output_latency_samples, 37);
        assert_eq!(plan.node(999_999_999).unwrap().join_latency_samples, 60);
        assert_eq!(plan.main_input(52).unwrap().delay.applied_samples(), 23);
        assert_eq!(plan.main_input(53).unwrap().delay.applied_samples(), 10);
        assert_eq!(plan.master_output_latency_samples(), 63);
        assert_eq!(plan.path_count(), 11);
        let identities = plan.paths().map(|path| path.identity).collect::<Vec<_>>();
        assert_eq!(identities.len(), 11);
        assert_eq!(
            identities[0],
            GraphPdcPathIdentity::RawSource { node_id: 10 }
        );
        assert_eq!(
            identities[5],
            GraphPdcPathIdentity::Generator {
                endpoint_id: u64::MAX - 1,
                channel_id: 70,
                destination_id: 30,
            }
        );
        assert!(matches!(
            identities[7],
            GraphPdcPathIdentity::MainInput { route_id: 53, .. }
        ));
    }

    #[test]
    fn graph_plan_rejects_reserved_identities_bad_dag_order_and_capacity() {
        let zero_node = [GraphPdcNode {
            node_id: 0,
            runtime_slot: 0,
            stage_latency_samples: 0,
        }];
        assert_eq!(
            GraphPdcPlan::build(graph_input(&zero_node, &[0], &[], &[], 0), 1).unwrap_err(),
            GraphPdcPlanError::ZeroNodeId
        );

        let nodes = [
            GraphPdcNode {
                node_id: 1,
                runtime_slot: 1,
                stage_latency_samples: 0,
            },
            GraphPdcNode {
                node_id: 9,
                runtime_slot: 0,
                stage_latency_samples: 0,
            },
        ];
        let zero_route = [GraphPdcMainInput {
            route_id: 0,
            runtime_slot: 0,
            source_id: 1,
            destination_id: 9,
            tap: MixerRouteTap::PostFader,
        }];
        assert_eq!(
            GraphPdcPlan::build(graph_input(&nodes, &[1, 9], &zero_route, &[], 9), 1,).unwrap_err(),
            GraphPdcPlanError::ZeroMainInputId
        );

        let zero_endpoint = [GraphPdcGenerator {
            endpoint_id: 0,
            channel_id: 1,
            destination_id: 9,
            latency_samples: 0,
        }];
        assert_eq!(
            GraphPdcPlan::build(graph_input(&nodes, &[1, 9], &[], &zero_endpoint, 9), 1,)
                .unwrap_err(),
            GraphPdcPlanError::ZeroGeneratorEndpoint
        );

        let backwards = [GraphPdcMainInput {
            route_id: 1,
            runtime_slot: 0,
            source_id: 1,
            destination_id: 9,
            tap: MixerRouteTap::PostFader,
        }];
        assert_eq!(
            GraphPdcPlan::build(graph_input(&nodes, &[9, 1], &backwards, &[], 9), 1,).unwrap_err(),
            GraphPdcPlanError::MainInputNotTopological { route_id: 1 }
        );
        let duplicate_slot = [
            backwards[0],
            GraphPdcMainInput {
                route_id: 2,
                ..backwards[0]
            },
        ];
        assert_eq!(
            GraphPdcPlan::build(graph_input(&nodes, &[1, 9], &duplicate_slot, &[], 9), 1,)
                .unwrap_err(),
            GraphPdcPlanError::DuplicateMainInputRuntimeSlot(0)
        );

        let too_many_nodes = vec![nodes[0]; PDC_GRAPH_MAX_NODES + 1];
        assert_eq!(
            GraphPdcPlan::build(graph_input(&too_many_nodes, &[], &[], &[], 9), 1,).unwrap_err(),
            GraphPdcPlanError::TooManyNodes(PDC_GRAPH_MAX_NODES + 1)
        );
        let too_many_edges = vec![backwards[0]; PDC_GRAPH_MAX_MAIN_INPUTS + 1];
        assert_eq!(
            GraphPdcPlan::build(graph_input(&nodes, &[1, 9], &too_many_edges, &[], 9), 1,)
                .unwrap_err(),
            GraphPdcPlanError::TooManyMainInputs(PDC_GRAPH_MAX_MAIN_INPUTS + 1)
        );
        let too_many_generators = vec![
            GraphPdcGenerator {
                endpoint_id: 1,
                channel_id: 1,
                destination_id: 9,
                latency_samples: 0,
            };
            PDC_MAX_GENERATORS + 1
        ];
        assert_eq!(
            GraphPdcPlan::build(
                graph_input(&nodes, &[1, 9], &[], &too_many_generators, 9),
                1,
            )
            .unwrap_err(),
            GraphPdcPlanError::TooManyGenerators(PDC_MAX_GENERATORS + 1)
        );
    }

    #[test]
    fn graph_plan_owns_only_fixed_storage_and_requires_no_drop() {
        assert!(!std::hint::black_box(std::mem::needs_drop::<GraphPdcPlan>()));
        assert!(!std::hint::black_box(std::mem::needs_drop::<
            GraphPdcPathCompensation,
        >()));
    }

    #[test]
    fn compiled_graph_adapter_preserves_slots_and_excludes_sidechains() {
        use crate::mixer_graph::{
            MASTER_MIXER_TRACK_ID, MixerGraphCompileError, MixerRouteDestination,
            compile_mixer_graph,
        };
        use crate::model::Project;

        let project = Project::blank();
        let graph = compile_mixer_graph(&project).unwrap();
        let mut stage_latencies = [0_u64; PDC_GRAPH_MAX_NODES];
        for (runtime_slot, latency) in stage_latencies.iter_mut().enumerate() {
            *latency = runtime_slot as u64;
        }
        stage_latencies[0] = 4;
        let generators = [GraphPdcGenerator {
            endpoint_id: 900,
            channel_id: 77,
            destination_id: 7,
            latency_samples: 100,
        }];
        let plan =
            GraphPdcPlan::build_for_mixer_graph(&graph, &stage_latencies, &generators, 1_000)
                .unwrap();

        assert_eq!(plan.master_node_id(), MASTER_MIXER_TRACK_ID);
        assert_eq!(plan.node_for_runtime_slot(7).unwrap().node_id, 7);
        assert_eq!(plan.main_input_for_runtime_slot(0).unwrap().route_id, 1);
        assert_eq!(plan.main_input_for_runtime_slot(0).unwrap().source_id, 1);
        assert_eq!(plan.node(7).unwrap().join_latency_samples, 100);
        assert_eq!(plan.master_output_latency_samples(), 111);

        let mut sidechain_project = Project::blank();
        sidechain_project.mixer_routes[0].destination = MixerRouteDestination::PluginSidechain {
            mixer_track_id: MASTER_MIXER_TRACK_ID,
            slot: 0,
            input_bus: 1,
        };
        assert!(matches!(
            compile_mixer_graph(&sidechain_project),
            Err(MixerGraphCompileError::ActiveSidechainUnsupported { route_id: 1 })
        ));
    }

    fn compiled_graph_with_route_count(route_count: usize) -> CompiledMixerGraph {
        use crate::mixer_graph::{MixerRoute, MixerRouteDestination, compile_mixer_graph};
        use crate::model::Project;

        assert!(route_count <= PDC_GRAPH_MAX_MAIN_INPUTS);
        let mut project = Project::blank();
        project.mixer_routes.clear();
        if route_count == 0 {
            return compile_mixer_graph(&project).unwrap();
        }
        'routes: for source in 1_u8..PDC_GRAPH_MAX_NODES as u8 {
            for destination in (source + 1)..PDC_GRAPH_MAX_NODES as u8 {
                let runtime_slot = project.mixer_routes.len();
                project.mixer_routes.push(MixerRoute {
                    id: runtime_slot as u64 + 1,
                    runtime_slot: runtime_slot as u8,
                    source_mixer_track_id: u64::from(source),
                    destination: MixerRouteDestination::MainInput {
                        mixer_track_id: u64::from(destination),
                    },
                    tap: MixerRouteTap::PostFader,
                    gain: 1.0,
                    enabled: true,
                });
                if project.mixer_routes.len() == route_count {
                    break 'routes;
                }
            }
        }
        assert_eq!(project.mixer_routes.len(), route_count);
        compile_mixer_graph(&project).unwrap()
    }

    #[test]
    fn sparse_graph_delay_bank_allocates_only_active_routes() {
        let graph_31 = compiled_graph_with_route_count(31);
        let bank_31 = PreparedMixerGraphDelayBank::new(&graph_31, 3).unwrap();
        assert_eq!(bank_31.route_count(), 31);
        assert_eq!(bank_31.allocated_samples(), 31 * (3 + 1) * 2);
        assert_eq!(bank_31.route_id_for_runtime_slot(0), Some(1));
        assert_eq!(bank_31.route_id_for_runtime_slot(30), Some(31));
        assert_eq!(bank_31.route_id_for_runtime_slot(31), None);

        let graph_128 = compiled_graph_with_route_count(128);
        let bank_128 = PreparedMixerGraphDelayBank::new(&graph_128, 0).unwrap();
        assert_eq!(bank_128.route_count(), 128);
        assert_eq!(bank_128.allocated_samples(), 128 * 2);
        assert_eq!(bank_128.route_id_for_runtime_slot(127), Some(128));
        assert_eq!(
            PreparedMixerGraphDelayBank::new(&graph_31, PDC_HARD_MAX_DELAY_SAMPLES + 1,)
                .unwrap_err(),
            PreparedMixerGraphDelayBankError::DelayCapacityTooLarge(PDC_HARD_MAX_DELAY_SAMPLES + 1,)
        );
    }

    #[test]
    fn graph_delay_bank_preflight_is_transactional_and_validates_identity() {
        use crate::mixer_graph::compile_mixer_graph;
        use crate::model::Project;

        let project = Project::blank();
        let graph = compile_mixer_graph(&project).unwrap();
        let mut stage_latencies = [0_u64; PDC_GRAPH_MAX_NODES];
        for (slot, latency) in stage_latencies.iter_mut().enumerate() {
            *latency = slot as u64;
        }
        let plan = GraphPdcPlan::build_for_mixer_graph(&graph, &stage_latencies, &[], 64).unwrap();
        let mut bank = PreparedMixerGraphDelayBank::new(&graph, 64).unwrap();
        assert_eq!(bank.current_delay_samples(0), Some(0));
        assert_eq!(bank.target_delay_samples(0), Some(0));
        bank.preflight_plan(&plan).unwrap();
        assert_eq!(bank.current_delay_samples(0), Some(0));
        assert_eq!(bank.target_delay_samples(0), Some(0));
        bank.request_plan(&plan, 8).unwrap();
        assert_eq!(bank.current_delay_samples(0), Some(0));
        assert_eq!(bank.target_delay_samples(0), Some(30));

        let mut different_project = Project::blank();
        different_project.mixer_routes[0].id = 999;
        let different_graph = compile_mixer_graph(&different_project).unwrap();
        let different_plan =
            GraphPdcPlan::build_for_mixer_graph(&different_graph, &stage_latencies, &[], 64)
                .unwrap();
        assert!(matches!(
            bank.preflight_plan(&different_plan),
            Err(PreparedMixerGraphDelayBankError::GraphFingerprintMismatch {
                expected,
                actual,
            }) if expected == graph.fingerprint() && actual == different_graph.fingerprint()
        ));

        let too_small = PreparedMixerGraphDelayBank::new(&graph, 1).unwrap();
        assert_eq!(
            too_small.preflight_plan(&plan).unwrap_err(),
            PreparedMixerGraphDelayBankError::PlanDelayOutOfRange {
                route_id: 1,
                runtime_slot: 0,
                requested: 30,
                maximum: 1,
            }
        );

        let minimal_nodes = [
            GraphPdcNode {
                node_id: 1,
                runtime_slot: 1,
                stage_latency_samples: 0,
            },
            GraphPdcNode {
                node_id: 9,
                runtime_slot: 0,
                stage_latency_samples: 0,
            },
        ];
        let wrong_route = [GraphPdcMainInput {
            route_id: 999,
            runtime_slot: 0,
            source_id: 1,
            destination_id: 9,
            tap: MixerRouteTap::PostFader,
        }];
        let wrong_identity = GraphPdcPlan::build(
            graph_input(&minimal_nodes, &[1, 9], &wrong_route, &[], 9),
            64,
        )
        .unwrap();
        assert_eq!(
            bank.preflight_plan(&wrong_identity).unwrap_err(),
            PreparedMixerGraphDelayBankError::RouteIdentityMismatch {
                runtime_slot: 0,
                expected_route_id: 1,
                actual_route_id: 999,
            }
        );

        let missing =
            GraphPdcPlan::build(graph_input(&minimal_nodes, &[1, 9], &[], &[], 9), 64).unwrap();
        assert_eq!(
            bank.preflight_plan(&missing).unwrap_err(),
            PreparedMixerGraphDelayBankError::MissingPlanRoute {
                route_id: 1,
                runtime_slot: 0,
            }
        );

        let empty_graph = compiled_graph_with_route_count(0);
        let empty_bank = PreparedMixerGraphDelayBank::new(&empty_graph, 64).unwrap();
        assert_eq!(
            empty_bank.preflight_plan(&wrong_identity).unwrap_err(),
            PreparedMixerGraphDelayBankError::UnexpectedPlanRoute {
                route_id: 999,
                runtime_slot: 0,
            }
        );
    }

    #[test]
    fn graph_delay_bank_reset_retains_target_and_hides_old_history() {
        use crate::mixer_graph::{MASTER_MIXER_TRACK_ID, compile_mixer_graph};
        use crate::model::Project;

        let mut project = Project::blank();
        project.mixer_routes.truncate(1);
        let graph = compile_mixer_graph(&project).unwrap();
        let stages = [0_u64; PDC_GRAPH_MAX_NODES];
        let generators = [GraphPdcGenerator {
            endpoint_id: 1,
            channel_id: 1,
            destination_id: MASTER_MIXER_TRACK_ID,
            latency_samples: 2,
        }];
        let plan = GraphPdcPlan::build_for_mixer_graph(&graph, &stages, &generators, 4).unwrap();
        let mut bank = PreparedMixerGraphDelayBank::new(&graph, 4).unwrap();

        assert_eq!(bank.process_sample(0, [0.25, -0.5]), Some([0.25, -0.5]));
        assert_eq!(bank.process_sample(1, [1.0; 2]), None);
        bank.request_plan(&plan, 0).unwrap();
        assert_eq!(bank.target_delay_samples(0), Some(2));
        for _ in 0..8 {
            let _ = bank.process_sample(0, [1.0; 2]).unwrap();
        }
        bank.reset();
        assert_eq!(bank.current_delay_samples(0), Some(2));
        assert_eq!(bank.target_delay_samples(0), Some(2));
        assert_eq!(bank.process_sample(0, [0.0; 2]), Some([0.0; 2]));
        assert_eq!(bank.process_sample(0, [0.0; 2]), Some([0.0; 2]));
        assert_eq!(bank.process_sample(0, [0.0; 2]), Some([0.0; 2]));
    }

    #[test]
    fn stereo_delay_outputs_an_exact_sample_delay() {
        let mut line = StereoDelayLine::new(8).unwrap();
        line.request_delay(3, 0).unwrap();
        let input = [[1.0, -1.0], [2.0, -2.0], [3.0, -3.0], [4.0, -4.0]];
        let output = input.map(|sample| line.process_sample(sample));
        assert_eq!(output[0], [0.0; 2]);
        assert_eq!(output[1], [0.0; 2]);
        assert_eq!(output[2], [0.0; 2]);
        assert_eq!(output[3], [1.0, -1.0]);
    }

    fn render_aligned_impulses(
        total_path_delays: [u32; 3],
        callback_splits: &[usize],
    ) -> [f32; 32] {
        assert_eq!(callback_splits.iter().sum::<usize>(), 32);
        let mut lines = total_path_delays.map(|delay| {
            let mut line = StereoDelayLine::new(16).unwrap();
            line.request_delay(delay, 0).unwrap();
            line
        });
        let mut output = [0.0; 32];
        let mut frame = 0;
        for &callback_frames in callback_splits {
            for _ in 0..callback_frames {
                let input = if frame == 0 { [1.0, -1.0] } else { [0.0; 2] };
                output[frame] = lines
                    .iter_mut()
                    .map(|line| line.process_sample(input)[0])
                    .sum();
                frame += 1;
            }
        }
        output
    }

    #[test]
    fn graph_plan_impulse_alignment_is_independent_of_callback_splits() {
        let nodes = [
            GraphPdcNode {
                node_id: 1,
                runtime_slot: 1,
                stage_latency_samples: 7,
            },
            GraphPdcNode {
                node_id: 9,
                runtime_slot: 0,
                stage_latency_samples: 0,
            },
        ];
        let edges = [GraphPdcMainInput {
            route_id: 70,
            runtime_slot: 0,
            source_id: 1,
            destination_id: 9,
            tap: MixerRouteTap::PostFader,
        }];
        let generators = [GraphPdcGenerator {
            endpoint_id: 500,
            channel_id: 6,
            destination_id: 9,
            latency_samples: 3,
        }];
        let plan =
            GraphPdcPlan::build(graph_input(&nodes, &[1, 9], &edges, &generators, 9), 16).unwrap();
        let total_path_delays = [
            plan.node(9).unwrap().raw_source_delay.applied_samples(),
            u32::try_from(
                plan.generator(500, 6).unwrap().path_latency_samples
                    + u64::from(plan.generator(500, 6).unwrap().delay.applied_samples()),
            )
            .unwrap(),
            u32::try_from(
                plan.main_input(70).unwrap().source_latency_samples
                    + u64::from(plan.main_input(70).unwrap().delay.applied_samples()),
            )
            .unwrap(),
        ];
        assert_eq!(total_path_delays, [7, 7, 7]);

        let contiguous = render_aligned_impulses(total_path_delays, &[32]);
        let split = render_aligned_impulses(total_path_delays, &[1, 2, 5, 3, 8, 1, 12]);
        assert_eq!(contiguous, split);
        assert_eq!(contiguous[7], 3.0);
        assert_eq!(
            contiguous.iter().filter(|sample| **sample != 0.0).count(),
            1
        );
    }

    #[test]
    fn virtual_reset_never_leaks_samples_from_the_previous_epoch() {
        let mut line = StereoDelayLine::new(4).unwrap();
        line.request_delay(2, 0).unwrap();
        for _ in 0..8 {
            let _ = line.process_sample([1.0, 1.0]);
        }
        line.reset();
        assert_eq!(line.process_sample([0.0; 2]), [0.0; 2]);
        assert_eq!(line.process_sample([0.0; 2]), [0.0; 2]);
        assert_eq!(line.process_sample([0.0; 2]), [0.0; 2]);
    }

    #[test]
    fn tap_change_crossfades_and_finishes_on_the_new_delay() {
        let mut line = StereoDelayLine::new(8).unwrap();
        for _ in 0..8 {
            let _ = line.process_sample([1.0, -1.0]);
        }
        line.request_delay(4, 4).unwrap();
        for expected in [0.25, 0.5, 0.75, 1.0] {
            // Both taps contain the same constant signal, so the blend remains
            // exactly constant while still exercising fade state progression.
            assert_eq!(line.process_sample([1.0, -1.0]), [1.0, -1.0]);
            let progressed = 1.0 - line.fade_remaining as f32 / 4.0;
            assert!((progressed - expected).abs() < f32::EPSILON);
        }
        assert_eq!(line.current_delay_samples(), 4);
        assert_eq!(line.target_delay_samples(), 4);
    }

    #[test]
    fn delay_line_rejects_out_of_range_requests() {
        let mut line = StereoDelayLine::new(2).unwrap();
        assert_eq!(
            line.request_delay(3, 16).unwrap_err(),
            StereoDelayError::DelayOutOfRange {
                requested: 3,
                maximum: 2,
            }
        );
        assert_eq!(
            StereoDelayLine::new(PDC_HARD_MAX_DELAY_SAMPLES + 1).unwrap_err(),
            StereoDelayError::CapacityTooLarge(PDC_HARD_MAX_DELAY_SAMPLES + 1)
        );
    }

    #[test]
    fn control_delay_matches_the_audio_sample_domain() {
        let mut line = ControlDelayLine::new(8, 0.25).unwrap();
        line.request_delay(3, 0).unwrap();
        let output = [1.0, 2.0, 3.0, 4.0].map(|value| line.process_sample(value));
        assert_eq!(output, [0.25, 0.25, 0.25, 1.0]);
    }

    #[test]
    fn control_delay_reset_uses_chased_value_and_never_leaks_an_old_epoch() {
        let mut line = ControlDelayLine::new(4, 0.0).unwrap();
        line.request_delay(2, 0).unwrap();
        for _ in 0..8 {
            let _ = line.process_sample(1.0);
        }
        line.reset(0.4).unwrap();
        assert_eq!(line.process_sample(0.8), 0.4);
        assert_eq!(line.process_sample(0.9), 0.4);
        assert_eq!(line.process_sample(1.0), 0.8);
    }

    #[test]
    fn control_delay_crossfade_and_validation_are_deterministic() {
        let mut line = ControlDelayLine::new(4, 1.0).unwrap();
        for _ in 0..4 {
            assert_eq!(line.process_sample(1.0), 1.0);
        }
        line.request_delay(2, 2).unwrap();
        assert_eq!(line.process_sample(1.0), 1.0);
        assert_eq!(line.process_sample(1.0), 1.0);
        assert_eq!(line.current_delay_samples(), 2);
        assert_eq!(
            line.request_delay(5, 0).unwrap_err(),
            ControlDelayError::DelayOutOfRange {
                requested: 5,
                maximum: 4,
            }
        );
        assert_eq!(
            ControlDelayLine::new(1, f32::NAN).unwrap_err(),
            ControlDelayError::NonFiniteValue
        );
        assert_eq!(
            line.reset(f32::INFINITY).unwrap_err(),
            ControlDelayError::NonFiniteValue
        );
    }

    fn ramp(start_frame: u64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|offset| (start_frame + offset as u64) as f32)
            .collect()
    }

    #[test]
    fn q128_history_allocates_only_the_required_phase_samples() {
        assert_eq!(
            Q128ControlHistory::new(0).unwrap().phase_sample_capacity(),
            1
        );
        assert_eq!(
            Q128ControlHistory::new(1).unwrap().phase_sample_capacity(),
            1
        );
        assert_eq!(
            Q128ControlHistory::new(128)
                .unwrap()
                .phase_sample_capacity(),
            1
        );
        assert_eq!(
            Q128ControlHistory::new(129)
                .unwrap()
                .phase_sample_capacity(),
            2
        );
        assert_eq!(
            Q128ControlHistory::new(PDC_HARD_MAX_DELAY_SAMPLES)
                .unwrap()
                .phase_sample_capacity(),
            (PDC_HARD_MAX_DELAY_SAMPLES / 128) as usize
        );
        assert_eq!(
            Q128ControlHistory::new(PDC_HARD_MAX_DELAY_SAMPLES + 1).unwrap_err(),
            Q128ControlHistoryError::CapacityTooLarge(PDC_HARD_MAX_DELAY_SAMPLES + 1)
        );
    }

    #[test]
    fn q128_history_reset_validates_epoch_value_delay_and_phase() {
        let mut history = Q128ControlHistory::new(129).unwrap();
        assert_eq!(
            history.reset(0, 0.0, 0).unwrap_err(),
            Q128ControlHistoryError::EpochZero
        );
        assert_eq!(
            history.reset(1, f32::NAN, 0).unwrap_err(),
            Q128ControlHistoryError::NonFiniteInitialValue
        );
        assert_eq!(
            history.reset(1, 0.0, 130).unwrap_err(),
            Q128ControlHistoryError::DelayOutOfRange {
                requested: 130,
                maximum: 129,
            }
        );

        for (epoch, delay, phase) in [
            (1, 0, 0),
            (2, 1, 127),
            (3, 65, 63),
            (4, 128, 0),
            (5, 129, 127),
        ] {
            history.reset(epoch, 0.25, delay).unwrap();
            assert_eq!(history.epoch(), epoch);
            assert_eq!(history.delay_samples(), delay);
            assert_eq!(history.source_phase(), phase);
            assert_eq!(history.next_frame(), 0);
        }
    }

    #[test]
    fn q128_delay_zero_reads_the_current_per_sample_stream() {
        let mut history = Q128ControlHistory::new(256).unwrap();
        history.reset(7, -1.0, 0).unwrap();
        let values = ramp(0, 300);
        let block = history.begin_block(7, 0, &values).unwrap();

        assert_eq!(block.boundary_count(), 3);
        assert_eq!(block.boundary_frame(0), Some(0));
        assert_eq!(block.boundary_frame(1), Some(128));
        assert_eq!(block.boundary_frame(2), Some(256));
        assert_eq!(block.boundary_frame(3), None);
        assert_eq!(block.input_value(1), Some(128.0));
        assert_eq!(block.delayed_value(0), Some(0.0));
        assert_eq!(block.delayed_value(1), Some(128.0));
        assert_eq!(block.delayed_value(2), Some(256.0));
        block.commit_block();
        assert_eq!(history.next_frame(), 300);
    }

    #[test]
    fn q128_delay_65_reads_exact_frame_63_without_second_quantization() {
        let mut history = Q128ControlHistory::new(256).unwrap();
        history.reset(9, -1.0, 65).unwrap();
        let values = ramp(0, 200);
        let block = history.begin_block(9, 0, &values).unwrap();

        assert_eq!(block.boundary_count(), 2);
        assert_eq!(block.delayed_value(0), Some(-1.0));
        assert_eq!(block.boundary_frame(1), Some(128));
        assert_eq!(block.delayed_value(1), Some(63.0));
        assert_ne!(block.delayed_value(1), Some(0.0));
        block.commit_block();
    }

    #[test]
    fn q128_exact_delay_is_invariant_under_arbitrary_callback_splits() {
        let mut history = Q128ControlHistory::new(512).unwrap();
        history.reset(11, -5.0, 65).unwrap();
        let splits = [7, 56, 1, 63, 2, 19, 109, 3, 127, 5, 137];
        let mut start_frame = 0_u64;
        let mut observed = Vec::new();

        for frames in splits {
            let values = ramp(start_frame, frames);
            let block = history.begin_block(11, start_frame, &values).unwrap();
            for index in 0..block.boundary_count() {
                observed.push((
                    block.boundary_frame(index).unwrap(),
                    block.delayed_value(index).unwrap(),
                ));
            }
            block.commit_block();
            start_frame += frames as u64;
        }

        assert!(observed.len() >= 4);
        for (target_frame, value) in observed {
            let expected = if target_frame < 65 {
                -5.0
            } else {
                (target_frame - 65) as f32
            };
            assert_eq!(value, expected, "target frame {target_frame}");
        }
    }

    #[test]
    fn q128_abort_leaves_samples_and_continuity_uncommitted() {
        let mut history = Q128ControlHistory::new(256).unwrap();
        history.reset(13, -1.0, 65).unwrap();
        let mut rejected = vec![0.0; 128];
        rejected[63] = 11.0;
        history.begin_block(13, 0, &rejected).unwrap().abort();
        assert_eq!(history.next_frame(), 0);
        assert_eq!(history.committed_phase_samples, 0);

        let mut accepted = vec![0.0; 128];
        accepted[63] = 22.0;
        history
            .begin_block(13, 0, &accepted)
            .unwrap()
            .commit_block();
        let next = [0.0];
        let block = history.begin_block(13, 128, &next).unwrap();
        assert_eq!(block.boundary_frame(0), Some(128));
        assert_eq!(block.delayed_value(0), Some(22.0));
        block.commit_block();
    }

    #[test]
    fn q128_reset_changes_only_metadata_and_never_exposes_an_old_epoch() {
        let mut history = Q128ControlHistory::new(256).unwrap();
        history.reset(17, -1.0, 65).unwrap();
        let mut old_values = vec![0.0; 128];
        old_values[63] = 99.0;
        history
            .begin_block(17, 0, &old_values)
            .unwrap()
            .commit_block();
        let allocation = history.phase_samples.as_ptr();
        assert_eq!(history.phase_samples[0], 99.0);

        history.reset(18, 0.25, 129).unwrap();
        assert_eq!(history.phase_samples.as_ptr(), allocation);
        assert_eq!(history.phase_samples[0], 99.0);
        assert_eq!(history.committed_phase_samples, 0);
        let first = [0.0];
        let block = history.begin_block(18, 0, &first).unwrap();
        assert_eq!(block.delayed_value(0), Some(0.25));
        block.commit_block();

        let middle = vec![0.0; 128];
        let block = history.begin_block(18, 1, &middle).unwrap();
        assert_eq!(block.boundary_frame(0), Some(128));
        assert_eq!(block.delayed_value(0), Some(0.25));
        block.commit_block();

        let tail = vec![0.0; 128];
        let block = history.begin_block(18, 129, &tail).unwrap();
        assert_eq!(block.boundary_frame(0), Some(256));
        assert_eq!(block.delayed_value(0), Some(0.0));
        block.commit_block();
    }

    #[test]
    fn q128_block_validation_is_transactional_for_epoch_frames_and_values() {
        let mut history = Q128ControlHistory::new(128).unwrap();
        assert_eq!(
            history.begin_block(1, 0, &[0.0]).unwrap_err(),
            Q128ControlHistoryError::NotReset
        );
        history.reset(23, 0.0, 1).unwrap();
        assert_eq!(
            history.begin_block(0, 0, &[0.0]).unwrap_err(),
            Q128ControlHistoryError::EpochZero
        );
        assert_eq!(
            history.begin_block(24, 0, &[0.0]).unwrap_err(),
            Q128ControlHistoryError::EpochMismatch {
                expected: 23,
                actual: 24,
            }
        );
        assert_eq!(
            history.begin_block(23, 1, &[0.0]).unwrap_err(),
            Q128ControlHistoryError::FrameDiscontinuity {
                expected: 0,
                actual: 1,
            }
        );
        assert_eq!(
            history.begin_block(23, 0, &[]).unwrap_err(),
            Q128ControlHistoryError::EmptyBlock
        );
        assert_eq!(
            history.begin_block(23, 0, &[f32::INFINITY]).unwrap_err(),
            Q128ControlHistoryError::NonFiniteBlockValue { sample_offset: 0 }
        );
        assert_eq!(history.next_frame(), 0);

        history
            .begin_block(23, 0, &[1.0, 2.0])
            .unwrap()
            .commit_block();
        assert_eq!(history.next_frame(), 2);
        assert_eq!(
            history.begin_block(23, 3, &[3.0]).unwrap_err(),
            Q128ControlHistoryError::FrameDiscontinuity {
                expected: 2,
                actual: 3,
            }
        );
    }

    #[test]
    fn q128_maximum_delay_keeps_the_oldest_required_phase_until_query() {
        let mut history = Q128ControlHistory::new(PDC_HARD_MAX_DELAY_SAMPLES).unwrap();
        history.reset(29, -1.0, PDC_HARD_MAX_DELAY_SAMPLES).unwrap();
        assert_eq!(history.source_phase(), 0);
        assert_eq!(history.phase_sample_capacity(), 8_192);

        for quantum in 0_u64..8_192 {
            let values = [quantum as f32; 128];
            history
                .begin_block(29, quantum * 128, &values)
                .unwrap()
                .commit_block();
        }
        let target = [9_999.0];
        let block = history
            .begin_block(29, u64::from(PDC_HARD_MAX_DELAY_SAMPLES), &target)
            .unwrap();
        assert_eq!(block.boundary_frame(0), Some(1_048_576));
        assert_eq!(block.delayed_value(0), Some(0.0));
        block.commit_block();
    }

    #[test]
    fn q128_half_open_boundaries_are_not_duplicated_across_callbacks() {
        let mut history = Q128ControlHistory::new(128).unwrap();
        history.reset(31, 0.0, 0).unwrap();
        let first = vec![1.0; 128];
        let block = history.begin_block(31, 0, &first).unwrap();
        assert_eq!(block.boundary_count(), 1);
        assert_eq!(block.boundary_frame(0), Some(0));
        block.commit_block();

        let second = vec![2.0; 128];
        let block = history.begin_block(31, 128, &second).unwrap();
        assert_eq!(block.boundary_count(), 1);
        assert_eq!(block.boundary_frame(0), Some(128));
        assert_eq!(block.input_value(0), Some(2.0));
        block.commit_block();
    }
}
