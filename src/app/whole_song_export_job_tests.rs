//! Real App + identified source worker + callback + Timeline notification path.
//! Renderer factory alone is a closed mock; actual captured authority, PCM,
//! checked WAV publication and Project/history images remain production code.
#[path = "whole_song_export_cancel_tests.rs"]
mod cancel_origin_tests;
#[path = "whole_song_export_identity_tests.rs"]
mod loaded_identity_tests;
#[path = "whole_serial_app_tests.rs"]
mod serial_effect_tests;

use super::*;
use crate::{
    app::clip_render_job::tests::Fixture,
    export_options::WavLevelPolicy,
    instrument_bounce::{
        BouncePluginMetadata, CapturedInstrumentBounce,
        render::{BackendFailure, BlockReply, DisposalObservation, RenderBackend},
    },
    plugins::plugin_runtime::{
        BounceSourceCallbackIdentity, BounceSourceSettlement, BounceSourceSettlementObservation,
        PluginTransport,
    },
};
use std::sync::atomic::{AtomicBool, Ordering};
type MidiLedger = Arc<std::sync::Mutex<Vec<(u64, [u8; 3])>>>;

struct WholeMockBackend {
    metadata: BouncePluginMetadata,
    disposed: bool,
    next: u64,
    midi: MidiLedger,
    prepare_failure: Option<BackendFailure>,
    disposal_failure: Option<BackendFailure>,
}
impl RenderBackend for WholeMockBackend {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        _: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        assert!(capture.plan().is_master_song());
        assert_eq!(capture.state.opaque_state, vec![7, 8, 9]);
        assert_eq!(capture.state.parameter_bases.get(&17), Some(&0.75));
        if let Some(failure) = &self.prepare_failure {
            return Err(failure.clone());
        }
        Ok(self.metadata.clone())
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        Ok(self.metadata.clone())
    }
    fn send_midi(&mut self, message: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.midi
            .lock()
            .unwrap()
            .push((self.next + u64::from(offset), message));
        Ok(())
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        assert!(transport.sample_position >= 0);
        assert_eq!(transport.sample_position as u64, self.next);
        assert_eq!(frames, 128);
        self.next += frames as u64;
        Ok(BlockReply {
            channels: vec![vec![0.125; frames], vec![-0.125; frames]],
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        })
    }
    fn dispose(&mut self, _: Instant) -> DisposalObservation {
        self.disposed = true;
        DisposalObservation {
            terminated: true,
            error: self.disposal_failure.clone(),
            diagnostics: self.disposal_failure.as_ref().map_or_else(Vec::new, |_| {
                vec!["Strict clone required forced termination".into()]
            }),
        }
    }
}
impl Drop for WholeMockBackend {
    fn drop(&mut self) {
        assert!(
            self.disposed,
            "actual strict mock owner must finish checked disposal"
        );
    }
}
fn options(fixture: &Fixture) -> WavExportOptions {
    WavExportOptions {
        sample_rate: fixture.app().audio.as_ref().unwrap().snapshot().sample_rate,
        sample_format: crate::export_options::WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds: 10,
    }
}
fn begin(fixture: &mut Fixture) -> (PathBuf, Arc<AtomicBool>) {
    let options = options(fixture);
    let (path, started, _) = begin_with_options(fixture, options);
    (path, started)
}
fn reviewed_options(
    fixture: &mut Fixture,
    options: WavExportOptions,
    accept: bool,
) -> Option<(WavExportOptions, bool)> {
    let ctx = fixture.ctx();
    let session = fixture.app().project_session;
    let dialog = &mut fixture.app_mut().export_dialog;
    dialog.open_whole_song(session, options.sample_rate, false);
    dialog.test_set_options(options);
    assert!(!export_ui::tests::click(&ctx, dialog, "Review…"));
    assert!(!dialog.confirmed_whole_fixed_tail());
    if accept {
        assert!(!export_ui::tests::click(
            &ctx,
            dialog,
            "Confirm fixed VST tail window"
        ));
    }
    let requested = export_ui::tests::click(&ctx, dialog, "Choose destination and export…");
    assert_eq!(requested, accept);
    dialog
        .begin_destination(session, false)
        .map(|options| (options, dialog.confirmed_whole_fixed_tail()))
}
fn begin_with_options(
    fixture: &mut Fixture,
    options: WavExportOptions,
) -> (PathBuf, Arc<AtomicBool>, MidiLedger) {
    let (options, confirmed) = reviewed_options(fixture, options, true).unwrap();
    assert!(confirmed);
    let review = fixture
        .app()
        .prepare_whole_song_export_review(options, confirmed)
        .unwrap();
    let path = fixture.directory().join("whole-master.wav");
    fixture
        .app_mut()
        .begin_whole_song_export(review, path.clone())
        .unwrap();
    fixture.app_mut().export_dialog.finish_destination(true);
    let started = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&started);
    let midi = Arc::new(std::sync::Mutex::new(Vec::new()));
    let worker_midi = Arc::clone(&midi);
    fixture
        .app_mut()
        .whole_song_export_job
        .as_mut()
        .unwrap()
        .render_starter = Some(Box::new(
        move |admission, project, capture, options, path, control, reviewed_fixed_tail| {
            assert!(
                reviewed_fixed_tail,
                "actual Review confirmation must reach the renderer permit"
            );
            let metadata = capture.metadata().clone();
            admission.start_master_song_mock(
                project,
                capture,
                options,
                path,
                control,
                reviewed_fixed_tail,
                move || {
                    observed.store(true, Ordering::Release);
                    WholeMockBackend {
                        metadata,
                        disposed: false,
                        next: 0,
                        midi: worker_midi,
                        prepare_failure: None,
                        disposal_failure: None,
                    }
                },
            )
        },
    ));
    (path, started, midi)
}

