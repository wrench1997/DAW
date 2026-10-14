//! Pointer ownership is independent of score history and of the Legacy scheduler.

use super::*;
use crate::audio::{AuditionDestination, AuditionStatus, AuditionToken};

const MAX_PENDING_AUDITIONS: usize = 32;

#[derive(Clone, Copy)]
struct PendingAudition {
    token: AuditionToken,
    due_frame: Option<u64>,
    end_requested: bool,
    end_queued: bool,
}

#[derive(Default)]
pub(super) struct KeyboardAuditionState {
    held: Option<AuditionToken>,
    pending: Vec<PendingAudition>,
    blocked_until_release: bool,
    tail_limit_engine_session: u64,
    seen_tail_limit_events: u64,
}

fn token_channel(token: AuditionToken) -> u32 {
    match token.destination {
        AuditionDestination::Native { channel_id, .. } => channel_id,
        AuditionDestination::Generator(route) => route.channel_id,
    }
}

impl CitrusApp {
    fn audition_destination(
        &self,
        channel_id: u32,
        mixer_track: usize,
    ) -> Result<AuditionDestination, &'static str> {
        let Some(channel) = self
            .project
            .channels
            .iter()
            .find(|channel| channel.id == channel_id)
        else {
            return Err("The audition channel is no longer available");
        };
        if self.clip_dormant_channel_audition_refused(channel_id) {
            return Err("Undo the conversion before auditioning its retained instrument");
        }
        let Some(instance_id) = channel.instrument_plugin_instance_id else {
            return Ok(AuditionDestination::Native {
                channel_id,
                mixer_track,
            });
        };
        let Some(plugin) = self
            .project
            .plugin_instances
            .iter()
            .find(|plugin| plugin.id == instance_id)
        else {
            return Err("The assigned instrument is missing; audition is unavailable");
        };
        if plugin.midi_ports.input.is_some()
            || plugin.midi_ports.output.is_some_and(|port| {
                self.project
                    .plugin_instances
                    .iter()
                    .any(|plugin| plugin.midi_ports.input == Some(port))
            })
        {
            return Err("Keyboard audition through a plug-in MIDI chain is not supported yet");
        }
        let Some(chain) = self.running_generator_chains.get(&channel_id) else {
            return Err("The assigned instrument is not ready; audition is unavailable");
        };
        if !plugin_runtime_session_is_current(chain.project_session, self.project_session)
            || !generator_instance_is_active(&self.project, channel_id, instance_id)
            || chain.instance_id != instance_id
            || chain.control.stats().faults != 0
        {
            return Err("The assigned instrument is not ready; audition is unavailable");
        }
        Ok(AuditionDestination::Generator(MidiGeneratorRouteStamp {
            project_session: self.project_session,
            channel_id,
            endpoint_id: chain.endpoint_id,
            plugin_instance_id: chain.instance_id,
            slot: Some(0),
        }))
    }

    fn queue_owned_audition(
        &mut self,
        channel_id: u32,
        note: u8,
        velocity: f32,
        mixer_track: usize,
        duration_frames: Option<u64>,
    ) -> Option<AuditionToken> {
        if self.clip_render_reject_control() {
            return None;
        }
        if self.keyboard_audition.pending.len() >= MAX_PENDING_AUDITIONS {
            self.notify("Audition is waiting for previous notes to release".into());
            return None;
        }
        let destination = match self.audition_destination(channel_id, mixer_track) {
            Ok(destination) => destination,
            Err(message) => {
                self.notify(message.into());
                return None;
            }
        };
        let audio = self.audio.as_mut()?;
        let due_frame = duration_frames.map(|duration| {
            audio
                .snapshot()
                .device_frame
                .saturating_add(duration.max(1))
        });
        let Some(token) = audio.begin_audition(destination, note, velocity) else {
            self.notify("Audition could not enter the audio queue; release and try again".into());
            return None;
        };
        self.keyboard_audition.pending.push(PendingAudition {
            token,
            due_frame,
            end_requested: false,
            end_queued: false,
        });
        Some(token)
    }

    fn cancel_keyboard_held_audition(&mut self, block_until_release: bool) {
        if let Some(token) = self.keyboard_audition.held.take() {
            if let Some(pending) = self
                .keyboard_audition
                .pending
                .iter_mut()
                .find(|pending| pending.token == token)
            {
                pending.end_requested = true;
            }
            self.keyboard_audition.blocked_until_release |= block_until_release;
        }
    }

    pub(super) fn cancel_all_keyboard_auditions(&mut self) {
        self.cancel_keyboard_held_audition(true);
        for pending in &mut self.keyboard_audition.pending {
            pending.end_requested = true;
        }
    }

    pub(super) fn drive_owned_audition_releases(&mut self) {
        let Some(audio) = self.audio.as_mut() else {
            self.keyboard_audition.pending.clear();
            self.keyboard_audition.blocked_until_release |= self.keyboard_audition.held.is_some();
            self.keyboard_audition.held = None;
            return;
        };
        let engine_session = audio.engine_session();
        if self.keyboard_audition.tail_limit_engine_session != engine_session {
            self.keyboard_audition.tail_limit_engine_session = engine_session;
            self.keyboard_audition.seen_tail_limit_events = 0;
        }
        let tail_limit_events = audio.audition_tail_limit_reached();
        let tail_limited = tail_limit_events > self.keyboard_audition.seen_tail_limit_events;
        self.keyboard_audition.seen_tail_limit_events = tail_limit_events;
        let snapshot = audio.snapshot();
        let mut index = 0;
        let mut rejected = false;
        while index < self.keyboard_audition.pending.len() {
            let pending = &mut self.keyboard_audition.pending[index];
            let token = pending.token;
            let status = audio.audition_status(token);
            let old_engine = token.engine_session != audio.engine_session();
            if old_engine || status.is_some_and(AuditionStatus::is_terminal) {
                rejected |= status == Some(AuditionStatus::Rejected);
                if self.keyboard_audition.held == Some(token) {
                    self.keyboard_audition.held = None;
                    self.keyboard_audition.blocked_until_release = true;
                }
                audio.finish_audition_receipt(token);
                self.keyboard_audition.pending.remove(index);
                continue;
            }
            if token.project_session != self.project_session
                || token.transport_epoch != snapshot.transport_epoch
            {
                pending.end_requested = true;
                if self.keyboard_audition.held == Some(token) {
                    self.keyboard_audition.held = None;
                    self.keyboard_audition.blocked_until_release = true;
                }
            }
            if pending
                .due_frame
                .is_some_and(|due| snapshot.device_frame >= due)
            {
                pending.end_requested = true;
            }
            if pending.end_requested && !pending.end_queued {
                pending.end_queued = audio.end_audition(token);
            }
            index += 1;
        }
        if rejected {
            self.notify("The audio callback rejected audition; release and press again".into());
        }
        if tail_limited {
            self.notify("Keyboard release monitoring reached its 30-second tail limit; the complete effect tail was not verified".into());
        }
    }

    pub(super) fn drive_keyboard_audition(&mut self, ctx: &egui::Context) {
        let (focused, primary_down) =
            ctx.input(|input| (input.focused, input.pointer.primary_down()));
        if !primary_down {
            self.keyboard_audition.blocked_until_release = false;
        }
        let selected_channel = self
            .project
            .channels
            .get(self.selected_channel)
            .map(|channel| channel.id);
        let held_route_changed = self.keyboard_audition.held.is_some_and(|token| {
            let channel_id = token_channel(token);
            let mixer_track = self
                .project
                .channels
                .iter()
                .find(|channel| channel.id == channel_id)
                .and_then(|channel| self.project.mixer_runtime_slot(channel.mixer_track));
            mixer_track.and_then(|track| self.audition_destination(channel_id, track).ok())
                != Some(token.destination)
        });
        let blocked = !focused
            || !primary_down
            || self.workspace.focused != StudioView::PianoRoll
            || !self.workspace.windows[workspace::index(StudioView::PianoRoll)].visible
            || self.piano_pointer_blocked()
            || ctx.any_popup_open()
            || self.project_snapshot_transition_pending()
            || self.project_lifecycle_barriers_active()
            || self.audio_restart_state.locks_session_actions()
            || held_route_changed
            || self
                .keyboard_audition
                .held
                .is_some_and(|token| Some(token_channel(token)) != selected_channel);
        if blocked {
            self.cancel_keyboard_held_audition(primary_down);
        }
        self.drive_owned_audition_releases();
        if !self.keyboard_audition.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(8));
        }
    }

    pub(super) fn update_keyboard_held_audition(
        &mut self,
        pitch: Option<u8>,
        route: Option<(u32, usize)>,
    ) {
        let desired = pitch.zip(route);
        let same = desired.is_some_and(|(pitch, (channel_id, mixer_track))| {
            self.keyboard_audition.held.is_some_and(|token| {
                token.note == pitch
                    && token_channel(token) == channel_id
                    && self.audition_destination(channel_id, mixer_track).ok()
                        == Some(token.destination)
            })
        });
        if same {
            return;
        }
        self.cancel_keyboard_held_audition(false);
        self.drive_owned_audition_releases();
        if self.keyboard_audition.blocked_until_release {
            return;
        }
        if let Some((pitch, (channel_id, mixer_track))) = desired {
            self.keyboard_audition.held =
                self.queue_owned_audition(channel_id, pitch, 0.75, mixer_track, None);
            if self.keyboard_audition.held.is_none() {
                self.keyboard_audition.blocked_until_release = true;
            }
        }
    }

    pub(super) fn trigger_channel_audition_note(
        &mut self,
        channel_id: u32,
        note: u8,
        velocity: f32,
        length_beats: f32,
        mixer_track: usize,
    ) {
        let Some(snapshot) = self.audio.as_ref().map(AudioEngine::snapshot) else {
            return;
        };
        let duration = note_duration_frames(
            self.tempo_map.as_ref(),
            self.beat_position,
            length_beats,
            self.effective_tempo,
            snapshot.sample_rate,
        );
        self.queue_owned_audition(channel_id, note, velocity, mixer_track, Some(duration));
    }

    pub(super) fn keyboard_auditions_pending(&self) -> bool {
        !self.keyboard_audition.pending.is_empty()
    }

    #[cfg(test)]
    pub(super) fn keyboard_held_token(&self) -> Option<AuditionToken> {
        self.keyboard_audition.held
    }

    #[cfg(test)]
    pub(super) fn keyboard_audition_pending_count(&self) -> usize {
        self.keyboard_audition.pending.len()
    }
}
