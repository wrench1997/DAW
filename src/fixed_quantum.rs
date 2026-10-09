//! Callback-safe adaptation from arbitrary device callback sizes to fixed plug-in quanta.
//!
//! [`AudioThreadEndpoint`] deliberately matches one submitted block with the immediately
//! preceding block.  Device callbacks, however, are not guaranteed to have a stable length. This
//! adapter accumulates the input stream into a fixed power-of-two quantum and drains the returned
//! stream through a fixed ring.  Its timeline is intentionally conservative and invariant under
//! callback splitting: one quantum of accumulation pre-roll plus the endpoint's one-quantum
//! bridge delay, for an exact total of `2 * quantum_frames`.
//!
//! Construction (including the boxed scratch allocation) belongs on the control thread. The two
//! processing methods allocate nothing, lock nothing and perform no I/O.

use crate::plugins::plugin_runtime::{
    AudioThreadEndpoint, BridgeStats, MAX_PLUGIN_BLOCK_FRAMES, MidiMessage, ParameterEditId,
    PluginEndpointManifest, PluginEndpointSnapshot, PluginLatencySnapshot, PluginMidiBatch,
    PluginMidiOutput, PluginTransport, RealtimeOutputSource, RealtimeProcessStatus, SubmitStatus,
};
use crate::timeline::{
    TIMELINE_ENDPOINT_MAX_EVENTS_PER_CALLBACK, TIMELINE_ENDPOINT_MAX_EVENTS_PER_QUANTUM,
};

/// Largest device callback accepted by the adapter.
pub const MAX_DEVICE_CALLBACK_FRAMES: usize = MAX_PLUGIN_BLOCK_FRAMES;
/// Events copied from one device callback before stable offset sorting.
pub const MAX_FRAME_EVENTS_PER_CALLBACK: usize = TIMELINE_ENDPOINT_MAX_EVENTS_PER_CALLBACK;
/// Events retained for one fixed quantum. This matches the bridge's inline event capacity.
pub const MAX_FRAME_EVENTS_PER_QUANTUM: usize = TIMELINE_ENDPOINT_MAX_EVENTS_PER_QUANTUM;
const MIN_QUANTUM_FRAMES: usize = 64;
const OUTPUT_RING_FRAMES: usize = MAX_DEVICE_CALLBACK_FRAMES + MAX_PLUGIN_BLOCK_FRAMES;
const INITIAL_EPOCH: u64 = 1;

/// Latency added by fixed-size adaptation and the asynchronous plug-in bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedQuantumLatency {
    pub input_accumulation_frames: usize,
    pub bridge_frames: usize,
    pub total_frames: usize,
}

/// A realtime event whose offset is relative to the beginning of the current device callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameEvent {
    pub sample_offset: u16,
    pub kind: FrameEventKind,
}

impl FrameEvent {
    pub const fn midi(sample_offset: u16, slot: Option<u8>, data: [u8; 3]) -> Self {
        Self {
            sample_offset,
            kind: FrameEventKind::Midi { slot, data },
        }
    }

    pub const fn parameter(sample_offset: u16, slot: u8, id: u32, normalized: f32) -> Self {
        Self {
            sample_offset,
            kind: FrameEventKind::Parameter {
                slot,
                id,
                normalized,
                edit_id: None,
            },
        }
    }

    /// Physical Live-lane marker for an edit that has already acquired reliable endpoint
    /// admission. The adapter must not enqueue it a second time when the quantum completes.
    pub const fn admitted_parameter(
        sample_offset: u16,
        slot: u8,
        id: u32,
        normalized: f32,
        edit_id: ParameterEditId,
    ) -> Self {
        Self {
            sample_offset,
            kind: FrameEventKind::Parameter {
                slot,
                id,
                normalized,
                edit_id: Some(edit_id),
            },
        }
    }
}

/// Realtime event payload supported by [`AudioThreadEndpoint`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameEventKind {
    Midi {
        slot: Option<u8>,
        data: [u8; 3],
    },
    Parameter {
        slot: u8,
        id: u32,
        normalized: f32,
        /// `Some` is a Q128 reservation marker for an edit already admitted by the endpoint.
        edit_id: Option<ParameterEditId>,
    },
}

const EMPTY_EVENT: FrameEvent = FrameEvent::parameter(0, 0, 0, 0.0);

/// Construction error. It is never produced on the audio callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedQuantumError {
    QuantumOutOfRange { frames: usize },
    QuantumNotPowerOfTwo { frames: usize },
    EndpointBlockTooSmall { required: usize, available: usize },
}

/// Result of one callback invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedQuantumProcessStatus {
    Processed {
        frames: usize,
        completed_quanta: usize,
        /// Worker outputs rejected by coherent latency attestation during this callback.
        latency_drift_quanta: usize,
        epoch_changed: bool,
    },
    InvalidEpoch,
    InvalidFrameCount,
}

/// Cumulative callback-side diagnostics. All fields are owned by the callback thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixedQuantumStats {
    pub callbacks: u64,
    pub callback_frames: u64,
    pub completed_quanta: u64,
    pub submitted_quanta: u64,
    pub bridge_gaps: u64,
    pub plugin_output_quanta: u64,
    pub delayed_dry_quanta: u64,
    pub latency_drift_quanta: u64,
    pub latency_drift_frames: u64,
    /// Delayed-dry blocks silenced because they do not contain the plug-in's additional latency.
    pub unaligned_delayed_dry_quanta: u64,
    pub unaligned_delayed_dry_samples: u64,
    pub startup_silence_frames: u64,
    pub epoch_resets: u64,
    pub partial_input_frames_discarded: u64,
    pub output_frames_discarded_on_reset: u64,
    pub output_underflow_frames: u64,
    pub output_overflow_frames: u64,
    pub frame_events_received: u64,
    pub frame_events_staged: u64,
    pub frame_event_overflows: u64,
    pub invalid_frame_events: u64,
    pub endpoint_event_rejections: u64,
    pub events_dropped_on_gap: u64,
    pub events_dropped_on_epoch: u64,
    /// Parameters can be assigned to the correct fixed block, but the current bridge parameter
    /// command has no sample-offset field. Nonzero offsets are therefore applied at block start.
    pub parameter_events_quantized_to_block_start: u64,
    pub non_finite_input_samples: u64,
    pub non_finite_output_samples: u64,
    pub invalid_callbacks: u64,
}

/// Narrow callback surface used by the production endpoint and deterministic tests.
pub trait FixedQuantumEndpoint {
    fn fixed_bridge_lookahead_quanta(&self) -> usize {
        1
    }
    fn fixed_set_transport(&mut self, _transport: PluginTransport) {}
    fn fixed_take_midi_output(&mut self) -> PluginMidiBatch {
        PluginMidiBatch::default()
    }
    fn fixed_max_block_frames(&self) -> usize;
    fn fixed_epoch(&self) -> u64;
    /// Stable creation-time physical-slot identity for this exact endpoint.
    fn fixed_plugin_endpoint_manifest(&self) -> PluginEndpointManifest;
    /// Coherent worker-published latency identity, or `None` while a bounded read collides with
    /// publication or before the worker has published its initial snapshot.
    fn fixed_plugin_latency_snapshot(&self) -> Option<PluginLatencySnapshot>;
    /// Current total bridge latency, including its current block and plug-in reported latency.
    fn fixed_reported_latency_samples(&self) -> u32;
    fn fixed_set_epoch(&mut self, epoch: u64) -> bool;
    fn fixed_send_midi(&mut self, slot: Option<usize>, message: MidiMessage) -> bool;
    fn fixed_set_parameter(&mut self, slot: usize, id: u32, normalized: f32) -> bool;
    /// Reliable tagged edit hook. The default rejects admission so test/legacy endpoints cannot
    /// accidentally claim receipt semantics they do not implement.
    fn fixed_set_parameter_tagged(
        &mut self,
        slot: usize,
        id: u32,
        normalized: f32,
        edit_id: ParameterEditId,
    ) -> bool {
        let _ = (slot, id, normalized, edit_id);
        false
    }
    fn fixed_process(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) -> RealtimeProcessStatus;

    /// Attested processing hook. Implementations that do not expose coherent plug-in latency keep
    /// the legacy behavior through this default method.
    fn fixed_process_with_expected_latency_revision(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
        expected_latency_revision: u64,
    ) -> RealtimeProcessStatus {
        let _ = expected_latency_revision;
        self.fixed_process(input_left, input_right, output_left, output_right)
    }
}

impl FixedQuantumEndpoint for AudioThreadEndpoint {
    fn fixed_bridge_lookahead_quanta(&self) -> usize {
        self.bridge_lookahead_quanta()
    }
    fn fixed_set_transport(&mut self, transport: PluginTransport) {
        self.set_transport(transport);
    }
    fn fixed_take_midi_output(&mut self) -> PluginMidiBatch {
        self.take_midi_output()
    }

    fn fixed_max_block_frames(&self) -> usize {
        self.max_block_frames()
    }

    fn fixed_epoch(&self) -> u64 {
        self.epoch()
    }

    fn fixed_plugin_endpoint_manifest(&self) -> PluginEndpointManifest {
        self.plugin_endpoint_manifest()
    }

    fn fixed_plugin_latency_snapshot(&self) -> Option<PluginLatencySnapshot> {
        self.plugin_latency_snapshot()
    }

    fn fixed_reported_latency_samples(&self) -> u32 {
        self.stats().latency_samples
    }