#[test]
fn actual_whole_render_failure_shows_primary_and_cleanup_without_publishing_or_changing_source() {
    use crate::instrument_bounce::render::{BackendFailureKind, FailureKind};
    for fail_prepare in [false, true] {
        let mut fixture = Fixture::new();
        settle_initial_source(&mut fixture);
        let before = fixture.bytes();
        let source = exact_source(&fixture);
        let selected = options(&fixture);
        let (path, started, midi) = begin_with_options(&mut fixture, selected);
        std::fs::write(&path, b"existing destination marker").unwrap();
        let observed = Arc::clone(&started);
        let preparation = fail_prepare.then(|| {
            BackendFailure::new(BackendFailureKind::Backend, "original state-load failure")
                .with_strict_context("prepare.load_state")
        });
        let cleanup = BackendFailure::new(
            BackendFailureKind::Backend,
            "Strict clone did not terminate gracefully",
        )
        .with_strict_context("dispose.wait_for_exit");
        let expected_primary = preparation
            .as_ref()
            .unwrap_or(&cleanup)
            .diagnostic()
            .to_owned();
        fixture
            .app_mut()
            .whole_song_export_job
            .as_mut()
            .unwrap()
            .render_starter = Some(Box::new(
            move |admission, project, capture, options, path, control, confirmed| {
                let metadata = capture.metadata().clone();
                admission.start_master_song_mock(
                    project,
                    capture,
                    options,
                    path,
                    control,
                    confirmed,
                    move || {
                        observed.store(true, Ordering::Release);
                        WholeMockBackend {
                            metadata,
                            disposed: false,
                            next: 0,
                            midi,
                            prepare_failure: preparation,
                            disposal_failure: Some(cleanup),
                        }
                    },
                )
            },
        ));
        until(&mut fixture, |fixture| {
            !fixture.app().whole_song_export_fence_active()
        });
        let job = fixture.app().whole_song_export_job.as_ref().unwrap();
        let Some(MasterSongRenderOutcome::Failed(failure)) = &job.outcome else {
            panic!("failed clone disposal must not publish");
        };
        assert_eq!(failure.kind, FailureKind::Backend);
        assert_eq!(failure.primary, expected_primary);
        assert_eq!(job.phase, WholeExportPhase::Failed);
        assert!(!job.cancel_requested && job.first_cancellation.is_none());
        assert!(job.render.is_none() && !job.admission.is_busy());
        let message = fixture.app().export_error.as_ref().unwrap();
        assert_eq!(message.lines().next(), Some(expected_primary.as_str()));
        assert!(message.contains("dispose.wait_for_exit"));
        assert!(message.contains("Strict clone required forced termination"));
        if fail_prepare {
            assert!(message.contains("prepare.load_state"));
            assert!(message.contains("Strict clone did not terminate gracefully"));
        }
        assert!(started.load(Ordering::Acquire));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"existing destination marker"
        );
        assert_eq!(fixture.bytes(), before);
        assert_eq!(exact_source(&fixture), source);
    }
}

#[test]
fn whole_render_failure_diagnostic_keeps_typed_context_without_plugin_text_or_false_delivery() {
    use crate::instrument_bounce::render::{BackendDelivery, BackendFailureKind};
    use vst3_host::strict_offline::FailureKind as StrictKind;
    let primary = BackendFailure::new(BackendFailureKind::Backend, "private plug-in primary text")
        .with_delivery(BackendDelivery::MayHaveApplied)
        .with_strict_context("prepare.load_state");
    let mut cleanup = BackendFailure::new(BackendFailureKind::Deadline, "private cleanup text")
        .with_delivery(BackendDelivery::NotSent)
        .with_strict_context("dispose.shutdown");
    cleanup.strict_kind = Some(StrictKind::Deadline);
    cleanup.last_wire_request = None;
    let mut failure = RenderFailure::from(primary);
    failure.cleanup_backend = Some(Box::new(cleanup));
    failure.cleanup_grace_elapsed = true;
    failure.cleanup = vec![
        "private cleanup text".into(),
        "private path/state detail".into(),
    ];
    let original = failure.clone();
    let message = whole_render_failure_message(&failure);
    assert_eq!(message.lines().next(), Some("private plug-in primary text"));
    assert!(message.contains("private cleanup text"));
    assert!(message.contains("private path/state detail"));
    assert_eq!(message.matches("private cleanup text").count(), 1);
    let mut output = Vec::new();
    write_whole_render_failure_diagnostic(&mut output, &failure).unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("WHOLE_EXPORT_RENDER_FAILED kind=Backend"));
    assert!(output.contains("delivery=Some(MayHaveApplied), stage=Some(\"prepare.load_state\")"));
    assert!(output.contains("delivery=Some(NotSent), stage=Some(\"dispose.shutdown\")"));
    assert!(output.contains("wire_request=None"));
    assert!(output.contains("cleanup_grace_elapsed=true cleanup_details=2"));
    assert!(!output.contains("private"));
    // A cleanup Shutdown write failure has no reply/request receipt. Formatting
    // must not invent one from the stage label, nor mutate the retained failure.
    assert_eq!(failure, original);
}

fn settle_initial_source(fixture: &mut Fixture) {
    fixture.until(|fixture| {
        let app = fixture.app();
        let audio = app.audio.as_ref().unwrap();
        let (generation, epoch) = audio.confirmed_timeline_identity().unwrap();
        let channel_id = app.project.channels[0].id;
        let source = &app.running_generator_chains[&channel_id];
        app.stopped_bounce_observe(channel_id).is_ok()
            && source
                .control
                .observe_bounce_source_settlement(BounceSourceCallbackIdentity {
                    engine_session: audio.engine_session(),
                    project_session: app.project_session,
                    timeline_generation: generation,
                    epoch,
                    transport_request: audio.transport_request_id(),
                    channel_id,
                    endpoint_id: source.endpoint_id,
                    instance_id: source.instance_id,
                })
                .settlement
                == BounceSourceSettlement::Ready
    });
}

#[test]
fn actual_fixture_wait_asserts_the_success_sample_without_resampling_a_one_shot_condition() {
    let mut fixture = Fixture::new();
    let calls = std::cell::Cell::new(0);
    fixture.until(|_| {
        calls.set(calls.get() + 1);
        calls.get() == 1
    });
    assert_eq!(calls.get(), 1, "a successful wait is observed once");
}

