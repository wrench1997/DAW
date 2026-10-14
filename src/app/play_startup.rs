//! Control-plane Play admission. Worker preparation happens while the callback
//! and musical clock remain stopped; the eventual request reuses that epoch.
use super::*;

const PLAYBACK_PREPARATION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(super) struct PendingPlaybackStart {
    engine_session: u64,
    project_session: u64,
    generation: u64,
    fingerprint: u64,
    seek_serial: u64,
    timing_revision: u64,
    topology_revision: u64,
    pub(super) deadline: Instant,
    pub(super) issued_request: Option<u64>,
}

#[derive(Debug)]
pub(super) struct PendingLoopRestartWait {
    engine_session: u64,
    project_session: u64,
    epoch: u64,
    pub(super) deadline: Instant,
}

impl CitrusApp {
    pub(super) fn has_current_loop_restart_wait(&self) -> bool {
        self.audio.as_ref().is_some_and(|audio| {
            let snapshot = audio.snapshot();
            snapshot.transport_loop_restart_pending
                || (!snapshot.transport_playing
                    && self.loop_restart_wait.as_ref().is_some_and(|pending| {
                        pending.engine_session == audio.engine_session()
                            && pending.project_session == audio.project_session()
                            && pending.epoch == snapshot.transport_epoch
                    }))
        })
    }

    pub(super) fn cancel_current_loop_restart(&mut self) {
        // The caller has already observed the current loop wait. Worker completion
        // between that observation and this action cannot revoke the user's Pause.
        self.prepared_playback_start = None;
        self.loop_restart_wait = None;
        self.playing = false;
        self.transport_clock_playing = false;
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
    }

    pub(super) fn poll_loop_restart_wait(&mut self, ctx: &egui::Context) {
        let Some(audio) = self.audio.as_ref() else {
            self.loop_restart_wait = None;
            return;
        };
        let engine_session = audio.engine_session();
        let project_session = audio.project_session();
        let snapshot = audio.snapshot();
        let had_same_session_wait = self.loop_restart_wait.as_ref().is_some_and(|pending| {
            pending.engine_session == engine_session && pending.project_session == project_session
        });
        if self.loop_restart_wait.as_ref().is_some_and(|pending| {
            pending.engine_session != engine_session
                || pending.project_session != project_session
                || pending.epoch != snapshot.transport_epoch
        }) {
            // A wait from a discarded engine/project/epoch never controls its replacement.
            self.loop_restart_wait = None;
        }
        if !snapshot.transport_loop_restart_pending {
            self.loop_restart_wait = None;
            if had_same_session_wait && !snapshot.transport_playing {
                self.playing = false;
                self.transport_clock_playing = false;
            }
            return;
        }
        let pending = self
            .loop_restart_wait
            .get_or_insert_with(|| PendingLoopRestartWait {
                engine_session,
                project_session,
                epoch: snapshot.transport_epoch,
                deadline: Instant::now() + PLAYBACK_PREPARATION_TIMEOUT,
            });
        let timed_out = Instant::now() >= pending.deadline;
        let canceled = !self.playing
            || !self.project_lifecycle.is_idle()
            || !self.audio_restart_state.allows_realtime_session_actions();
        if timed_out || canceled {
            // Only the callback may resume a settled loop. UI cancellation is a newer
            // stopped request, so late worker completion cannot restore Play intent.
            self.loop_restart_wait = None;
            self.playing = false;
            self.transport_clock_playing = false;
            audio.set_playing(false);
            if timed_out {
                self.notify("Loop restart preparation timed out after 5 seconds; transport remains stopped. Press Play to prepare again".into());
            }
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(8));
    }

    pub(super) fn cancel_prepared_playback_start(&mut self) {
        let canceled_start = self.prepared_playback_start.take().is_some();
        let previous_loop_wait = self.loop_restart_wait.take();
        let canceled_loop = self.audio.as_ref().is_some_and(|audio| {
            audio.snapshot().transport_loop_restart_pending
                || previous_loop_wait.is_some_and(|pending| {
                    pending.engine_session == audio.engine_session()
                        && pending.project_session == audio.project_session()
                })
        });
        if canceled_start || canceled_loop {
            self.playing = false;
            self.transport_clock_playing = false;
            if let Some(audio) = &self.audio {
                audio.set_playing(false);
            }
        }
    }

