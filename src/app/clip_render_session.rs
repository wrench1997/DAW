//! Post-save/discard session intent barrier for restored conversion sources.
//! Bindings and verified media remain in the old session until the actual old
//! callback removal and exact source JoinHandles have both been observed.
use super::*;
use crate::plugins::plugin_runtime::RetainedShutdownPoll;

struct SessionSource {
    channel_id: u32,
    endpoint_id: u64,
    instance_id: u64,
    queued: bool,
    acknowledged: bool,
    source: Option<RunningGeneratorChain>,
    joined: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionAdmission {
    NotSent,
    StopQueued,
    RemovalAdmitted,
}

pub(super) struct ClipSessionRetirement {
    intent: ProjectIntent,
    prepared_open: Option<Project>,
    prepared_open_path: Option<PathBuf>,
    project_session: u64,
    engine_session: u64,
    model_revision: u64,
    sources: Vec<SessionSource>,
    preflight_completed: bool,
    admission: SessionAdmission,
    stop_observed: bool,
    failure_pause_requested: bool,
    failure: Option<String>,
}
impl ClipSessionRetirement {
    fn fail(&mut self, reason: &str) {
        if self.failure.is_none() {
            self.failure = Some(reason.into());
        }
    }
    #[cfg(test)]
    pub(in crate::app) fn abandon_test_intent(&mut self) {
        self.fail("Fixture cleanup retained the session retirement owners");
    }
    #[cfg(test)]
    pub(in crate::app) fn poll_test_shutdown_retained(&mut self) -> Result<bool, &'static str> {
        let mut terminal = true;
        for target in &mut self.sources {
            if let Some(source) = &mut target.source {
                match source.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => terminal = false,
                    RetainedShutdownPoll::Joined => target.joined = true,
                    RetainedShutdownPoll::WorkerPanicked => {
                        return Err("session source worker panicked during fixture cleanup");
                    }
                }
            }
        }
        Ok(terminal)
    }
}
impl CitrusApp {
    pub(super) fn clip_session_retirement_pending(&self) -> bool {
        self.clip_session_retirement.is_some()
    }
    pub(super) fn clip_session_retirement_reason(&self) -> Option<&str> {
        self.clip_session_retirement.as_ref().map(|stage| {
            stage.failure.as_deref().unwrap_or(
                "Waiting for old restored sources to be removed and joined before changing session",
            )
        })
    }
    pub(super) fn can_cancel_clip_session_retirement(&self) -> bool {
        let Some(stage) = &self.clip_session_retirement else {
            return false;
        };
        // Admission can fail before reserving/building any target list. No
        // transport/removal command or guard transfer occurred, so dismissal
        // does not require a nonempty list or manufacture an ownership proof.
        if !stage.preflight_completed
            && stage.admission == SessionAdmission::NotSent
            && stage.sources.iter().all(|target| {
                !target.queued && !target.acknowledged && !target.joined && target.source.is_none()
            })
        {
            return true;
        }
        stage.preflight_completed
            && stage.admission != SessionAdmission::RemovalAdmitted
            && !stage.sources.is_empty()
            && self.project_session == stage.project_session
            && self.clip_render_model_revision == stage.model_revision
            && self.audio.as_ref().is_some_and(|audio| {
                audio.engine_session() == stage.engine_session
                    && audio.project_session() == stage.project_session
            })
            && self
                .running_generator_chains
                .values()
                .filter(|source| {
                    self.clip_render_restorations
                        .contains_key(&source.instance_id)
                })
                .count()
                == stage.sources.len()
            && stage.sources.iter().all(|target| {
                !target.queued
                    && !target.acknowledged
                    && !target.joined
                    && target.source.is_none()
                    && !self
                        .pending_generator_removals
                        .contains_key(&target.channel_id)
                    && !self
                        .pending_generator_chains
                        .contains_key(&target.channel_id)
                    && self
                        .running_generator_chains
                        .get(&target.channel_id)
                        .is_some_and(|source| {
                            source.project_session == stage.project_session
                                && source.endpoint_id == target.endpoint_id
                                && source.instance_id == target.instance_id
                        })
            })
    }
    /// Explicit user cancellation is safe only before any removal admission or
    /// ownership transfer, with the same actual source maps still intact.
    pub(super) fn cancel_clip_session_retirement(&mut self) -> bool {
        if !self.can_cancel_clip_session_retirement() {
            return false;
        }
        self.clip_session_retirement = None;
        self.pending_discard_quit_session = None;
        self.playing = false;
        self.transport_clock_playing = false;
        if let Some(audio) = &self.audio {
            audio.set_playing(false);
        }
        true
    }
    /// Called only at the existing post-save/discard commit-intent boundary.
    /// Ordinary intents with no restored bound source retain their old path.
    pub(super) fn defer_clip_session_intent(
        &mut self,
        intent: ProjectIntent,
    ) -> Option<ProjectIntent> {
        if self.clip_session_retirement.is_some() {
            return None;
        }
        let count = self
            .running_generator_chains
            .values()
            .filter(|source| {
                self.clip_render_restorations
                    .contains_key(&source.instance_id)
            })
            .count();
        if count == 0 {
            return Some(intent);
        }
        // Keep the accepted intent outside every fallible preflight/catch. Even
        // an Open read failure retains its exact path and the old session.
        let mut stage = ClipSessionRetirement {
            intent,
            prepared_open: None,
            prepared_open_path: None,
            project_session: self.project_session,
            engine_session: self.audio.as_ref().map_or(0, AudioEngine::engine_session),
            model_revision: self.clip_render_model_revision,
            sources: Vec::new(),
            preflight_completed: false,
            admission: SessionAdmission::NotSent,
            stop_observed: false,
            failure_pause_requested: false,
            failure: None,
        };
        let prepared =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                if self.clip_render_fence_active()
                    || self.clip_restoration_pending()
                    || self.plugin_topology_is_settling()
                    || !self.audio_restart_state.allows_realtime_session_actions()
                    || count > 4096
                {
                    return Err(
                        "Finish the active conversion/restoration before changing session".into(),
                    );
                }
                let audio = self
                    .audio
                    .as_ref()
                    .ok_or("Audio is unavailable for exact restored-source retirement")?;
                if audio.project_session() != self.project_session {
                    return Err(
                        "Audio and Project sessions differ before restored-source retirement"
                            .into(),
                    );
                }
                stage
                    .sources
                    .try_reserve_exact(count)
                    .map_err(|_| "Unable to reserve restored-source retirement owners")?;
                self.pending_generator_removals
                    .try_reserve(count)
                    .map_err(|_| "Unable to reserve exact restored-source removal receipts")?;
                for (&channel_id, source) in &self.running_generator_chains {
                    let Some(binding) = self.clip_render_restorations.get(&source.instance_id)
                    else {
                        continue;
                    };
                    if source.project_session != self.project_session
                        || binding.project_session() != self.project_session
                        || self.pending_generator_removals.contains_key(&channel_id)
                        || self.pending_generator_chains.contains_key(&channel_id)
                    {
                        return Err(
                            "A restored source changed before session retirement admission".into(),
                        );
                    }
                    stage.sources.push(SessionSource {
                        channel_id,
                        endpoint_id: source.endpoint_id,
                        instance_id: source.instance_id,
                        queued: false,
                        acknowledged: false,
                        source: None,
                        joined: false,
                    });
                }
                if let ProjectIntent::Open { path } = &stage.intent {
                    if path.as_os_str().len() > 4096 {
                        return Err("Open path exceeds the bounded session-intent storage".into());
                    }
                    let mut preserved = PathBuf::new();
                    preserved
                        .try_reserve_exact(path.as_os_str().len())
                        .map_err(|_| "Unable to reserve the prepared Open path")?;
                    preserved.push(path);
                    // Existing Open validation, exactly once after the save/discard
                    // decision and before any source-removal command. No clone.
                    stage.prepared_open = Some(Project::load(path).map_err(|error| {
                        format!("Open could not be prepared before source retirement: {error}")
                    })?);
                    stage.prepared_open_path = Some(preserved);
                }
                stage.preflight_completed = true;
                Ok(())
            }));
        match prepared {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => stage.failure = Some(reason),
            Err(_) => stage.fail(
                "Session preflight panicked; accepted intent and old source owners remain held",
            ),
        }
        let admitted = stage.preflight_completed;
        self.clip_session_retirement = Some(stage);
        if admitted {
            self.playing = false;
            self.transport_clock_playing = false;
            if let Some(audio) = &self.audio {
                audio.set_playing(false);
            }
        }
        None
    }
    /// This transfer performs no allocation or panic-capable operation before
    /// the incoming guard is installed into its pre-reserved exact owner slot.
    // Refusal retains the exact incoming worker owner without error-path allocation.
    #[allow(clippy::result_large_err)]
    pub(super) fn retain_clip_session_source(
        &mut self,
        source: RunningGeneratorChain,
    ) -> Result<(), RunningGeneratorChain> {
        let Some(stage) = &mut self.clip_session_retirement else {
            return Err(source);
        };
        if source.project_session != stage.project_session {
            return Err(source);
        }
        let Some(target) = stage.sources.iter_mut().find(|target| {
            target.endpoint_id == source.endpoint_id
                && target.instance_id == source.instance_id
                && target.source.is_none()
                && !target.joined
        }) else {
            return Err(source);
        };
        target.source = Some(source);
        stage.fail("Old audio session disappeared during source retirement; removal outcome is unknown and the exact owner is retained, playback paused");
        Ok(())
    }
    pub(super) fn consume_clip_session_removal(
        &mut self,
        channel_id: u32,
        endpoint_id: Option<u64>,
        plugin_instance_id: Option<u64>,
    ) -> bool {
        if !self.clip_session_retirement.as_ref().is_some_and(|stage| {
            stage
                .sources
                .iter()
                .any(|target| target.channel_id == channel_id)
        }) {
            return false;
        }
        let mut stage = self
            .clip_session_retirement
            .take()
            .expect("checked session owner");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.consume_clip_session_removal_owned(
                &mut stage,
                channel_id,
                endpoint_id,
                plugin_instance_id,
            );
        }));
        if result.is_err() {
            stage.fail("Session source removal handling panicked; exact owners remain retained and playback paused");
        }
        self.clip_session_retirement = Some(stage);
        true
    }
    fn consume_clip_session_removal_owned(
        &mut self,
        stage: &mut ClipSessionRetirement,
        channel_id: u32,
        endpoint_id: Option<u64>,
        instance_id: Option<u64>,
    ) {
        let index = stage
            .sources
            .iter()
            .position(|target| target.channel_id == channel_id)
            .expect("matched channel");
        let target = &stage.sources[index];
        if target.acknowledged {
            return;
        } // Duplicate receipt cannot take another source.
        let current = self.project_session == stage.project_session
            && self.audio.as_ref().is_some_and(|audio| {
                audio.engine_session() == stage.engine_session
                    && audio.project_session() == stage.project_session
            });
        let exact_pending = self
            .pending_generator_removals
            .get(&channel_id)
            .is_some_and(|pending| {
                pending.project_session == stage.project_session
                    && pending.endpoint_id == Some(target.endpoint_id)
                    && pending.plugin_instance_id == Some(target.instance_id)
            });
        if !current
            || !target.queued
            || !exact_pending
            || endpoint_id != Some(target.endpoint_id)
            || instance_id != Some(target.instance_id)
        {
            stage.fail("Old source removal receipt did not match the retained session/engine/endpoint; outcome unknown, playback paused");
            return;
        }
        let source_matches = self
            .running_generator_chains
            .get(&channel_id)
            .is_some_and(|source| {
                source.project_session == stage.project_session
                    && source.endpoint_id == target.endpoint_id
                    && source.instance_id == target.instance_id
            });
        if !source_matches {
            stage
                .fail("Exact removed source owner is unavailable; session transition remains held");
            return;
        }
        // Install the exact guard before any control cleanup or diagnostic work.
        stage.sources[index].source = self.running_generator_chains.remove(&channel_id);
        stage.sources[index].acknowledged = true;
        self.pending_generator_removals.remove(&channel_id);
        self.deferred_generator_rebuilds.remove(&channel_id);
        self.pending_generator_routes.remove(&channel_id);
        self.pending_generator_note_offs
            .retain(|note| note.channel_id != channel_id);
        self.cancel_plugin_parameter_endpoint_edits(
            stage.project_session,
            ParameterEndpoint {
                kind: ParameterEndpointKind::Generator,
                id: stage.sources[index].endpoint_id,
            },
            ParameterEditCancelReason::EndpointReplaced,
        );
    }
    pub(super) fn drive_clip_session_retirement(&mut self) {
        let Some(mut stage) = self.clip_session_retirement.take() else {
            return;
        };
        // The local stage owns every transferred guard across the catch boundary.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.drive_clip_session_retirement_owned(&mut stage)
        }));
        let finished = match result {
            Ok(finished) => finished,
            Err(_) => {
                stage.fail("Session retirement panicked; exact source owners remain held and playback paused");
                false
            }
        };
        if !finished {
            self.clip_session_retirement = Some(stage);
            return;
        }
        // Every old source was actually joined. Keep the exact accepted intent
        // in the local owner while invoking only its already-prepared action.
        let completed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &stage.intent {
                ProjectIntent::New => {
                    self.commit_project_intent(ProjectIntent::New);
                    self.project_session != stage.project_session
                }
                ProjectIntent::Quit => {
                    self.commit_project_intent(ProjectIntent::Quit);
                    self.project_lifecycle.exit_is_armed()
                }
                ProjectIntent::Open { .. } => {
                    let project = stage
                        .prepared_open
                        .take()
                        .expect("Open Project prepared before removal");
                    let path = stage
                        .prepared_open_path
                        .take()
                        .expect("Open path reserved before removal");
                    self.finish_open_project(project, path)
                }
            }));
        let failure = match completed {
            Err(_) => Some(
                "Old sources are joined but the requested session change panicked; accepted intent remains held, playback paused",
            ),
            Ok(false) => Some(
                "Old sources are joined but the requested session change was refused; accepted intent remains held, playback paused",
            ),
            Ok(true)
                if self.project_session == stage.project_session
                    && !self.project_lifecycle.exit_is_armed() =>
            {
                Some(
                    "Session completion reported success without a new session or armed exit; accepted intent remains held, playback paused",
                )
            }
            Ok(true) => None,
        };
        if let Some(reason) = failure {
            stage.fail(reason);
            if self.clip_session_retirement.is_none() {
                self.clip_session_retirement = Some(stage);
            }
        }
    }
    fn drive_clip_session_retirement_owned(&mut self, stage: &mut ClipSessionRetirement) -> bool {
        // Even a failed stage drains only owners already transferred to it.
        for target in &mut stage.sources {
            if let Some(source) = &mut target.source {
                match source.guard.poll_shutdown_retained() {
                    RetainedShutdownPoll::Pending => {}
                    RetainedShutdownPoll::Joined => {
                        target.joined = true;
                        target.source = None;
                    }
                    RetainedShutdownPoll::WorkerPanicked => {
                        target.joined = true;
                        target.source = None;
                        if stage.failure.is_none() {
                            stage.failure = Some("A restored source panicked during joined session shutdown; playback remains paused".into());
                        }
                    }
                }
            }
        }
        if !stage.preflight_completed {
            return false;
        }
        // Admission published one false mailbox request. Observe thereafter;
        // a new request on every tick would prevent an exact stopped boundary.
        let playing = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.snapshot().transport_playing);
        if self.playing || self.transport_clock_playing || (stage.stop_observed && playing) {
            stage.fail("Playback changed during restored-source session retirement; intent held");
            self.playing = false;
            self.transport_clock_playing = false;
            if !stage.failure_pause_requested {
                stage.failure_pause_requested = true;
                if let Some(audio) = &self.audio {
                    audio.set_playing(false);
                }
            }
        }
        if self.project_session != stage.project_session
            || self.clip_render_model_revision != stage.model_revision
            || self.audio.as_ref().is_none_or(|audio| {
                audio.engine_session() != stage.engine_session
                    || audio.project_session() != stage.project_session
            })
        {
            stage.fail("Session/engine/model changed before restored-source retirement completed; removal outcome unknown, exact owners retained");
        }
        if stage.failure.is_some() {
            return false;
        }
        if stage.admission == SessionAdmission::NotSent {
            if self
                .audio
                .as_mut()
                .is_some_and(|audio| audio.command(AudioCommand::StopAll))
            {
                stage.admission = SessionAdmission::StopQueued;
            }
            return false;
        }
        if self
            .audio
            .as_ref()
            .is_none_or(|audio| audio.snapshot().transport_playing)
        {
            return false;
        }
        stage.stop_observed = true;
        for target in &mut stage.sources {
            if target.queued || target.acknowledged {
                continue;
            }
            if !self
                .running_generator_chains
                .get(&target.channel_id)
                .is_some_and(|source| {
                    source.project_session == stage.project_session
                        && source.endpoint_id == target.endpoint_id
                        && source.instance_id == target.instance_id
                })
                || self
                    .pending_generator_removals
                    .contains_key(&target.channel_id)
            {
                stage.failure = Some("Exact restored source changed before removal was queued; session transition remains held".into());
                return false;
            }
            self.pending_generator_removals.insert(
                target.channel_id,
                PendingGeneratorRemoval {
                    project_session: stage.project_session,
                    endpoint_id: Some(target.endpoint_id),
                    plugin_instance_id: Some(target.instance_id),
                },
            );
            // Mark unknown before enqueue so an unwind cannot silently retry an
            // already-issued removal against a potentially different endpoint.
            let previous_admission = stage.admission;
            stage.admission = SessionAdmission::RemovalAdmitted;
            target.queued = true;
            if !self
                .audio
                .as_mut()
                .expect("current engine checked")
                .remove_generator_endpoint(target.channel_id)
            {
                target.queued = false;
                stage.admission = previous_admission;
                self.pending_generator_removals.remove(&target.channel_id);
                break;
            }
        }
        stage
            .sources
            .iter()
            .all(|source| source.acknowledged && source.joined && source.source.is_none())
    }
}

#[cfg(test)]
#[path = "clip_render_session_tests.rs"]
mod tests;