#[test]
fn actual_fixture_wait_keeps_observed_ready_but_later_foreign_request_is_not_ready() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let channel_id = fixture.app().project.channels[0].id;
    let binding = fixture.app().stopped_bounce_observe(channel_id).unwrap();
    let original_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    let saw_ready = std::cell::Cell::new(false);
    fixture.until(|fixture| {
        assert!(
            !saw_ready.get(),
            "the wait must not sample again after success"
        );
        let observed = reviewed_source_settlement(fixture, binding);
        let ready = observed.settlement == BounceSourceSettlement::Ready;
        if ready {
            saw_ready.set(true);
            // A real control request supersedes the callback identity after
            // this read. No callback pump or readiness tuple is fabricated.
            fixture.app().audio.as_ref().unwrap().set_playing(false);
        }
        ready
    });
    assert!(saw_ready.get());
    assert_ne!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        original_request
    );
    assert_ne!(
        reviewed_source_settlement(&fixture, binding).settlement,
        BounceSourceSettlement::Ready,
        "the old successful observation does not authorize a later foreign request"
    );
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert!(fixture.app().whole_song_export_job.is_none());
}
fn reviewed_source_settlement(
    fixture: &Fixture,
    binding: LiveBinding,
) -> BounceSourceSettlementObservation {
    let app = fixture.app();
    let audio = app.audio.as_ref().unwrap();
    app.running_generator_chains[&binding.stamp.channel_id]
        .control
        .observe_bounce_source_settlement(BounceSourceCallbackIdentity {
            engine_session: binding.stamp.engine_session,
            project_session: binding.stamp.project_session,
            timeline_generation: binding.stamp.timeline_revision,
            epoch: binding.stamp.epoch,
            transport_request: audio.transport_request_id(),
            channel_id: binding.stamp.channel_id,
            endpoint_id: binding.stamp.endpoint_id,
            instance_id: binding.stamp.instance_id,
        })
}
fn settle_reviewed_source(
    fixture: &mut Fixture,
    review: &WholeSongExportReview,
) -> BounceSourceSettlementObservation {
    let settled = std::cell::Cell::new(None);
    fixture.until(|fixture| {
        if !fixture
            .app()
            .stopped_bounce_observe(review.initial.stamp.channel_id)
            .is_ok_and(|current| current == review.initial)
        {
            return false;
        }
        let observed = reviewed_source_settlement(fixture, review.initial);
        let ready = observed.settlement == BounceSourceSettlement::Ready;
        if ready {
            settled.set(Some(observed));
        }
        ready
    });
    // Ready attests this actual observation, not a lease over later worker
    // admin/feedback publications. Return the sample that completed the wait.
    settled
        .get()
        .expect("the successful wait retained its source settlement")
}
fn tick(fixture: &mut Fixture) {
    fixture.tick();
}
fn until(fixture: &mut Fixture, predicate: impl Fn(&Fixture) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate(fixture) {
        assert!(
            Instant::now() < deadline,
            "actual whole App fixture condition did not complete; {}",
            whole_fixture_diagnostic(fixture)
        );
        tick(fixture);
    }
}
fn bounded_whole_failure_text(text: &str) -> String {
    let mut chars = text.chars();
    let mut bounded = chars.by_ref().take(256).collect::<String>();
    if chars.next().is_some() {
        bounded.push('…');
    }
    bounded
}
fn whole_fixture_diagnostic(fixture: &Fixture) -> String {
    let app = fixture.app();
    let job = app.whole_song_export_job.as_ref();
    let source = app.running_generator_chains.values().next();
    format!(
        "job(phase,pending,failure,capture_failure)={:?}, capture(pending,ack)={:?}, renderer(present,busy)={:?}, stop={:?}, settlement={:?}, source(ids,faults,reset_faults,stopped,detached,timeouts)={:?}, backend(captures,resets,processes)={:?}, timeline(desired,compiled,confirmed,sync)={:?}",
        job.map(|job| (
            job.phase.label(),
            job.pending(),
            job.failure.as_deref().map(bounded_whole_failure_text),
            job.capture_failure
                .as_ref()
                .map(|error| bounded_whole_failure_text(&error.to_string())),
        )),
        job.map(|job| (
            job.actor.owns_pending_capture(),
            job.actor
                .capture
                .as_ref()
                .map(|capture| capture.termination_acknowledged()),
        )),
        job.map(|job| (job.render.is_some(), job.admission.is_busy())),
        job.and_then(|job| job.actor.stop_preparation),
        job.and_then(|job| job.actor.stop_settlement),
        source.map(|source| {
            let stats = source.control.stats();
            (
                (
                    source.project_session,
                    source.endpoint_id,
                    source.instance_id,
                ),
                stats.faults,
                stats.reset_faults,
                stats.stopped,
                stats.detached_workers,
                stats.shutdown_timeouts,
            )
        }),
        (
            fixture.source_handle().capture_count(),
            fixture.source_handle().reset_count(),
            fixture.source_handle().process_count(),
        ),
        (
            app.timeline_desired_generation,
            app.timeline_compiled_generation,
            app.audio
                .as_ref()
                .and_then(AudioEngine::confirmed_timeline_identity),
            app.timeline_audio_sync.phase,
        ),
    )
}

struct WholeFailureTrace {
    case: String,
    stage: &'static str,
    observation: String,
}
impl WholeFailureTrace {
    fn at(&mut self, stage: &'static str, observation: String) {
        self.stage = stage;
        self.observation = observation;
    }
}

fn diagnose_whole_failure(test: &'static str, run: impl FnOnce(&mut WholeFailureTrace)) {
    let mut trace = WholeFailureTrace {
        case: String::new(),
        stage: "construct Fixture",
        observation: String::new(),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut trace)));
    if let Err(payload) = result {
        let panic = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic payload");
        // libtest normally withholds assertion output until every test ends.
        // Write this bounded control-only context directly so a later hung
        // fixture cannot hide the first failure; preserve the original panic.
        use std::io::Write;
        let _ = writeln!(
            std::io::stderr().lock(),
            "Whole test failure: test={test}, case={}, stage={}, panic={:?}; {}",
            trace.case,
            trace.stage,
            bounded_whole_failure_text(panic),
            trace.observation,
        );
        std::panic::resume_unwind(payload);
    }
}
fn exact_source(fixture: &Fixture) -> (u64, u64, u64) {
    let channel = fixture.app().project.channels[0].id;
    let source = &fixture.app().running_generator_chains[&channel];
    (
        source.project_session,
        source.endpoint_id,
        source.instance_id,
    )
}

#[test]
fn actual_whole_cancel_before_first_drive_does_not_issue_reset_chase_capture_or_renderer() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    let resets = fixture.source_handle().reset_count();
    let (path, started) = begin(&mut fixture);
    assert!(fixture.app_mut().cancel_whole_song_export());
    fixture.app_mut().drive_whole_song_export();
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::Cancelled);
    assert!(job.actor.old_epoch.is_none() && job.actor.stop_preparation.is_none());
    assert!(!job.actor.owns_pending_capture() && !job.pending());
    for _ in 0..3 {
        tick(&mut fixture);
    }
    assert_eq!(fixture.source_handle().reset_count(), resets);
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert!(!started.load(Ordering::Acquire));
    assert!(!path.exists());
    assert_eq!(exact_source(&fixture), source);
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_whole_cancel_during_held_reset_finishes_without_waiting_for_ready_or_stop_timeout() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    let resets = fixture.source_handle().reset_count();
    fixture.source_handle().pause_reset();
    let (path, started) = begin(&mut fixture);
    until(&mut fixture, |fixture| {
        fixture.source_handle().reset_count() > resets
    });
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert!(fixture.app_mut().cancel_whole_song_export());
    fixture.app_mut().drive_whole_song_export();
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::Cancelled && job.failure.is_none());
    assert!(!job.pending());
    assert!(!started.load(Ordering::Acquire));
    assert!(!path.exists());
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
    fixture.source_handle().release_gates();
    for _ in 0..3 {
        tick(&mut fixture);
    }
    assert_eq!(fixture.source_handle().capture_count(), 0);
}

#[test]
fn actual_whole_capture_cancel_retains_fence_and_private_owner_until_worker_ack() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    fixture.source_handle().pause_capture();
    let (path, started) = begin(&mut fixture);
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });
    assert!(fixture.app_mut().cancel_whole_song_export());
    for _ in 0..3 {
        fixture.app_mut().drive_whole_song_export();
    }
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.actor.owns_pending_capture());
    assert!(job.pending());
    assert!(
        fixture.app().clip_operation_fence_active() && fixture.app().clip_render_blocks_playback()
    );
    assert!(!started.load(Ordering::Acquire));
    assert!(!path.exists());
    fixture.source_handle().release_gates();
    until(&mut fixture, |fixture| {
        !fixture.app().whole_song_export_fence_active()
    });
    assert!(
        fixture.app().whole_song_export_job.as_ref().unwrap().phase == WholeExportPhase::Cancelled
    );
    assert_eq!(fixture.source_handle().capture_count(), 1);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
}