    fn fixed_set_epoch(&mut self, epoch: u64) -> bool {
        self.set_epoch(epoch)
    }

    fn fixed_send_midi(&mut self, slot: Option<usize>, message: MidiMessage) -> bool {
        self.try_send_midi(slot, message)
    }

    fn fixed_set_parameter(&mut self, slot: usize, id: u32, normalized: f32) -> bool {
        self.try_set_parameter(slot, id, normalized)
    }

    fn fixed_set_parameter_tagged(
        &mut self,
        slot: usize,
        id: u32,
        normalized: f32,
        edit_id: ParameterEditId,
    ) -> bool {
        self.try_set_parameter_tagged(slot, id, normalized, edit_id)
    }

    fn fixed_process(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) -> RealtimeProcessStatus {
        self.process_realtime(input_left, input_right, output_left, output_right)
    }

    fn fixed_process_with_expected_latency_revision(
        &mut self,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
        expected_latency_revision: u64,
    ) -> RealtimeProcessStatus {
        self.process_realtime_with_expected_latency_revision(
            input_left,
            input_right,
            output_left,
            output_right,
            expected_latency_revision,
        )
    }
}

const OUTPUT_MIDI_RING_CAPACITY: usize = 4096;
const EMPTY_MIDI_OUTPUT: PluginMidiOutput = PluginMidiOutput {
    slot: 0,
    message: MidiMessage {
        data: [0; 3],
        sample_offset: 0,
    },
};

#[derive(Clone, Copy)]
struct ScheduledMidiOutput {
    frame: u64,
    event: PluginMidiOutput,
}
const EMPTY_SCHEDULED_OUTPUT: ScheduledMidiOutput = ScheduledMidiOutput {
    frame: 0,
    event: EMPTY_MIDI_OUTPUT,
};

/// Sample-aligned MIDI output of the last device callback. Loss requires downstream panic.
pub struct CallbackMidiOutput {
    pub events: [PluginMidiOutput; OUTPUT_MIDI_RING_CAPACITY],
    pub len: usize,
    pub lost: bool,
    pub audio_lost: bool,
}
impl CallbackMidiOutput {
    fn new() -> Self {
        Self {
            events: [EMPTY_MIDI_OUTPUT; OUTPUT_MIDI_RING_CAPACITY],
            len: 0,
            lost: false,
            audio_lost: false,
        }
    }
}

struct AdapterScratch {
    transport: PluginTransport,
    pending_transport: PluginTransport,
    sample_rate: f64,
    output_position: u64,
    output_written: u64,
    midi_ring: [ScheduledMidiOutput; OUTPUT_MIDI_RING_CAPACITY],
    midi_head: usize,
    midi_len: usize,
    midi_fault_positions: [(u64, bool); 64],
    midi_fault_count: usize,
    midi_callback: CallbackMidiOutput,
    input_left: [f32; MAX_PLUGIN_BLOCK_FRAMES],
    input_right: [f32; MAX_PLUGIN_BLOCK_FRAMES],
    quantum_left: [f32; MAX_PLUGIN_BLOCK_FRAMES],
    quantum_right: [f32; MAX_PLUGIN_BLOCK_FRAMES],
    output_left: [f32; OUTPUT_RING_FRAMES],
    output_right: [f32; OUTPUT_RING_FRAMES],
    callback_events: [FrameEvent; MAX_FRAME_EVENTS_PER_CALLBACK],
    pending_events: [FrameEvent; MAX_FRAME_EVENTS_PER_QUANTUM],
    input_fill: usize,
    output_head: usize,
    output_len: usize,
    callback_event_count: usize,
    pending_event_count: usize,
}

const _: () = assert!(std::mem::size_of::<AdapterScratch>() < 512 * 1024);

impl AdapterScratch {
    fn new() -> Self {
        Self {
            transport: PluginTransport::default(),
            pending_transport: PluginTransport::default(),
            sample_rate: 48000.0,
            output_position: 0,
            output_written: 0,
            midi_ring: [EMPTY_SCHEDULED_OUTPUT; OUTPUT_MIDI_RING_CAPACITY],
            midi_head: 0,
            midi_len: 0,
            midi_fault_positions: [(0, false); 64],
            midi_fault_count: 0,
            midi_callback: CallbackMidiOutput::new(),
            input_left: [0.0; MAX_PLUGIN_BLOCK_FRAMES],
            input_right: [0.0; MAX_PLUGIN_BLOCK_FRAMES],
            quantum_left: [0.0; MAX_PLUGIN_BLOCK_FRAMES],
            quantum_right: [0.0; MAX_PLUGIN_BLOCK_FRAMES],
            output_left: [0.0; OUTPUT_RING_FRAMES],
            output_right: [0.0; OUTPUT_RING_FRAMES],
            callback_events: [EMPTY_EVENT; MAX_FRAME_EVENTS_PER_CALLBACK],
            pending_events: [EMPTY_EVENT; MAX_FRAME_EVENTS_PER_QUANTUM],
            input_fill: 0,
            output_head: 0,
            output_len: 0,
            callback_event_count: 0,
            pending_event_count: 0,
        }
    }

    fn clear_stream(&mut self) {
        self.output_position = 0;
        self.output_written = 0;
        self.midi_head = 0;
        self.midi_len = 0;
        self.midi_fault_count = 0;
        self.midi_callback.len = 0;
        self.midi_callback.lost = true;
        self.midi_callback.audio_lost = true;
        self.input_fill = 0;
        self.output_head = 0;
        self.output_len = 0;
        self.callback_event_count = 0;
        self.pending_event_count = 0;
    }

    fn push_output(&mut self, left: f32, right: f32) -> bool {
        if self.output_len == OUTPUT_RING_FRAMES {
            return false;
        }
        let index = (self.output_head + self.output_len) % OUTPUT_RING_FRAMES;
        self.output_left[index] = left;
        self.output_right[index] = right;
        self.output_len += 1;
        self.output_written += 1;
        true
    }

    fn pop_output(&mut self) -> Option<(f32, f32)> {
        if self.output_len == 0 {
            return None;
        }
        let index = self.output_head;
        self.output_head = (self.output_head + 1) % OUTPUT_RING_FRAMES;
        self.output_len -= 1;
        Some((self.output_left[index], self.output_right[index]))
    }
}

/// Fixed-quantum callback adapter. `E` defaults to the production audio endpoint.
pub struct FixedQuantumAdapter<E = AudioThreadEndpoint> {
    endpoint: E,
    quantum_frames: usize,
    epoch: u64,
    expected_latency_revision: u64,
    scratch: Box<AdapterScratch>,
    stats: FixedQuantumStats,
}

impl FixedQuantumAdapter<AudioThreadEndpoint> {
    pub fn bridge_lookahead_quanta(&self) -> usize {
        self.endpoint.bridge_lookahead_quanta()
    }

    pub fn configure_timing_plan(
        &mut self,
        plan: crate::plugin_timing::PreparedPluginTimingPlan,
    ) -> bool {
        self.scratch.input_fill == 0 && self.endpoint.configure_timing_plan(plan)
    }

    pub fn expected_sequence(&self) -> u64 {
        self.endpoint.expected_sequence()
    }

    pub fn processing_fault(
        &self,
    ) -> Option<(crate::plugin_timing::PluginProcessingFaultReason, u64)> {
        self.endpoint.processing_fault()
    }

    pub fn block_midi_until_epoch(&self) {
        self.endpoint.block_midi_until_epoch();
    }
    pub fn midi_capabilities(&self) -> (bool, bool) {
        self.endpoint.midi_capabilities()
    }

    pub fn new(
        endpoint: AudioThreadEndpoint,
        quantum_frames: usize,
    ) -> Result<Self, FixedQuantumError> {
        Self::with_endpoint(endpoint, quantum_frames)
    }

    /// Snapshot the wrapped bridge's atomic diagnostics.
    pub fn bridge_stats(&self) -> BridgeStats {
        self.endpoint.stats()
    }
}

impl<E: FixedQuantumEndpoint> FixedQuantumAdapter<E> {
    /// Construct and pre-prime the adapter. Call only from the control thread.
    pub fn with_endpoint(endpoint: E, quantum_frames: usize) -> Result<Self, FixedQuantumError> {
        validate_quantum(quantum_frames, endpoint.fixed_max_block_frames())?;
        let epoch = endpoint.fixed_epoch();
        let mut adapter = Self {
            endpoint,
            quantum_frames,
            epoch: if epoch == 0 { INITIAL_EPOCH } else { epoch },
            expected_latency_revision: 0,
            scratch: Box::new(AdapterScratch::new()),
            stats: FixedQuantumStats::default(),
        };
        adapter.prime_accumulation_silence();
        Ok(adapter)
    }

    pub fn quantum_frames(&self) -> usize {
        self.quantum_frames
    }

