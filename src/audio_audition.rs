//! Exact, bounded ownership for pointer-held keyboard notes. Queue admission is
//! separate from callback acceptance; no callback allocation or channel panic.

use super::*;

pub(super) const MAX_OWNED_AUDITIONS: usize = 16;
const AUDITION_RECEIPT_CAPACITY: usize = 32;
/// Interactive monitoring has a finite safety budget. This is not an export
/// tail guarantee; the App reports when an unknown/long tail reaches this limit.
const AUDITION_RELEASE_MONITOR_LIMIT_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditionDestination {
    Native { channel_id: u32, mixer_track: usize },
    Generator(MidiGeneratorRouteStamp),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuditionToken {
    pub engine_session: u64,
    pub project_session: u64,
    pub transport_epoch: u64,
    pub destination: AuditionDestination,
    pub note: u8,
    pub id: u64,
    receipt_slot: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum AuditionStatus {
    Pending,
    Accepted,
    Rejected,
    ReleasePending,
    Released,
    Cancelled,
}

impl AuditionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Rejected | Self::Released | Self::Cancelled)
    }
}

#[derive(Debug, Default)]
struct AuditionReceiptCell {
    owner: AtomicU64,
    status: AtomicU32,
}

#[derive(Debug)]
pub(super) struct AuditionReceipts {
    cells: [AuditionReceiptCell; AUDITION_RECEIPT_CAPACITY],
    tail_limit_reached: AtomicU64,
}

impl Default for AuditionReceipts {
    fn default() -> Self {
        Self {
            cells: std::array::from_fn(|_| AuditionReceiptCell::default()),
            tail_limit_reached: AtomicU64::new(0),
        }
    }
}

impl AuditionReceipts {
    fn reserve(&self, id: u64) -> Option<usize> {
        let index = self.cells.iter().position(|cell| {
            cell.owner
                .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        })?;
        self.cells[index]
            .status
            .store(AuditionStatus::Pending as u32, Ordering::Release);
        Some(index)
    }

    fn publish(&self, token: AuditionToken, status: AuditionStatus) {
        if let Some(cell) = self.cells.get(token.receipt_slot)
            && cell.owner.load(Ordering::Acquire) == token.id
        {
            cell.status.store(status as u32, Ordering::Release);
        }
    }

    fn read(&self, token: AuditionToken) -> Option<AuditionStatus> {
        let cell = self.cells.get(token.receipt_slot)?;
        if cell.owner.load(Ordering::Acquire) != token.id {
            return None;
        }
        let status = match cell.status.load(Ordering::Acquire) {
            0 => AuditionStatus::Pending,
            1 => AuditionStatus::Accepted,
            2 => AuditionStatus::Rejected,
            3 => AuditionStatus::ReleasePending,
            4 => AuditionStatus::Released,
            5 => AuditionStatus::Cancelled,
            _ => return None,
        };
        (cell.owner.load(Ordering::Acquire) == token.id).then_some(status)
    }

