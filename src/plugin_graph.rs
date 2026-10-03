//! Fixed-capacity plugin graph control plane and real-time snapshot.
//!
//! This module intentionally does not load or call VST code. Heap-backed plugin
//! descriptions live only in [`PluginGraphState`] on the control thread. The audio
//! callback owns [`RealtimePluginGraph`], whose state and commands are fixed-size
//! `Copy` values exchanged over bounded SPSC queues.

use std::{
    error::Error,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use rtrb::{Consumer, Producer, PushError, RingBuffer};
use serde::{Deserialize, Serialize};

pub const MIXER_INSERT_COUNT: usize = 32;
pub const PLUGIN_SLOTS_PER_INSERT: usize = 10;
pub const MAX_PLUGIN_INSTANCES: usize = MIXER_INSERT_COUNT * PLUGIN_SLOTS_PER_INSERT;
pub const DEFAULT_GRAPH_COMMAND_CAPACITY: usize = 256;
pub const DEFAULT_GRAPH_EVENT_CAPACITY: usize = 256;
pub const MAX_GRAPH_COMMANDS_PER_CALLBACK: usize = 64;
pub const PLUGIN_GRAPH_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginInstanceId(pub u64);

impl PluginInstanceId {
    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFormat {
    Vst2,
    Vst3,
    Internal,
    #[default]
    Unknown,
}

/// Serializable plugin identity. No part of this value enters the audio callback.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    pub format: PluginFormat,
    /// Stable format-specific identity (VST2 unique ID or VST3 class ID).
    pub plugin_uid: String,
    pub display_name: String,
    pub vendor: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFaultKind {
    #[default]
    None,
    ProcessError,
    Panic,
    NonFiniteOutput,
    DeadlineExceeded,
    BackendDisconnected,
}

/// Fault state is numeric and `Copy`, so the real-time side can quarantine a slot
/// without allocating an error message or consulting the UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginFaultState {
    pub kind: PluginFaultKind,
    pub quarantined: bool,
    pub consecutive_faults: u32,
    pub total_faults: u64,
}

impl PluginFaultState {
    fn isolated(self, kind: PluginFaultKind) -> Self {
        Self {
            kind,
            quarantined: kind != PluginFaultKind::None,
            consecutive_faults: self.consecutive_faults.saturating_add(1),
            total_faults: self.total_faults.saturating_add(1),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginControlSlot {
    pub instance_id: PluginInstanceId,
    pub descriptor: PluginDescriptor,
    pub bypass: bool,
    pub wet: f32,
    pub reported_latency_samples: u32,
    pub reported_tail_samples: u64,
    pub fault: PluginFaultState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerInsertControl {
    pub slots: [Option<PluginControlSlot>; PLUGIN_SLOTS_PER_INSERT],
}

impl Default for MixerInsertControl {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
        }
    }
}

/// Serializable desired graph state. Slot index is chain order; instance IDs are
/// never derived from position, so moving a plugin preserves automation identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginGraphState {
    pub schema_version: u32,
    pub next_instance_id: u64,
    pub inserts: [MixerInsertControl; MIXER_INSERT_COUNT],
}

impl Default for PluginGraphState {
    fn default() -> Self {
        Self {
            schema_version: PLUGIN_GRAPH_SCHEMA_VERSION,
            next_instance_id: 1,
            inserts: std::array::from_fn(|_| MixerInsertControl::default()),
        }
    }
}

impl PluginGraphState {
    pub fn validate(&self) -> Result<(), PluginGraphError> {
        if self.schema_version != PLUGIN_GRAPH_SCHEMA_VERSION {
            return Err(PluginGraphError::UnsupportedSchema(self.schema_version));
        }
        let mut maximum_id = 0_u64;
        for insert in 0..MIXER_INSERT_COUNT {
            for slot in 0..PLUGIN_SLOTS_PER_INSERT {
                let Some(plugin) = &self.inserts[insert].slots[slot] else {
                    continue;
                };
                if !plugin.instance_id.is_valid() {
                    return Err(PluginGraphError::InvalidInstanceId);
                }
                if plugin.descriptor.plugin_uid.trim().is_empty() {
                    return Err(PluginGraphError::EmptyPluginUid);
                }
                if !plugin.wet.is_finite() || !(0.0..=1.0).contains(&plugin.wet) {
                    return Err(PluginGraphError::InvalidWet(plugin.wet));
                }
                maximum_id = maximum_id.max(plugin.instance_id.get());
                for other_insert in insert..MIXER_INSERT_COUNT {
                    let first_slot = if other_insert == insert { slot + 1 } else { 0 };
                    for other_slot in first_slot..PLUGIN_SLOTS_PER_INSERT {
                        if self.inserts[other_insert].slots[other_slot]
                            .as_ref()
                            .is_some_and(|other| other.instance_id == plugin.instance_id)
                        {
                            return Err(PluginGraphError::DuplicateInstanceId(plugin.instance_id));
                        }
                    }
                }
            }
        }
        if self.next_instance_id == 0 || self.next_instance_id <= maximum_id {
            return Err(PluginGraphError::InvalidNextInstanceId {
                next: self.next_instance_id,
                maximum_existing: maximum_id,
            });
        }
        Ok(())
    }

    pub fn ordered_chain(
        &self,
        insert: usize,
    ) -> Result<impl DoubleEndedIterator<Item = &PluginControlSlot>, PluginGraphError> {
        let insert = self
            .inserts
            .get(insert)
            .ok_or(PluginGraphError::InvalidInsert(insert))?;
        Ok(insert.slots.iter().filter_map(Option::as_ref))
    }

    fn find_instance(&self, instance_id: PluginInstanceId) -> Option<(usize, usize)> {
        self.inserts.iter().enumerate().find_map(|(insert, state)| {
            state
                .slots
                .iter()
                .position(|slot| {
                    slot.as_ref()
                        .is_some_and(|plugin| plugin.instance_id == instance_id)
                })
                .map(|slot| (insert, slot))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginGraphError {
    InvalidInsert(usize),
    InvalidSlot(usize),
    InvalidInstanceId,
    InstanceNotFound(PluginInstanceId),
    DuplicateInstanceId(PluginInstanceId),
    SlotOccupied { insert: usize, slot: usize },
    EmptyPluginUid,
    InvalidWet(f32),
    UnsupportedSchema(u32),
    InvalidNextInstanceId { next: u64, maximum_existing: u64 },
    InvalidQueueCapacity,
    CommandQueueFull,
    InstanceIdExhausted,
    SequenceExhausted,
}

impl fmt::Display for PluginGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInsert(insert) => write!(formatter, "Invalid mixer insert {insert}"),
            Self::InvalidSlot(slot) => write!(formatter, "Invalid plugin slot {slot}"),
            Self::InvalidInstanceId => formatter.write_str("Plugin instance ID zero is reserved"),
            Self::InstanceNotFound(id) => {
                write!(formatter, "Plugin instance {} was not found", id.get())
            }
            Self::DuplicateInstanceId(id) => {
                write!(formatter, "Plugin instance ID {} is duplicated", id.get())
            }
            Self::SlotOccupied { insert, slot } => {
                write!(formatter, "Mixer insert {insert} slot {slot} is occupied")
            }
            Self::EmptyPluginUid => formatter.write_str("Plugin UID cannot be empty"),
            Self::InvalidWet(wet) => write!(formatter, "Plugin wet value {wet} is outside 0..=1"),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "Unsupported plugin graph schema {version}")
            }
            Self::InvalidNextInstanceId {
                next,
                maximum_existing,
            } => write!(
                formatter,
                "Next plugin instance ID {next} must exceed existing ID {maximum_existing}"
            ),
            Self::InvalidQueueCapacity => {
                formatter.write_str("Plugin graph queue capacity must be non-zero")
            }
            Self::CommandQueueFull => formatter.write_str("Plugin graph command queue is full"),
            Self::InstanceIdExhausted => formatter.write_str("Plugin instance IDs are exhausted"),
            Self::SequenceExhausted => {
                formatter.write_str("Plugin graph sequence IDs are exhausted")
            }
        }
    }
}

impl Error for PluginGraphError {}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RealtimePluginSlot {
    pub instance_id: Option<PluginInstanceId>,
    pub bypass: bool,
    pub wet: f32,
    pub reported_latency_samples: u32,
    pub reported_tail_samples: u64,
    pub fault: PluginFaultState,
}

impl RealtimePluginSlot {
    pub fn is_occupied(self) -> bool {
        self.instance_id.is_some()
    }

    /// Whether a future processor should call this plugin for the current block.
    pub fn should_process(self) -> bool {
        self.is_occupied() && !self.bypass && !self.fault.quarantined
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InsertPluginTotals {
    pub active_plugins: u8,
    pub bypassed_plugins: u8,
    pub quarantined_plugins: u8,
    /// Serial insert-chain latency: saturating sum of active plugin latencies.
    pub latency_samples: u64,
    /// Conservative serial-chain tail: saturating sum of active plugin tails.
    pub tail_samples: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RealtimeInsertSnapshot {
    pub slots: [RealtimePluginSlot; PLUGIN_SLOTS_PER_INSERT],
    pub totals: InsertPluginTotals,
}

impl Default for RealtimeInsertSnapshot {
    fn default() -> Self {
        Self {
            slots: [RealtimePluginSlot::default(); PLUGIN_SLOTS_PER_INSERT],
            totals: InsertPluginTotals::default(),
        }
    }
}

impl RealtimeInsertSnapshot {
    fn recompute_totals(&mut self) {
        let mut totals = InsertPluginTotals::default();
        for plugin in self.slots.iter().copied() {
            if !plugin.is_occupied() {
                continue;
            }
            if plugin.fault.quarantined {
                totals.quarantined_plugins = totals.quarantined_plugins.saturating_add(1);
                continue;
            }
            if plugin.bypass {
                totals.bypassed_plugins = totals.bypassed_plugins.saturating_add(1);
                continue;
            }
            totals.active_plugins = totals.active_plugins.saturating_add(1);
            totals.latency_samples = totals
                .latency_samples
                .saturating_add(u64::from(plugin.reported_latency_samples));
            totals.tail_samples = totals
                .tail_samples
                .saturating_add(plugin.reported_tail_samples);
        }
        self.totals = totals;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RealtimeGraphSnapshot {
    pub revision: u64,
    pub inserts: [RealtimeInsertSnapshot; MIXER_INSERT_COUNT],
}

impl Default for RealtimeGraphSnapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            inserts: [RealtimeInsertSnapshot::default(); MIXER_INSERT_COUNT],
        }
    }
}

impl RealtimeGraphSnapshot {
    fn from_control(state: &PluginGraphState) -> Self {
        let mut snapshot = Self::default();
        for insert in 0..MIXER_INSERT_COUNT {
            for slot in 0..PLUGIN_SLOTS_PER_INSERT {
                if let Some(plugin) = &state.inserts[insert].slots[slot] {
                    snapshot.inserts[insert].slots[slot] = RealtimePluginSlot {
                        instance_id: Some(plugin.instance_id),
                        bypass: plugin.bypass,
                        wet: plugin.wet,
                        reported_latency_samples: plugin.reported_latency_samples,
                        reported_tail_samples: plugin.reported_tail_samples,
                        fault: plugin.fault,
                    };
                }
            }
            snapshot.inserts[insert].recompute_totals();
        }
        snapshot
    }

    pub fn insert(&self, insert: usize) -> Option<&RealtimeInsertSnapshot> {
        self.inserts.get(insert)
    }

    pub fn find_instance(
        &self,
        instance_id: PluginInstanceId,
    ) -> Option<(usize, usize, RealtimePluginSlot)> {
        self.inserts.iter().enumerate().find_map(|(insert, state)| {
            state
                .slots
                .iter()
                .copied()
                .enumerate()
                .find(|(_, slot)| slot.instance_id == Some(instance_id))
                .map(|(slot, state)| (insert, slot, state))
        })
    }

    fn apply(&mut self, command: GraphCommand) -> Result<(), GraphRejectReason> {
        match command {
            GraphCommand::Insert {
                insert,
                slot,
                plugin,
            } => {
                let insert_index = usize::from(insert);
                let slot_index = usize::from(slot);
                if insert_index >= MIXER_INSERT_COUNT || slot_index >= PLUGIN_SLOTS_PER_INSERT {
                    return Err(GraphRejectReason::InvalidAddress);
                }
                let Some(instance_id) = plugin.instance_id else {
                    return Err(GraphRejectReason::InvalidInstanceId);
                };
                if self.find_instance(instance_id).is_some() {
                    return Err(GraphRejectReason::DuplicateInstanceId);
                }
                if self.inserts[insert_index].slots[slot_index].is_occupied() {
                    return Err(GraphRejectReason::SlotOccupied);
                }
                self.inserts[insert_index].slots[slot_index] = plugin;
                self.inserts[insert_index].recompute_totals();
            }
            GraphCommand::Remove { instance_id } => {
                let Some((insert, slot, _)) = self.find_instance(instance_id) else {
                    return Err(GraphRejectReason::InstanceNotFound);
                };
                self.inserts[insert].slots[slot] = RealtimePluginSlot::default();
                self.inserts[insert].recompute_totals();
            }
            GraphCommand::Move {
                instance_id,
                insert,
                slot,
            } => {
                let destination_insert = usize::from(insert);
                let destination_slot = usize::from(slot);
                if destination_insert >= MIXER_INSERT_COUNT
                    || destination_slot >= PLUGIN_SLOTS_PER_INSERT
                {
                    return Err(GraphRejectReason::InvalidAddress);
                }
                let Some((source_insert, source_slot, plugin)) = self.find_instance(instance_id)
                else {
                    return Err(GraphRejectReason::InstanceNotFound);
                };
                if source_insert == destination_insert && source_slot == destination_slot {
                    return Ok(());
                }
                if self.inserts[destination_insert].slots[destination_slot].is_occupied() {
                    return Err(GraphRejectReason::SlotOccupied);
                }
                self.inserts[source_insert].slots[source_slot] = RealtimePluginSlot::default();
                self.inserts[destination_insert].slots[destination_slot] = plugin;
                self.inserts[source_insert].recompute_totals();
                if source_insert != destination_insert {
                    self.inserts[destination_insert].recompute_totals();
                }
            }
            GraphCommand::SetBypass {
                instance_id,
                bypass,
            } => {
                let Some((insert, slot, _)) = self.find_instance(instance_id) else {
                    return Err(GraphRejectReason::InstanceNotFound);
                };
                self.inserts[insert].slots[slot].bypass = bypass;
                self.inserts[insert].recompute_totals();
            }
            GraphCommand::SetWet { instance_id, wet } => {
                if !wet.is_finite() || !(0.0..=1.0).contains(&wet) {
                    return Err(GraphRejectReason::InvalidParameter);
                }
                let Some((insert, slot, _)) = self.find_instance(instance_id) else {
                    return Err(GraphRejectReason::InstanceNotFound);
                };
                self.inserts[insert].slots[slot].wet = wet;
            }
            GraphCommand::SetMetrics {
                instance_id,
                latency_samples,
                tail_samples,
            } => {
                let Some((insert, slot, _)) = self.find_instance(instance_id) else {
                    return Err(GraphRejectReason::InstanceNotFound);
                };
                let plugin = &mut self.inserts[insert].slots[slot];
                plugin.reported_latency_samples = latency_samples;
                plugin.reported_tail_samples = tail_samples;
                self.inserts[insert].recompute_totals();
            }
            GraphCommand::ClearFault { instance_id } => {
                let Some((insert, slot, _)) = self.find_instance(instance_id) else {
                    return Err(GraphRejectReason::InstanceNotFound);
                };
                self.inserts[insert].slots[slot].fault = PluginFaultState::default();
                self.inserts[insert].recompute_totals();
            }
            GraphCommand::ClearAll => {
                let revision = self.revision;
                *self = Self::default();
                self.revision = revision;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GraphCommand {
    Insert {
        insert: u8,
        slot: u8,
        plugin: RealtimePluginSlot,
    },
    Remove {
        instance_id: PluginInstanceId,
    },
    Move {
        instance_id: PluginInstanceId,
        insert: u8,
        slot: u8,
    },
    SetBypass {
        instance_id: PluginInstanceId,
        bypass: bool,
    },
    SetWet {
        instance_id: PluginInstanceId,
        wet: f32,
    },
    SetMetrics {
        instance_id: PluginInstanceId,
        latency_samples: u32,
        tail_samples: u64,
    },
    ClearFault {
        instance_id: PluginInstanceId,
    },
    ClearAll,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SequencedGraphCommand {
    pub sequence: u64,
    pub command: GraphCommand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphRejectReason {
    InvalidAddress,
    InvalidInstanceId,
    InstanceNotFound,
    DuplicateInstanceId,
    SlotOccupied,
    InvalidParameter,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PluginGraphEvent {
    CommandApplied {
        sequence: u64,
        revision: u64,
    },
    CommandRejected {
        sequence: u64,
        reason: GraphRejectReason,
    },
    FaultIsolated {
        instance_id: PluginInstanceId,
        fault: PluginFaultState,
        revision: u64,
    },
}

#[derive(Default)]
struct GraphQueueStats {
    command_queue_full: AtomicU64,
    event_queue_full: AtomicU64,
    rejected_commands: AtomicU64,
    isolated_faults: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphQueueSnapshot {
    pub command_queue_full: u64,
    pub event_queue_full: u64,
    pub rejected_commands: u64,
    pub isolated_faults: u64,
}

impl GraphQueueStats {
    fn snapshot(&self) -> GraphQueueSnapshot {
        GraphQueueSnapshot {
            command_queue_full: self.command_queue_full.load(Ordering::Relaxed),
            event_queue_full: self.event_queue_full.load(Ordering::Relaxed),
            rejected_commands: self.rejected_commands.load(Ordering::Relaxed),
            isolated_faults: self.isolated_faults.load(Ordering::Relaxed),
        }
    }
}

/// Control-thread owner. Its serializable state is the desired graph; use events
/// to determine when the callback has actually applied queued changes.
pub struct PluginGraphController {
    state: PluginGraphState,
    commands: Producer<SequencedGraphCommand>,
    events: Consumer<PluginGraphEvent>,
    stats: Arc<GraphQueueStats>,
    next_sequence: u64,
    last_queued_sequence: u64,
    last_applied_sequence: u64,
    realtime_revision: u64,
    needs_resync: bool,
}

impl PluginGraphController {
    pub fn state(&self) -> &PluginGraphState {
        &self.state
    }

    pub fn into_state(self) -> PluginGraphState {
        self.state
    }

    pub fn is_synchronized(&self) -> bool {
        !self.needs_resync() && self.last_applied_sequence == self.last_queued_sequence
    }

    pub fn needs_resync(&self) -> bool {
        self.needs_resync || self.stats.event_queue_full.load(Ordering::Relaxed) != 0
    }

    pub fn realtime_revision(&self) -> u64 {
        self.realtime_revision
    }

    pub fn queue_stats(&self) -> GraphQueueSnapshot {
        self.stats.snapshot()
    }

    pub fn insert_plugin(
        &mut self,
        insert: usize,
        slot: usize,
        descriptor: PluginDescriptor,
    ) -> Result<PluginInstanceId, PluginGraphError> {
        validate_address(insert, slot)?;
        if descriptor.plugin_uid.trim().is_empty() {
            return Err(PluginGraphError::EmptyPluginUid);
        }
        if self.state.inserts[insert].slots[slot].is_some() {
            return Err(PluginGraphError::SlotOccupied { insert, slot });
        }
        let instance_id = PluginInstanceId(self.state.next_instance_id);
        if !instance_id.is_valid() {
            return Err(PluginGraphError::InstanceIdExhausted);
        }
        let next_instance_id = self
            .state
            .next_instance_id
            .checked_add(1)
            .ok_or(PluginGraphError::InstanceIdExhausted)?;
        let control_slot = PluginControlSlot {
            instance_id,
            descriptor,
            bypass: false,
            wet: 1.0,
            reported_latency_samples: 0,
            reported_tail_samples: 0,
            fault: PluginFaultState::default(),
        };
        let realtime_slot = realtime_slot_from_control(&control_slot);
        self.queue(GraphCommand::Insert {
            insert: insert as u8,
            slot: slot as u8,
            plugin: realtime_slot,
        })?;
        self.state.inserts[insert].slots[slot] = Some(control_slot);
        self.state.next_instance_id = next_instance_id;
        Ok(instance_id)
    }

    pub fn remove_plugin(&mut self, instance_id: PluginInstanceId) -> Result<(), PluginGraphError> {
        let (insert, slot) = self
            .state
            .find_instance(instance_id)
            .ok_or(PluginGraphError::InstanceNotFound(instance_id))?;
        self.queue(GraphCommand::Remove { instance_id })?;
        self.state.inserts[insert].slots[slot] = None;
        Ok(())
    }

    pub fn move_plugin(
        &mut self,
        instance_id: PluginInstanceId,
        destination_insert: usize,
        destination_slot: usize,
    ) -> Result<(), PluginGraphError> {
        validate_address(destination_insert, destination_slot)?;
        let (source_insert, source_slot) = self
            .state
            .find_instance(instance_id)
            .ok_or(PluginGraphError::InstanceNotFound(instance_id))?;
        if source_insert == destination_insert && source_slot == destination_slot {
            return Ok(());
        }
        if self.state.inserts[destination_insert].slots[destination_slot].is_some() {
            return Err(PluginGraphError::SlotOccupied {
                insert: destination_insert,
                slot: destination_slot,
            });
        }
        self.queue(GraphCommand::Move {
            instance_id,
            insert: destination_insert as u8,
            slot: destination_slot as u8,
        })?;
        let plugin = self.state.inserts[source_insert].slots[source_slot]
            .take()
            .expect("located plugin must exist");
        self.state.inserts[destination_insert].slots[destination_slot] = Some(plugin);
        Ok(())
    }

    pub fn set_bypass(
        &mut self,
        instance_id: PluginInstanceId,
        bypass: bool,
    ) -> Result<(), PluginGraphError> {
        let (insert, slot) = self.locate(instance_id)?;
        self.queue(GraphCommand::SetBypass {
            instance_id,
            bypass,
        })?;
        self.state.inserts[insert].slots[slot]
            .as_mut()
            .expect("located plugin must exist")
            .bypass = bypass;
        Ok(())
    }

    pub fn set_wet(
        &mut self,
        instance_id: PluginInstanceId,
        wet: f32,
    ) -> Result<(), PluginGraphError> {
        if !wet.is_finite() || !(0.0..=1.0).contains(&wet) {
            return Err(PluginGraphError::InvalidWet(wet));
        }
        let (insert, slot) = self.locate(instance_id)?;
        self.queue(GraphCommand::SetWet { instance_id, wet })?;
        self.state.inserts[insert].slots[slot]
            .as_mut()
            .expect("located plugin must exist")
            .wet = wet;
        Ok(())
    }

    pub fn set_reported_metrics(
        &mut self,
        instance_id: PluginInstanceId,
        latency_samples: u32,
        tail_samples: u64,
    ) -> Result<(), PluginGraphError> {
        let (insert, slot) = self.locate(instance_id)?;
        self.queue(GraphCommand::SetMetrics {
            instance_id,
            latency_samples,
            tail_samples,
        })?;
        let plugin = self.state.inserts[insert].slots[slot]
            .as_mut()
            .expect("located plugin must exist");
        plugin.reported_latency_samples = latency_samples;
        plugin.reported_tail_samples = tail_samples;
        Ok(())
    }

    pub fn clear_fault(&mut self, instance_id: PluginInstanceId) -> Result<(), PluginGraphError> {
        let (insert, slot) = self.locate(instance_id)?;
        self.queue(GraphCommand::ClearFault { instance_id })?;
        self.state.inserts[insert].slots[slot]
            .as_mut()
            .expect("located plugin must exist")
            .fault = PluginFaultState::default();
        Ok(())
    }

    pub fn clear_all(&mut self) -> Result<(), PluginGraphError> {
        self.queue(GraphCommand::ClearAll)?;
        self.state.inserts = std::array::from_fn(|_| MixerInsertControl::default());
        Ok(())
    }

    /// Pops one callback-originated event and reconciles control-only fault state.
    pub fn poll_event(&mut self) -> Option<PluginGraphEvent> {
        let event = self.events.pop().ok()?;
        match event {
            PluginGraphEvent::CommandApplied { sequence, revision } => {
                self.last_applied_sequence = self.last_applied_sequence.max(sequence);
                self.realtime_revision = self.realtime_revision.max(revision);
            }
            PluginGraphEvent::CommandRejected { .. } => {
                self.needs_resync = true;
            }
            PluginGraphEvent::FaultIsolated {
                instance_id,
                fault,
                revision,
            } => {
                self.realtime_revision = self.realtime_revision.max(revision);
                if let Some((insert, slot)) = self.state.find_instance(instance_id) {
                    self.state.inserts[insert].slots[slot]
                        .as_mut()
                        .expect("located plugin must exist")
                        .fault = fault;
                } else {
                    self.needs_resync = true;
                }
            }
        }
        Some(event)
    }

    fn locate(&self, instance_id: PluginInstanceId) -> Result<(usize, usize), PluginGraphError> {
        self.state
            .find_instance(instance_id)
            .ok_or(PluginGraphError::InstanceNotFound(instance_id))
    }

    fn queue(&mut self, command: GraphCommand) -> Result<u64, PluginGraphError> {
        let sequence = self.next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(PluginGraphError::SequenceExhausted)?;
        match self
            .commands
            .push(SequencedGraphCommand { sequence, command })
        {
            Ok(()) => {
                self.next_sequence = next_sequence;
                self.last_queued_sequence = sequence;
                Ok(sequence)
            }
            Err(PushError::Full(_)) => {
                self.stats
                    .command_queue_full
                    .fetch_add(1, Ordering::Relaxed);
                Err(PluginGraphError::CommandQueueFull)
            }
        }
    }
}

/// Audio-callback owner. Applying commands and isolating faults performs only
/// bounded fixed-array work, atomic increments and SPSC pushes.
pub struct RealtimePluginGraph {
    snapshot: RealtimeGraphSnapshot,
    commands: Consumer<SequencedGraphCommand>,
    events: Producer<PluginGraphEvent>,
    stats: Arc<GraphQueueStats>,
}

impl RealtimePluginGraph {
    pub fn snapshot(&self) -> &RealtimeGraphSnapshot {
        &self.snapshot
    }

    pub fn pending_commands(&self) -> usize {
        self.commands.slots()
    }

    pub fn queue_stats(&self) -> GraphQueueSnapshot {
        self.stats.snapshot()
    }

    /// Applies at most [`MAX_GRAPH_COMMANDS_PER_CALLBACK`] changes. Call this once
    /// at a process-block boundary, before reading the snapshot for that block.
    pub fn apply_pending_commands(&mut self) -> usize {
        self.apply_pending_commands_with_budget(MAX_GRAPH_COMMANDS_PER_CALLBACK)
    }

    pub fn apply_pending_commands_with_budget(&mut self, budget: usize) -> usize {
        let budget = budget.min(MAX_GRAPH_COMMANDS_PER_CALLBACK);
        let mut applied = 0;
        while applied < budget {
            if self.events.slots() == 0 {
                break;
            }
            let Ok(command) = self.commands.pop() else {
                break;
            };
            let event = match self.snapshot.apply(command.command) {
                Ok(()) => {
                    self.snapshot.revision = self.snapshot.revision.saturating_add(1);
                    PluginGraphEvent::CommandApplied {
                        sequence: command.sequence,
                        revision: self.snapshot.revision,
                    }
                }
                Err(reason) => {
                    self.stats.rejected_commands.fetch_add(1, Ordering::Relaxed);
                    PluginGraphEvent::CommandRejected {
                        sequence: command.sequence,
                        reason,
                    }
                }
            };
            if self.events.push(event).is_err() {
                // A slot was preflighted above. Preserve the counter defensively if
                // an implementation invariant changes later.
                self.stats.event_queue_full.fetch_add(1, Ordering::Relaxed);
                break;
            }
            applied += 1;
        }
        applied
    }

    /// Immediately quarantines one instance. A future processor must stop calling
    /// it and use dry/bypass audio for the remainder of this and subsequent blocks.
    pub fn isolate_fault(&mut self, instance_id: PluginInstanceId, kind: PluginFaultKind) -> bool {
        if kind == PluginFaultKind::None {
            return false;
        }
        let Some((insert, slot, current)) = self.snapshot.find_instance(instance_id) else {
            return false;
        };
        let fault = current.fault.isolated(kind);
        self.snapshot.inserts[insert].slots[slot].fault = fault;
        self.snapshot.inserts[insert].recompute_totals();
        self.snapshot.revision = self.snapshot.revision.saturating_add(1);
        self.stats.isolated_faults.fetch_add(1, Ordering::Relaxed);
        let event = PluginGraphEvent::FaultIsolated {
            instance_id,
            fault,
            revision: self.snapshot.revision,
        };
        if self.events.push(event).is_err() {
            self.stats.event_queue_full.fetch_add(1, Ordering::Relaxed);
        }
        true
    }
}

pub fn create_plugin_graph(
    initial_state: PluginGraphState,
) -> Result<(PluginGraphController, RealtimePluginGraph), PluginGraphError> {
    create_plugin_graph_with_capacities(
        initial_state,
        DEFAULT_GRAPH_COMMAND_CAPACITY,
        DEFAULT_GRAPH_EVENT_CAPACITY,
    )
}

pub fn create_plugin_graph_with_capacities(
    initial_state: PluginGraphState,
    command_capacity: usize,
    event_capacity: usize,
) -> Result<(PluginGraphController, RealtimePluginGraph), PluginGraphError> {
    if command_capacity == 0 || event_capacity == 0 {
        return Err(PluginGraphError::InvalidQueueCapacity);
    }
    initial_state.validate()?;
    let snapshot = RealtimeGraphSnapshot::from_control(&initial_state);
    let (command_producer, command_consumer) = RingBuffer::new(command_capacity);
    let (event_producer, event_consumer) = RingBuffer::new(event_capacity);
    let stats = Arc::new(GraphQueueStats::default());
    let controller = PluginGraphController {
        state: initial_state,
        commands: command_producer,
        events: event_consumer,
        stats: stats.clone(),
        next_sequence: 1,
        last_queued_sequence: 0,
        last_applied_sequence: 0,
        realtime_revision: 0,
        needs_resync: false,
    };
    let realtime = RealtimePluginGraph {
        snapshot,
        commands: command_consumer,
        events: event_producer,
        stats,
    };
    Ok((controller, realtime))
}

fn validate_address(insert: usize, slot: usize) -> Result<(), PluginGraphError> {
    if insert >= MIXER_INSERT_COUNT {
        return Err(PluginGraphError::InvalidInsert(insert));
    }
    if slot >= PLUGIN_SLOTS_PER_INSERT {
        return Err(PluginGraphError::InvalidSlot(slot));
    }
    Ok(())
}

fn realtime_slot_from_control(plugin: &PluginControlSlot) -> RealtimePluginSlot {
    RealtimePluginSlot {
        instance_id: Some(plugin.instance_id),
        bypass: plugin.bypass,
        wet: plugin.wet,
        reported_latency_samples: plugin.reported_latency_samples,
        reported_tail_samples: plugin.reported_tail_samples,
        fault: plugin.fault,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(uid: &str) -> PluginDescriptor {
        PluginDescriptor {
            format: PluginFormat::Vst3,
            plugin_uid: uid.to_owned(),
            display_name: format!("Plugin {uid}"),
            vendor: "Citrus Test".to_owned(),
        }
    }

    fn apply_and_drain(controller: &mut PluginGraphController, realtime: &mut RealtimePluginGraph) {
        while realtime.apply_pending_commands() != 0 {}
        while controller.poll_event().is_some() {}
    }

    #[test]
    fn serializable_control_state_preserves_stable_ids_and_slot_order() {
        let (mut controller, mut realtime) =
            create_plugin_graph(PluginGraphState::default()).unwrap();
        let first = controller.insert_plugin(2, 0, descriptor("a")).unwrap();
        let second = controller.insert_plugin(2, 4, descriptor("b")).unwrap();
        controller.move_plugin(first, 2, 3).unwrap();
        apply_and_drain(&mut controller, &mut realtime);

        let ordered = controller
            .state()
            .ordered_chain(2)
            .unwrap()
            .map(|plugin| plugin.instance_id)
            .collect::<Vec<_>>();
        assert_eq!(ordered, vec![first, second]);
        assert_eq!(
            realtime.snapshot().inserts[2].slots[3].instance_id,
            Some(first)
        );
        assert_eq!(
            realtime.snapshot().inserts[2].slots[4].instance_id,
            Some(second)
        );

        let json = serde_json::to_string(controller.state()).unwrap();
        let restored: PluginGraphState = serde_json::from_str(&json).unwrap();
        restored.validate().unwrap();
        assert_eq!(&restored, controller.state());
        assert!(restored.next_instance_id > second.get());
    }

    #[test]
    fn enqueue_is_not_application_and_ack_makes_controller_synchronized() {
        let (mut controller, mut realtime) =
            create_plugin_graph(PluginGraphState::default()).unwrap();
        let id = controller
            .insert_plugin(0, 0, descriptor("queued"))
            .unwrap();

        assert!(!controller.is_synchronized());
        assert!(realtime.snapshot().find_instance(id).is_none());
        assert_eq!(realtime.apply_pending_commands(), 1);
        assert!(realtime.snapshot().find_instance(id).is_some());
        assert!(!controller.is_synchronized());
        assert!(matches!(
            controller.poll_event(),
            Some(PluginGraphEvent::CommandApplied { sequence: 1, .. })
        ));
        assert!(controller.is_synchronized());
    }

    #[test]
    fn command_queue_full_does_not_mutate_desired_state() {
        let (mut controller, _realtime) =
            create_plugin_graph_with_capacities(PluginGraphState::default(), 1, 4).unwrap();
        let first = controller.insert_plugin(0, 0, descriptor("first")).unwrap();
        let next_id = controller.state().next_instance_id;

        let error = controller
            .insert_plugin(0, 1, descriptor("rejected"))
            .unwrap_err();

        assert_eq!(error, PluginGraphError::CommandQueueFull);
        assert_eq!(controller.state().next_instance_id, next_id);
        assert!(controller.state().inserts[0].slots[1].is_none());
        assert_eq!(
            controller.state().inserts[0].slots[0]
                .as_ref()
                .unwrap()
                .instance_id,
            first
        );
        assert_eq!(controller.queue_stats().command_queue_full, 1);
    }

    #[test]
    fn callback_budget_caps_work_and_event_backpressure_preserves_commands() {
        let (mut controller, mut realtime) =
            create_plugin_graph_with_capacities(PluginGraphState::default(), 128, 1).unwrap();
        for slot in 0..PLUGIN_SLOTS_PER_INSERT {
            controller
                .insert_plugin(0, slot, descriptor(&format!("p{slot}")))
                .unwrap();
        }

        assert_eq!(realtime.apply_pending_commands_with_budget(usize::MAX), 1);
        assert_eq!(realtime.pending_commands(), PLUGIN_SLOTS_PER_INSERT - 1);
        assert_eq!(realtime.apply_pending_commands(), 0);
        assert!(controller.poll_event().is_some());
        assert_eq!(realtime.apply_pending_commands_with_budget(3), 1);
        assert_eq!(realtime.pending_commands(), PLUGIN_SLOTS_PER_INSERT - 2);

        while controller.poll_event().is_some() {}
        let mut processed = 2;
        while processed < PLUGIN_SLOTS_PER_INSERT {
            processed += realtime.apply_pending_commands();
            while controller.poll_event().is_some() {}
        }
        assert_eq!(processed, PLUGIN_SLOTS_PER_INSERT);
        assert!(controller.is_synchronized());

        let (mut controller, mut realtime) =
            create_plugin_graph_with_capacities(PluginGraphState::default(), 128, 128).unwrap();
        for insert in 0..7 {
            for slot in 0..PLUGIN_SLOTS_PER_INSERT {
                controller
                    .insert_plugin(insert, slot, descriptor(&format!("{insert}-{slot}")))
                    .unwrap();
            }
        }
        assert_eq!(realtime.apply_pending_commands_with_budget(usize::MAX), 64);
        assert_eq!(realtime.pending_commands(), 6);
    }

    #[test]
    fn latency_tail_bypass_and_fault_totals_follow_ordered_active_chain() {
        let (mut controller, mut realtime) =
            create_plugin_graph(PluginGraphState::default()).unwrap();
        let first = controller
            .insert_plugin(4, 0, descriptor("latency-a"))
            .unwrap();
        let second = controller
            .insert_plugin(4, 1, descriptor("latency-b"))
            .unwrap();
        controller.set_reported_metrics(first, 32, 1_000).unwrap();
        controller.set_reported_metrics(second, 64, 2_000).unwrap();
        apply_and_drain(&mut controller, &mut realtime);

        assert_eq!(
            realtime.snapshot().inserts[4].totals,
            InsertPluginTotals {
                active_plugins: 2,
                latency_samples: 96,
                tail_samples: 3_000,
                ..InsertPluginTotals::default()
            }
        );

        controller.set_bypass(first, true).unwrap();
        apply_and_drain(&mut controller, &mut realtime);
        assert_eq!(realtime.snapshot().inserts[4].totals.active_plugins, 1);
        assert_eq!(realtime.snapshot().inserts[4].totals.bypassed_plugins, 1);
        assert_eq!(realtime.snapshot().inserts[4].totals.latency_samples, 64);

        assert!(realtime.isolate_fault(second, PluginFaultKind::NonFiniteOutput));
        assert_eq!(realtime.snapshot().inserts[4].totals.active_plugins, 0);
        assert_eq!(realtime.snapshot().inserts[4].totals.quarantined_plugins, 1);
        assert_eq!(realtime.snapshot().inserts[4].totals.latency_samples, 0);
        assert!(!realtime.snapshot().inserts[4].slots[1].should_process());
    }

    #[test]
    fn fault_event_reconciles_control_state_and_clear_reenables_processing() {
        let (mut controller, mut realtime) =
            create_plugin_graph(PluginGraphState::default()).unwrap();
        let id = controller
            .insert_plugin(1, 2, descriptor("faulty"))
            .unwrap();
        apply_and_drain(&mut controller, &mut realtime);

        assert!(realtime.isolate_fault(id, PluginFaultKind::ProcessError));
        let event = controller.poll_event().unwrap();
        assert!(matches!(
            event,
            PluginGraphEvent::FaultIsolated {
                instance_id,
                fault: PluginFaultState {
                    quarantined: true,
                    total_faults: 1,
                    ..
                },
                ..
            } if instance_id == id
        ));
        let control_fault = controller.state().inserts[1].slots[2]
            .as_ref()
            .unwrap()
            .fault;
        assert!(control_fault.quarantined);

        controller.clear_fault(id).unwrap();
        apply_and_drain(&mut controller, &mut realtime);
        let slot = realtime.snapshot().find_instance(id).unwrap().2;
        assert_eq!(slot.fault, PluginFaultState::default());
        assert!(slot.should_process());
    }

    #[test]
    fn wet_remove_and_clear_commands_keep_instance_identity_and_state_consistent() {
        let (mut controller, mut realtime) =
            create_plugin_graph(PluginGraphState::default()).unwrap();
        let removed = controller
            .insert_plugin(3, 0, descriptor("remove"))
            .unwrap();
        let retained = controller
            .insert_plugin(3, 1, descriptor("retain"))
            .unwrap();
        controller.set_wet(retained, 0.25).unwrap();
        apply_and_drain(&mut controller, &mut realtime);
        assert_eq!(
            realtime.snapshot().find_instance(retained).unwrap().2.wet,
            0.25
        );

        controller.remove_plugin(removed).unwrap();
        apply_and_drain(&mut controller, &mut realtime);
        assert!(realtime.snapshot().find_instance(removed).is_none());
        assert_eq!(
            realtime
                .snapshot()
                .find_instance(retained)
                .unwrap()
                .2
                .instance_id,
            Some(retained)
        );

        let next_id = controller.state().next_instance_id;
        controller.clear_all().unwrap();
        apply_and_drain(&mut controller, &mut realtime);
        assert!(
            realtime
                .snapshot()
                .inserts
                .iter()
                .flat_map(|insert| insert.slots)
                .all(|slot| !slot.is_occupied())
        );
        assert_eq!(controller.state().next_instance_id, next_id);
        assert!(controller.is_synchronized());
    }

    #[test]
    fn lost_fault_event_is_counted_and_marks_control_plane_for_resync() {
        let (mut controller, mut realtime) =
            create_plugin_graph_with_capacities(PluginGraphState::default(), 4, 1).unwrap();
        let id = controller
            .insert_plugin(0, 0, descriptor("overflow"))
            .unwrap();
        assert_eq!(realtime.apply_pending_commands(), 1);
        assert!(realtime.isolate_fault(id, PluginFaultKind::DeadlineExceeded));

        assert!(
            realtime
                .snapshot()
                .find_instance(id)
                .unwrap()
                .2
                .fault
                .quarantined
        );
        assert_eq!(controller.queue_stats().event_queue_full, 1);
        assert!(controller.needs_resync());
        assert!(matches!(
            controller.poll_event(),
            Some(PluginGraphEvent::CommandApplied { .. })
        ));
        assert!(controller.needs_resync());
    }

    #[test]
    fn invalid_state_and_parameters_are_rejected_before_realtime_queue() {
        let mut state = PluginGraphState::default();
        state.inserts[0].slots[0] = Some(PluginControlSlot {
            instance_id: PluginInstanceId(7),
            descriptor: descriptor("duplicate"),
            bypass: false,
            wet: 1.0,
            reported_latency_samples: 0,
            reported_tail_samples: 0,
            fault: PluginFaultState::default(),
        });
        state.inserts[1].slots[0] = state.inserts[0].slots[0].clone();
        state.next_instance_id = 8;
        assert_eq!(
            state.validate(),
            Err(PluginGraphError::DuplicateInstanceId(PluginInstanceId(7)))
        );

        let (mut controller, _realtime) = create_plugin_graph(PluginGraphState::default()).unwrap();
        let id = controller.insert_plugin(0, 0, descriptor("valid")).unwrap();
        assert!(matches!(
            controller.set_wet(id, f32::NAN),
            Err(PluginGraphError::InvalidWet(wet)) if wet.is_nan()
        ));
        assert_eq!(
            controller.state().inserts[0].slots[0].as_ref().unwrap().wet,
            1.0
        );
        assert!(controller.insert_plugin(32, 0, descriptor("bad")).is_err());
        assert!(controller.insert_plugin(0, 10, descriptor("bad")).is_err());
    }

    #[test]
    fn full_fixed_graph_holds_exactly_three_hundred_twenty_instances() {
        let mut state = PluginGraphState::default();
        let mut next = 1_u64;
        for insert in &mut state.inserts {
            for slot in &mut insert.slots {
                *slot = Some(PluginControlSlot {
                    instance_id: PluginInstanceId(next),
                    descriptor: descriptor(&format!("full-{next}")),
                    bypass: false,
                    wet: 1.0,
                    reported_latency_samples: 1,
                    reported_tail_samples: 2,
                    fault: PluginFaultState::default(),
                });
                next += 1;
            }
        }
        state.next_instance_id = next;
        state.validate().unwrap();
        let (controller, realtime) = create_plugin_graph(state).unwrap();

        assert_eq!(MAX_PLUGIN_INSTANCES, 320);
        assert_eq!(
            controller
                .state()
                .inserts
                .iter()
                .flat_map(|insert| insert.slots.iter())
                .filter(|slot| slot.is_some())
                .count(),
            MAX_PLUGIN_INSTANCES
        );
        assert!(
            realtime
                .snapshot()
                .inserts
                .iter()
                .all(|insert| insert.totals.active_plugins == 10)
        );
    }

    #[test]
    fn realtime_types_are_send_and_snapshot_is_fixed_copy_data() {
        fn assert_send<T: Send>() {}
        fn assert_copy<T: Copy>() {}
        assert_send::<PluginGraphController>();
        assert_send::<RealtimePluginGraph>();
        assert_copy::<RealtimeGraphSnapshot>();
        assert_copy::<GraphCommand>();
        assert_eq!(
            std::mem::size_of::<RealtimeGraphSnapshot>(),
            std::mem::size_of::<RealtimeInsertSnapshot>() * MIXER_INSERT_COUNT + 8
        );
    }
}