#[test]
fn actual_whole_preflight_rejects_rate_and_unbound_inactive_insert_endpoints_before_stop_or_destination()
 {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let original = fixture.app().project.clone();
    let source = exact_source(&fixture);
    let resets = fixture.source_handle().reset_count();
    let mut wrong_rate = options(&fixture);
    wrong_rate.sample_rate /= 2;
    assert!(
        fixture
            .app()
            .prepare_whole_song_export_review(wrong_rate, false)
            .is_err()
    );
    let channel_id = original.channels[0].id;
    for (enabled, bypass) in [(false, false), (true, true), (false, true)] {
        let mut project = original.clone();
        let mut effect = project.plugin_instances[0].clone();
        effect.id = 99;
        effect.role = PluginRole::Effect;
        effect.enabled = enabled;
        effect.bypass = bypass;
        project.plugin_instances.push(effect);
        project.mixer_insert_slots.push(MixerInsertSlotRef {
            track: project.channels[0].mixer_track,
            slot: 0,
            plugin_instance_id: 99,
        });
        assert!(
            pure_master_plan(
                &project,
                channel_id,
                options(&fixture).sample_rate,
                options(&fixture)
            )
            .unwrap()
            .is_serial_master()
        );
        fixture.app_mut().project = project;
        assert!(
            fixture
                .app()
                .prepare_whole_song_export_review(options(&fixture), false)
                .is_err()
        );
        assert!(fixture.app().whole_song_export_job.is_none());
    }
    fixture.app_mut().project = original;
    assert_eq!(fixture.source_handle().reset_count(), resets);
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert_eq!(exact_source(&fixture), source);
    assert_eq!(fixture.bytes(), before);
    assert!(!fixture.directory().join("whole-master.wav").exists());
}

#[test]
fn actual_whole_export_uses_callback_notifications_capture_mock_worker_and_real_wav_without_model_edits()
 {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    let before_epoch = fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .snapshot()
        .transport_epoch;
    let (path, started) = begin(&mut fixture);
    until(&mut fixture, |fixture| {
        fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .is_some_and(|job| !job.pending())
    });
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::Complete);
    let stop = job.actor.stop_preparation.unwrap();
    let activation = stop.activation.unwrap();
    let ready_binding = job.actor.binding.unwrap();
    assert_eq!(activation.committed_epoch, Some(ready_binding.stamp.epoch));
    assert_ne!(ready_binding.stamp.epoch, before_epoch);
    assert_eq!(fixture.source_handle().capture_count(), 1);
    assert!(started.load(Ordering::Acquire));
    let Some(MasterSongRenderOutcome::Complete(report)) = &job.outcome else {
        panic!("actual master outcome missing");
    };
    let wav = crate::wav::read_wav(&path).unwrap();
    assert_eq!(wav.frame_count(), report.master_output.frames);
    assert_eq!(report.dry_render.origin_frame, 0);
    assert!(
        report.master_output.music_frames > 0
            && wav.samples.iter().any(|sample| sample.abs() > 0.01)
    );
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
    assert!(
        !fixture.app().playing
            && !fixture
                .app()
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .transport_playing
    );
    assert!(fixture.app().clip_render_job.is_none());
    assert!(
        fixture.app().retained_clip_media.is_empty()
            && fixture.app().clip_render_restorations.is_empty()
    );
    assert!(!fixture.app_mut().cancel_whole_song_export());
    assert!(
        fixture
            .app()
            .whole_song_export_job
            .as_ref()
            .unwrap()
            .first_cancellation
            .is_none(),
        "a refused post-publication request must not manufacture a cancellation origin"
    );
}

#[test]
fn actual_whole_snapshot_limit_is_rejected_before_review_stop_or_destination() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    let resets = fixture.source_handle().reset_count();
    let original = fixture.app().project.plugin_instances[0]
        .opaque_state
        .clone();
    // Persisted opaque bytes encode as hex, so this exceeds the shared 64 MiB
    // exact Project snapshot boundary while staying below raw capture's cap.
    fixture.app_mut().project.plugin_instances[0].opaque_state = vec![0; 33 * 1024 * 1024];
    let reason = fixture
        .app()
        .prepare_whole_song_export_review(options(&fixture), false)
        .err()
        .unwrap();
    assert!(reason.contains("64 MiB"));
    assert!(fixture.app().whole_song_export_job.is_none());
    fixture.app_mut().project.plugin_instances[0].opaque_state = original;
    assert_eq!(fixture.source_handle().reset_count(), resets);
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
    assert!(!fixture.directory().join("whole-master.wav").exists());
}

#[test]
fn actual_whole_engine_loss_retains_exact_capture_source_and_primary_cause_until_ack_and_join() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    fixture.source_handle().pause_capture();
    let (path, started) = begin(&mut fixture);
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });
    // The production hook transfers the original guard before destruction of
    // the actual callback and engine, just as the real engine-loss paths do.
    fixture.app_mut().retain_clip_source_before_engine_loss();
    fixture.detach_callback_and_audio_for_whole_test();
    for _ in 0..3 {
        tick(&mut fixture);
    }
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    let retained = job.retired_source.as_ref().unwrap();
    assert_eq!(
        (
            retained.project_session,
            retained.endpoint_id,
            retained.instance_id
        ),
        source
    );
    assert!(job.actor.owns_pending_capture() && job.pending());
    assert!(
        job.failure
            .as_deref()
            .unwrap()
            .starts_with("Audio/session loss retained the exact source worker")
    );
    assert!(fixture.app().clip_operation_fence_active());
    assert!(!started.load(Ordering::Acquire) && !path.exists());
    fixture.source_handle().release_gates();
    until(&mut fixture, |fixture| {
        !fixture.app().whole_song_export_fence_active()
    });
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::Failed && job.retired_source.is_none());
    assert!(
        job.failure
            .as_deref()
            .unwrap()
            .starts_with("Audio/session loss retained the exact source worker")
    );
    assert_eq!(job.capture_failure, Some(CapturePollError::Cancelled));
    assert_eq!(fixture.source_handle().capture_count(), 1);
    assert!(fixture.app().running_generator_chains.is_empty());
    assert_eq!(fixture.bytes(), before);
    assert!(!started.load(Ordering::Acquire) && !path.exists());
}