    pub fn latency(&self) -> FixedQuantumLatency {
        FixedQuantumLatency {
            input_accumulation_frames: self.quantum_frames,
            bridge_frames: self.quantum_frames * self.endpoint.fixed_bridge_lookahead_quanta(),
            total_frames: self.quantum_frames * (1 + self.endpoint.fixed_bridge_lookahead_quanta()),
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Bind every subsequently submitted fixed quantum to one coherent plug-in latency revision.
    /// Zero restores the legacy unchecked bridge contract.
    pub fn set_expected_latency_revision(&mut self, revision: u64) {
        self.expected_latency_revision = revision;
    }

    pub fn expected_latency_revision(&self) -> u64 {
        self.expected_latency_revision
    }

    /// Frames already accumulated into the fixed quantum currently being assembled.
    ///
    /// Zero means the next callback frame begins a new quantum. The value is callback-owned and
    /// always smaller than [`Self::quantum_frames`].
    pub fn input_phase_frames(&self) -> usize {
        self.scratch.input_fill
    }

    /// Offset from the beginning of the next callback to the next fixed-quantum input boundary.
    ///
    /// A zero phase returns offset zero: the callback begins on a boundary and an event at offset
    /// zero belongs to the new quantum. A partial quantum returns the remaining number of frames.
    pub fn next_quantum_boundary_offset(&self) -> usize {
        let phase = self.input_phase_frames();
        if phase == 0 {
            0
        } else {
            self.quantum_frames - phase
        }
    }

    /// Stable creation-time identity of the exact wrapped endpoint.
    pub fn plugin_endpoint_manifest(&self) -> PluginEndpointManifest {
        self.endpoint.fixed_plugin_endpoint_manifest()
    }

    /// Read the worker's coherent latency snapshot without consulting best-effort diagnostics.
    ///
    /// Callers should retain the last accepted graph snapshot and retry later when this returns
    /// `None`.
    pub fn plugin_latency_snapshot(&self) -> Option<PluginLatencySnapshot> {
        self.endpoint.fixed_plugin_latency_snapshot()
    }

    /// Pair this exact endpoint's physical-slot manifest with one coherent latency publication.
    ///
    /// Validation is bounded and allocation-free. Unknown legacy manifests and colliding,
    /// unpublished or malformed latency reads return `None` so realtime callers can fail closed.
    pub fn plugin_endpoint_snapshot(&self) -> Option<PluginEndpointSnapshot> {
        PluginEndpointSnapshot::try_new(
            self.plugin_endpoint_manifest(),
            self.plugin_latency_snapshot()?,
        )
        .ok()
    }

    /// Immediately begin a new nonzero transport epoch.
    ///
    /// This is callback-safe and bounded. It discards partial input, queued output and pending
    /// events, tells the endpoint to reject the old generation, then restores the accumulation
    /// pre-roll. Destruction still belongs on the control thread after the whole adapter has been
    /// returned through the engine's retire ring.
    pub fn set_epoch(&mut self, epoch: u64) -> bool {
        if epoch == 0 || epoch == self.epoch {
            return false;
        }
        self.reset_epoch(epoch);
        true
    }

    pub fn stats(&self) -> FixedQuantumStats {
        self.stats
    }

    /// Stage one reliable block-boundary parameter edit on the wrapped endpoint.
    ///
    /// A successful call is attached to the next submitted fixed quantum and terminates in one
    /// control-thread receipt even if that submission gaps or the epoch changes first.
    pub fn try_set_parameter_tagged(
        &mut self,
        slot: usize,
        id: u32,
        normalized: f32,
        edit_id: ParameterEditId,
    ) -> bool {
        self.endpoint
            .fixed_set_parameter_tagged(slot, id, normalized, edit_id)
    }

    /// Consume arbitrary planar stereo input and produce the same number of output frames.
    pub fn process(
        &mut self,
        epoch: u64,
        input_left: &[f32],
        input_right: &[f32],
        output_left: &mut [f32],
        output_right: &mut [f32],
        events: &[FrameEvent],
    ) -> FixedQuantumProcessStatus {
        let frames = input_left.len();
        if frames != input_right.len()
            || frames == 0
            || frames > MAX_DEVICE_CALLBACK_FRAMES
            || output_left.len() < frames
            || output_right.len() < frames
        {
            self.stats.invalid_callbacks += 1;
            return FixedQuantumProcessStatus::InvalidFrameCount;
        }
        self.process_inner(
            epoch,
            frames,
            Some((input_left, input_right)),
            output_left,
            output_right,
            events,
        )
    }

    /// Generator mode: submit deterministic zero input while preserving the same timing contract.
    pub fn process_generator(
        &mut self,
        epoch: u64,
        frames: usize,
        output_left: &mut [f32],
        output_right: &mut [f32],
        events: &[FrameEvent],
    ) -> FixedQuantumProcessStatus {
        if frames == 0
            || frames > MAX_DEVICE_CALLBACK_FRAMES
            || output_left.len() < frames
            || output_right.len() < frames
        {
            self.stats.invalid_callbacks += 1;
            return FixedQuantumProcessStatus::InvalidFrameCount;
        }
        self.process_inner(epoch, frames, None, output_left, output_right, events)
    }

    /// Recover endpoint ownership after callback-confirmed detachment.
    pub fn into_endpoint(self) -> E {
        self.endpoint
    }

    /// Context is stamped when the first sample of a partial quantum enters; later callbacks
    /// cannot overwrite it. The caller supplies the first input sample's content position.
    pub fn set_transport(&mut self, transport: PluginTransport, sample_rate: f64) {
        self.scratch.transport = transport;
        self.scratch.sample_rate = sample_rate;
    }

    pub fn midi_output(&self) -> &CallbackMidiOutput {
        &self.scratch.midi_callback
    }

    fn process_inner(
        &mut self,
        epoch: u64,
        frames: usize,
        input: Option<(&[f32], &[f32])>,
        output_left: &mut [f32],
        output_right: &mut [f32],
        events: &[FrameEvent],
    ) -> FixedQuantumProcessStatus {
        if epoch == 0 {
            output_left[..frames].fill(0.0);
            output_right[..frames].fill(0.0);
            self.stats.invalid_callbacks += 1;
            return FixedQuantumProcessStatus::InvalidEpoch;
        }

        let epoch_changed = self.set_epoch(epoch);
        self.scratch.midi_callback.len = 0;
        self.scratch.midi_callback.lost = epoch_changed;
        self.scratch.midi_callback.audio_lost = epoch_changed;
        let latency_drift_quanta_before = self.stats.latency_drift_quanta;
        self.stats.callbacks += 1;
        self.stats.callback_frames += frames as u64;
        self.prepare_callback_events(events, frames);

        let mut consumed = 0;
        let mut completed_quanta = 0;
        let mut event_cursor = 0;
        while consumed < frames {
            let old_fill = self.scratch.input_fill;
            if old_fill == 0 {
                let mut transport = self.scratch.transport;
                if transport.playing {
                    transport.sample_position =
                        transport.sample_position.saturating_add(consumed as i64);
                    transport.quarter_note_position +=
                        consumed as f64 * transport.tempo / (60.0 * self.scratch.sample_rate);
                }
                self.scratch.pending_transport = transport;
            }
            let chunk = (self.quantum_frames - old_fill).min(frames - consumed);
            let chunk_end = consumed + chunk;

            while event_cursor < self.scratch.callback_event_count
                && usize::from(self.scratch.callback_events[event_cursor].sample_offset) < chunk_end
            {
                let event = self.scratch.callback_events[event_cursor];
                let callback_offset = usize::from(event.sample_offset);
                if callback_offset >= consumed {
                    let local_offset = old_fill + callback_offset - consumed;
                    self.push_pending_event(FrameEvent {
                        sample_offset: local_offset as u16,
                        kind: event.kind,
                    });
                }
                event_cursor += 1;
            }

            let destination = old_fill..old_fill + chunk;
            if let Some((left, right)) = input {
                for (destination_index, source_index) in
                    destination.clone().zip(consumed..chunk_end)
                {
                    self.scratch.input_left[destination_index] =
                        sanitize_input(left[source_index], &mut self.stats);
                    self.scratch.input_right[destination_index] =
                        sanitize_input(right[source_index], &mut self.stats);
                }
            } else {
                self.scratch.input_left[destination.clone()].fill(0.0);
                self.scratch.input_right[destination].fill(0.0);
            }
            self.scratch.input_fill += chunk;
            consumed = chunk_end;

            if self.scratch.input_fill == self.quantum_frames {
                self.process_quantum();
                completed_quanta += 1;
            }
        }

        let callback_start = self.scratch.output_position;
        let callback_end = callback_start.saturating_add(frames as u64);
        while self.scratch.midi_len != 0 {
            let scheduled = self.scratch.midi_ring[self.scratch.midi_head];
            if scheduled.frame >= callback_end {
                break;
            }
            self.scratch.midi_head = (self.scratch.midi_head + 1) % OUTPUT_MIDI_RING_CAPACITY;
            self.scratch.midi_len -= 1;
            if scheduled.frame < callback_start {
                self.scratch.midi_callback.lost = true;
                continue;
            }
            let mut event = scheduled.event;
            event.message.sample_offset = (scheduled.frame - callback_start) as u16;
            let index = self.scratch.midi_callback.len;
            self.scratch.midi_callback.events[index] = event;
            self.scratch.midi_callback.len += 1;
        }
        let mut retained_faults = 0;
        for index in 0..self.scratch.midi_fault_count {
            let position = self.scratch.midi_fault_positions[index];
            if position.0 < callback_end {
                self.scratch.midi_callback.lost = true;
                self.scratch.midi_callback.audio_lost |= position.1;
            } else {
                self.scratch.midi_fault_positions[retained_faults] = position;
                retained_faults += 1;
            }
        }
        self.scratch.midi_fault_count = retained_faults;
        self.scratch.output_position = callback_end;
        for index in 0..frames {
            let (left, right) = match self.scratch.pop_output() {
                Some(samples) => samples,
                None => {
                    self.stats.output_underflow_frames += 1;
                    (0.0, 0.0)
                }
            };
            output_left[index] = sanitize_output(left, &mut self.stats);
            output_right[index] = sanitize_output(right, &mut self.stats);
        }

        FixedQuantumProcessStatus::Processed {
            frames,
            completed_quanta,
            latency_drift_quanta: self
                .stats
                .latency_drift_quanta
                .saturating_sub(latency_drift_quanta_before)
                as usize,
            epoch_changed,
        }
    }

    fn prepare_callback_events(&mut self, events: &[FrameEvent], frames: usize) {
        self.scratch.callback_event_count = 0;
        self.stats.frame_events_received += events.len() as u64;
        for event in events.iter().copied() {
            if usize::from(event.sample_offset) >= frames || !event_is_valid(event) {
                self.stats.invalid_frame_events += 1;
                continue;
            }
            if self.scratch.callback_event_count == MAX_FRAME_EVENTS_PER_CALLBACK {
                self.stats.frame_event_overflows += 1;
                continue;
            }
            let index = self.scratch.callback_event_count;
            self.scratch.callback_events[index] = event;
            self.scratch.callback_event_count += 1;
        }
        // Stable insertion sort keeps equal-offset caller order without heap scratch.
        for index in 1..self.scratch.callback_event_count {
            let event = self.scratch.callback_events[index];
            let mut cursor = index;
            while cursor > 0
                && self.scratch.callback_events[cursor - 1].sample_offset > event.sample_offset
            {
                self.scratch.callback_events[cursor] = self.scratch.callback_events[cursor - 1];
                cursor -= 1;
            }
            self.scratch.callback_events[cursor] = event;
        }
    }

    fn push_pending_event(&mut self, event: FrameEvent) {
        if self.scratch.pending_event_count == MAX_FRAME_EVENTS_PER_QUANTUM {
            self.stats.frame_event_overflows += 1;
            return;
        }
        self.scratch.pending_events[self.scratch.pending_event_count] = event;
        self.scratch.pending_event_count += 1;
    }

    fn process_quantum(&mut self) {
        let event_count = self.scratch.pending_event_count;
        let mut staged_count = 0_u64;
        for index in 0..event_count {
            let event = self.scratch.pending_events[index];
            let accepted = match event.kind {
                FrameEventKind::Midi { slot, data } => self.endpoint.fixed_send_midi(
                    slot.map(usize::from),
                    MidiMessage::new(data, usize::from(event.sample_offset)),
                ),
                FrameEventKind::Parameter {
                    slot,
                    id,
                    normalized,
                    edit_id,
                } => {
                    if event.sample_offset != 0 {
                        self.stats.parameter_events_quantized_to_block_start += 1;
                    }
                    edit_id.is_some()
                        || self
                            .endpoint
                            .fixed_set_parameter(usize::from(slot), id, normalized)
                }
            };
            if accepted {
                staged_count += 1;
                self.stats.frame_events_staged += 1;
            } else {
                self.stats.endpoint_event_rejections += 1;
            }
        }
        self.scratch.pending_event_count = 0;

        let quantum = self.quantum_frames;
        self.endpoint
            .fixed_set_transport(self.scratch.pending_transport);
        let status = self.endpoint.fixed_process_with_expected_latency_revision(
            &self.scratch.input_left[..quantum],
            &self.scratch.input_right[..quantum],
            &mut self.scratch.quantum_left[..quantum],
            &mut self.scratch.quantum_right[..quantum],
            self.expected_latency_revision,
        );
        self.scratch.input_fill = 0;
        self.stats.completed_quanta += 1;

        match status {
            RealtimeProcessStatus::Processed {
                sequence,
                submit,
                source,
                ..
            } => {
                match submit {
                    SubmitStatus::Submitted { .. } => self.stats.submitted_quanta += 1,
                    SubmitStatus::Gap { .. } => {
                        self.stats.bridge_gaps += 1;
                        self.stats.events_dropped_on_gap += staged_count;
                    }
                    SubmitStatus::InvalidFrameCount => self.stats.bridge_gaps += 1,
                }
                match source {
                    RealtimeOutputSource::Plugin => self.stats.plugin_output_quanta += 1,
                    RealtimeOutputSource::DelayedDry => {
                        self.stats.delayed_dry_quanta += 1;
                        if sequence == 0 {
                            self.stats.startup_silence_frames += quantum as u64;
                        }
                        // The bridge's delayed dry is aligned only to its own one-block delay. If
                        // the active plug-in chain reports additional latency, passing this dry
                        // through would arrive ahead of PDC-compensated paths. Fail closed at the
                        // same stream timestamp instead of leaking early audio.
                        if self.endpoint.fixed_reported_latency_samples() > quantum as u32 {
                            self.scratch.quantum_left[..quantum].fill(0.0);
                            self.scratch.quantum_right[..quantum].fill(0.0);
                            self.stats.unaligned_delayed_dry_quanta += 1;
                            self.stats.unaligned_delayed_dry_samples += quantum as u64;
                        }
                    }
                    RealtimeOutputSource::LatencyDrift => {
                        self.scratch.quantum_left[..quantum].fill(0.0);
                        self.scratch.quantum_right[..quantum].fill(0.0);
                        self.stats.latency_drift_quanta += 1;
                        self.stats.latency_drift_frames += quantum as u64;
                    }
                }
            }
            RealtimeProcessStatus::InvalidFrameCount => {
                self.scratch.quantum_left[..quantum].fill(0.0);
                self.scratch.quantum_right[..quantum].fill(0.0);
                self.stats.bridge_gaps += 1;
                self.stats.events_dropped_on_gap += staged_count;
            }
        }

        let batch = self.endpoint.fixed_take_midi_output();
        let valid = matches!(
            status,
            RealtimeProcessStatus::Processed {
                source: RealtimeOutputSource::Plugin,
                ..
            }
        );
        let startup = matches!(status, RealtimeProcessStatus::Processed { sequence: 0, .. });
        let audio_lost = batch.audio_lost || (!valid && !startup);
        let lost = batch.lost || audio_lost;
        let start = self.scratch.output_written;
        if lost {
            if self.scratch.midi_fault_count < self.scratch.midi_fault_positions.len() {
                self.scratch.midi_fault_positions[self.scratch.midi_fault_count] =
                    (start, audio_lost);
                self.scratch.midi_fault_count += 1;
            } else {
                self.scratch.midi_callback.lost = true;
                self.scratch.midi_callback.audio_lost |= audio_lost;
            }
        } else if valid {
            // Stable offset sort; at equal time note-off precedes note-on to avoid hanging
            // retriggers. The batch lives on bounded worker/callback scratch, never a Vec.
            let mut batch = batch;
            let key = |event: PluginMidiOutput| {
                (
                    event.message.sample_offset,
                    u8::from(
                        event.message.data[0] & 0xf0 != 0x80
                            && !(event.message.data[0] & 0xf0 == 0x90
                                && event.message.data[2] == 0),
                    ),
                )
            };
            for index in 1..batch.len {
                let event = batch.events[index];
                let mut cursor = index;
                while cursor > 0 && key(batch.events[cursor - 1]) > key(event) {
                    batch.events[cursor] = batch.events[cursor - 1];
                    cursor -= 1;
                }
                batch.events[cursor] = event;
            }
            for event in batch.events[..batch.len].iter().copied() {
                if usize::from(event.message.sample_offset) >= quantum
                    || self.scratch.midi_len == OUTPUT_MIDI_RING_CAPACITY
                {
                    self.scratch.midi_callback.lost = true;
                    continue;
                }
                let index =
                    (self.scratch.midi_head + self.scratch.midi_len) % OUTPUT_MIDI_RING_CAPACITY;
                self.scratch.midi_ring[index] = ScheduledMidiOutput {
                    frame: start + u64::from(event.message.sample_offset),
                    event,
                };
                self.scratch.midi_len += 1;
            }
        }

        for index in 0..quantum {
            if !self.scratch.push_output(
                self.scratch.quantum_left[index],
                self.scratch.quantum_right[index],
            ) {
                self.stats.output_overflow_frames += 1;
            }
        }
    }

    fn reset_epoch(&mut self, epoch: u64) {
        self.stats.epoch_resets += 1;
        self.stats.partial_input_frames_discarded += self.scratch.input_fill as u64;
        self.stats.output_frames_discarded_on_reset += self.scratch.output_len as u64;
        self.stats.events_dropped_on_epoch += self.scratch.pending_event_count as u64;
        self.scratch.clear_stream();
        let _ = self.endpoint.fixed_set_epoch(epoch);
        self.epoch = epoch;
        self.prime_accumulation_silence();
    }

    fn prime_accumulation_silence(&mut self) {
        for _ in 0..self.quantum_frames {
            let pushed = self.scratch.push_output(0.0, 0.0);
            debug_assert!(pushed);
        }
        self.stats.startup_silence_frames += self.quantum_frames as u64;
    }
}

fn validate_quantum(quantum_frames: usize, endpoint_max: usize) -> Result<(), FixedQuantumError> {
    if !(MIN_QUANTUM_FRAMES..=MAX_PLUGIN_BLOCK_FRAMES).contains(&quantum_frames) {
        return Err(FixedQuantumError::QuantumOutOfRange {
            frames: quantum_frames,
        });
    }
    if !quantum_frames.is_power_of_two() {
        return Err(FixedQuantumError::QuantumNotPowerOfTwo {
            frames: quantum_frames,
        });
    }
    if endpoint_max < quantum_frames {
        return Err(FixedQuantumError::EndpointBlockTooSmall {
            required: quantum_frames,
            available: endpoint_max,
        });
    }
    Ok(())
}

fn event_is_valid(event: FrameEvent) -> bool {
    match event.kind {
        FrameEventKind::Midi { .. } => true,
        FrameEventKind::Parameter { normalized, .. } => normalized.is_finite(),
    }
}

fn sanitize_input(sample: f32, stats: &mut FixedQuantumStats) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        stats.non_finite_input_samples += 1;
        0.0
    }
}

