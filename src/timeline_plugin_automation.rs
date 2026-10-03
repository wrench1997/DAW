//! Callback-owned, fixed-capacity candidate batches for plug-in endpoint events.
//!
//! Building, validating, aborting and reading a batch allocate no memory. A successful
//! [`TimelineEndpointBatchPlan::preflight`] produces stable event/class slices ordered by
//! callback offset, with `System` before `Timeline` before `Live` only when offsets are equal.

use crate::{
    fixed_quantum::{FrameEvent, FrameEventKind},
    plugins::plugin_runtime::{MAX_PLUGIN_CHAIN_SLOTS, ParameterEditId},
    timeline::{
        TIMELINE_CALLBACK_MAX_FRAMES, TIMELINE_CALLBACK_MAX_PLUGIN_ENDPOINTS,
        TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS, TIMELINE_ENDPOINT_MAX_EVENTS_PER_CALLBACK,
        TIMELINE_ENDPOINT_MAX_EVENTS_PER_QUANTUM, TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES,
    },
};

pub const TIMELINE_ENDPOINT_BATCH_MAX_ENDPOINTS: usize = TIMELINE_CALLBACK_MAX_PLUGIN_ENDPOINTS;
pub const TIMELINE_ENDPOINT_BATCH_MAX_FRAMES: usize = TIMELINE_CALLBACK_MAX_FRAMES;
pub const TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES: usize = TIMELINE_PLUGIN_FIXED_QUANTUM_FRAMES;
pub const TIMELINE_ENDPOINT_BATCH_MAX_QUANTA: usize = (TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES - 1
    + TIMELINE_ENDPOINT_BATCH_MAX_FRAMES)
    .div_ceil(TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES);
pub const TIMELINE_ENDPOINT_BATCH_MAX_EVENTS: usize = TIMELINE_ENDPOINT_MAX_EVENTS_PER_CALLBACK;
pub const SYSTEM_MAX_EVENTS_PER_CALLBACK: usize = TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS;
pub const SYSTEM_MAX_EVENTS_PER_QUANTUM: usize = TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS;
pub const LIVE_MAX_EVENTS_PER_QUANTUM: usize = TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS;
pub const LIVE_MAX_EVENTS_PER_CALLBACK: usize = TIMELINE_ENDPOINT_DISCONTINUITY_EVENTS;
pub const PHYSICAL_MAX_EVENTS_PER_QUANTUM: usize = TIMELINE_ENDPOINT_MAX_EVENTS_PER_QUANTUM;
pub const TIMELINE_MAX_EVENTS_PER_QUANTUM: usize =
    PHYSICAL_MAX_EVENTS_PER_QUANTUM - SYSTEM_MAX_EVENTS_PER_QUANTUM - LIVE_MAX_EVENTS_PER_QUANTUM;
pub const TIMELINE_MAX_EVENTS_PER_CALLBACK: usize = TIMELINE_ENDPOINT_BATCH_MAX_EVENTS
    - SYSTEM_MAX_EVENTS_PER_CALLBACK
    - LIVE_MAX_EVENTS_PER_CALLBACK;

const _: () = assert!(TIMELINE_ENDPOINT_BATCH_MAX_ENDPOINTS == 96);
const _: () = assert!(TIMELINE_ENDPOINT_BATCH_MAX_FRAMES == 2_048);
const _: () = assert!(TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES == 128);
const _: () = assert!(TIMELINE_ENDPOINT_BATCH_MAX_QUANTA == 17);
const _: () = assert!(SYSTEM_MAX_EVENTS_PER_CALLBACK == 16);
const _: () = assert!(SYSTEM_MAX_EVENTS_PER_QUANTUM == 16);
const _: () = assert!(TIMELINE_MAX_EVENTS_PER_QUANTUM == 96);
const _: () = assert!(TIMELINE_MAX_EVENTS_PER_CALLBACK == 224);
const _: () = assert!(LIVE_MAX_EVENTS_PER_QUANTUM == 16);
const _: () = assert!(LIVE_MAX_EVENTS_PER_CALLBACK == 16);
const _: () = assert!(PHYSICAL_MAX_EVENTS_PER_QUANTUM == 128);
const _: () = assert!(TIMELINE_ENDPOINT_BATCH_MAX_EVENTS == 256);

const SORT_RANK_COUNT: usize = 4;
const SORT_KEY_COUNT: usize = TIMELINE_ENDPOINT_BATCH_MAX_FRAMES * SORT_RANK_COUNT;
const EMPTY_EVENT: FrameEvent = FrameEvent::midi(0, None, [0; 3]);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum EndpointEventClass {
    #[default]
    System = 0,
    Timeline = 1,
    Live = 2,
}

impl EndpointEventClass {
    const fn sort_rank(self, event: FrameEvent) -> usize {
        match (self, event.kind) {
            (Self::System, _) => 0,
            (Self::Timeline, FrameEventKind::Parameter { .. }) => 1,
            (Self::Timeline, FrameEventKind::Midi { .. }) => 2,
            (Self::Live, _) => 3,
        }
    }
}

/// Physical callback endpoint addressed by a compiled timeline route.
///
/// Generator chains have one stable instrument identity, while a mixer insert
/// is one physical serial worker containing several independently identified
/// slots. Keeping that distinction in the key prevents an insert batch from
/// being registered once per automated slot and accidentally borrowing the
/// physical endpoint's callback/quantum quota multiple times.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimelineEndpointAddress {
    Generator {
        channel_id: u32,
        plugin_instance_id: u64,
    },
    MixerInsert {
        track: u8,
    },
}