#[test]
fn actual_whole_late_playback_is_paused_once_and_capture_owner_remains_until_ack() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    fixture.source_handle().pause_capture();
    let (path, started) = begin(&mut fixture);
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });
    // Exercise the real transport mailbox and callback. App fields/Ready
    // observations are not manufactured to simulate this foreign late request.
    fixture.app().audio.as_ref().unwrap().set_playing(true);
    let late_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .snapshot()
        .transport_playing
    {
        assert!(
            Instant::now() < deadline,
            "actual late playback request was not applied"
        );
        fixture.pump_callback_only();
    }
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .applied_transport_request,
        late_request
    );
    fixture.app_mut().drive_whole_song_export();
    let paused_request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    assert_ne!(paused_request, late_request);
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.failure_pause_requested && job.actor.owns_pending_capture() && job.pending());
    assert!(job.failure.as_deref().unwrap().contains("Playback changed"));
    assert!(
        fixture.app().clip_operation_fence_active()
            && !fixture.app().playing
            && !fixture.app().transport_clock_playing
    );
    for _ in 0..3 {
        fixture.app_mut().drive_whole_song_export();
    }
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        paused_request
    );
    while fixture
        .app()
        .audio
        .as_ref()
        .unwrap()
        .snapshot()
        .transport_playing
    {
        assert!(
            Instant::now() < deadline,
            "actual recovery pause request was not applied"
        );
        fixture.pump_callback_only();
    }
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .applied_transport_request,
        paused_request
    );
    fixture.source_handle().release_gates();
    until(&mut fixture, |fixture| {
        !fixture.app().whole_song_export_fence_active()
    });
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::Failed);
    assert!(job.failure.as_deref().unwrap().contains("Playback changed"));
    assert_eq!(job.capture_failure, Some(CapturePollError::Cancelled));
    assert_eq!(exact_source(&fixture), source);
    assert_eq!(fixture.bytes(), before);
    assert!(!started.load(Ordering::Acquire) && !path.exists());
    assert!(
        !fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .snapshot()
            .transport_playing
    );
}

#[test]
fn actual_whole_unconfirmed_fixed_window_cannot_choose_destination_stop_source_or_replace_marker() {
    for reported in [48_000 * 4, u32::MAX] {
        let mut fixture = Fixture::new_whole_tail_for_test(reported);
        settle_initial_source(&mut fixture);
        let before = fixture.bytes();
        let source = exact_source(&fixture);
        let resets = fixture.source_handle().reset_count();
        let request = fixture.app().audio.as_ref().unwrap().transport_request_id();
        let mut selected = options(&fixture);
        selected.tail_milliseconds = 0;
        let path = fixture.directory().join("whole-master.wav");
        std::fs::write(&path, b"existing destination marker").unwrap();
        assert!(reviewed_options(&mut fixture, selected, false).is_none());
        assert!(!fixture.app().export_dialog.confirmed_whole_fixed_tail());
        // The production controller also refuses a direct unchecked review;
        // no actual Stop mailbox, MIDI chase or private capture is issued.
        let review = fixture
            .app()
            .prepare_whole_song_export_review(selected, false)
            .unwrap();
        let reason = fixture
            .app_mut()
            .begin_whole_song_export(review, path.clone())
            .err()
            .unwrap();
        assert!(reason.contains("Confirm the selected fixed VST tail window"));
        assert!(fixture.app().whole_song_export_job.is_none());
        assert_eq!(
            fixture.app().audio.as_ref().unwrap().transport_request_id(),
            request
        );
        assert_eq!(fixture.source_handle().reset_count(), resets);
        assert_eq!(fixture.source_handle().capture_count(), 0);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"existing destination marker"
        );
        assert_eq!(fixture.bytes(), before);
        assert_eq!(exact_source(&fixture), source);
        let text = export_ui::whole_fixed_window_review_text(selected);
        assert!(text.contains("0 ms") && text.contains("file ends at the song end"));
        assert!(text.contains("may be truncated") && text.contains("does not guarantee"));
    }
}

#[test]
fn actual_whole_review_permit_tail_zero_and_three_seconds_keep_metadata_warning_and_tight_noteoff()
{
    diagnose_whole_failure(
        "review_permit_tail_zero_and_three_seconds",
        whole_tail_cases_with_failure_diagnostics,
    );
}

fn whole_tail_cases_with_failure_diagnostics(trace: &mut WholeFailureTrace) {
    for reported in [48_000 * 4, u32::MAX] {
        for tail_ms in [0, 3_000] {
            trace.case = format!("reported_tail={reported}, selected_tail_ms={tail_ms}");
            trace.at("construct Fixture", String::new());
            let mut fixture = Fixture::new_whole_tail_for_test(reported);
            trace.at("settle initial source", whole_fixture_diagnostic(&fixture));
            settle_initial_source(&mut fixture);
            let before = fixture.bytes();
            let source = exact_source(&fixture);
            let mut selected = options(&fixture);
            selected.tail_milliseconds = tail_ms;
            trace.at(
                "review and begin export",
                whole_fixture_diagnostic(&fixture),
            );
            let (path, started, midi) = begin_with_options(&mut fixture, selected);
            trace.at(
                "wait for actual export owner",
                whole_fixture_diagnostic(&fixture),
            );
            until(&mut fixture, |fixture| {
                !fixture.app().whole_song_export_fence_active()
            });
            trace.at(
                "verify rendered report and MIDI",
                whole_fixture_diagnostic(&fixture),
            );
            let job = fixture.app().whole_song_export_job.as_ref().unwrap();
            assert!(job.phase == WholeExportPhase::Complete && job.reviewed_fixed_tail);
            assert_eq!(fixture.source_handle().capture_count(), 1);
            assert!(started.load(Ordering::Acquire));
            let Some(MasterSongRenderOutcome::Complete(report)) = &job.outcome else {
                panic!("actual fixed-window report missing");
            };
            assert_eq!(report.dry_render.reported_tail, reported);
            assert_eq!(
                report.dry_render.fixed_tail as usize,
                selected.tail_frames().unwrap()
            );
            assert!(report.master_output.tail_limit_reached);
            let music_end = report.dry_render.musical_frames;
            let ledger = midi.lock().unwrap();
            assert!(
                ledger
                    .iter()
                    .any(|(frame, message)| *frame == music_end && message[0] & 0xf0 == 0x80)
            );
            assert!(
                ledger
                    .iter()
                    .filter(|(_, message)| message[0] & 0xf0 == 0x90)
                    .all(|(frame, _)| *frame < music_end)
            );
            let wav = crate::wav::read_wav(&path).unwrap();
            assert_eq!(
                wav.frame_count(),
                music_end as usize + selected.tail_frames().unwrap()
            );
            // This mock deliberately remains nonzero at the file boundary. The
            // result explains its metadata comparison and makes no silence claim.
            assert!(wav.samples.last().unwrap().abs() > 0.01);
            trace.at(
                "verify retained tail warning",
                whole_fixture_diagnostic(&fixture),
            );
            let result = fixture
                .app()
                .whole_song_export_tail_result
                .as_ref()
                .unwrap();
            assert!(result.open);
            assert_eq!(result.selected_tail_milliseconds, tail_ms);
            let text = result.text();
            assert!(text.contains(&format!("Selected tail: {tail_ms} ms")));
            assert!(
                text.contains("comparison uses plug-in metadata")
                    && text.contains("No tail-end acoustic silence measurement")
            );
            assert!(
                !text.contains("source release remains")
                    && !text.contains("increase the tail to avoid truncation")
            );
            if reported == u32::MAX {
                assert!(text.contains("unknown / unbounded (u32::MAX)"));
            } else {
                assert!(text.contains("192000 frames (4000.000 ms at 48000 Hz)"));
            }
            fixture
                .app_mut()
                .whole_song_export_tail_result
                .as_mut()
                .unwrap()
                .open = false;
            for _ in 0..3 {
                tick(&mut fixture);
            }
            assert!(fixture.app().whole_song_export_tail_result.is_some());
            fixture.app_mut().reopen_whole_song_tail_result();
            trace.at(
                "reopen retained tail warning",
                whole_fixture_diagnostic(&fixture),
            );
            let ctx = fixture.ctx();
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                fixture.app_mut().whole_song_export_progress_ui(ui.ctx())
            });
            assert!(
                fixture
                    .app()
                    .whole_song_export_tail_result
                    .as_ref()
                    .unwrap()
                    .open
            );
            assert_eq!(fixture.bytes(), before);
            assert_eq!(exact_source(&fixture), source);
            assert!(
                !fixture.app().playing
                    && !fixture
                        .app()
                        .audio
                        .as_ref()
                        .unwrap()
                        .snapshot()
                        .transport_playing
            );
            trace.at(
                "drain exact Fixture owners",
                whole_fixture_diagnostic(&fixture),
            );
        }
    }
}