    fn release(&self, token: AuditionToken) {
        if let Some(cell) = self.cells.get(token.receipt_slot) {
            let _ = cell
                .owner
                .compare_exchange(token.id, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct OwnedAudition {
    token: AuditionToken,
    releasing: bool,
    cancelled: bool,
}

#[derive(Clone, Copy)]
pub(super) struct AuditionMonitorTail {
    token: AuditionToken,
    released: bool,
    cancelled: bool,
    quiet_frames: u64,
    released_frames: u64,
}

pub(super) fn audition_owns_generator_pitch(
    notes: &[Option<OwnedAudition>; MAX_OWNED_AUDITIONS],
    channel_id: u32,
    endpoint_id: u64,
    plugin_instance_id: u64,
    note: u8,
) -> bool {
    notes.iter().flatten().any(|held| {
        held.token.note == note
            && matches!(held.token.destination, AuditionDestination::Generator(route)
                if route.channel_id == channel_id && route.endpoint_id == endpoint_id
                    && route.plugin_instance_id == plugin_instance_id)
    })
}

impl AudioEngine {
    /// Returns a queued token. Inspect `audition_status` for actual callback admission.
    pub fn begin_audition(
        &mut self,
        destination: AuditionDestination,
        note: u8,
        velocity: f32,
    ) -> Option<AuditionToken> {
        if note > 127
            || !velocity.is_finite()
            || velocity <= 0.0
            || self.project_session() == 0
            || matches!(destination, AuditionDestination::Native { mixer_track, .. } if mixer_track >= TRACK_COUNT)
        {
            return None;
        }
        if let AuditionDestination::Generator(route) = destination
            && (route.project_session != self.project_session()
                || route.endpoint_id == 0
                || route.plugin_instance_id == 0
                || route
                    .slot
                    .is_some_and(|slot| slot >= MAX_INSERT_PLUGIN_SLOTS))
        {
            return None;
        }
        let id = self.next_audition_id;
        self.next_audition_id = id.checked_add(1)?;
        let receipt_slot = self.status.audition_receipts.reserve(id)?;
        let token = AuditionToken {
            engine_session: self.engine_session,
            project_session: self.project_session(),
            transport_epoch: self.snapshot().transport_epoch,
            destination,
            note,
            id,
            receipt_slot,
        };
        if self.command(AudioCommand::BeginAudition {
            token,
            velocity: velocity.min(1.0),
        }) {
            Some(token)
        } else {
            self.status.audition_receipts.release(token);
            None
        }
    }

    /// False leaves release ownership with the caller; retry the exact token.
    pub fn end_audition(&mut self, token: AuditionToken) -> bool {
        token.engine_session == self.engine_session
            && self.command(AudioCommand::EndAudition(token))
    }

    pub fn audition_status(&self, token: AuditionToken) -> Option<AuditionStatus> {
        (token.engine_session == self.engine_session)
            .then(|| self.status.audition_receipts.read(token))
            .flatten()
    }

    pub fn finish_audition_receipt(&self, token: AuditionToken) {
        if self
            .audition_status(token)
            .is_some_and(AuditionStatus::is_terminal)
        {
            self.status.audition_receipts.release(token);
        }
    }

    pub fn audition_tail_limit_reached(&self) -> u64 {
        self.status
            .audition_receipts
            .tail_limit_reached
            .load(Ordering::Acquire)
    }
}

impl DspState {
    fn audition_identity_is_current(&self, token: AuditionToken) -> bool {
        token.id != 0
            && token.engine_session == self.engine_session
            && token.project_session == self.project_session.load(Ordering::Acquire)
            && token.transport_epoch == self.transport_epoch
            && match token.destination {
                AuditionDestination::Native {
                    channel_id,
                    mixer_track,
                } => self
                    .timeline_channel_bases
                    .get(channel_id)
                    .is_none_or(|base| base.mixer_track == mixer_track),
                AuditionDestination::Generator(_) => true,
            }
    }

    fn audition_generator_index(&self, token: AuditionToken) -> Option<usize> {
        let AuditionDestination::Generator(route) = token.destination else {
            return None;
        };
        if route.project_session != token.project_session {
            return None;
        }
        self.generator_endpoints.iter().position(|slot| {
            slot.as_ref().is_some_and(|slot| {
                slot.channel_id == route.channel_id
                    && slot.endpoint_id == route.endpoint_id
                    && slot.plugin_instance_id == route.plugin_instance_id
                    && slot.endpoint.project_session == token.project_session
                    && slot.endpoint.epoch() == token.transport_epoch
                    && !slot.endpoint.midi_port_input
                    && slot.endpoint.manifest.instance_id(route.slot.unwrap_or(0))
                        == Some(route.plugin_instance_id)
            })
        })
    }

    pub(super) fn begin_owned_audition(&mut self, token: AuditionToken, velocity: f32) {
        let Some(index) = self.owned_auditions.iter().position(Option::is_none) else {
            self.audition_receipts
                .publish(token, AuditionStatus::Rejected);
            return;
        };
        if !velocity.is_finite()
            || velocity <= 0.0
            || token.note > 127
            || matches!(token.destination, AuditionDestination::Native { mixer_track, .. } if mixer_track >= TRACK_COUNT)
            || !self.audition_identity_is_current(token)
            || self.plugin_fault.is_some()
            || self.plugin_processing_suspended()
            || self.plugin_topology_replan_pending
            || self.audition_receipts.read(token) != Some(AuditionStatus::Pending)
        {
            self.audition_receipts
                .publish(token, AuditionStatus::Rejected);
            return;
        }
        let accepted = match token.destination {
            AuditionDestination::Native { mixer_track, .. } => {
                if let Some(voice_index) = self.voices.iter().position(|voice| !voice.active) {
                    let frequency = 440.0 * 2.0_f32.powf((token.note as f32 - 69.0) / 12.0);
                    self.voices[voice_index] = Voice {
                        phase: 0.0,
                        phase_step: frequency / self.sample_rate,
                        envelope: velocity.min(1.0) * 0.28,
                        decay: 1.0,
                        active: true,
                        mixer_track: mixer_track.min(TRACK_COUNT - 1),
                        timeline_note_id: None,
                        timeline_channel_id: match token.destination {
                            AuditionDestination::Native { channel_id, .. } => Some(channel_id),
                            _ => None,
                        },
                        audition_token_id: Some(token.id),
                    };
                    self.native_audition_tracks |= 1_u32 << mixer_track;
                    true
                } else {
                    false
                }
            }
            AuditionDestination::Generator(route) => {
                if let Some(generator_index) = self.audition_generator_index(token) {
                    let owned = audition_owns_generator_pitch(
                        &self.owned_auditions,
                        route.channel_id,
                        route.endpoint_id,
                        route.plugin_instance_id,
                        token.note,
                    ) || self.timeline_generator_notes.iter().flatten().any(|note| {
                        note.channel_id == route.channel_id
                            && note.endpoint_id == route.endpoint_id
                            && note.plugin_instance_id == route.plugin_instance_id
                            && note.note == token.note
                    });
                    owned
                        || self.stage_audition_midi(
                            generator_index,
                            FrameEvent::midi(
                                0,
                                route.slot.map(|slot| slot as u8),
                                [0x90, token.note, ((velocity * 127.0).round() as u8).max(1)],
                            ),
                        )
                } else {
                    false
                }
            }
        };
        if accepted {
            self.owned_audition_paused_isolation = true;
            self.owned_auditions[index] = Some(OwnedAudition {
                token,
                releasing: false,
                cancelled: false,
            });
            if let Some(generator_index) = self.audition_generator_index(token) {
                self.audition_monitor_tails[generator_index] = Some(AuditionMonitorTail {
                    token,
                    released: false,
                    cancelled: false,
                    quiet_frames: 0,
                    released_frames: 0,
                });
            }
        }
        self.audition_receipts.publish(
            token,
            if accepted {
                AuditionStatus::Accepted
            } else {
                AuditionStatus::Rejected
            },
        );
    }

    pub(super) fn end_owned_audition(&mut self, token: AuditionToken) {
        if let Some(held) = self
            .owned_auditions
            .iter_mut()
            .flatten()
            .find(|held| held.token == token)
        {
            held.releasing = true;
            self.audition_receipts
                .publish(token, AuditionStatus::ReleasePending);
            self.service_owned_auditions();
        }
    }

    pub(super) fn service_owned_auditions(&mut self) {
        for index in 0..MAX_OWNED_AUDITIONS {
            let Some(mut held) = self.owned_auditions[index] else {
                continue;
            };
            let token = held.token;
            let exact = self.audition_identity_is_current(token)
                && (!matches!(token.destination, AuditionDestination::Generator(_))
                    || self.audition_generator_index(token).is_some());
            if held.cancelled
                && (token.transport_epoch != self.transport_epoch
                    || self.audition_generator_index(token).is_none()
                    || self.plugin_fault.is_some()
                    || self.plugin_topology_replan_pending)
            {
                self.cancel_owned_audition(index);
                continue;
            }
            if !held.cancelled
                && (!exact || self.plugin_fault.is_some() || self.plugin_topology_replan_pending)
            {
                self.cancel_owned_audition(index);
                let Some(cancelled) = self.owned_auditions[index] else {
                    continue;
                };
                held = cancelled;
            }
            if !held.releasing {
                continue;
            }
            let released = match token.destination {
                AuditionDestination::Native { .. } => {
                    for voice in &mut self.voices {
                        if voice.audition_token_id == Some(token.id) {
                            voice.decay = 0.995;
                        }
                    }
                    true
                }
                AuditionDestination::Generator(route) => {
                    let other = self.owned_auditions.iter().enumerate().any(|(other_index, other)| {
                        other_index != index && other.is_some_and(|other| {
                            other.token.note == token.note
                                && matches!(other.token.destination, AuditionDestination::Generator(other_route)
                                    if other_route.channel_id == route.channel_id
                                        && other_route.endpoint_id == route.endpoint_id
                                        && other_route.plugin_instance_id == route.plugin_instance_id)
                        })
                    }) || self.timeline_generator_notes.iter().flatten().any(|note| {
                        note.channel_id == route.channel_id && note.endpoint_id == route.endpoint_id
                            && note.plugin_instance_id == route.plugin_instance_id && note.note == token.note
                    });
                    let generator_index = self.audition_generator_index(token).unwrap();
                    other
                        || self.stage_audition_midi(
                            generator_index,
                            FrameEvent::midi(
                                0,
                                route.slot.map(|slot| slot as u8),
                                [0x80, token.note, 0],
                            ),
                        )
                }
            };
            if released {
                self.owned_auditions[index] = None;
                if let Some(generator_index) = self.audition_generator_index(token)
                    && !self.owned_auditions.iter().flatten().any(|other| {
                        self.audition_generator_index(other.token) == Some(generator_index)
                    })
                    && let Some(tail) = self.audition_monitor_tails[generator_index].as_mut()
                {
                    tail.released = true;
                }
                self.audition_receipts.publish(
                    token,
                    if held.cancelled {
                        AuditionStatus::Cancelled
                    } else {
                        AuditionStatus::Released
                    },
                );
            }
        }
    }

    fn cancel_owned_audition(&mut self, index: usize) {
        if let Some(held) = self.owned_auditions[index] {
            // Pause/clear in the same physical epoch must actually deliver an
            // owned Off. Keep its bounded lease when the Live lane is full;
            // global callback service retries without fencing a future press.
            if let Some(generator_index) = self.audition_generator_index(held.token)
                && held.token.transport_epoch == self.transport_epoch
                && self.plugin_fault.is_none()
                && !self.plugin_topology_replan_pending
            {
                let owned = self.owned_auditions[index].as_mut().unwrap();
                owned.releasing = true;
                owned.cancelled = true;
                if let Some(tail) = self.audition_monitor_tails[generator_index].as_mut() {
                    tail.cancelled = true;
                }
                self.audition_receipts
                    .publish(held.token, AuditionStatus::ReleasePending);
                return;
            }
            self.owned_auditions[index] = None;
            for voice in &mut self.voices {
                if voice.audition_token_id == Some(held.token.id) {
                    voice.active = false;
                    voice.audition_token_id = None;
                }
            }
            self.audition_receipts
                .publish(held.token, AuditionStatus::Cancelled);
        }
    }

    fn stage_audition_midi(&mut self, index: usize, event: FrameEvent) -> bool {
        let endpoint = &mut self.generator_endpoints[index].as_mut().unwrap().endpoint;
        let events = [event];
        let classes = [EndpointEventClass::Live];
        if endpoint.events.scratch.live_faulted || !endpoint.can_stage_batch(&events, &classes) {
            return false;
        }
        endpoint.stage_preflighted_batch(&events, &classes);
        true
    }

    pub(super) fn cancel_all_owned_auditions(&mut self) {
        for index in 0..MAX_OWNED_AUDITIONS {
            self.cancel_owned_audition(index);
        }
        for index in 0..MAX_GENERATOR_ENDPOINTS {
            if self.audition_monitor_tails[index].is_some_and(|tail| {
                tail.token.transport_epoch != self.transport_epoch
                    || self.audition_generator_index(tail.token) != Some(index)
            }) {
                self.audition_monitor_tails[index] = None;
            }
        }
        self.native_audition_tracks = 0;
        self.native_audition_quiet_frames.fill(0);
    }

    /// A faulted worker cannot accept an Off. Retire only these exact leases;
    /// recovery creates/fences its own processing epoch before music can resume.
    pub(super) fn cancel_faulted_owned_auditions(&mut self) {
        self.cancel_all_owned_auditions();
        self.audition_monitor_tails.fill(None);
    }

    pub(super) fn cancel_endpoint_auditions(&mut self, endpoint_id: u64, plugin_instance_id: u64) {
        for index in 0..MAX_OWNED_AUDITIONS {
            if self.owned_auditions[index].is_some_and(|held| {
                matches!(held.token.destination, AuditionDestination::Generator(route)
                    if route.endpoint_id == endpoint_id && route.plugin_instance_id == plugin_instance_id)
            }) { self.cancel_owned_audition(index); }
        }
        for tail in &mut self.audition_monitor_tails {
            if tail.is_some_and(|tail| matches!(tail.token.destination, AuditionDestination::Generator(route)
                if route.endpoint_id == endpoint_id && route.plugin_instance_id == plugin_instance_id))
            { *tail = None; }
        }
    }

    pub(super) fn next_non_audition_voice(&self) -> usize {
        (0..MAX_VOICES)
            .map(|offset| (self.next_voice + offset) % MAX_VOICES)
            .find(|index| {
                self.voices[*index].audition_token_id.is_none() || self.voices[*index].decay < 1.0
            })
            .expect("bounded auditions leave at least one native timeline voice")
    }

    pub(super) fn has_paused_owned_auditions(&self) -> bool {
        self.native_audition_tracks != 0 || self.audition_monitor_tails.iter().any(Option::is_some)
    }

    /// Paused keyboard monitoring advances only owned sources and their downstream
    /// effects. Timeline cursors, unrelated sources and unrelated delay lines freeze.
    pub(super) fn render_paused_owned_auditions(
        &mut self,
        status: &AudioStatus,
        frames: usize,
        external: Option<PausedMidiMonitorRoute>,
    ) -> PausedEndpointProgress {
        let mut progress = PausedEndpointProgress::default();
        let mut downstream = [false; TRACK_COUNT];
        let audible_frames = self.owned_audition_monitor_output_budget(frames, external.is_some());
        self.meter_graph_rendered = false;
        for track in 0..TRACK_COUNT {
            self.track_block
                [track * MAX_MIXER_BLOCK_FRAMES..track * MAX_MIXER_BLOCK_FRAMES + frames]
                .fill([0.0; 2]);
        }
        self.master_block[..frames].fill([0.0; 2]);
        if self.mixer_graph_was_activated {
            if !self.mixer_graph_binding_is_exact() || !self.refresh_graph_pdc_plan(status) {
                self.fail_mixer_graph_render(frames);
                return progress;
            }
        } else {
            self.refresh_pdc_plan(status, frames);
        }
        for (track, active) in downstream.iter_mut().enumerate() {
            *active = self.native_audition_tracks & (1_u32 << track) != 0;
        }
        for frame in 0..frames {
            self.render_synth_frame_filtered(frame, true);
        }
        for (track, active) in downstream.iter().copied().enumerate() {
            if !active {
                continue;
            }
            let mut quiet = true;
            for frame in 0..frames {
                let index = track * MAX_MIXER_BLOCK_FRAMES + frame;
                let source = self.track_block[index];
                let delayed = self.pdc_raw_track_delays[track].process_sample(source);
                quiet &= source == [0.0; 2] && delayed == [0.0; 2];
                self.track_block[index] = delayed;
            }
            self.native_audition_quiet_frames[track] = if quiet {
                self.native_audition_quiet_frames[track].saturating_add(frames as u64)
            } else {
                0
            };
        }
        for index in 0..MAX_GENERATOR_ENDPOINTS {
            let tail = self.audition_monitor_tails[index];
            let external_route = external.filter(|external| external.generator_index == index);
            if tail.is_none() && external_route.is_none() {
                continue;
            }
            if let Some(tail) = tail
                && ((!tail.cancelled && !self.audition_identity_is_current(tail.token))
                    || tail.token.transport_epoch != self.transport_epoch
                    || self.audition_generator_index(tail.token) != Some(index))
            {
                self.audition_monitor_tails[index] = None;
                if external_route.is_none() {
                    continue;
                }
            }
            let Some(slot) = self.generator_endpoints[index].as_mut() else {
                continue;
            };
            let (track, volume, pan, audible) = self
                .timeline_channel_bases
                .get(slot.channel_id)
                .map_or((slot.mixer_track, 1.0, 0.0, true), |base| {
                    (
                        base.mixer_track,
                        base.volume,
                        base.pan,
                        self.timeline_channel_bases.is_audible(base),
                    )
                });
            let had_admitted_marker = slot.endpoint.has_admitted_live_edit_marker();
            let before = slot.endpoint.stats();
            let process = slot.endpoint.process_generator(
                self.transport_epoch,
                frames,
                &mut self.plugin_output_left[..frames],
                &mut self.plugin_output_right[..frames],
            );
            progress.generator_mask |= 1_u64 << index;
            let diagnostics = fixed_quantum_stats_delta(before, slot.endpoint.stats());
            if had_admitted_marker && !slot.endpoint.has_admitted_live_edit_marker() {
                self.admitted_live_edit_endpoint_count =
                    self.admitted_live_edit_endpoint_count.saturating_sub(1);
            }
            let exact = matches!(process, FixedQuantumProcessStatus::Processed { frames: count, .. } if count == frames);
            let suppressed = slot.suppress_output_frames != 0;
            slot.suppress_output_frames = slot.suppress_output_frames.saturating_sub(frames);
            let mut quiet = true;
            for frame in 0..frames {
                let source = if exact
                    && !suppressed
                    && audible
                    && !tail.is_some_and(|tail| tail.cancelled)
                {
                    [
                        safe_plugin_sample(self.plugin_output_left[frame])
                            * volume
                            * if pan > 0.0 { 1.0 - pan } else { 1.0 },
                        safe_plugin_sample(self.plugin_output_right[frame])
                            * volume
                            * if pan < 0.0 { 1.0 + pan } else { 1.0 },
                    ]
                } else {
                    [0.0; 2]
                };
                let delayed = slot.pdc_delay.process_sample(source);
                quiet &= source == [0.0; 2] && delayed == [0.0; 2];
                let destination = &mut self.track_block[track * MAX_MIXER_BLOCK_FRAMES + frame];
                destination[0] += delayed[0];
                destination[1] += delayed[1];
            }
            downstream[track] = true;
            self.accumulate_fixed_quantum_diagnostics(diagnostics);
            if diagnostics.event_overflows != 0
                || diagnostics.endpoint_event_rejections != 0
                || diagnostics.invalid_events != 0
                || diagnostics.latency_drift_quanta != 0
            {
                self.fail_timeline_block();
                self.master_block[..frames].fill([0.0; 2]);
                return progress;
            }
            if let Some(tail) = self.audition_monitor_tails[index].as_mut() {
                tail.quiet_frames = if quiet {
                    tail.quiet_frames.saturating_add(frames as u64)
                } else {
                    0
                };
                // Drain Note Off, Q128 output, source PDC and the actual instrument
                // release instead of truncating it at pointer-up. A continuous
                // audible instrument legitimately keeps its monitor active.
            }
        }
        if self.mixer_graph_was_activated {
            let progress = self.finish_paused_monitor_graph(status, frames, downstream, progress);
            self.master_block[audible_frames..frames].fill([0.0; 2]);
            self.finish_owned_audition_monitor_tails(frames);
            return progress;
        }
        let any_solo = self.track_solo[1..].iter().any(|solo| *solo);
        for (track, active) in downstream.iter().copied().enumerate().skip(1) {
            if !active {
                continue;
            }
            if self.insert_endpoints[track].is_some() {
                progress.insert_mask |= 1_u32 << track;
            }
            if !self.process_insert_endpoint(track, frames) {
                self.master_block[..frames].fill([0.0; 2]);
                return progress;
            }
            if self.track_muted[track] || (any_solo && !self.track_solo[track]) {
                continue;
            }
            let gain = self.track_gains[track];
            let pan = self.track_pans[track];
            for frame in 0..frames {
                let source = self.track_block[track * MAX_MIXER_BLOCK_FRAMES + frame];
                self.master_block[frame][0] +=
                    source[0] * gain * if pan > 0.0 { 1.0 - pan } else { 1.0 };
                self.master_block[frame][1] +=
                    source[1] * gain * if pan < 0.0 { 1.0 + pan } else { 1.0 };
            }
        }
        for frame in 0..frames {
            self.master_block[frame][0] += self.track_block[frame][0];
            self.master_block[frame][1] += self.track_block[frame][1];
        }
        if self.insert_endpoints[0].is_some() {
            progress.insert_mask |= 1;
        }
        if !self.process_insert_endpoint(0, frames) {
            self.master_block[..frames].fill([0.0; 2]);
            return progress;
        }
        for frame in &mut self.master_block[..frames] {
            *frame = if self.track_muted[0] {
                [0.0; 2]
            } else {
                [
                    (frame[0]
                        * self.master
                        * if self.master_pan > 0.0 {
                            1.0 - self.master_pan
                        } else {
                            1.0
                        })
                    .tanh(),
                    (frame[1]
                        * self.master
                        * if self.master_pan < 0.0 {
                            1.0 + self.master_pan
                        } else {
                            1.0
                        })
                    .tanh(),
                ]
            };
        }
        self.master_block[audible_frames..frames].fill([0.0; 2]);
        self.publish_plugin_epoch_status(status);
        self.finish_owned_audition_monitor_tails(frames);
        progress
    }

    /// If every remaining source belongs solely to released generator leases,
    /// their absolute interactive deadline bounds this callback's audible prefix.
    /// A still-held key or physical MIDI monitor keeps its own authorization.
    fn owned_audition_monitor_output_budget(&self, frames: usize, external: bool) -> usize {
        if external || self.native_audition_tracks != 0 {
            return frames;
        }
        let limit =
            (self.sample_rate as u64).saturating_mul(AUDITION_RELEASE_MONITOR_LIMIT_SECONDS);
        let mut remaining = 0;
        for tail in self.audition_monitor_tails.iter().flatten() {
            if !tail.released {
                return frames;
            }
            remaining = remaining.max(limit.saturating_sub(tail.released_frames));
        }
        remaining.min(frames as u64) as usize
    }

    fn finish_owned_audition_monitor_tails(&mut self, frames: usize) {
        let master_quiet = self.master_block[..frames]
            .iter()
            .all(|frame| *frame == [0.0; 2]);
        let limit =
            (self.sample_rate as u64).saturating_mul(AUDITION_RELEASE_MONITOR_LIMIT_SECONDS);
        for track in 0..TRACK_COUNT {
            if self.native_audition_tracks & (1_u32 << track) == 0 {
                continue;
            }
            let (budget, limited) = self.owned_audition_drain_budget(track, None);
            let quiet = self.native_audition_quiet_frames[track];
            if (master_quiet && quiet >= budget) || quiet >= limit {
                self.native_audition_tracks &= !(1_u32 << track);
                if limited || !master_quiet {
                    self.audition_receipts
                        .tail_limit_reached
                        .fetch_add(1, Ordering::Release);
                }
            }
        }
        for index in 0..MAX_GENERATOR_ENDPOINTS {
            let Some(mut tail) = self.audition_monitor_tails[index] else {
                continue;
            };
            if !tail.released {
                continue;
            }
            tail.released_frames = tail.released_frames.saturating_add(frames as u64);
            self.audition_monitor_tails[index] = Some(tail);
            let Some(slot) = self.generator_endpoints[index].as_ref() else {
                continue;
            };
            let track = self
                .timeline_channel_bases
                .get(slot.channel_id)
                .map_or(slot.mixer_track, |base| base.mixer_track);
            let (budget, limited) = self.owned_audition_drain_budget(track, Some(index));
            if (master_quiet && tail.quiet_frames >= budget) || tail.released_frames >= limit {
                self.audition_monitor_tails[index] = None;
                if limited || !master_quiet || tail.quiet_frames < budget {
                    self.audition_receipts
                        .tail_limit_reached
                        .fetch_add(1, Ordering::Release);
                }
            }
        }
    }

    /// Conservatively sum only the monitored downstream paths. A delayed effect
    /// may be silent now while retaining a future release, so source silence plus
    /// one zero Master block alone is insufficient to retire its processing.
    fn owned_audition_drain_budget(
        &self,
        source_track: usize,
        generator: Option<usize>,
    ) -> (u64, bool) {
        let limit =
            (self.sample_rate as u64).saturating_mul(AUDITION_RELEASE_MONITOR_LIMIT_SECONDS);
        let mut budget = self.sample_rate as u64;
        let mut unknown = false;
        let mut downstream = [false; TRACK_COUNT];
        downstream[source_track] = true;
        if self.mixer_graph_was_activated {
            for source in self
                .mixer_graph_plan
                .topological_runtime_slots()
                .iter()
                .copied()
            {
                if !downstream[usize::from(source)] {
                    continue;
                }
                for route_slot in self.mixer_graph_plan.outgoing_route_slots(source) {
                    if let Some(route) = self.mixer_graph_plan.route_at_runtime_slot(*route_slot) {
                        downstream[usize::from(route.destination_runtime_slot)] = true;
                        if let Some(bank) = self
                            .timeline_runtime
                            .as_ref()
                            .and_then(RealtimeTimelineRuntime::active_mixer_delay_bank)
                        {
                            let delay = bank
                                .current_delay_samples(usize::from(*route_slot))
                                .unwrap_or(0)
                                .max(
                                    bank.target_delay_samples(usize::from(*route_slot))
                                        .unwrap_or(0),
                                );
                            budget = budget.saturating_add(u64::from(delay));
                        }
                    }
                }
            }
        } else {
            downstream[0] = true;
        }
        for (track, active) in downstream.into_iter().enumerate() {
            if !active {
                continue;
            }
            if let Some(slot) = &self.insert_endpoints[track] {
                let (frames, infinite) = Self::owned_endpoint_drain_budget(&slot.endpoint);
                budget = budget.saturating_add(frames);
                unknown |= infinite;
            }
        }
        if let Some(index) = generator {
            if let Some(slot) = &self.generator_endpoints[index] {
                let (frames, infinite) = Self::owned_endpoint_drain_budget(&slot.endpoint);
                budget = budget.saturating_add(frames).saturating_add(u64::from(
                    slot.pdc_delay
                        .current_delay_samples()
                        .max(slot.pdc_delay.target_delay_samples()),
                ));
                unknown |= infinite;
            }
        } else {
            let delay = &self.pdc_raw_track_delays[source_track];
            budget = budget.saturating_add(u64::from(
                delay
                    .current_delay_samples()
                    .max(delay.target_delay_samples()),
            ));
        }
        let limited = unknown || budget > limit;
        (if limited { limit } else { budget }, limited)
    }

    fn owned_endpoint_drain_budget(endpoint: &PreparedFixedEndpoint) -> (u64, bool) {
        let Some(snapshot) = endpoint.coherent_latency_snapshot() else {
            return (0, true);
        };
        let unknown = snapshot.tail_samples == u32::MAX;
        let tail = if unknown {
            0
        } else {
            u64::from(snapshot.tail_samples)
        };
        (
            tail.saturating_add(u64::from(snapshot.total_plugin_latency_samples))
                .saturating_add(endpoint.adapter.latency().total_frames as u64),
            unknown,
        )
    }
}