impl Default for TimelineEndpointAddress {
    fn default() -> Self {
        Self::Generator {
            channel_id: 0,
            plugin_instance_id: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TimelineEndpointKey {
    pub address: TimelineEndpointAddress,
    pub endpoint_id: u64,
}

impl TimelineEndpointKey {
    pub const fn new(channel_id: u32, endpoint_id: u64, plugin_instance_id: u64) -> Self {
        Self {
            address: TimelineEndpointAddress::Generator {
                channel_id,
                plugin_instance_id,
            },
            endpoint_id,
        }
    }

    pub const fn mixer_insert(track: u8, endpoint_id: u64) -> Self {
        Self {
            address: TimelineEndpointAddress::MixerInsert { track },
            endpoint_id,
        }
    }

    pub const fn address(self) -> TimelineEndpointAddress {
        self.address
    }

    pub const fn channel_id(self) -> u32 {
        match self.address {
            TimelineEndpointAddress::Generator { channel_id, .. } => channel_id,
            TimelineEndpointAddress::MixerInsert { .. } => 0,
        }
    }

    pub const fn endpoint_id(self) -> u64 {
        self.endpoint_id
    }

    pub const fn plugin_instance_id(self) -> u64 {
        match self.address {
            TimelineEndpointAddress::Generator {
                plugin_instance_id, ..
            } => plugin_instance_id,
            TimelineEndpointAddress::MixerInsert { .. } => 0,
        }
    }

    pub const fn mixer_track(self) -> Option<u8> {
        match self.address {
            TimelineEndpointAddress::Generator { .. } => None,
            TimelineEndpointAddress::MixerInsert { track } => Some(track),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineEndpointHandle(u8);

impl TimelineEndpointHandle {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineEndpointIdentity {
    pub key: TimelineEndpointKey,
    pub phase: u8,
    pub frames: u16,
    pub quantum_bucket_count: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineEndpointQuantumUsage {
    pub system: u16,
    pub timeline: u16,
    pub live: u16,
    pub total: u16,
}

impl TimelineEndpointQuantumUsage {
    pub const fn from_class_counts(system: u16, timeline: u16, live: u16) -> Self {
        Self {
            system,
            timeline,
            live,
            total: system.saturating_add(timeline).saturating_add(live),
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.system == 0 && self.timeline == 0 && self.live == 0 && self.total == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineEndpointBatchError {
    BatchAlreadyPrepared,
    EndpointCapacityExceeded,
    DuplicateEndpoint(TimelineEndpointKey),
    InvalidEndpointHandle,
    InvalidPhase { phase: usize },
    InvalidFrameCount { frames: usize },
    NonEmptySeedAtQuantumBoundary,
    InvalidFirstQuantumUsage,
    EventOutsideCallback { sample_offset: u16, frames: u16 },
    ParameterRequiresQ128Push,
    InvalidSystemEvent,
    InvalidMidiChannel { channel: u8 },
    NonFiniteParameter,
    ParameterNotOnQuantumBoundary { sample_offset: u16, phase: u8 },
    InvalidParameterSlot { slot: u8 },
    SystemQuantumCapacityExceeded { quantum: u8 },
    SystemCallbackCapacityExceeded,
    TimelineQuantumCapacityExceeded { quantum: u8 },
    TimelineCallbackCapacityExceeded,
    LiveQuantumCapacityExceeded { quantum: u8 },
    LiveCallbackCapacityExceeded,
    PhysicalQuantumCapacityExceeded { quantum: u8 },
    CallbackCapacityExceeded,
    CorruptCandidate,
}

#[derive(Clone, Copy)]
struct EndpointCandidate {
    identity: TimelineEndpointIdentity,
    events: [FrameEvent; TIMELINE_ENDPOINT_BATCH_MAX_EVENTS],
    classes: [EndpointEventClass; TIMELINE_ENDPOINT_BATCH_MAX_EVENTS],
    event_count: u16,
    system_count: u16,
    timeline_count: u16,
    live_count: u16,
    first_quantum_seed: TimelineEndpointQuantumUsage,
    quantum_usage: [TimelineEndpointQuantumUsage; TIMELINE_ENDPOINT_BATCH_MAX_QUANTA],
}

impl EndpointCandidate {
    const EMPTY: Self = Self {
        identity: TimelineEndpointIdentity {
            key: TimelineEndpointKey {
                address: TimelineEndpointAddress::Generator {
                    channel_id: 0,
                    plugin_instance_id: 0,
                },
                endpoint_id: 0,
            },
            phase: 0,
            frames: 0,
            quantum_bucket_count: 0,
        },
        events: [EMPTY_EVENT; TIMELINE_ENDPOINT_BATCH_MAX_EVENTS],
        classes: [EndpointEventClass::System; TIMELINE_ENDPOINT_BATCH_MAX_EVENTS],
        event_count: 0,
        system_count: 0,
        timeline_count: 0,
        live_count: 0,
        first_quantum_seed: TimelineEndpointQuantumUsage {
            system: 0,
            timeline: 0,
            live: 0,
            total: 0,
        },
        quantum_usage: [TimelineEndpointQuantumUsage {
            system: 0,
            timeline: 0,
            live: 0,
            total: 0,
        }; TIMELINE_ENDPOINT_BATCH_MAX_QUANTA],
    };

    fn reset(&mut self) {
        self.identity = TimelineEndpointIdentity::default();
        self.event_count = 0;
        self.system_count = 0;
        self.timeline_count = 0;
        self.live_count = 0;
        self.first_quantum_seed = TimelineEndpointQuantumUsage::default();
        self.quantum_usage
            .fill(TimelineEndpointQuantumUsage::default());
    }
}

/// A prepared endpoint view. Its two event slices are parallel and have identical lengths.
#[derive(Clone, Copy, Debug)]
pub struct PreparedTimelineEndpoint<'a> {
    identity: TimelineEndpointIdentity,
    events: &'a [FrameEvent],
    classes: &'a [EndpointEventClass],
    quantum_usage: &'a [TimelineEndpointQuantumUsage],
}

impl<'a> PreparedTimelineEndpoint<'a> {
    pub const fn identity(self) -> TimelineEndpointIdentity {
        self.identity
    }

    pub const fn events(self) -> &'a [FrameEvent] {
        self.events
    }

    pub const fn classes(self) -> &'a [EndpointEventClass] {
        self.classes
    }

    pub const fn quantum_usage(self) -> &'a [TimelineEndpointQuantumUsage] {
        self.quantum_usage
    }
}

/// Fixed-capacity transaction prepared on the callback thread and consumed only after commit.
pub struct TimelineEndpointBatchPlan {
    endpoints: [EndpointCandidate; TIMELINE_ENDPOINT_BATCH_MAX_ENDPOINTS],
    endpoint_count: u8,
    prepared: bool,
    sort_counts: [u16; SORT_KEY_COUNT],
    sort_events: [FrameEvent; TIMELINE_ENDPOINT_BATCH_MAX_EVENTS],
    sort_classes: [EndpointEventClass; TIMELINE_ENDPOINT_BATCH_MAX_EVENTS],
}

impl Default for TimelineEndpointBatchPlan {
    fn default() -> Self {
        Self::new()
    }
}

impl TimelineEndpointBatchPlan {
    pub const fn new() -> Self {
        Self {
            endpoints: [EndpointCandidate::EMPTY; TIMELINE_ENDPOINT_BATCH_MAX_ENDPOINTS],
            endpoint_count: 0,
            prepared: false,
            sort_counts: [0; SORT_KEY_COUNT],
            sort_events: [EMPTY_EVENT; TIMELINE_ENDPOINT_BATCH_MAX_EVENTS],
            sort_classes: [EndpointEventClass::System; TIMELINE_ENDPOINT_BATCH_MAX_EVENTS],
        }
    }

    /// Allocate and initialize the plan directly in heap storage.
    ///
    /// `Self` is intentionally large. Wrapping [`Self::new`] in `Box::new` can first materialize
    /// the complete value on a small audio/test thread stack. This constructor initializes every
    /// array element in its final allocation and never creates a plan-sized stack temporary.
    pub fn new_boxed() -> Box<Self> {
        let mut storage = Box::<Self>::new_uninit();
        let plan = storage.as_mut_ptr();
        // SAFETY: Every field of `Self`, every endpoint field, and every array element is written
        // exactly once before `assume_init`. All written values are valid for their field types,
        // and the allocation remains exclusively owned by `storage` throughout initialization.
        unsafe {
            let endpoints = std::ptr::addr_of_mut!((*plan).endpoints).cast::<EndpointCandidate>();
            for endpoint_index in 0..TIMELINE_ENDPOINT_BATCH_MAX_ENDPOINTS {
                let endpoint = endpoints.add(endpoint_index);
                std::ptr::addr_of_mut!((*endpoint).identity)
                    .write(TimelineEndpointIdentity::default());

                let events = std::ptr::addr_of_mut!((*endpoint).events).cast::<FrameEvent>();
                for event_index in 0..TIMELINE_ENDPOINT_BATCH_MAX_EVENTS {
                    events.add(event_index).write(EMPTY_EVENT);
                }
                let classes =
                    std::ptr::addr_of_mut!((*endpoint).classes).cast::<EndpointEventClass>();
                for class_index in 0..TIMELINE_ENDPOINT_BATCH_MAX_EVENTS {
                    classes.add(class_index).write(EndpointEventClass::System);
                }

                std::ptr::addr_of_mut!((*endpoint).event_count).write(0);
                std::ptr::addr_of_mut!((*endpoint).system_count).write(0);
                std::ptr::addr_of_mut!((*endpoint).timeline_count).write(0);
                std::ptr::addr_of_mut!((*endpoint).live_count).write(0);
                std::ptr::addr_of_mut!((*endpoint).first_quantum_seed)
                    .write(TimelineEndpointQuantumUsage::default());
                let quantum_usage = std::ptr::addr_of_mut!((*endpoint).quantum_usage)
                    .cast::<TimelineEndpointQuantumUsage>();
                for quantum_index in 0..TIMELINE_ENDPOINT_BATCH_MAX_QUANTA {
                    quantum_usage
                        .add(quantum_index)
                        .write(TimelineEndpointQuantumUsage::default());
                }
            }

            std::ptr::addr_of_mut!((*plan).endpoint_count).write(0);
            std::ptr::addr_of_mut!((*plan).prepared).write(false);
            std::ptr::write_bytes(
                std::ptr::addr_of_mut!((*plan).sort_counts).cast::<u16>(),
                0,
                SORT_KEY_COUNT,
            );
            let sort_events = std::ptr::addr_of_mut!((*plan).sort_events).cast::<FrameEvent>();
            for event_index in 0..TIMELINE_ENDPOINT_BATCH_MAX_EVENTS {
                sort_events.add(event_index).write(EMPTY_EVENT);
            }
            let sort_classes =
                std::ptr::addr_of_mut!((*plan).sort_classes).cast::<EndpointEventClass>();
            for class_index in 0..TIMELINE_ENDPOINT_BATCH_MAX_EVENTS {
                sort_classes
                    .add(class_index)
                    .write(EndpointEventClass::System);
            }
            storage.assume_init()
        }
    }

    pub fn reset(&mut self) {
        for endpoint in &mut self.endpoints[..usize::from(self.endpoint_count)] {
            endpoint.reset();
        }
        self.endpoint_count = 0;
        self.prepared = false;
    }

    pub fn abort(&mut self) {
        self.reset();
    }

    pub const fn endpoint_count(&self) -> usize {
        self.endpoint_count as usize
    }

    pub const fn is_prepared(&self) -> bool {
        self.prepared
    }

    pub fn register_endpoint(
        &mut self,
        key: TimelineEndpointKey,
        phase: usize,
        frames: usize,
    ) -> Result<TimelineEndpointHandle, TimelineEndpointBatchError> {
        self.register_endpoint_with_seed(
            key,
            phase,
            frames,
            TimelineEndpointQuantumUsage::default(),
        )
    }

    /// Register an endpoint whose first physical quantum may already contain events from the
    /// preceding callback. Seeded events consume first-bucket class/physical capacity, but are
    /// intentionally absent from this callback's event slices and callback quotas.
    pub fn register_endpoint_with_seed(
        &mut self,
        key: TimelineEndpointKey,
        phase: usize,
        frames: usize,
        first_quantum_seed: TimelineEndpointQuantumUsage,
    ) -> Result<TimelineEndpointHandle, TimelineEndpointBatchError> {
        self.require_building()?;
        if phase >= TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES {
            return Err(TimelineEndpointBatchError::InvalidPhase { phase });
        }
        if !(1..=TIMELINE_ENDPOINT_BATCH_MAX_FRAMES).contains(&frames) {
            return Err(TimelineEndpointBatchError::InvalidFrameCount { frames });
        }
        Self::validate_first_quantum_seed(phase, first_quantum_seed)?;
        if self.endpoints[..usize::from(self.endpoint_count)]
            .iter()
            .any(|endpoint| endpoint.identity.key == key)
        {
            return Err(TimelineEndpointBatchError::DuplicateEndpoint(key));
        }
        if usize::from(self.endpoint_count) == TIMELINE_ENDPOINT_BATCH_MAX_ENDPOINTS {
            return Err(TimelineEndpointBatchError::EndpointCapacityExceeded);
        }

        let handle = TimelineEndpointHandle(self.endpoint_count);
        let endpoint = &mut self.endpoints[handle.index()];
        endpoint.reset();
        endpoint.identity = TimelineEndpointIdentity {
            key,
            phase: phase as u8,
            frames: frames as u16,
            quantum_bucket_count: (phase + frames).div_ceil(TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES)
                as u8,
        };
        endpoint.first_quantum_seed = first_quantum_seed;
        endpoint.quantum_usage[0] = first_quantum_seed;
        self.endpoint_count += 1;
        Ok(handle)
    }

    /// Push a non-parameter endpoint event. Parameters must use [`Self::push_parameter_q128`].
    pub fn push_event(
        &mut self,
        endpoint: TimelineEndpointHandle,
        class: EndpointEventClass,
        event: FrameEvent,
    ) -> Result<(), TimelineEndpointBatchError> {
        if matches!(event.kind, FrameEventKind::Parameter { .. }) {
            return Err(TimelineEndpointBatchError::ParameterRequiresQ128Push);
        }
        if class == EndpointEventClass::System && !is_all_notes_off(event) {
            return Err(TimelineEndpointBatchError::InvalidSystemEvent);
        }
        self.push_checked(endpoint, class, event)
    }

    /// Push the only event admitted to the protected system lane: MIDI CC123 value zero.
    pub fn push_all_notes_off(
        &mut self,
        endpoint: TimelineEndpointHandle,
        sample_offset: u16,
        channel: u8,
    ) -> Result<(), TimelineEndpointBatchError> {
        if channel > 15 {
            return Err(TimelineEndpointBatchError::InvalidMidiChannel { channel });
        }
        self.push_event(
            endpoint,
            EndpointEventClass::System,
            FrameEvent::midi(sample_offset, None, [0xB0 | channel, 123, 0]),
        )
    }

    /// Push a slot-zero parameter command exactly at a physical Q128 boundary.
    /// This compatibility entry point is used by one-slot generator chains.
    pub fn push_parameter_q128(
        &mut self,
        endpoint: TimelineEndpointHandle,
        sample_offset: u16,
        id: u32,
        normalized: f32,
    ) -> Result<(), TimelineEndpointBatchError> {
        self.push_parameter_at_slot_q128(endpoint, sample_offset, 0, id, normalized)
    }

    /// Push one serial-chain slot parameter at a physical Q128 boundary.
    /// Every slot shares the registered endpoint's class and physical quotas.
    pub fn push_parameter_at_slot_q128(
        &mut self,
        endpoint: TimelineEndpointHandle,
        sample_offset: u16,
        slot: u8,
        id: u32,
        normalized: f32,
    ) -> Result<(), TimelineEndpointBatchError> {
        self.require_building()?;
        if usize::from(slot) >= MAX_PLUGIN_CHAIN_SLOTS {
            return Err(TimelineEndpointBatchError::InvalidParameterSlot { slot });
        }
        let candidate = self
            .endpoints
            .get(endpoint.index())
            .filter(|_| endpoint.index() < usize::from(self.endpoint_count))
            .ok_or(TimelineEndpointBatchError::InvalidEndpointHandle)?;
        if !normalized.is_finite() {
            return Err(TimelineEndpointBatchError::NonFiniteParameter);
        }
        if (usize::from(candidate.identity.phase) + usize::from(sample_offset))
            % TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES
            != 0
        {
            return Err(TimelineEndpointBatchError::ParameterNotOnQuantumBoundary {
                sample_offset,
                phase: candidate.identity.phase,
            });
        }
        self.push_checked(
            endpoint,
            EndpointEventClass::Timeline,
            FrameEvent::parameter(sample_offset, slot, id, normalized.clamp(0.0, 1.0)),
        )
    }

    /// Reserve one reliable, already-admitted edit in the protected Live lane of the physical
    /// Q128 quantum currently being accumulated. Admission remains owned by the endpoint; this
    /// marker supplies deterministic Timeline-before-Live ordering and the callback/quantum
    /// capacity proof even when the device callback begins part-way through that quantum.
    pub fn push_admitted_live_parameter_reservation(
        &mut self,
        endpoint: TimelineEndpointHandle,
        sample_offset: u16,
        slot: u8,
        id: u32,
        normalized: f32,
        edit_id: ParameterEditId,
    ) -> Result<(), TimelineEndpointBatchError> {
        self.require_building()?;
        if usize::from(slot) >= MAX_PLUGIN_CHAIN_SLOTS {
            return Err(TimelineEndpointBatchError::InvalidParameterSlot { slot });
        }
        let candidate = self
            .endpoints
            .get(endpoint.index())
            .filter(|_| endpoint.index() < usize::from(self.endpoint_count))
            .ok_or(TimelineEndpointBatchError::InvalidEndpointHandle)?;
        if !normalized.is_finite() {
            return Err(TimelineEndpointBatchError::NonFiniteParameter);
        }
        let _ = candidate;
        self.push_checked(
            endpoint,
            EndpointEventClass::Live,
            FrameEvent::admitted_parameter(
                sample_offset,
                slot,
                id,
                normalized.clamp(0.0, 1.0),
                edit_id,
            ),
        )
    }

    /// Validate all quotas and create ordered read-only slices without allocation.
    pub fn preflight(&mut self) -> Result<(), TimelineEndpointBatchError> {
        self.require_building()?;
        for index in 0..usize::from(self.endpoint_count) {
            self.validate_endpoint(index)?;
        }
        for index in 0..usize::from(self.endpoint_count) {
            self.order_endpoint(index);
        }
        self.prepared = true;
        Ok(())
    }

    /// Exact endpoint identity is available before runtime commit and before preflight.
    pub fn endpoint_identity(
        &self,
        endpoint: TimelineEndpointHandle,
    ) -> Option<TimelineEndpointIdentity> {
        (endpoint.index() < usize::from(self.endpoint_count))
            .then_some(self.endpoints[endpoint.index()].identity)
    }

    pub fn endpoint_identity_at(&self, index: usize) -> Option<TimelineEndpointIdentity> {
        (index < usize::from(self.endpoint_count)).then_some(self.endpoints[index].identity)
    }

    pub fn prepared_endpoint(
        &self,
        endpoint: TimelineEndpointHandle,
    ) -> Option<PreparedTimelineEndpoint<'_>> {
        self.prepared_endpoint_at(endpoint.index())
    }

    pub fn prepared_endpoint_at(&self, index: usize) -> Option<PreparedTimelineEndpoint<'_>> {
        if !self.prepared || index >= usize::from(self.endpoint_count) {
            return None;
        }
        let endpoint = &self.endpoints[index];
        let event_count = usize::from(endpoint.event_count);
        let bucket_count = usize::from(endpoint.identity.quantum_bucket_count);
        Some(PreparedTimelineEndpoint {
            identity: endpoint.identity,
            events: &endpoint.events[..event_count],
            classes: &endpoint.classes[..event_count],
            quantum_usage: &endpoint.quantum_usage[..bucket_count],
        })
    }

    fn require_building(&self) -> Result<(), TimelineEndpointBatchError> {
        if self.prepared {
            Err(TimelineEndpointBatchError::BatchAlreadyPrepared)
        } else {
            Ok(())
        }
    }

    fn validate_first_quantum_seed(
        phase: usize,
        seed: TimelineEndpointQuantumUsage,
    ) -> Result<(), TimelineEndpointBatchError> {
        if phase == 0 && !seed.is_empty() {
            return Err(TimelineEndpointBatchError::NonEmptySeedAtQuantumBoundary);
        }
        let class_total = u32::from(seed.system) + u32::from(seed.timeline) + u32::from(seed.live);
        if class_total != u32::from(seed.total) {
            return Err(TimelineEndpointBatchError::InvalidFirstQuantumUsage);
        }
        if usize::from(seed.system) > SYSTEM_MAX_EVENTS_PER_QUANTUM {
            return Err(TimelineEndpointBatchError::SystemQuantumCapacityExceeded { quantum: 0 });
        }
        if usize::from(seed.timeline) > TIMELINE_MAX_EVENTS_PER_QUANTUM {
            return Err(TimelineEndpointBatchError::TimelineQuantumCapacityExceeded { quantum: 0 });
        }
        if usize::from(seed.live) > LIVE_MAX_EVENTS_PER_QUANTUM {
            return Err(TimelineEndpointBatchError::LiveQuantumCapacityExceeded { quantum: 0 });
        }
        if usize::from(seed.total) > PHYSICAL_MAX_EVENTS_PER_QUANTUM {
            return Err(TimelineEndpointBatchError::PhysicalQuantumCapacityExceeded { quantum: 0 });
        }
        Ok(())
    }

    fn push_checked(
        &mut self,
        handle: TimelineEndpointHandle,
        class: EndpointEventClass,
        event: FrameEvent,
    ) -> Result<(), TimelineEndpointBatchError> {
        self.require_building()?;
        if handle.index() >= usize::from(self.endpoint_count) {
            return Err(TimelineEndpointBatchError::InvalidEndpointHandle);
        }
        let endpoint = &mut self.endpoints[handle.index()];
        if usize::from(event.sample_offset) >= usize::from(endpoint.identity.frames) {
            return Err(TimelineEndpointBatchError::EventOutsideCallback {
                sample_offset: event.sample_offset,
                frames: endpoint.identity.frames,
            });
        }
        let quantum = (usize::from(endpoint.identity.phase) + usize::from(event.sample_offset))
            / TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES;
        let usage = endpoint.quantum_usage[quantum];
        match class {
            EndpointEventClass::System
                if usize::from(endpoint.system_count) == SYSTEM_MAX_EVENTS_PER_CALLBACK =>
            {
                return Err(TimelineEndpointBatchError::SystemCallbackCapacityExceeded);
            }
            EndpointEventClass::System
                if usize::from(usage.system) == SYSTEM_MAX_EVENTS_PER_QUANTUM =>
            {
                return Err(TimelineEndpointBatchError::SystemQuantumCapacityExceeded {
                    quantum: quantum as u8,
                });
            }
            EndpointEventClass::Timeline
                if usize::from(endpoint.timeline_count) == TIMELINE_MAX_EVENTS_PER_CALLBACK =>
            {
                return Err(TimelineEndpointBatchError::TimelineCallbackCapacityExceeded);
            }
            EndpointEventClass::Timeline
                if usize::from(usage.timeline) == TIMELINE_MAX_EVENTS_PER_QUANTUM =>
            {
                return Err(
                    TimelineEndpointBatchError::TimelineQuantumCapacityExceeded {
                        quantum: quantum as u8,
                    },
                );
            }
            EndpointEventClass::Live
                if usize::from(endpoint.live_count) == LIVE_MAX_EVENTS_PER_CALLBACK =>
            {
                return Err(TimelineEndpointBatchError::LiveCallbackCapacityExceeded);
            }
            EndpointEventClass::Live if usize::from(usage.live) == LIVE_MAX_EVENTS_PER_QUANTUM => {
                return Err(TimelineEndpointBatchError::LiveQuantumCapacityExceeded {
                    quantum: quantum as u8,
                });
            }
            _ => {}
        }
        if usize::from(usage.total) == PHYSICAL_MAX_EVENTS_PER_QUANTUM {
            return Err(
                TimelineEndpointBatchError::PhysicalQuantumCapacityExceeded {
                    quantum: quantum as u8,
                },
            );
        }
        if usize::from(endpoint.event_count) == TIMELINE_ENDPOINT_BATCH_MAX_EVENTS {
            return Err(TimelineEndpointBatchError::CallbackCapacityExceeded);
        }

        let event_index = usize::from(endpoint.event_count);
        endpoint.events[event_index] = event;
        endpoint.classes[event_index] = class;
        endpoint.event_count += 1;
        let usage = &mut endpoint.quantum_usage[quantum];
        usage.total += 1;
        match class {
            EndpointEventClass::System => {
                endpoint.system_count += 1;
                usage.system += 1;
            }
            EndpointEventClass::Timeline => {
                endpoint.timeline_count += 1;
                usage.timeline += 1;
            }
            EndpointEventClass::Live => {
                endpoint.live_count += 1;
                usage.live += 1;
            }
        }
        Ok(())
    }

    fn validate_endpoint(&self, index: usize) -> Result<(), TimelineEndpointBatchError> {
        let endpoint = &self.endpoints[index];
        if usize::from(endpoint.event_count)
            != usize::from(endpoint.system_count)
                + usize::from(endpoint.timeline_count)
                + usize::from(endpoint.live_count)
        {
            return Err(TimelineEndpointBatchError::CorruptCandidate);
        }
        let mut expected_usage =
            [TimelineEndpointQuantumUsage::default(); TIMELINE_ENDPOINT_BATCH_MAX_QUANTA];
        expected_usage[0] = endpoint.first_quantum_seed;
        for event_index in 0..usize::from(endpoint.event_count) {
            let event = endpoint.events[event_index];
            let class = endpoint.classes[event_index];
            if usize::from(event.sample_offset) >= usize::from(endpoint.identity.frames) {
                return Err(TimelineEndpointBatchError::CorruptCandidate);
            }
            if class == EndpointEventClass::System && !is_all_notes_off(event) {
                return Err(TimelineEndpointBatchError::CorruptCandidate);
            }
            let invalid_parameter = match event.kind {
                FrameEventKind::Parameter {
                    slot,
                    normalized,
                    edit_id,
                    ..
                } => {
                    class
                        != if edit_id.is_some() {
                            EndpointEventClass::Live
                        } else {
                            EndpointEventClass::Timeline
                        }
                        || usize::from(slot) >= MAX_PLUGIN_CHAIN_SLOTS
                        || !normalized.is_finite()
                        || !(0.0..=1.0).contains(&normalized)
                        || (edit_id.is_none()
                            && (usize::from(endpoint.identity.phase)
                                + usize::from(event.sample_offset))
                                % TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES
                                != 0)
                }
                FrameEventKind::Midi { .. } => false,
            };
            if invalid_parameter {
                return Err(TimelineEndpointBatchError::CorruptCandidate);
            }
            let quantum = (usize::from(endpoint.identity.phase) + usize::from(event.sample_offset))
                / TIMELINE_ENDPOINT_BATCH_QUANTUM_FRAMES;
            let usage = &mut expected_usage[quantum];
            usage.total += 1;
            match class {
                EndpointEventClass::System => usage.system += 1,
                EndpointEventClass::Timeline => usage.timeline += 1,
                EndpointEventClass::Live => usage.live += 1,
            }
        }
        let bucket_count = usize::from(endpoint.identity.quantum_bucket_count);
        if expected_usage[..bucket_count] != endpoint.quantum_usage[..bucket_count] {
            return Err(TimelineEndpointBatchError::CorruptCandidate);
        }
        Ok(())
    }

    fn order_endpoint(&mut self, index: usize) {
        let endpoint = &mut self.endpoints[index];
        let event_count = usize::from(endpoint.event_count);
        if event_count < 2 {
            return;
        }
        let key_count = usize::from(endpoint.identity.frames) * SORT_RANK_COUNT;
        self.sort_counts[..key_count].fill(0);
        for event_index in 0..event_count {
            let event = endpoint.events[event_index];
            let key = usize::from(event.sample_offset) * SORT_RANK_COUNT
                + endpoint.classes[event_index].sort_rank(event);
            self.sort_counts[key] += 1;
        }
        let mut end = 0_u16;
        for count in &mut self.sort_counts[..key_count] {
            end += *count;
            *count = end;
        }
        // Reverse stable scatter lets each prefix-end counter double as the insertion cursor.
        for event_index in (0..event_count).rev() {
            let event = endpoint.events[event_index];
            let class = endpoint.classes[event_index];
            let key = usize::from(event.sample_offset) * SORT_RANK_COUNT + class.sort_rank(event);
            self.sort_counts[key] -= 1;
            let destination = usize::from(self.sort_counts[key]);
            self.sort_events[destination] = event;
            self.sort_classes[destination] = class;
        }
        endpoint.events[..event_count].copy_from_slice(&self.sort_events[..event_count]);
        endpoint.classes[..event_count].copy_from_slice(&self.sort_classes[..event_count]);

        // A physical mixer endpoint may carry parameters for several serial
        // slots. The compiled capacity contract limits each endpoint to eight
        // driven parameters, so bounded insertion sorting of each equal-offset
        // parameter run gives a canonical `(slot, id)` order without enlarging
        // the counting-sort key space or allocating on the callback.
        let mut run_start = 0_usize;
        while run_start < event_count {
            let event = endpoint.events[run_start];
            if !matches!(event.kind, FrameEventKind::Parameter { .. }) {
                run_start += 1;
                continue;
            }
            let offset = event.sample_offset;
            let mut run_end = run_start + 1;
            while run_end < event_count
                && endpoint.events[run_end].sample_offset == offset
                && matches!(
                    endpoint.events[run_end].kind,
                    FrameEventKind::Parameter { .. }
                )
            {
                run_end += 1;
            }
            for index in run_start + 1..run_end {
                let event = endpoint.events[index];
                let class = endpoint.classes[index];
                let key = parameter_sort_key(event);
                let mut destination = index;
                while destination > run_start
                    && parameter_sort_key(endpoint.events[destination - 1]) > key
                {
                    endpoint.events[destination] = endpoint.events[destination - 1];
                    endpoint.classes[destination] = endpoint.classes[destination - 1];
                    destination -= 1;
                }
                endpoint.events[destination] = event;
                endpoint.classes[destination] = class;
            }
            run_start = run_end;
        }
    }
}

fn parameter_sort_key(event: FrameEvent) -> (u8, u32) {
    match event.kind {
        FrameEventKind::Parameter { slot, id, .. } => (slot, id),
        FrameEventKind::Midi { .. } => (u8::MAX, u32::MAX),
    }
}

fn is_all_notes_off(event: FrameEvent) -> bool {
    matches!(
        event.kind,
        FrameEventKind::Midi {
            slot: None,
            data: [status, 123, 0],
        } if status & 0xF0 == 0xB0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(channel_id: u32) -> TimelineEndpointKey {
        TimelineEndpointKey::new(
            channel_id,
            u64::from(channel_id) + 100,
            u64::from(channel_id) + 200,
        )
    }

    #[test]
    fn boxed_constructor_initializes_and_drops_on_a_small_stack() {
        const TEST_STACK_BYTES: usize = 128 * 1_024;
        assert!(std::mem::size_of::<TimelineEndpointBatchPlan>() > TEST_STACK_BYTES);
        std::thread::Builder::new()
            .name("timeline-batch-small-stack".to_owned())
            .stack_size(TEST_STACK_BYTES)
            .spawn(|| {
                let mut plan = TimelineEndpointBatchPlan::new_boxed();
                assert_eq!(plan.endpoint_count(), 0);
                assert!(!plan.is_prepared());
                let endpoint = plan.register_endpoint(key(1), 0, 128).unwrap();
                plan.push_parameter_q128(endpoint, 0, 7, 0.5).unwrap();
                plan.preflight().unwrap();
                assert_eq!(plan.prepared_endpoint(endpoint).unwrap().events().len(), 1);
                // Dropping at the end of the closure exercises the fully initialized allocation.
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn physical_endpoint_plan_accepts_64_generators_plus_32_mixer_chains_only() {
        let mut plan = TimelineEndpointBatchPlan::new_boxed();
        for channel_id in 1..=64 {
            plan.register_endpoint(key(channel_id), 0, 128).unwrap();
        }
        for track in 0..32_u8 {
            plan.register_endpoint(
                TimelineEndpointKey::mixer_insert(track, 10_000 + u64::from(track)),
                0,
                128,
            )
            .unwrap();
        }
        assert_eq!(plan.endpoint_count(), 96);
        assert_eq!(
            plan.register_endpoint(key(65), 0, 128),
            Err(TimelineEndpointBatchError::EndpointCapacityExceeded)
        );
        plan.preflight().unwrap();
    }

    #[test]
    fn phase_127_exposes_seventeen_physical_buckets() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let endpoint = plan.register_endpoint(key(1), 127, 2_048).unwrap();
        plan.push_event(
            endpoint,
            EndpointEventClass::System,
            FrameEvent::midi(0, None, [0xB0, 123, 0]),
        )
        .unwrap();
        plan.push_event(
            endpoint,
            EndpointEventClass::Live,
            FrameEvent::midi(2_047, Some(0), [0x90, 60, 100]),
        )
        .unwrap();
        plan.preflight().unwrap();
        let prepared = plan.prepared_endpoint(endpoint).unwrap();
        assert_eq!(prepared.quantum_usage().len(), 17);
        assert_eq!(prepared.quantum_usage()[0].total, 1);
        assert_eq!(prepared.quantum_usage()[16].total, 1);
    }

    #[test]
    fn q128_parameters_are_slot_zero_aligned_finite_and_clamped() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let endpoint = plan.register_endpoint(key(1), 64, 256).unwrap();
        assert_eq!(
            plan.push_parameter_q128(endpoint, 0, 7, 0.5),
            Err(TimelineEndpointBatchError::ParameterNotOnQuantumBoundary {
                sample_offset: 0,
                phase: 64,
            })
        );
        assert_eq!(
            plan.push_parameter_q128(endpoint, 64, 7, f32::NAN),
            Err(TimelineEndpointBatchError::NonFiniteParameter)
        );
        plan.push_parameter_q128(endpoint, 64, 7, 3.0).unwrap();
        plan.preflight().unwrap();
        assert_eq!(
            plan.prepared_endpoint(endpoint).unwrap().events()[0],
            FrameEvent::parameter(64, 0, 7, 1.0)
        );
    }

    #[test]
    fn admitted_live_edit_reserves_the_current_partial_quantum_without_borrowing() {
        let edit = ParameterEditId::new(41).unwrap();
        let mut plan = TimelineEndpointBatchPlan::new();
        let accepted = plan
            .register_endpoint_with_seed(
                key(1),
                64,
                1,
                TimelineEndpointQuantumUsage::from_class_counts(0, 0, 15),
            )
            .unwrap();
        plan.push_admitted_live_parameter_reservation(accepted, 0, 0, 7, 0.5, edit)
            .unwrap();
        plan.preflight().unwrap();
        let prepared = plan.prepared_endpoint(accepted).unwrap();
        assert_eq!(prepared.quantum_usage()[0].live, 16);
        assert!(matches!(
            prepared.events()[0].kind,
            FrameEventKind::Parameter {
                edit_id: Some(actual),
                ..
            } if actual == edit
        ));

        plan.reset();
        let rejected = plan
            .register_endpoint_with_seed(
                key(1),
                64,
                1,
                TimelineEndpointQuantumUsage::from_class_counts(0, 0, 16),
            )
            .unwrap();
        assert_eq!(
            plan.push_admitted_live_parameter_reservation(rejected, 0, 0, 7, 0.5, edit),
            Err(TimelineEndpointBatchError::LiveQuantumCapacityExceeded { quantum: 0 })
        );
    }

    #[test]
    fn equal_offset_admitted_live_edit_sorts_after_timeline_parameter_and_midi() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let endpoint = plan.register_endpoint(key(1), 0, 1).unwrap();
        plan.push_admitted_live_parameter_reservation(
            endpoint,
            0,
            0,
            8,
            0.75,
            ParameterEditId::new(42).unwrap(),
        )
        .unwrap();
        plan.push_event(
            endpoint,
            EndpointEventClass::Timeline,
            FrameEvent::midi(0, None, [0x90, 60, 1]),
        )
        .unwrap();
        plan.push_parameter_q128(endpoint, 0, 7, 0.5).unwrap();
        plan.preflight().unwrap();
        let prepared = plan.prepared_endpoint(endpoint).unwrap();
        assert_eq!(
            prepared.classes(),
            &[
                EndpointEventClass::Timeline,
                EndpointEventClass::Timeline,
                EndpointEventClass::Live,
            ]
        );
        assert!(matches!(
            prepared.events()[0].kind,
            FrameEventKind::Parameter { edit_id: None, .. }
        ));
        assert!(matches!(
            prepared.events()[1].kind,
            FrameEventKind::Midi { .. }
        ));
        assert!(matches!(
            prepared.events()[2].kind,
            FrameEventKind::Parameter {
                edit_id: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn mixer_slots_share_one_physical_endpoint_and_validate_the_slot_bound() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let key = TimelineEndpointKey::mixer_insert(31, 9_001);
        let endpoint = plan.register_endpoint(key, 0, 128).unwrap();
        plan.push_parameter_at_slot_q128(endpoint, 0, 9, 7, 0.25)
            .unwrap();
        plan.push_parameter_at_slot_q128(endpoint, 0, 0, 7, 0.75)
            .unwrap();
        assert_eq!(
            plan.register_endpoint(key, 0, 128),
            Err(TimelineEndpointBatchError::DuplicateEndpoint(key))
        );
        assert_eq!(
            plan.push_parameter_at_slot_q128(endpoint, 0, 10, 7, 0.5),
            Err(TimelineEndpointBatchError::InvalidParameterSlot { slot: 10 })
        );
        plan.preflight().unwrap();
        let prepared = plan.prepared_endpoint(endpoint).unwrap();
        assert_eq!(prepared.identity().key.mixer_track(), Some(31));
        assert_eq!(prepared.events().len(), 2);
        assert!(matches!(
            prepared.events()[0].kind,
            FrameEventKind::Parameter { slot: 0, id: 7, .. }
        ));
        assert!(matches!(
            prepared.events()[1].kind,
            FrameEventKind::Parameter { slot: 9, id: 7, .. }
        ));
    }

    #[test]
    fn eight_mixer_targets_share_one_q128_quota_and_the_ninth_is_rejected() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let endpoint = plan
            .register_endpoint(TimelineEndpointKey::mixer_insert(7, 7_007), 0, 128)
            .unwrap();
        for slot in (0..8_u8).rev() {
            plan.push_parameter_at_slot_q128(
                endpoint,
                0,
                slot,
                100 + u32::from(slot),
                f32::from(slot) / 8.0,
            )
            .unwrap();
        }
        for note in 0..88_u8 {
            plan.push_event(
                endpoint,
                EndpointEventClass::Timeline,
                FrameEvent::midi(0, Some(0), [0x90, note, 1]),
            )
            .unwrap();
        }
        assert_eq!(
            plan.push_parameter_at_slot_q128(endpoint, 0, 8, 108, 1.0),
            Err(TimelineEndpointBatchError::TimelineQuantumCapacityExceeded { quantum: 0 })
        );
        plan.preflight().unwrap();
        let prepared = plan.prepared_endpoint(endpoint).unwrap();
        assert_eq!(prepared.events().len(), 96);
        for (slot, event) in prepared.events()[..8].iter().enumerate() {
            assert!(matches!(
                event.kind,
                FrameEventKind::Parameter {
                    slot: actual_slot,
                    id,
                    ..
                } if usize::from(actual_slot) == slot && id == 100 + slot as u32
            ));
        }
    }

    #[test]
    fn class_quotas_do_not_borrow_and_equal_offsets_have_priority_order() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let endpoint = plan.register_endpoint(key(1), 0, 256).unwrap();
        for note in 0..LIVE_MAX_EVENTS_PER_CALLBACK {
            plan.push_event(
                endpoint,
                EndpointEventClass::Live,
                FrameEvent::midi(0, None, [0x90, note as u8, 1]),
            )
            .unwrap();
        }
        assert_eq!(
            plan.push_event(
                endpoint,
                EndpointEventClass::Live,
                FrameEvent::midi(128, None, [0x90, 99, 1]),
            ),
            Err(TimelineEndpointBatchError::LiveCallbackCapacityExceeded)
        );
        plan.push_event(
            endpoint,
            EndpointEventClass::Timeline,
            FrameEvent::midi(0, None, [0x90, 100, 1]),
        )
        .unwrap();
        plan.push_parameter_q128(endpoint, 0, 7, 0.5).unwrap();
        plan.push_event(
            endpoint,
            EndpointEventClass::System,
            FrameEvent::midi(0, None, [0xB0, 123, 0]),
        )
        .unwrap();
        plan.preflight().unwrap();
        let prepared = plan.prepared_endpoint(endpoint).unwrap();
        assert_eq!(prepared.classes()[0], EndpointEventClass::System);
        assert_eq!(prepared.classes()[1], EndpointEventClass::Timeline);
        assert!(matches!(
            prepared.events()[1].kind,
            FrameEventKind::Parameter { .. }
        ));
        assert_eq!(prepared.classes()[2], EndpointEventClass::Timeline);
        assert!(matches!(
            prepared.events()[2].kind,
            FrameEventKind::Midi { .. }
        ));
        assert!(
            prepared.classes()[3..]
                .iter()
                .all(|class| *class == EndpointEventClass::Live)
        );
    }

    #[test]
    fn exact_callback_capacity_is_accepted_without_borrowing_reserves() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let endpoint = plan.register_endpoint(key(1), 0, 512).unwrap();
        for _ in 0..SYSTEM_MAX_EVENTS_PER_CALLBACK {
            plan.push_event(
                endpoint,
                EndpointEventClass::System,
                FrameEvent::midi(0, None, [0xB0, 123, 0]),
            )
            .unwrap();
        }
        for index in 0..TIMELINE_MAX_EVENTS_PER_CALLBACK {
            let offset = match index {
                0..96 => 0,
                96..192 => 128,
                _ => 256,
            };
            plan.push_event(
                endpoint,
                EndpointEventClass::Timeline,
                FrameEvent::midi(offset, Some(0), [0x90, (index % 128) as u8, 1]),
            )
            .unwrap();
        }
        for _ in 0..LIVE_MAX_EVENTS_PER_CALLBACK {
            plan.push_event(
                endpoint,
                EndpointEventClass::Live,
                FrameEvent::midi(256, Some(0), [0x90, 60, 1]),
            )
            .unwrap();
        }
        assert_eq!(
            plan.push_event(
                endpoint,
                EndpointEventClass::Timeline,
                FrameEvent::midi(384, Some(0), [0x90, 60, 1]),
            ),
            Err(TimelineEndpointBatchError::TimelineCallbackCapacityExceeded)
        );
        plan.preflight().unwrap();
        let prepared = plan.prepared_endpoint(endpoint).unwrap();
        assert_eq!(prepared.events().len(), TIMELINE_ENDPOINT_BATCH_MAX_EVENTS);
        assert_eq!(prepared.quantum_usage()[0].total, 112);
        assert_eq!(prepared.quantum_usage()[1].timeline, 96);
        assert_eq!(prepared.quantum_usage()[2].total, 48);
    }

    #[test]
    fn endpoints_are_isolated_and_abort_makes_storage_reusable() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let first = plan.register_endpoint(key(1), 0, 128).unwrap();
        let second = plan.register_endpoint(key(2), 127, 128).unwrap();
        plan.push_event(
            first,
            EndpointEventClass::System,
            FrameEvent::midi(0, None, [0xB0, 123, 0]),
        )
        .unwrap();
        plan.push_parameter_q128(second, 1, 42, 0.25).unwrap();
        plan.preflight().unwrap();
        assert_eq!(
            plan.prepared_endpoint(first).unwrap().identity().key,
            key(1)
        );
        assert_eq!(
            plan.prepared_endpoint(second).unwrap().identity().key,
            key(2)
        );

        plan.abort();
        assert_eq!(plan.endpoint_count(), 0);
        assert!(!plan.is_prepared());
        let reused = plan.register_endpoint(key(1), 64, 128).unwrap();
        plan.push_parameter_q128(reused, 64, 7, 0.5).unwrap();
        plan.preflight().unwrap();
        assert_eq!(plan.prepared_endpoint(reused).unwrap().events().len(), 1);
    }

    #[test]
    fn callback_splits_preserve_absolute_q128_parameter_boundaries() {
        let callbacks = [(0_usize, 64_usize, 0_u16), (64, 100, 64), (164, 93, 92)];
        let mut plan = TimelineEndpointBatchPlan::new();
        let mut absolute_boundaries = Vec::new();
        for (start, frames, offset) in callbacks {
            plan.reset();
            let endpoint = plan.register_endpoint(key(1), start % 128, frames).unwrap();
            plan.push_parameter_q128(endpoint, offset, 9, 0.5).unwrap();
            plan.preflight().unwrap();
            let event = plan.prepared_endpoint(endpoint).unwrap().events()[0];
            absolute_boundaries.push(start + usize::from(event.sample_offset));
        }
        assert_eq!(absolute_boundaries, [0, 128, 256]);
    }

    #[test]
    fn first_quantum_seed_carries_timeline_usage_across_callbacks() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let accepted = plan
            .register_endpoint_with_seed(
                key(1),
                64,
                64,
                TimelineEndpointQuantumUsage::from_class_counts(0, 95, 0),
            )
            .unwrap();
        plan.push_event(
            accepted,
            EndpointEventClass::Timeline,
            FrameEvent::midi(0, None, [0x90, 60, 1]),
        )
        .unwrap();
        plan.preflight().unwrap();
        let accepted = plan.prepared_endpoint(accepted).unwrap();
        assert_eq!(accepted.events().len(), 1);
        assert_eq!(accepted.quantum_usage()[0].timeline, 96);

        plan.reset();
        let rejected = plan
            .register_endpoint_with_seed(
                key(1),
                64,
                64,
                TimelineEndpointQuantumUsage::from_class_counts(0, 96, 0),
            )
            .unwrap();
        assert_eq!(
            plan.push_event(
                rejected,
                EndpointEventClass::Timeline,
                FrameEvent::midi(0, None, [0x90, 60, 1]),
            ),
            Err(TimelineEndpointBatchError::TimelineQuantumCapacityExceeded { quantum: 0 })
        );
    }

    #[test]
    fn quantum_boundary_rejects_seed_and_system_lane_rejects_spoofing() {
        let mut plan = TimelineEndpointBatchPlan::new();
        assert_eq!(
            plan.register_endpoint_with_seed(
                key(1),
                0,
                128,
                TimelineEndpointQuantumUsage::from_class_counts(1, 0, 0),
            ),
            Err(TimelineEndpointBatchError::NonEmptySeedAtQuantumBoundary)
        );
        assert_eq!(
            plan.register_endpoint_with_seed(
                key(1),
                1,
                128,
                TimelineEndpointQuantumUsage::from_class_counts(17, 0, 0),
            ),
            Err(TimelineEndpointBatchError::SystemQuantumCapacityExceeded { quantum: 0 })
        );
        let endpoint = plan.register_endpoint(key(1), 0, 128).unwrap();
        assert_eq!(
            plan.push_event(
                endpoint,
                EndpointEventClass::System,
                FrameEvent::midi(0, None, [0x90, 60, 1]),
            ),
            Err(TimelineEndpointBatchError::InvalidSystemEvent)
        );
        assert_eq!(
            plan.push_all_notes_off(endpoint, 0, 16),
            Err(TimelineEndpointBatchError::InvalidMidiChannel { channel: 16 })
        );
        plan.push_all_notes_off(endpoint, 0, 15).unwrap();
    }

    #[test]
    fn system_seed_fills_only_the_partial_physical_quantum() {
        let mut plan = TimelineEndpointBatchPlan::new();
        let endpoint = plan
            .register_endpoint_with_seed(
                key(1),
                64,
                192,
                TimelineEndpointQuantumUsage::from_class_counts(16, 0, 0),
            )
            .unwrap();
        assert_eq!(
            plan.push_all_notes_off(endpoint, 0, 0),
            Err(TimelineEndpointBatchError::SystemQuantumCapacityExceeded { quantum: 0 })
        );
        plan.push_all_notes_off(endpoint, 64, 0).unwrap();
        plan.preflight().unwrap();
        let usage = plan.prepared_endpoint(endpoint).unwrap().quantum_usage();
        assert_eq!(usage[0].system, 16);
        assert_eq!(usage[1].system, 1);
    }
}