// Add-on regressions exercise existing App/service/owner methods. The only
// backend substitution remains the closed strict-mock factory from sealed04.
fn authoritative_whole_parameter_route(fixture: &mut Fixture) -> ParameterEditRoute {
    let previous_capture = fixture
        .app()
        .running_generator_chains
        .values()
        .next()
        .unwrap()
        .control
        .native_editor_snapshot(0)
        .unwrap()
        .capture_serial;
    assert!(
        fixture
            .app()
            .running_generator_chains
            .values()
            .next()
            .unwrap()
            .control
            .request_state(0)
    );
    until(fixture, |fixture| {
        fixture
            .app()
            .running_generator_chains
            .values()
            .next()
            .unwrap()
            .control
            .native_editor_snapshot(0)
            .is_some_and(|snapshot| {
                snapshot.capture_serial > previous_capture && !snapshot.has_uncaptured_changes()
            })
    });
    fixture.app_mut().poll_native_editor_snapshots();
    let app = fixture.app();
    let source = app.running_generator_chains.values().next().unwrap();
    app.plugin_parameter_edit_route(
        app.project_session,
        source.endpoint_id,
        source.instance_id,
        0,
        17,
        true,
    )
    .unwrap()
}

#[test]
fn actual_whole_review_callback_ack_does_not_complete_a_held_prior_worker_reset() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let selected = options(&fixture);
    let old_review = fixture
        .app()
        .prepare_whole_song_export_review(selected, true)
        .unwrap();
    let old_epoch = old_review.initial.stamp.epoch;
    let old_resets = fixture.source_handle().reset_count();
    fixture.source_handle().pause_reset();
    // A real pre-existing seek can be callback-acknowledged while its reset
    // remains owned by the worker. No export or synthetic ACK is issued here.
    fixture.app_mut().publish_transport_seek(0.5);
    fixture.until(|fixture| {
        fixture.source_handle().reset_count() > old_resets
            && fixture
                .app()
                .prepare_whole_song_export_review(selected, true)
                .is_ok_and(|review| review.initial.stamp.epoch != old_epoch)
    });
    let current_review = fixture
        .app()
        .prepare_whole_song_export_review(selected, true)
        .unwrap();
    let held = reviewed_source_settlement(&fixture, current_review.initial);
    assert_eq!(held.settlement, BounceSourceSettlement::Waiting);
    assert!(held.coherent && held.busy > 0);
    assert_eq!(held.requested_epoch, current_review.initial.stamp.epoch);
    assert_ne!(
        held.reset_completed_epoch,
        current_review.initial.stamp.epoch
    );
    assert_eq!(fixture.source_handle().capture_count(), 0);

    let request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    let held_resets = fixture.source_handle().reset_count();
    let before = fixture.bytes();
    let path = fixture.directory().join("held-prior-reset-refusal.wav");
    let reason = fixture
        .app_mut()
        .begin_whole_song_export(old_review, path.clone())
        .err()
        .unwrap();
    assert!(reason.contains("reviewed source or Project changed"));
    assert!(fixture.app().whole_song_export_job.is_none());
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        request
    );
    assert_eq!(fixture.source_handle().reset_count(), held_resets);
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert!(!path.exists());
    assert_eq!(fixture.bytes(), before);

    fixture.source_handle().release_reset();
    let settled = settle_reviewed_source(&mut fixture, &current_review);
    assert_eq!(settled.settlement, BounceSourceSettlement::Ready);
    assert_eq!(
        settled.reset_completed_epoch,
        current_review.initial.stamp.epoch
    );
    assert_eq!(fixture.source_handle().capture_count(), 0);
    assert_eq!(fixture.bytes(), before);
}