fn sanitize_output(sample: f32, stats: &mut FixedQuantumStats) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        stats.non_finite_output_samples += 1;
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::plugin_runtime::{
        BackendSlot, MAX_PLUGIN_CHAIN_SLOTS, PluginBackend, PluginChain, PluginPrepareConfig,
    };
    use std::collections::VecDeque;

    struct ParameterBackend {
        value: f32,
    }

    impl PluginBackend for ParameterBackend {
        fn name(&self) -> &str {
            "fixed tagged parameter"
        }

        fn prepare(&mut self, _config: PluginPrepareConfig) -> Result<(), String> {
            Ok(())
        }

        fn process(
            &mut self,
            _left: &mut [f32],
            _right: &mut [f32],
            _frames: usize,
        ) -> Result<(), String> {
            Ok(())
        }

        fn send_midi(&mut self, _message: MidiMessage) -> Result<(), String> {
            Ok(())
        }

        fn set_parameter(&mut self, _id: u32, normalized: f32) -> Result<(), String> {
            self.value = normalized;
            Ok(())
        }

        fn get_parameter(&mut self, _id: u32) -> Result<f32, String> {
            Ok(self.value)
        }

        fn save_state(&mut self) -> Result<Vec<u8>, String> {
            Ok(Vec::new())
        }

        fn load_state(&mut self, _state: &[u8]) -> Result<(), String> {
            Ok(())
        }

        fn latency_samples(&self) -> u32 {
            0
        }

        fn tail_samples(&self) -> u32 {
            0
        }
    }

    struct MockEndpoint {
        epoch: u64,
        maximum: usize,
        manifest: PluginEndpointManifest,
        latency_snapshot: Option<PluginLatencySnapshot>,
        previous: Option<(Vec<f32>, Vec<f32>)>,
        submitted: Vec<(Vec<f32>, Vec<f32>)>,
        midi: Vec<MidiMessage>,
        parameters: Vec<(usize, u32, f32)>,
        tagged_parameters: Vec<(usize, u32, f32, ParameterEditId)>,
        gaps: VecDeque<bool>,
        delayed_dry: VecDeque<bool>,
        reported_latency_samples: u32,
        poison_output: bool,
        expected_latency_revisions: Vec<u64>,
        drift_on_expected_revision: Option<u64>,
    }

    impl MockEndpoint {
        fn new() -> Self {
            Self {
                epoch: 1,
                maximum: MAX_PLUGIN_BLOCK_FRAMES,
                manifest: PluginEndpointManifest::unknown_for_slots(0).unwrap(),
                latency_snapshot: None,
                previous: None,
                submitted: Vec::new(),
                midi: Vec::new(),
                parameters: Vec::new(),
                tagged_parameters: Vec::new(),
                gaps: VecDeque::new(),
                delayed_dry: VecDeque::new(),
                reported_latency_samples: 0,
                poison_output: false,
                expected_latency_revisions: Vec::new(),
                drift_on_expected_revision: None,
            }
        }
    }

    impl FixedQuantumEndpoint for MockEndpoint {
        fn fixed_max_block_frames(&self) -> usize {
            self.maximum
        }

        fn fixed_epoch(&self) -> u64 {
            self.epoch
        }

        fn fixed_plugin_endpoint_manifest(&self) -> PluginEndpointManifest {
            self.manifest
        }

        fn fixed_plugin_latency_snapshot(&self) -> Option<PluginLatencySnapshot> {
            self.latency_snapshot
        }

        fn fixed_reported_latency_samples(&self) -> u32 {
            self.reported_latency_samples
        }

        fn fixed_set_epoch(&mut self, epoch: u64) -> bool {
            if epoch == 0 || epoch == self.epoch {
                return false;
            }
            self.epoch = epoch;
            self.previous = None;
            true
        }

        fn fixed_send_midi(&mut self, _slot: Option<usize>, message: MidiMessage) -> bool {
            self.midi.push(message);
            true
        }

        fn fixed_set_parameter(&mut self, slot: usize, id: u32, normalized: f32) -> bool {
            self.parameters.push((slot, id, normalized));
            true
        }

        fn fixed_set_parameter_tagged(
            &mut self,
            slot: usize,
            id: u32,
            normalized: f32,
            edit_id: ParameterEditId,
        ) -> bool {
            self.tagged_parameters.push((slot, id, normalized, edit_id));
            true
        }

        fn fixed_process(
            &mut self,
            input_left: &[f32],
            input_right: &[f32],
            output_left: &mut [f32],
            output_right: &mut [f32],
        ) -> RealtimeProcessStatus {
            let gap = self.gaps.pop_front().unwrap_or(false);
            let sequence = self.submitted.len() as u64;
            let forced_dry = self.delayed_dry.pop_front().unwrap_or(false);
            let source = if let Some((left, right)) = self.previous.take() {
                output_left.copy_from_slice(&left);
                output_right.copy_from_slice(&right);
                if forced_dry {
                    RealtimeOutputSource::DelayedDry
                } else {
                    RealtimeOutputSource::Plugin
                }
            } else {
                output_left.fill(0.0);
                output_right.fill(0.0);
                RealtimeOutputSource::DelayedDry
            };
            self.previous = Some((input_left.to_vec(), input_right.to_vec()));
            self.submitted
                .push((input_left.to_vec(), input_right.to_vec()));
            if self.poison_output && sequence > 0 {
                output_left[0] = f32::NAN;
            }
            RealtimeProcessStatus::Processed {
                sequence,
                frames: input_left.len(),
                submit: if gap {
                    SubmitStatus::Gap {
                        sequence: sequence + 1,
                    }
                } else {
                    SubmitStatus::Submitted {
                        sequence: sequence + 1,
                    }
                },
                source,
            }
        }

        fn fixed_process_with_expected_latency_revision(
            &mut self,
            input_left: &[f32],
            input_right: &[f32],
            output_left: &mut [f32],
            output_right: &mut [f32],
            expected_latency_revision: u64,
        ) -> RealtimeProcessStatus {
            self.expected_latency_revisions
                .push(expected_latency_revision);
            let status = self.fixed_process(input_left, input_right, output_left, output_right);
            if self.drift_on_expected_revision != Some(expected_latency_revision) {
                return status;
            }
            match status {
                RealtimeProcessStatus::Processed {
                    sequence,
                    frames,
                    submit,
                    ..
                } => RealtimeProcessStatus::Processed {
                    sequence,
                    frames,
                    submit,
                    source: RealtimeOutputSource::LatencyDrift,
                },
                RealtimeProcessStatus::InvalidFrameCount => {
                    RealtimeProcessStatus::InvalidFrameCount
                }
            }
        }
    }

    fn render(split: &[usize], input: &[f32]) -> (Vec<f32>, FixedQuantumAdapter<MockEndpoint>) {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 64).unwrap();
        let mut rendered = Vec::new();
        let mut offset = 0;
        for &frames in split {
            let mut left = vec![0.0; frames];
            let mut right = vec![0.0; frames];
            let status = adapter.process(
                1,
                &input[offset..offset + frames],
                &input[offset..offset + frames],
                &mut left,
                &mut right,
                &[],
            );
            assert!(matches!(
                status,
                FixedQuantumProcessStatus::Processed { .. }
            ));
            rendered.extend(left);
            offset += frames;
        }
        (rendered, adapter)
    }

    #[test]
    fn validates_quantum_and_reports_exact_latency() {
        assert!(matches!(
            FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 63),
            Err(FixedQuantumError::QuantumOutOfRange { .. })
        ));
        assert!(matches!(
            FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 96),
            Err(FixedQuantumError::QuantumNotPowerOfTwo { .. })
        ));
        let adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 256).unwrap();
        assert_eq!(
            adapter.latency(),
            FixedQuantumLatency {
                input_accumulation_frames: 256,
                bridge_frames: 256,
                total_frames: 512,
            }
        );
    }

    #[test]
    fn expected_latency_revision_propagates_per_quantum_and_drift_is_explicit() {
        let mut endpoint = MockEndpoint::new();
        endpoint.previous = Some((vec![1.0; 64], vec![1.0; 64]));
        endpoint.drift_on_expected_revision = Some(77);
        let mut adapter = FixedQuantumAdapter::with_endpoint(endpoint, 64).unwrap();
        adapter.set_expected_latency_revision(77);
        assert_eq!(adapter.expected_latency_revision(), 77);

        let input = [2.0; 64];
        let mut left = [9.0; 64];
        let mut right = [9.0; 64];
        let status = adapter.process(1, &input, &input, &mut left, &mut right, &[]);
        assert!(matches!(
            status,
            FixedQuantumProcessStatus::Processed {
                completed_quanta: 1,
                latency_drift_quanta: 1,
                ..
            }
        ));
        assert_eq!(left, [0.0; 64]);
        assert_eq!(right, [0.0; 64]);
        assert_eq!(adapter.stats().plugin_output_quanta, 0);
        assert_eq!(adapter.stats().delayed_dry_quanta, 0);

        adapter.set_expected_latency_revision(0);
        let status = adapter.process(1, &input, &input, &mut left, &mut right, &[]);
        assert!(matches!(
            status,
            FixedQuantumProcessStatus::Processed {
                completed_quanta: 1,
                latency_drift_quanta: 0,
                ..
            }
        ));
        // The first worker output was deliberately nonzero. The adapter must keep the rejected
        // quantum silent while it drains from the future-output ring on this callback.
        assert_eq!(left, [0.0; 64]);
        assert_eq!(right, [0.0; 64]);
        assert_eq!(adapter.stats().latency_drift_quanta, 1);
        assert_eq!(adapter.stats().latency_drift_frames, 64);
        assert_eq!(adapter.stats().plugin_output_quanta, 1);
        assert_eq!(adapter.stats().delayed_dry_quanta, 0);

        let endpoint = adapter.into_endpoint();
        assert_eq!(endpoint.expected_latency_revisions, vec![77, 0]);
    }

    #[test]
    fn endpoint_manifest_preserves_unknown_and_identified_identity() {
        let mut unknown_endpoint = MockEndpoint::new();
        unknown_endpoint.manifest = PluginEndpointManifest::unknown_for_slots(2).unwrap();
        let unknown = FixedQuantumAdapter::with_endpoint(unknown_endpoint, 128).unwrap();
        let unknown_manifest = unknown.plugin_endpoint_manifest();
        assert!(!unknown_manifest.is_identified());
        assert_eq!(unknown_manifest.slot_count, 2);
        assert_eq!(
            unknown_manifest.instance_ids,
            [None; MAX_PLUGIN_CHAIN_SLOTS]
        );
        assert!(PluginEndpointManifest::unknown_for_slots(MAX_PLUGIN_CHAIN_SLOTS + 1).is_err());

        let mut identified_endpoint = MockEndpoint::new();
        identified_endpoint.manifest = PluginEndpointManifest::identified(&[91, 7, 42]).unwrap();
        let identified = FixedQuantumAdapter::with_endpoint(identified_endpoint, 128).unwrap();
        let identified_manifest = identified.plugin_endpoint_manifest();
        assert!(identified_manifest.is_identified());
        assert_eq!(identified_manifest.slot_count, 3);
        assert_eq!(identified_manifest.instance_id(0), Some(91));
        assert_eq!(identified_manifest.instance_id(1), Some(7));
        assert_eq!(identified_manifest.instance_id(2), Some(42));
        assert_eq!(identified_manifest.instance_id(3), None);
    }

    #[test]
    fn coherent_latency_snapshot_is_forwarded_without_reconstruction() {
        let expected = PluginLatencySnapshot {
            revision: 73,
            active_mask: 0b1011,
            slot_latency_samples: [11, 13, 0, 17, 0, 0, 0, 0, 0, 0],
            total_plugin_latency_samples: 41,
            tail_samples: 4096,
        };
        let mut endpoint = MockEndpoint::new();
        endpoint.latency_snapshot = Some(expected);
        let adapter = FixedQuantumAdapter::with_endpoint(endpoint, 128).unwrap();

        let actual = adapter.plugin_latency_snapshot().unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.revision, 73);
        assert_eq!(actual.prefix_latency_before(3), Some(24));
        assert_eq!(actual.tail_samples, 4096);
    }

    #[test]
    fn exact_endpoint_snapshot_is_validated_and_forwarded_without_allocation() {
        let mut endpoint = MockEndpoint::new();
        endpoint.manifest = PluginEndpointManifest::identified(&[91, 7, 42]).unwrap();
        endpoint.latency_snapshot = Some(PluginLatencySnapshot {
            revision: 73,
            active_mask: 0b101,
            slot_latency_samples: [11, 0, 17, 0, 0, 0, 0, 0, 0, 0],
            total_plugin_latency_samples: 28,
            tail_samples: 4_096,
        });
        let adapter = FixedQuantumAdapter::with_endpoint(endpoint, 128).unwrap();

        let snapshot = adapter.plugin_endpoint_snapshot().unwrap();
        assert_eq!(snapshot.slot_count(), 3);
        assert_eq!(snapshot.revision(), 73);
        assert_eq!(snapshot.active_mask(), 0b101);
        assert_eq!(snapshot.slot(0).unwrap().instance_id(), 91);
        assert_eq!(snapshot.slot(1).unwrap().latency_samples(), 0);
        assert_eq!(snapshot.slot(2).unwrap().prefix_latency_samples(), 11);
        assert_eq!(snapshot.total_plugin_latency_samples(), 28);
        assert_eq!(snapshot.tail_samples(), 4_096);
    }

    #[test]
    fn exact_endpoint_snapshot_fails_closed_for_unknown_or_malformed_metadata() {
        let mut unknown_endpoint = MockEndpoint::new();
        unknown_endpoint.manifest = PluginEndpointManifest::unknown_for_slots(1).unwrap();
        unknown_endpoint.latency_snapshot = Some(PluginLatencySnapshot {
            revision: 1,
            active_mask: 1,
            slot_latency_samples: [7, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            total_plugin_latency_samples: 7,
            tail_samples: 0,
        });
        let unknown = FixedQuantumAdapter::with_endpoint(unknown_endpoint, 128).unwrap();
        assert_eq!(unknown.plugin_endpoint_snapshot(), None);

        let mut malformed_endpoint = MockEndpoint::new();
        malformed_endpoint.manifest = PluginEndpointManifest::identified(&[91]).unwrap();
        malformed_endpoint.latency_snapshot = Some(PluginLatencySnapshot {
            revision: 1,
            active_mask: 1,
            slot_latency_samples: [7, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            total_plugin_latency_samples: 8,
            tail_samples: 0,
        });
        let malformed = FixedQuantumAdapter::with_endpoint(malformed_endpoint, 128).unwrap();
        assert_eq!(malformed.plugin_endpoint_snapshot(), None);
    }

    #[test]
    fn input_phase_and_boundary_offset_are_invariant_under_callback_splits() {
        let splits = [13, 7, 101, 6, 1, 129, 255, 64];
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 128).unwrap();
        assert_eq!(adapter.input_phase_frames(), 0);
        assert_eq!(adapter.next_quantum_boundary_offset(), 0);

        let mut stream_frames = 0;
        for frames in splits {
            let input = vec![0.0; frames];
            let mut left = vec![0.0; frames];
            let mut right = vec![0.0; frames];
            let status = adapter.process(1, &input, &input, &mut left, &mut right, &[]);
            assert!(matches!(
                status,
                FixedQuantumProcessStatus::Processed { .. }
            ));
            stream_frames += frames;
            let expected_phase = stream_frames % 128;
            let expected_boundary = if expected_phase == 0 {
                0
            } else {
                128 - expected_phase
            };
            assert_eq!(adapter.input_phase_frames(), expected_phase);
            assert_eq!(adapter.next_quantum_boundary_offset(), expected_boundary);
        }
    }

    #[test]
    fn epoch_reset_restarts_fixed_quantum_phase_at_boundary() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 128).unwrap();
        let input = [0.0; 37];
        let mut left = [0.0; 37];
        let mut right = [0.0; 37];
        let _ = adapter.process(1, &input, &input, &mut left, &mut right, &[]);
        assert_eq!(adapter.input_phase_frames(), 37);
        assert_eq!(adapter.next_quantum_boundary_offset(), 91);

        assert!(adapter.set_epoch(2));
        assert_eq!(adapter.input_phase_frames(), 0);
        assert_eq!(adapter.next_quantum_boundary_offset(), 0);

        let _ = adapter.process(2, &input, &input, &mut left, &mut right, &[]);
        assert_eq!(adapter.input_phase_frames(), 37);
        assert_eq!(adapter.next_quantum_boundary_offset(), 91);
    }

    #[test]
    fn callback_splits_are_stream_equivalent() {
        let input: Vec<f32> = (0..256).map(|value| value as f32).collect();
        let (a, _) = render(&[64, 64, 64, 64], &input);
        let (b, _) = render(&[100, 100, 56], &input);
        assert_eq!(a, b);
    }

    #[test]
    fn one_frame_tails_do_not_change_order() {
        let input: Vec<f32> = (0..256).map(|value| value as f32).collect();
        let (regular, _) = render(&[128, 128], &input);
        let (tailed, _) = render(&[63, 64, 64, 64, 1], &input);
        assert_eq!(regular, tailed);
    }

    #[test]
    fn impulse_has_exact_two_quantum_delay() {
        let mut input = vec![0.0; 256];
        input[0] = 1.0;
        let (output, _) = render(&[64, 64, 64, 64], &input);
        assert_eq!(output.iter().position(|sample| *sample == 1.0), Some(128));
    }

    #[test]
    fn epoch_change_discards_partial_state_and_reprimes() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 64).unwrap();
        let input = [1.0; 32];
        let mut output = [9.0; 32];
        let _ = adapter.process(1, &input, &input, &mut output, &mut [0.0; 32], &[]);
        let status = adapter.process(2, &input, &input, &mut output, &mut [0.0; 32], &[]);
        assert!(matches!(
            status,
            FixedQuantumProcessStatus::Processed {
                epoch_changed: true,
                ..
            }
        ));
        assert_eq!(output, [0.0; 32]);
        assert_eq!(adapter.stats().partial_input_frames_discarded, 32);
        assert_eq!(adapter.stats().epoch_resets, 1);
    }

    #[test]
    fn explicit_epoch_change_is_immediate_bounded_and_idempotent() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 64).unwrap();
        let input = [1.0; 32];
        let mut left = [0.0; 32];
        let mut right = [0.0; 32];
        let _ = adapter.process(1, &input, &input, &mut left, &mut right, &[]);
        assert!(adapter.set_epoch(7));
        assert_eq!(adapter.epoch(), 7);
        assert!(!adapter.set_epoch(7));
        assert!(!adapter.set_epoch(0));
        let status = adapter.process(7, &input, &input, &mut left, &mut right, &[]);
        assert!(matches!(
            status,
            FixedQuantumProcessStatus::Processed {
                epoch_changed: false,
                ..
            }
        ));
        assert_eq!(left, [0.0; 32]);
    }

    #[test]
    fn events_cross_callbacks_into_the_correct_fixed_block() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 64).unwrap();
        let input = [0.0; 40];
        let mut output = [0.0; 40];
        let first = [FrameEvent::midi(39, None, [0x90, 60, 100])];
        let _ = adapter.process(1, &input, &input, &mut output, &mut [0.0; 40], &first);
        let second = [FrameEvent::midi(24, None, [0x80, 60, 0])];
        let _ = adapter.process(1, &input, &input, &mut output, &mut [0.0; 40], &second);
        assert_eq!(adapter.endpoint.midi.len(), 1);
        assert_eq!(adapter.endpoint.midi[0].sample_offset, 39);
        let _ = adapter.process(1, &input, &input, &mut output, &mut [0.0; 40], &[]);
        let _ = adapter.process(1, &input, &input, &mut output, &mut [0.0; 40], &[]);
        assert_eq!(adapter.endpoint.midi.len(), 2);
        assert_eq!(adapter.endpoint.midi[1].sample_offset, 0);
    }

    #[test]
    fn tagged_parameter_survives_callback_split_until_the_next_fixed_quantum() {
        let chain = PluginChain::spawn_with_backend_factory(
            || vec![BackendSlot::new(Box::new(ParameterBackend { value: 0.0 }))],
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 64,
            },
        )
        .unwrap();
        let PluginChain {
            audio,
            mut control,
            guard: _guard,
        } = chain;
        let mut adapter = FixedQuantumAdapter::new(audio, 64).unwrap();
        assert!(adapter.try_set_parameter_tagged(0, 7, 0.75, ParameterEditId::new(700).unwrap(),));

        let input_31 = [0.0; 31];
        let mut left_31 = [0.0; 31];
        let mut right_31 = [0.0; 31];
        let _ = adapter.process(1, &input_31, &input_31, &mut left_31, &mut right_31, &[]);
        assert!(control.try_next_parameter_edit_receipt().is_none());

        let input_33 = [0.0; 33];
        let mut left_33 = [0.0; 33];
        let mut right_33 = [0.0; 33];
        let _ = adapter.process(1, &input_33, &input_33, &mut left_33, &mut right_33, &[]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(receipt) = control.try_next_parameter_edit_receipt() {
                assert!(matches!(
                    receipt,
                    crate::plugins::plugin_runtime::ParameterEditReceipt::Applied {
                        edit_id,
                        slot: 0,
                        id: 7,
                        requested: 0.75,
                        effective: 0.75,
                        readback_confirmed: true,
                    } if edit_id.get() == 700
                ));
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn parameter_offset_is_explicitly_quantized_and_counted() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 64).unwrap();
        let input = [0.0; 64];
        let mut output = [0.0; 64];
        let event = [FrameEvent::parameter(31, 2, 9, 0.5)];
        let _ = adapter.process(1, &input, &input, &mut output, &mut [0.0; 64], &event);
        assert_eq!(adapter.endpoint.parameters, vec![(2, 9, 0.5)]);
        assert_eq!(adapter.stats().parameter_events_quantized_to_block_start, 1);
    }

    #[test]
    fn event_capacity_overflow_is_observable_and_never_deferred() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 64).unwrap();
        let input = [0.0; 64];
        let mut output = [0.0; 64];
        let events = vec![FrameEvent::midi(0, None, [0x90, 60, 1]); 140];
        let _ = adapter.process(1, &input, &input, &mut output, &mut [0.0; 64], &events);
        assert_eq!(adapter.endpoint.midi.len(), MAX_FRAME_EVENTS_PER_QUANTUM);
        assert_eq!(adapter.stats().frame_event_overflows, 12);
        let _ = adapter.process(1, &input, &input, &mut output, &mut [0.0; 64], &[]);
        assert_eq!(adapter.endpoint.midi.len(), MAX_FRAME_EVENTS_PER_QUANTUM);
    }

    #[test]
    fn queue_gap_is_reported_without_reordering_output() {
        let mut endpoint = MockEndpoint::new();
        endpoint.gaps.extend([false, true, false]);
        let mut adapter = FixedQuantumAdapter::with_endpoint(endpoint, 64).unwrap();
        let input: Vec<f32> = (0..192).map(|value| value as f32).collect();
        let mut output = vec![0.0; 192];
        let _ = adapter.process(1, &input, &input, &mut output, &mut vec![0.0; 192], &[]);
        assert_eq!(adapter.stats().bridge_gaps, 1);
        assert_eq!(&output[128..192], &input[..64]);
    }

    #[test]
    fn delayed_dry_with_plugin_latency_is_fail_closed_before_output_ring() {
        let mut endpoint = MockEndpoint::new();
        endpoint.reported_latency_samples = 96;
        endpoint.delayed_dry.extend([false, true, false]);
        let mut adapter = FixedQuantumAdapter::with_endpoint(endpoint, 64).unwrap();
        let input = [1.0; 192];
        let mut left = [9.0; 192];
        let mut right = [9.0; 192];
        let _ = adapter.process(1, &input, &input, &mut left, &mut right, &[]);

        // Frames 128..192 would contain the first block's nonzero delayed dry. The endpoint's
        // reported 32 samples beyond the bridge quantum make that fallback PDC-unaligned.
        assert_eq!(left, [0.0; 192]);
        assert_eq!(right, [0.0; 192]);
        assert_eq!(adapter.stats().unaligned_delayed_dry_quanta, 2);
        assert_eq!(adapter.stats().unaligned_delayed_dry_samples, 128);
    }

    #[test]
    fn bridge_only_delayed_dry_remains_time_aligned() {
        let mut endpoint = MockEndpoint::new();
        endpoint.reported_latency_samples = 64;
        endpoint.delayed_dry.extend([false, true, false]);
        let mut adapter = FixedQuantumAdapter::with_endpoint(endpoint, 64).unwrap();
        let input = [1.0; 192];
        let mut left = [0.0; 192];
        let mut right = [0.0; 192];
        let _ = adapter.process(1, &input, &input, &mut left, &mut right, &[]);

        assert_eq!(&left[128..], &[1.0; 64]);
        assert_eq!(&right[128..], &[1.0; 64]);
        assert_eq!(adapter.stats().unaligned_delayed_dry_quanta, 0);
    }

    #[test]
    fn generator_submits_zero_input() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 64).unwrap();
        let mut left = [0.0; 128];
        let mut right = [0.0; 128];
        let _ = adapter.process_generator(1, 128, &mut left, &mut right, &[]);
        assert_eq!(adapter.endpoint.submitted.len(), 2);
        assert!(
            adapter
                .endpoint
                .submitted
                .iter()
                .all(|(left, right)| left.iter().chain(right).all(|sample| *sample == 0.0))
        );
    }

    #[test]
    fn non_finite_samples_are_sanitized_at_both_edges() {
        let mut endpoint = MockEndpoint::new();
        endpoint.poison_output = true;
        let mut adapter = FixedQuantumAdapter::with_endpoint(endpoint, 64).unwrap();
        let mut input = [0.0; 192];
        input[0] = f32::NAN;
        let mut left = [0.0; 192];
        let mut right = [0.0; 192];
        let _ = adapter.process(1, &input, &input, &mut left, &mut right, &[]);
        assert!(left.iter().all(|sample| sample.is_finite()));
        assert_eq!(adapter.stats().non_finite_input_samples, 2);
        assert!(adapter.stats().non_finite_output_samples >= 1);
    }

    #[test]
    fn output_ring_wrap_preserves_a_long_stream() {
        let input: Vec<f32> = (0..8192).map(|value| value as f32).collect();
        let split = vec![127; 64];
        let used = split.iter().sum::<usize>();
        let (output, adapter) = render(&split, &input[..used]);
        assert_eq!(&output[128..], &input[..used - 128]);
        assert_eq!(adapter.stats().output_underflow_frames, 0);
        assert_eq!(adapter.stats().output_overflow_frames, 0);
    }

    #[test]
    fn invalid_epoch_outputs_silence_without_advancing_stream() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(MockEndpoint::new(), 64).unwrap();
        let input = [1.0; 64];
        let mut left = [9.0; 64];
        let mut right = [9.0; 64];
        assert_eq!(
            adapter.process(0, &input, &input, &mut left, &mut right, &[]),
            FixedQuantumProcessStatus::InvalidEpoch
        );
        assert_eq!(left, [0.0; 64]);
        assert!(adapter.endpoint.submitted.is_empty());
    }
    /// Deterministic worker that makes no progress during a device callback. Only the
    /// test driver's callback boundary promotes submitted work to completed work.
    struct BurstWorker {
        lookahead: usize,
        epoch: u64,
        sequence: u64,
        ready: VecDeque<(u64, PluginTransport)>,
        submitted: Vec<(u64, PluginTransport)>,
        transport: PluginTransport,
        contexts: Vec<PluginTransport>,
        output: PluginMidiBatch,
        misses: usize,
    }
    impl BurstWorker {
        fn new() -> Self {
            Self {
                lookahead: 16,
                epoch: 1,
                sequence: 1,
                ready: VecDeque::new(),
                submitted: Vec::new(),
                transport: PluginTransport::default(),
                contexts: Vec::new(),
                output: PluginMidiBatch::default(),
                misses: 0,
            }
        }
        fn work_between_callbacks(&mut self) {
            self.ready.extend(self.submitted.drain(..));
        }
    }
    impl FixedQuantumEndpoint for BurstWorker {
        fn fixed_bridge_lookahead_quanta(&self) -> usize {
            self.lookahead
        }
        fn fixed_max_block_frames(&self) -> usize {
            128
        }
        fn fixed_epoch(&self) -> u64 {
            self.epoch
        }
        fn fixed_plugin_endpoint_manifest(&self) -> PluginEndpointManifest {
            PluginEndpointManifest::unknown_for_slots(1).unwrap()
        }
        fn fixed_plugin_latency_snapshot(&self) -> Option<PluginLatencySnapshot> {
            None
        }
        fn fixed_reported_latency_samples(&self) -> u32 {
            (128 * self.lookahead) as u32
        }
        fn fixed_set_epoch(&mut self, epoch: u64) -> bool {
            if epoch == self.epoch {
                return false;
            }
            self.epoch = epoch;
            self.sequence = 1;
            self.ready.clear();
            self.submitted.clear();
            self.output = PluginMidiBatch::default();
            true
        }
        fn fixed_send_midi(&mut self, _: Option<usize>, _: MidiMessage) -> bool {
            true
        }
        fn fixed_set_parameter(&mut self, _: usize, _: u32, _: f32) -> bool {
            true
        }
        fn fixed_set_transport(&mut self, transport: PluginTransport) {
            self.transport = transport;
        }
        fn fixed_take_midi_output(&mut self) -> PluginMidiBatch {
            std::mem::take(&mut self.output)
        }
        fn fixed_process(
            &mut self,
            input_left: &[f32],
            _: &[f32],
            left: &mut [f32],
            right: &mut [f32],
        ) -> RealtimeProcessStatus {
            let sequence = self.sequence;
            self.sequence += 1;
            let expected = sequence.saturating_sub(self.lookahead as u64);
            left.fill(0.0);
            right.fill(0.0);
            let mut source = RealtimeOutputSource::DelayedDry;
            if expected != 0 {
                if self
                    .ready
                    .front()
                    .is_some_and(|(sequence, _)| *sequence == expected)
                {
                    self.ready.pop_front();
                    source = RealtimeOutputSource::Plugin;
                    for (offset, data) in [
                        (0, [0x90, 60, 100]),
                        (1, [0x80, 60, 0]),
                        (127, [0x90, 64, 90]),
                    ] {
                        self.output.push(0, MidiMessage::new(data, offset));
                        left[offset] = data[0] as f32;
                        right[offset] = data[1] as f32;
                    }
                } else {
                    self.misses += 1;
                }
            }
            self.submitted.push((sequence, self.transport));
            self.contexts.push(self.transport);
            RealtimeProcessStatus::Processed {
                sequence: expected,
                frames: input_left.len(),
                submit: SubmitStatus::Submitted { sequence },
                source,
            }
        }
    }

    type GeneratedMidiRender = (
        Vec<(usize, [u8; 3])>,
        Vec<f32>,
        FixedQuantumAdapter<BurstWorker>,
    );

    fn render_generated_midi(split: &[usize]) -> GeneratedMidiRender {
        let mut adapter = FixedQuantumAdapter::with_endpoint(BurstWorker::new(), 128).unwrap();
        let mut absolute = 0;
        let mut midi = Vec::new();
        let mut audio = Vec::new();
        while absolute < 8192 {
            let frames = split[(audio.len() / 8192 + absolute) % split.len()].min(8192 - absolute);
            let mut left = vec![0.0; frames];
            let mut right = vec![0.0; frames];
            adapter.set_transport(
                PluginTransport {
                    sample_position: absolute as i64,
                    quarter_note_position: absolute as f64 / 24000.0,
                    tempo: 120.0,
                    playing: true,
                    ..PluginTransport::default()
                },
                48000.0,
            );
            adapter.process_generator(1, frames, &mut left, &mut right, &[]);
            assert!(
                !adapter.midi_output().lost,
                "false output loss at {absolute}, frames {frames}"
            );
            for event in &adapter.midi_output().events[..adapter.midi_output().len] {
                let offset = usize::from(event.message.sample_offset);
                assert_eq!(left[offset], event.message.data[0] as f32);
                assert_eq!(right[offset], event.message.data[1] as f32);
                midi.push((absolute + offset, event.message.data));
            }
            audio.extend(left);
            absolute += frames;
            adapter.endpoint.work_between_callbacks();
        }
        (midi, audio, adapter)
    }

    #[test]
    fn prepared_timing_covers_all_callback_phases_and_profiles_without_worker_progress_inside_callback()
     {
        use crate::plugin_timing::{PLUGIN_CALLBACK_PROFILES, PreparedPluginTimingPlan};
        for rate in [48_000, 384_000] {
            for budget in PLUGIN_CALLBACK_PROFILES {
                let plan = PreparedPluginTimingPlan::new(1, rate, budget).unwrap();
                for phase in 0..128 {
                    for partition in [1, 31, 64, 127, 128, 129, 255, 256, 512, 2048] {
                        if partition > budget as usize {
                            continue;
                        }
                        let mut endpoint = BurstWorker::new();
                        endpoint.lookahead = plan.lookahead_quanta as usize;
                        let mut adapter =
                            FixedQuantumAdapter::with_endpoint(endpoint, 128).unwrap();
                        let total = plan.bridge_latency_frames as usize + 384;
                        let mut absolute = 0;
                        let mut midi = Vec::new();
                        let mut audio = Vec::new();
                        let mut left = [0.0; 2048];
                        let mut right = [0.0; 2048];
                        while absolute < total {
                            let frames = if absolute == 0 && phase != 0 {
                                phase
                            } else {
                                partition
                            }
                            .min(total - absolute);
                            assert!(plan.admits_callback(frames));
                            adapter.process_generator(
                                1,
                                frames,
                                &mut left[..frames],
                                &mut right[..frames],
                                &[],
                            );
                            for event in &adapter.midi_output().events[..adapter.midi_output().len]
                            {
                                midi.push((
                                    absolute + usize::from(event.message.sample_offset),
                                    event.message.data,
                                ));
                            }
                            audio.extend_from_slice(&left[..frames]);
                            absolute += frames;
                            adapter.endpoint.work_between_callbacks();
                        }
                        assert_eq!(
                            adapter.endpoint.misses, 0,
                            "rate={rate} B={budget} phase={phase} partition={partition}"
                        );
                        let latency = plan.bridge_latency_frames as usize;
                        assert_eq!(
                            &midi[..3],
                            &[
                                (latency, [0x90, 60, 100]),
                                (latency + 1, [0x80, 60, 0]),
                                (latency + 127, [0x90, 64, 90])
                            ]
                        );
                        for (frame, data) in midi {
                            assert_eq!(audio[frame], data[0] as f32);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn generated_midi_follows_audio_with_callback_independent_worker_lookahead() {
        let (expected, audio, adapter) = render_generated_midi(&[128]);
        assert_eq!(adapter.latency().total_frames, 2176);
        assert_eq!(
            &expected[..3],
            &[
                (2176, [0x90, 60, 100]),
                (2177, [0x80, 60, 0]),
                (2303, [0x90, 64, 90])
            ]
        );
        for split in [
            &[1][..],
            &[31][..],
            &[64][..],
            &[127][..],
            &[255][..],
            &[256][..],
            &[512][..],
            &[2048][..],
            &[1, 31, 64, 127, 128, 255, 512, 2048][..],
        ] {
            let (actual, rendered, adapter) = render_generated_midi(split);
            assert_eq!(actual, expected, "split {split:?}");
            assert_eq!(rendered, audio, "audio split {split:?}");
            assert_eq!(adapter.endpoint.misses, 0);
            for (index, context) in adapter.endpoint.contexts.iter().enumerate() {
                assert_eq!(context.sample_position, (index * 128) as i64);
                assert!(
                    (context.quarter_note_position - (index * 128) as f64 / 24000.0).abs() < 1e-10
                );
            }
        }
    }

    #[test]
    fn generated_midi_epoch_reset_drops_audio_ring_events_and_restarts_preroll() {
        let mut adapter = FixedQuantumAdapter::with_endpoint(BurstWorker::new(), 128).unwrap();
        let mut left = [0.0; 128];
        let mut right = [0.0; 128];
        for _ in 0..20 {
            adapter.process_generator(1, 128, &mut left, &mut right, &[]);
            adapter.endpoint.work_between_callbacks();
        }
        assert!(adapter.midi_output().len > 0);
        adapter.process_generator(2, 128, &mut left, &mut right, &[]);
        assert_eq!(adapter.midi_output().len, 0);
        assert!(adapter.midi_output().lost);
        assert!(left.iter().all(|sample| *sample == 0.0));
    }
}