    pub(super) fn begin_prepared_playback_start(&mut self, callback_owned: bool) {
        self.playing = false;
        self.transport_clock_playing = false;
        if self.save_barrier.is_some() {
            self.notify("Playback waits until the current project save is complete".into());
            return;
        }
        if !callback_owned || !self.timeline_audio_source_has_current_authority() {
            self.notify("Playback remains stopped until the current Timeline and plug-in graph are confirmed".into());
            return;
        }
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
        if !self.request_timeline_resume_chase() {
            self.notify("Playback preparation could not create the current stopped chase".into());
            return;
        }
        let audio = self.audio.as_ref().expect("resume chase requires audio");
        let source = self
            .timeline_audio_source
            .as_ref()
            .expect("resume chase requires source");
        self.prepared_playback_start = Some(PendingPlaybackStart {
            engine_session: audio.engine_session(),
            project_session: self.project_session,
            generation: source.generation,
            fingerprint: source.fingerprint,
            seek_serial: self.timeline_audio_sync.desired_seek.serial,
            timing_revision: audio.requested_plugin_timing_revision(),
            topology_revision: audio.plugin_topology_revision(),
            deadline: Instant::now() + PLAYBACK_PREPARATION_TIMEOUT,
            issued_request: None,
        });
    }

    pub(super) fn poll_prepared_playback_start(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.prepared_playback_start else {
            return;
        };
        let invalid = self.plugin_processing_blocks_playback()
            || self.save_barrier.is_some()
            || !self.audio_restart_state.allows_realtime_session_actions()
            || !self.project_lifecycle.is_idle()
            || self.pending_plugin_slot_config.is_some()
            || self.clip_render_blocks_playback()
            || self.clip_restoration_playback_pending
            || self.project_session != pending.project_session
            || self.timeline_desired_generation != pending.generation
            || self.timeline_audio_sync.desired_seek.serial != pending.seek_serial
            || !self.timeline_audio_source_has_current_authority()
            || self
                .timeline_audio_source
                .as_ref()
                .is_none_or(|source| source.fingerprint != pending.fingerprint)
            || self.audio.as_ref().is_none_or(|audio| {
                audio.engine_session() != pending.engine_session
                    || audio.project_session() != pending.project_session
                    || audio.requested_plugin_timing_revision() != pending.timing_revision
                    || audio.plugin_topology_revision() != pending.topology_revision
            });
        if invalid || Instant::now() >= pending.deadline {
            self.cancel_prepared_playback_start();
            self.notify(if invalid {
                "Playback preparation canceled because its project, seek or processing plan changed; transport remains stopped".into()
            } else {
                "Plug-in playback preparation timed out after 5 seconds; transport remains stopped. Press Play to prepare again".into()
            });
            return;
        }
        let audio = self.audio.as_ref().expect("validated preparation engine");
        if let Some(request) = pending.issued_request {
            let snapshot = audio.snapshot();
            if snapshot.applied_transport_request == request {
                self.prepared_playback_start = None;
                self.playing = snapshot.transport_playing;
                if !self.playing {
                    self.notify("The callback rejected a stale playback preparation; transport remains stopped".into());
                }
                return;
            }
            if audio.transport_request_id() != request {
                self.cancel_prepared_playback_start();
                self.notify(
                    "Playback preparation was superseded; transport remains stopped".into(),
                );
                return;
            }
        } else if self.timeline_audio_matches_desired()
            && let Some(token) = audio.prepared_playback_token()
            && token.engine_session == pending.engine_session
            && token.project_session == pending.project_session
            && token.timeline_revision == pending.generation
            && token.timing_revision == pending.timing_revision
            && token.topology_revision == pending.topology_revision
            && let TimelineAudioSyncPhase::Active { epoch, .. } = self.timeline_audio_sync.phase
            && token.epoch == epoch
            && token.timeline_frame == self.timeline_audio_sync.desired_seek.frame
            && let Some(request) = audio.start_prepared_playback(token)
        {
            self.prepared_playback_start
                .as_mut()
                .unwrap()
                .issued_request = Some(request);
        }
        ctx.request_repaint_after(Duration::from_millis(8));
    }
}