#[test]
fn actual_whole_frozen_destination_review_refuses_a_real_parameter_receipt_before_any_stop_or_capture()
 {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let route = authoritative_whole_parameter_route(&mut fixture);
    // The authoritative state receipt can itself change the Timeline image.
    // Freeze the initial review only after that current image and worker settle.
    settle_initial_source(&mut fixture);
    let selected = options(&fixture);
    let (selected, confirmed) = reviewed_options(&mut fixture, selected, true).unwrap();
    let review = fixture
        .app()
        .prepare_whole_song_export_review(selected, confirmed)
        .unwrap();
    let source = exact_source(&fixture);
    let captures = fixture.source_handle().capture_count();
    let control_revision = fixture.app().clip_render_control_revision;
    let path = fixture.directory().join("stale-review-marker.wav");
    std::fs::write(&path, b"existing destination marker").unwrap();

    fixture.app_mut().request_plugin_parameter_edit(route, 0.5);
    assert!(fixture.app().clip_render_control_revision > control_revision);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Same real service entry points as App::update, with callback/worker
        // replies consumed by App rather than a handwritten receipt or stamp.
        fixture.app_mut().poll_plugin_parameter_callback_receipts();
        fixture.app_mut().poll_plugin_parameter_worker_receipts();
        fixture.app_mut().drive_plugin_parameter_edit_drafts();
        tick(&mut fixture);
        if fixture.app().project.plugin_instances[0]
            .parameters
            .get(&17)
            == Some(&0.5)
            && fixture.app().plugin_parameter_edits.reserved_len() == 0
            && fixture.app().plugin_parameter_edit_draft_desired.is_empty()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "actual parameter callback/worker receipt did not complete"
        );
    }
    // A source-state reply is not the new Project's Timeline acknowledgment.
    // Service the same native/parameter paths and callback/compile notifications
    // until both have settled; no Ready, stamp or receipt is manufactured here.
    let _current_route = authoritative_whole_parameter_route(&mut fixture);
    let mut fresh_review = fixture
        .app()
        .prepare_whole_song_export_review(selected, confirmed);
    let initial_review_error = fresh_review.as_ref().err().cloned();
    if let Some(reason) = &initial_review_error {
        let app = fixture.app();
        eprintln!(
            "fresh Whole Review before actual Timeline settlement: {reason}; desired={}, compiled={:?}, confirmed={:?}, current_fp={}, source_fp={:?}",
            app.timeline_desired_generation,
            app.timeline_compiled_generation,
            app.audio.as_ref().unwrap().confirmed_timeline_identity(),
            app.clip_timeline_fingerprint(app.timeline_sample_rate()),
            app.timeline_audio_source
                .as_ref()
                .map(|source| source.fingerprint),
        );
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while fresh_review.is_err() && Instant::now() < deadline {
        fixture.app_mut().poll_plugin_parameter_callback_receipts();
        fixture.app_mut().poll_plugin_parameter_worker_receipts();
        fixture.app_mut().drive_plugin_parameter_edit_drafts();
        fixture.app_mut().poll_native_editor_snapshots();
        tick(&mut fixture);
        fresh_review = fixture
            .app()
            .prepare_whole_song_export_review(selected, confirmed);
    }
    assert!(
        fresh_review.is_ok(),
        "the changed source must be admissible with a fresh review after actual receipts and Timeline acknowledgment; initial_error={initial_review_error:?}, final_error={:?}, desired={}, compiled={:?}, confirmed={:?}",
        fresh_review.as_ref().err(),
        fixture.app().timeline_desired_generation,
        fixture.app().timeline_compiled_generation,
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_identity(),
    );
    let fresh_review = fresh_review.unwrap();
    let current_binding = fixture
        .app()
        .stopped_bounce_observe(fresh_review.initial.stamp.channel_id)
        .expect("the settled current source must still admit review before testing the old one");
    assert_eq!(current_binding, fresh_review.initial);
    assert_ne!(current_binding, review.initial);
    assert!(
        clip_render::project_matches_snapshot(&fixture.app().project, &fresh_review.snapshot)
            .unwrap()
    );
    assert!(
        !clip_render::project_matches_snapshot(&fixture.app().project, &review.snapshot).unwrap()
    );
    assert_ne!(
        fresh_review.initial.stamp.timeline_revision, review.initial.stamp.timeline_revision,
        "the accepted parameter base belongs to a genuinely new compiled Timeline"
    );
    assert_eq!(
        fixture
            .app()
            .audio
            .as_ref()
            .unwrap()
            .confirmed_timeline_identity(),
        Some((
            fresh_review.initial.stamp.timeline_revision,
            fresh_review.initial.stamp.epoch
        )),
    );
    assert_eq!(
        fixture.app().project.plugin_instances[0]
            .parameters
            .get(&17),
        Some(&0.5)
    );
    assert_eq!(fixture.app().plugin_parameter_edits.reserved_len(), 0);
    assert!(fixture.app().plugin_parameter_edit_draft_desired.is_empty());
    assert_eq!(fixture.source_handle().capture_count(), captures);
    // Review binds callback/Project/native identity, but does not attest the
    // worker's asynchronous epoch reset. Finish the accepted edit's real prior
    // work before measuring whether rejecting the old review issues any more.
    let settlement = settle_reviewed_source(&mut fixture, &fresh_review);
    assert_eq!(settlement.settlement, BounceSourceSettlement::Ready);
    assert_eq!(
        settlement.reset_completed_epoch,
        fresh_review.initial.stamp.epoch
    );
    let request = fixture.app().audio.as_ref().unwrap().transport_request_id();
    let resets = fixture.source_handle().reset_count();
    let after_edit = fixture.bytes();
    let reason = fixture
        .app_mut()
        .begin_whole_song_export(review, path.clone())
        .err()
        .unwrap();
    assert!(reason.contains("reviewed source or Project changed"));
    fixture.app_mut().export_dialog.finish_destination(false);
    assert!(fixture.app().export_dialog.is_open());
    assert!(fixture.app().whole_song_export_job.is_none());
    assert_eq!(
        fixture.app().audio.as_ref().unwrap().transport_request_id(),
        request
    );
    assert_eq!(fixture.source_handle().reset_count(), resets);
    assert_eq!(fixture.source_handle().capture_count(), captures);
    assert_eq!(std::fs::read(path).unwrap(), b"existing destination marker");
    assert_eq!(exact_source(&fixture), source);
    assert_eq!(
        fixture.bytes(),
        after_edit,
        "rejecting the old review must preserve the real accepted edit/history"
    );
}

struct WholeDisposalGate {
    open: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}
impl WholeDisposalGate {
    fn new() -> Self {
        Self {
            open: Arc::new(AtomicBool::new(false)),
            entered: Arc::new(AtomicBool::new(false)),
            dropped: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl Drop for WholeDisposalGate {
    fn drop(&mut self) {
        self.open.store(true, Ordering::Release);
    }
}
struct WholeHeldDisposalBackend {
    inner: WholeMockBackend,
    open: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}
impl RenderBackend for WholeHeldDisposalBackend {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        self.inner.prepare(capture, control)
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        self.inner.metadata()
    }
    fn send_midi(&mut self, message: [u8; 3], offset: u16) -> Result<(), BackendFailure> {
        self.inner.send_midi(message, offset)
    }
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure> {
        self.inner.process(transport, frames)
    }
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation {
        self.entered.store(true, Ordering::Release);
        if self.open.load(Ordering::Acquire) {
            self.inner.dispose(deadline)
        } else {
            DisposalObservation {
                terminated: false,
                error: None,
                diagnostics: vec![],
            }
        }
    }
}
impl Drop for WholeHeldDisposalBackend {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}
fn whole_close_input() -> egui::RawInput {
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .events
        .push(egui::ViewportEvent::Close);
    input
}
fn has_whole_viewport_command(output: &egui::FullOutput, close: bool) -> bool {
    output.viewport_output[&egui::ViewportId::ROOT]
        .commands
        .iter()
        .any(|command| {
            if close {
                matches!(command, egui::ViewportCommand::Close)
            } else {
                matches!(command, egui::ViewportCommand::CancelClose)
            }
        })
}

#[test]
fn actual_whole_window_close_retains_a_pending_clone_disposal_receipt_until_its_real_ack() {
    let mut fixture = Fixture::new();
    settle_initial_source(&mut fixture);
    let before = fixture.bytes();
    let source = exact_source(&fixture);
    // Declared after Fixture: assertion unwind opens this gate before Fixture
    // attempts its existing retained owner/finished-only source cleanup.
    let gate = WholeDisposalGate::new();
    let selected = options(&fixture);
    let (path, started, midi) = begin_with_options(&mut fixture, selected);
    std::fs::write(&path, b"existing destination marker").unwrap();
    let open = Arc::clone(&gate.open);
    let entered = Arc::clone(&gate.entered);
    let dropped = Arc::clone(&gate.dropped);
    let observed = Arc::clone(&started);
    fixture
        .app_mut()
        .whole_song_export_job
        .as_mut()
        .unwrap()
        .render_starter = Some(Box::new(
        move |admission, project, capture, options, path, control, reviewed_fixed_tail| {
            let metadata = capture.metadata().clone();
            admission.start_master_song_mock(
                project,
                capture,
                options,
                path,
                control,
                reviewed_fixed_tail,
                move || {
                    observed.store(true, Ordering::Release);
                    WholeHeldDisposalBackend {
                        inner: WholeMockBackend {
                            metadata,
                            disposed: false,
                            next: 0,
                            midi,
                            prepare_failure: None,
                            disposal_failure: None,
                        },
                        open,
                        entered,
                        dropped,
                    }
                },
            )
        },
    ));
    until(&mut fixture, |_| gate.entered.load(Ordering::Acquire));
    let ctx = fixture.ctx();
    let output = cancel_origin_tests::actual_app_frame(&mut fixture, whole_close_input());
    assert!(has_whole_viewport_command(&output, false));
    assert!(!has_whole_viewport_command(&output, true));
    let output = cancel_origin_tests::actual_app_frame(&mut fixture, egui::RawInput::default());
    assert!(
        !has_whole_viewport_command(&output, true),
        "an ordinary frame may not publish Close while the original clone is retained"
    );
    for _ in 0..3 {
        tick(&mut fixture);
    }
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(job.phase == WholeExportPhase::RenderingDisposing && job.pending());
    assert_eq!(
        job.first_cancellation,
        Some(WholeExportCancellation {
            source: WholeExportCancelSource::WindowCloseRequest,
            phase: WholeExportPhase::RenderingDisposing,
        })
    );
    assert!(job.render.is_some() && job.admission.is_busy());
    assert!(!job.render.as_ref().unwrap().termination_acknowledged());
    assert!(!gate.dropped.load(Ordering::Acquire));
    assert!(
        fixture.app().clip_operation_fence_active() && fixture.app().project_lifecycle.is_idle()
    );
    assert!(started.load(Ordering::Acquire));
    assert_eq!(fixture.source_handle().capture_count(), 1);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"existing destination marker"
    );
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);

    gate.open.store(true, Ordering::Release);
    until(&mut fixture, |fixture| {
        !fixture.app().whole_song_export_fence_active()
    });
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(
        job.phase == WholeExportPhase::Cancelled
            && job.render.is_none()
            && !job.admission.is_busy()
    );
    assert!(gate.dropped.load(Ordering::Acquire));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"existing destination marker"
    );
    assert_eq!(fixture.bytes(), before);
    assert_eq!(exact_source(&fixture), source);
    let _ = ctx.run_ui(whole_close_input(), |_| {
        fixture.app_mut().intercept_window_close_request(&ctx);
    });
    // Ordinary quit may ask to discard native-state/model changes observed by
    // its real dirty guard. Exercise that actual App action, never fabricate
    // ExitArmed or bypass an owned operation. The Fixture owns no recovery file.
    if !fixture.app().project_lifecycle.exit_is_armed() {
        assert!(matches!(
            fixture.app().project_lifecycle,
            ProjectLifecycle::AwaitingDecision { .. }
        ));
        fixture.app_mut().autosave_path = None;
        fixture.app_mut().discard_project_for_lifecycle();
    }
    let output = ctx.run_ui(egui::RawInput::default(), |_| {
        fixture.app_mut().dispatch_armed_window_close(&ctx, false);
    });
    assert!(
        has_whole_viewport_command(&output, true),
        "ordinary clean close must become eligible after actual owner termination"
    );
    let output = ctx.run_ui(egui::RawInput::default(), |_| {
        fixture.app_mut().dispatch_armed_window_close(&ctx, false);
    });
    assert!(
        !has_whole_viewport_command(&output, true),
        "Close publication remains one-shot"
    );
}

#[test]
fn actual_whole_forced_app_drop_keeps_the_cancelled_capture_and_exact_source_until_worker_join() {
    diagnose_whole_failure(
        "forced_app_drop_exact_source_until_worker_join",
        whole_forced_drop_with_failure_diagnostics,
    );
}

fn whole_forced_drop_with_failure_diagnostics(trace: &mut WholeFailureTrace) {
    let mut fixture = Fixture::new_whole_drop_observer_for_test();
    trace.at("settle initial source", whole_fixture_diagnostic(&fixture));
    settle_initial_source(&mut fixture);
    fixture.source_handle().pause_capture();
    trace.at(
        "review and begin held capture",
        whole_fixture_diagnostic(&fixture),
    );
    let (path, started) = begin(&mut fixture);
    std::fs::write(&path, b"existing destination marker").unwrap();
    until(&mut fixture, |fixture| {
        fixture.source_handle().capture_count() == 1
    });
    trace.at(
        "verify held capture before App Drop",
        format!(
            "{}; {}",
            whole_fixture_diagnostic(&fixture),
            fixture.whole_drop_observation_metadata_for_test()
        ),
    );
    let job = fixture.app().whole_song_export_job.as_ref().unwrap();
    assert!(
        job.actor.owns_pending_capture()
            && !job
                .actor
                .capture
                .as_ref()
                .unwrap()
                .termination_acknowledged()
    );
    assert!(job.render.is_none() && !job.admission.is_busy());
    assert_eq!(fixture.whole_drop_join_count_for_test(), 0);
    fixture.force_drop_app_for_whole_test();
    trace.at(
        "verify resources retained after actual App Drop",
        fixture.whole_drop_observation_metadata_for_test(),
    );
    // App/actual callback are gone. The original source gate has not been
    // released by normal Fixture drain; shutdown polling runs independently.
    assert!(fixture.whole_drop_resources_retained_for_test());
    assert_eq!(fixture.whole_drop_join_count_for_test(), 0);
    assert_eq!(fixture.source_handle().capture_count(), 1);
    assert!(!started.load(Ordering::Acquire));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"existing destination marker"
    );

    fixture.source_handle().release_gates();
    let joined = fixture.wait_whole_drop_join_for_test(Instant::now() + Duration::from_secs(5));
    trace.at(
        "verify exact worker Join result",
        format!(
            "join_result={joined:?}; {}",
            fixture.whole_drop_observation_metadata_for_test()
        ),
    );
    joined.unwrap();
    assert_eq!(fixture.whole_drop_join_count_for_test(), 1);
    assert!(fixture.whole_drop_resources_retained_for_test());
    assert_eq!(fixture.source_handle().capture_count(), 1);
    assert!(!started.load(Ordering::Acquire));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"existing destination marker"
    );
    // The production worker drops its private lease (real ACK) before its
    // next Shutdown command, and the separate owner cannot request/join until
    // its original ACK-gated coordinator poll returns terminal. WorkerJoin is
    // the existing finished-only join tap, not backend destruction or a sleep.
    // Fixture Drop keeps files/admission/observer until that exact proof; an
    // assertion or cleanup failure retains them with the original poison path.
    trace.at(
        "drain exact Fixture owners",
        fixture.whole_drop_observation_metadata_for_test(),
    );
}
