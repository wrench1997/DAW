//! Explicit short actual NOIRE Whole export. The source and clone are real;
//! only the existing device-free callback drives the production App receipt path.
//! No conversion, Undo, native Save window, device monitoring or listening claim.
use super::*;
use crate::{
    automation::{AutomationCurve, AutomationLane, AutomationPoint, AutomationTarget},
    export_options::{WavExportOptions, WavLevelPolicy, WavSampleFormat},
    instrument_bounce::render::{MasterSongRenderOutcome, MasterSongRenderReport},
    mixer_graph::{MASTER_MIXER_TRACK_ID, MixerRouteDestination, MixerRouteTap},
    model::ProjectAutomation,
    plugins::plugin_runtime::BounceSourceSettlement,
};

#[path = "windows_noire_whole_song_oracle.rs"]
mod whole_oracle;

const WHOLE_SELECTOR: &str = "app::clip_render_job::windows_noire_bounce_qa::windows_noire_whole_song_qa::windows_noire_two_pattern_whole_master_export";
const WHOLE_OPT_IN: &str = "actual-noire-two-pattern-whole-master-export-v1";
const WHOLE_RATE: u32 = 48_000;
const WHOLE_TEMPO: f32 = 84.0;

pub(super) fn whole_scratch(mut project: Project) -> Check<Project> {
    require(
        project.channels[0].volume.is_finite() && project.channels[0].volume >= 0.0,
        "the saved source channel gain must be finite and nonnegative",
    )?;
    project.name = "Private NOIRE two-pattern Whole master qualification".into();
    project.tempo = WHOLE_TEMPO;
    project.swing = 0.0;
    project.song_length_beats = 2.0;
    let first = project.patterns[0].clone();
    project.patterns.clear();
    let template = project.clips[0].clone();
    project.clips.clear();
    let channel = project.channels[0].id;
    for (index, note) in [60, 64].into_iter().enumerate() {
        let mut pattern = first.clone();
        pattern.id = index as u32 + 1;
        pattern.name = format!("Whole NOIRE pattern {}", index + 1);
        pattern.length_steps = 4;
        pattern.channel_steps = vec![[false; 16]];
        pattern.notes = vec![PianoNote {
            id: index as u64 + 1,
            channel_id: Some(channel),
            group_id: None,
            note,
            start: 0.0,
            length: 1.0,
            velocity: 0.2,
            selected: false,
            muted: false,
        }];
        let mut clip = template.clone();
        clip.id = index as u32 + 1;
        clip.pattern_id = pattern.id;
        clip.name = format!("Whole NOIRE placement {}", index + 1);
        clip.start = index as f32;
        clip.length = 1.0;
        clip.source_offset = 0.0;
        project.patterns.push(pattern);
        project.clips.push(clip);
    }
    let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
    lane.set_curve(AutomationCurve::Linear);
    lane.replace_points(vec![
        AutomationPoint::new(0.0, 0.85),
        AutomationPoint::new(2.0, 0.35),
    ]);
    // An unplaced canonical Master lane is globally active. Keep exactly two
    // actual Pattern placements; no hidden third music/audio/automation clip.
    project.automation_lanes = vec![ProjectAutomation {
        id: 91001,
        name: "Whole NOIRE Master fade".into(),
        lane,
    }];
    require(
        project.patterns.len() == 2
            && project.clips.len() == 2
            && project
                .clips
                .iter()
                .all(|clip| clip.kind == ClipKind::Pattern)
            && project.plugin_instances.len() == 1
            && project.mixer_insert_slots.is_empty()
            && project.audio_assets.is_empty()
            && project.piano_notes.is_empty()
            && project.channels[0].pan == 0.0
            && !project.channels[0].muted
            && !project.channels[0].solo
            && project.mixer_tracks.iter().all(|track| {
                track.volume == 1.0 && track.pan == 0.0 && !track.muted && !track.solo
            })
            && project.mixer_routes.iter().all(|route| {
                route.enabled
                    && route.gain == 1.0
                    && route.tap == MixerRouteTap::PostFader
                    && route.destination
                        == (MixerRouteDestination::MainInput {
                            mixer_track_id: MASTER_MIXER_TRACK_ID,
                        })
            })
            && project
                .mixer_routes
                .iter()
                .filter(|route| route.source_mixer_track_id == project.channels[0].mixer_track)
                .count()
                == 1,
        "private Whole fixture is not the full two-pattern/two-placement single-source song",
    )?;
    Ok(project)
}

fn whole_options() -> WavExportOptions {
    WavExportOptions {
        sample_rate: WHOLE_RATE,
        sample_format: WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds: 3_000,
    }
}

fn pinned_hash(actual: &str, environment: &str) -> Check {
    let expected = std::env::var(environment).map_err(error)?;
    require(
        expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "explicit Whole executable/helper pin must be 64 hexadecimal digits",
    )?;
    require(
        actual.eq_ignore_ascii_case(&expected),
        "actual Whole executable/helper pin differs",
    )
}

fn model_image(app: &CitrusApp) -> Check<Vec<u8>> {
    serde_json::to_vec(&(
        &app.project,
        &app.history_snapshot,
        app.history_fingerprint,
        &app.undo_stack,
        &app.redo_stack,
        app.dirty,
        &app.saved_path,
    ))
    .map_err(error)
}

pub(super) fn persist_generated_raw_pcm(
    records: &[Arc<observe::Record>],
    directory: &Path,
) -> Check<Value> {
    let mut bytes = Vec::new();
    let mut blocks = 0;
    let mut nonfinite = 0;
    let mut instance = None;
    for record in records {
        let RecordKind::StrictPcm {
            frames, channels, ..
        } = &record.kind
        else {
            continue;
        };
        let identity = record
            .instance
            .ok_or("retained raw PCM has no actual clone identity")?;
        require(
            identity.phase == InstancePhase::StrictClone,
            "retained raw PCM belongs to a foreign source",
        )?;
        if let Some(expected) = instance {
            require(
                identity == expected,
                "retained raw PCM combines different strict clones",
            )?;
        } else {
            instance = Some(identity);
        }
        blocks += 1;
        require(
            blocks <= CLONE_REPLY_LIMIT
                && *frames == 128
                && channels.len() == 2
                && channels.iter().all(|channel| channel.len() == *frames),
            "retained raw PCM exceeds the existing clone observation bound or Q128 stereo shape",
        )?;
        for (left, right) in channels[0].iter().zip(&channels[1]) {
            for sample in [*left, *right] {
                nonfinite += usize::from(!sample.is_finite());
                bytes.extend_from_slice(&sample.to_bits().to_le_bytes());
            }
        }
    }
    if blocks == 0 {
        return Ok(json!({"available":false,"blocks":0,"no_raw_pcm_invented":true}));
    }
    let path = directory.join("whole-raw-generated-stereo.f32");
    new_write(&path, &bytes)?;
    let hash = oracles::sha256_hex(&bytes);
    let readback = read_bounded(&path, (CLONE_REPLY_LIMIT * 128 * 2 * 4) as u64)?;
    require(
        readback.len() == bytes.len() && oracles::sha256_hex(&readback) == hash,
        "actual retained raw PCM file differs on bounded readback",
    )?;
    Ok(
        json!({"available":true,"path":path,"blocks":blocks,"frames":blocks*128,
        "bytes":bytes.len(),"sha256":hash,"nonfinite_samples":nonfinite,
        "format":"actual observer Q128 replies in ordinal order, little-endian interleaved stereo f32 including latency/padding",
        "extra_sdk_process_or_capture":false,"dry_render_publication_claim":false}),
    )
}

pub(super) fn audit_short_whole(
    records: &[Arc<observe::Record>],
    observer_failure: Option<&str>,
    strict_instance: observe::InstanceIdentity,
    metadata: &crate::instrument_bounce::BouncePluginMetadata,
    report: &MasterSongRenderReport,
    source_channel_volume: f32,
    wav_bytes: &[u8],
) -> Check<(Value, Check)> {
    let audit = whole_oracle::audit_whole_noire(whole_oracle::WholeNoireAuditInput {
        records,
        observer_failure,
        strict_instance,
        metadata,
        report,
        source_channel_volume,
        wav_bytes,
    })?;
    Ok((audit.evidence(), audit.qualify_acoustics()))
}

// Driver-only diagnostic fold over already-retained actual worker observations.
// This does not change a deadline, qualification predicate, SDK call or tap.
pub(super) fn actual_strict_process_timing(
    records: &[Arc<observe::Record>],
    snapshot_failure: Option<&str>,
) -> Value {
    let mut strict_identity = None;
    let mut first_attempt = None;
    let mut last_attempt = None;
    let mut first_returned = None;
    let mut last_returned = None;
    let mut open_attempt = None::<(u64, u64)>;
    let mut previous_point = None::<(u64, u64)>;
    let mut attempts = 0_u64;
    let mut returned = 0_u64;
    let mut returned_ok = 0_u64;
    let mut returned_error = 0_u64;
    let mut pairs = 0_u64;
    let mut paired_call_ns = 0_u64;
    let mut minimum_call_ns = None::<u64>;
    let mut maximum_call_ns = 0_u64;
    let mut diagnostic_error = None;
    for record in records {
        let RecordKind::Call {
            operation: Operation::StrictProcess,
            boundary,
            outcome,
            ..
        } = &record.kind
        else {
            continue;
        };
        let Some(identity) = record
            .instance
            .filter(|identity| identity.phase == InstancePhase::StrictClone)
        else {
            diagnostic_error.get_or_insert("StrictProcess lacks its actual StrictClone identity");
            continue;
        };
        if strict_identity.is_some_and(|prior| prior != identity) {
            diagnostic_error.get_or_insert("StrictProcess belongs to more than one exact clone");
            continue;
        }
        strict_identity.get_or_insert(identity);
        let point = (record.ordinal, record.monotonic_elapsed_ns);
        if let Some((previous_ordinal, previous_ns)) = previous_point.replace(point)
            && (point.0 <= previous_ordinal || point.1 < previous_ns)
        {
            diagnostic_error.get_or_insert("StrictProcess record clock/order regressed");
        }
        match boundary {
            Boundary::Attempt => {
                if outcome.is_some() {
                    diagnostic_error.get_or_insert("StrictProcess attempt has a returned outcome");
                }
                attempts += 1;
                first_attempt.get_or_insert(point);
                last_attempt = Some(point);
                if open_attempt.replace(point).is_some() {
                    diagnostic_error
                        .get_or_insert("StrictProcess attempt precedes its prior return");
                }
            }
            Boundary::Returned => {
                returned += 1;
                first_returned.get_or_insert(point);
                last_returned = Some(point);
                if matches!(outcome, Some(observe::CallOutcome::Ok)) {
                    returned_ok += 1;
                } else {
                    returned_error += 1;
                }
                if let Some((attempt_ordinal, attempt_ns)) = open_attempt.take() {
                    if let Some(duration) = point
                        .1
                        .checked_sub(attempt_ns)
                        .filter(|_| point.0 > attempt_ordinal)
                    {
                        pairs += 1;
                        minimum_call_ns =
                            Some(minimum_call_ns.map_or(duration, |v| v.min(duration)));
                        maximum_call_ns = maximum_call_ns.max(duration);
                        if let Some(total) = paired_call_ns.checked_add(duration) {
                            paired_call_ns = total;
                        } else {
                            diagnostic_error.get_or_insert("StrictProcess duration sum overflow");
                        }
                    } else {
                        diagnostic_error
                            .get_or_insert("StrictProcess return clock/order regressed");
                    }
                } else {
                    diagnostic_error.get_or_insert("StrictProcess return lacks its actual attempt");
                }
            }
        }
    }
    if open_attempt.is_some() {
        diagnostic_error.get_or_insert("StrictProcess still has an unreturned actual attempt");
    }
    let point_json = |point: Option<(u64, u64)>| {
        point.map(|(ordinal, ns)| json!({"record_ordinal":ordinal,"monotonic_elapsed_ns":ns}))
    };
    let span_ns = first_attempt
        .zip(last_returned)
        .and_then(|(first, last)| last.1.checked_sub(first.1));
    let instance = strict_identity.map(|identity| {
        json!({
            "ordinal":identity.ordinal,"phase":format!("{:?}",identity.phase),
            "worker_id":identity.worker_id,"source_instance_id":identity.source_instance_id,
        })
    });
    let points = json!({
        "first_attempt":point_json(first_attempt),"last_attempt":point_json(last_attempt),
        "first_returned":point_json(first_returned),"last_returned":point_json(last_returned),
    });
    let calls = json!({
        "attempts":attempts,"returned":returned,"returned_ok":returned_ok,
        "returned_error":returned_error,"complete_pairs":pairs,
        "complete_success_pairs":attempts>0 && attempts==returned && returned==returned_ok
            && returned==pairs && diagnostic_error.is_none() && snapshot_failure.is_none(),
        "sum_paired_call_duration_ns":paired_call_ns,
        "minimum_paired_call_duration_ns":minimum_call_ns,
        "maximum_paired_call_duration_ns":maximum_call_ns,
    });
    json!({
        "source":"existing actual worker Record monotonic timestamps, queried driver-side",
        "strict_instance":instance,"points":points,"calls":calls,
        "first_attempt_to_last_returned_ns":span_ns,"diagnostic_error":diagnostic_error,
        "snapshot_failure":snapshot_failure,"snapshot_record_count":records.len(),
        "clock_origin":"this ObservationRun installation; not helper wall clock",
        "no_extra_sdk_process_capture_or_tap":true,
        "deadline_and_qualification_predicates_unchanged":true,
        "budget_claim":"measured span only; no full-song rate or deadline guarantee",
    })
}

// Diagnostic-only: explicit white-list summaries of already-owned operands.
// Never Debug a RecordKind: StateLoad/Capture/StrictPcm contain private payloads.
const INITIAL_DRIVER_RECORD_LIMIT: usize = 64;
const INITIAL_DRIVER_OWNER_LIMIT: usize = 4;

fn initial_driver_text(value: &str) -> Value {
    let mut end = value.len().min(4096);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    json!({"text":&value[..end],"utf8_bytes":value.len(),"truncated":end<value.len()})
}

fn initial_driver_outcome(outcome: &observe::CallOutcome) -> Value {
    match outcome {
        observe::CallOutcome::Ok => json!({"kind":"Ok"}),
        observe::CallOutcome::Error(reason) => {
            json!({"kind":"Error","reason":initial_driver_text(reason)})
        }
        observe::CallOutcome::StrictError(reason) => {
            json!({"kind":"StrictError","backend":backend_failure_json(reason)})
        }
    }
}

fn initial_driver_runtime(event: &crate::plugins::plugin_runtime::RuntimeEvent) -> Value {
    use crate::plugins::plugin_runtime::RuntimeEvent;
    match event {
        RuntimeEvent::SlotReady {
            slot,
            name,
            latency_samples,
            tail_samples,
            ..
        } => json!({
            "kind":"SlotReady","slot":slot,"name":initial_driver_text(name),
            "latency_samples":latency_samples,"tail_samples":tail_samples,
        }),
        RuntimeEvent::SlotFault { slot, message } => json!({
            "kind":"SlotFault","slot":slot,"message":initial_driver_text(message),
        }),
        RuntimeEvent::StateRejected {
            slot,
            request_id,
            epoch,
            reason,
        } => json!({
            "kind":"StateRejected","slot":slot,"request_id":request_id,"epoch":epoch,
            "reason":format!("{reason:?}"),
        }),
        RuntimeEvent::ParameterCommandFailed {
            slot,
            request_id,
            command,
            id,
            message,
        } => json!({
            "kind":"ParameterCommandFailed","slot":slot,"request_id":request_id,
            "command":format!("{command:?}"),"id":id,"message":initial_driver_text(message),
        }),
        RuntimeEvent::State {
            slot,
            request_id,
            epoch,
            bytes,
        } => json!({
            "kind":"State","slot":slot,"request_id":request_id,"epoch":epoch,
            "byte_len":bytes.len(),"private_bytes_omitted":true,
        }),
        RuntimeEvent::ShutdownComplete => json!({"kind":"ShutdownComplete"}),
        RuntimeEvent::ParameterValue { .. } => json!({"kind":"ParameterValue"}),
        RuntimeEvent::ParameterCatalogPage { .. } => json!({"kind":"ParameterCatalogPage"}),
        RuntimeEvent::ParameterSetAck { .. } => json!({"kind":"ParameterSetAck"}),
    }
}

fn initial_driver_record(record: &observe::Record) -> Value {
    let identity = record.instance.map(|i| {
        json!({
            "ordinal":i.ordinal,"phase":format!("{:?}",i.phase),
            "worker_id":i.worker_id,"source_instance_id":i.source_instance_id,
        })
    });
    let detail = match &record.kind {
        RecordKind::Call {
            operation,
            boundary,
            frames,
            outcome,
            ..
        } => json!({
            "kind":"Call","operation":format!("{operation:?}"),"boundary":format!("{boundary:?}"),
            "frames":frames,"outcome":outcome.as_ref().map(initial_driver_outcome),
        }),
        RecordKind::Lifecycle(site) => json!({"kind":"Lifecycle","site":site}),
        RecordKind::StateLoad { bytes, outcome } => json!({
            "kind":"StateLoad","byte_len":bytes.len(),"outcome":initial_driver_outcome(outcome),
            "private_bytes_omitted":true,
        }),
        RecordKind::BaseRead { site, id, raw_bits } => json!({
            "kind":"BaseRead","site":site,"id":id,
            "raw_bits":raw_bits.as_ref().ok(),"error":raw_bits.as_ref().err().map(|e|initial_driver_text(e)),
        }),
        RecordKind::Metadata { site, result } => json!({
            "kind":"Metadata","site":site,
            "metadata":result.as_ref().ok().map(metadata_json),
            "error":result.as_ref().err().map(|e|initial_driver_text(e)),
        }),
        RecordKind::AudioDeclaration {
            input_buses,
            output_buses,
        } => json!({
            "kind":"AudioDeclaration","input_bus_count":input_buses.len(),"output_bus_count":output_buses.len(),
        }),
        RecordKind::EventDeclaration { inputs, outputs } => json!({
            "kind":"EventDeclaration","input_bus_count":inputs.len(),"output_bus_count":outputs.len(),
        }),
        RecordKind::RuntimeEvent {
            endpoint_id,
            project_session,
            instance_id,
            event,
        } => json!({
            "kind":"RuntimeEvent","endpoint_id":endpoint_id,"project_session":project_session,
            "instance_id":instance_id,"event":initial_driver_runtime(event),
        }),
        RecordKind::OrdinaryState {
            endpoint_id,
            project_session,
            instance_id,
            slot,
            request_id,
            epoch,
            byte_len,
        } => json!({
            "kind":"OrdinaryState","endpoint_id":endpoint_id,"project_session":project_session,
            "instance_id":instance_id,"slot":slot,"request_id":request_id,"epoch":epoch,
            "byte_len":byte_len,"private_bytes_omitted":true,
        }),
        RecordKind::AppGeneratorEndpointEvent(event) => json!({
            "kind":"AppGeneratorEndpointEvent","scalar_endpoint_event":format!("{event:?}"),
        }),
        RecordKind::WorkerJoin { worker_id, outcome } => json!({
            "kind":"WorkerJoin","worker_id":worker_id,"outcome":format!("{outcome:?}"),
        }),
        RecordKind::Worker {
            site,
            worker_id,
            epoch,
            sequence,
            stats,
            ..
        } => json!({
            "kind":"Worker","site":site,"worker_id":worker_id,"epoch":epoch,"sequence":sequence,
            "stats":format!("{stats:?}"),
        }),
        RecordKind::Capture { state, .. } => {
            json!({"kind":"Capture","opaque_byte_len":state.opaque_state.len(),"private_bytes_omitted":true})
        }
        RecordKind::StrictPcm { .. } => json!({"kind":"StrictPcm","private_pcm_omitted":true}),
        RecordKind::Midi { .. } => json!({"kind":"Midi"}),
        RecordKind::AppAudioAssetEvent(_) => json!({"kind":"AppAudioAssetEvent"}),
        RecordKind::AppTimelineRuntimeEvent { .. } => json!({"kind":"AppTimelineRuntimeEvent"}),
        RecordKind::WorkerCaptureGuard { worker_id, .. } => {
            json!({"kind":"WorkerCaptureGuard","worker_id":worker_id})
        }
        RecordKind::ResetReport {
            frames,
            process_error,
            ..
        } => {
            json!({"kind":"ResetReport","frames":frames,"process_error":process_error.as_ref().map(|e|initial_driver_text(e))})
        }
        RecordKind::StrictCaptureIdentity(_) => json!({"kind":"StrictCaptureIdentity"}),
        RecordKind::RenderReport(_) => json!({"kind":"RenderReport"}),
        RecordKind::Disposal {
            terminated, error, ..
        } => {
            json!({"kind":"Disposal","terminated":terminated,"error":error.as_ref().map(backend_failure_json)})
        }
        RecordKind::HookInventory { site, inactive } => {
            json!({"kind":"HookInventory","site":site,"inactive":inactive})
        }
    };
    json!({"ordinal":record.ordinal,"monotonic_elapsed_ns":record.monotonic_elapsed_ns,
        "stage":format!("{:?}",record.stage),"instance":identity,"detail":detail})
}

fn initial_driver_trace(records: &[Arc<observe::Record>], total_eligible: usize) -> Value {
    let eligible = |record: &&Arc<observe::Record>| {
        record
            .instance
            .is_none_or(|identity| identity.phase == InstancePhase::InitialSource)
    };
    let rows = records
        .iter()
        .filter(eligible)
        .take(INITIAL_DRIVER_RECORD_LIMIT)
        .map(|record| initial_driver_record(record))
        .collect::<Vec<_>>();
    json!({"read_only":true,"matched_records":total_eligible,"row_limit":INITIAL_DRIVER_RECORD_LIMIT,
        "copied_scope_records":records.len(),"truncated":total_eligible>rows.len(),"records":rows,"raw_state_or_pcm_serialized":false,
        "scope":"InitialSource incarnation plus driver/worker control records; StrictClone and UndoSource excluded"})
}

struct WholeOwner {
    owner: Owner,
    report: Option<MasterSongRenderReport>,
    expected_source: Option<LiveBinding>,
    before_model: Option<Vec<u8>>,
}
impl WholeOwner {
    fn new(owner: Owner) -> Self {
        Self {
            owner,
            report: None,
            expected_source: None,
            before_model: None,
        }
    }
    fn tick(&mut self, deadline: Instant, qualify: bool) -> Check {
        self.owner.tick(deadline, qualify)?;
        let app = self.owner.app_mut();
        app.poll_plugin_parameter_callback_receipts();
        app.drive_plugin_parameter_edit_drafts();
        app.poll_native_editor_snapshots();
        app.drive_whole_song_export();
        if let Some(job) = self.owner.app().whole_song_export_job.as_ref() {
            require(
                job.genuine_whole_production_factory_selected(),
                "a test render starter replaced the production StrictBackend factory",
            )?;
            if qualify && let Some(reason) = job.genuine_whole_failure() {
                return Err(reason.to_owned());
            }
        }
        Ok(())
    }
    fn exercise(&mut self, project: Project, descriptor: PluginDescriptor) -> Check {
        let channel = self
            .owner
            .begin_source_initialization(project, descriptor)?;
        // Whole admission must not borrow the sole-occurrence geometry policy.
        self.owner.wait(40, |owner| {
            owner.app().pending_generator_chains.is_empty()
                && owner.app().running_generator_chains.len() == 1
                && owner.paused()
                && owner.app().stopped_bounce_observe(channel).is_ok()
        })?;
        require(
            self.owner
                .app()
                .audio
                .as_ref()
                .unwrap()
                .snapshot()
                .sample_rate
                == WHOLE_RATE,
            "actual offline source sample rate is not 48 kHz",
        )?;
        let deadline = self
            .owner
            .deadline
            .min(Instant::now() + Duration::from_secs(40));
        loop {
            self.owner.app_mut().poll_native_editor_snapshots();
            if self
                .owner
                .app()
                .prepare_whole_song_export_review(whole_options(), false)
                .is_ok()
            {
                break;
            }
            self.tick(deadline, true)?;
        }
        // Initial ordinary native snapshots may legitimately normalize saved
        // bases. Establish the clean history image only after those real
        // receipts and the resulting full Timeline have converged.
        self.owner.finish_source_initialization()?;
        self.expected_source = Some(self.owner.app().stopped_bounce_observe(channel)?);
        self.before_model = Some(model_image(self.owner.app())?);
        require(
            !self.owner.app().dirty,
            "source initialization did not establish a clean baseline",
        )?;

        let ctx = self.owner.ctx.clone();
        let session = self.owner.app().project_session;
        let dialog = &mut self.owner.app_mut().export_dialog;
        dialog.open_whole_song(session, WHOLE_RATE, false);
        dialog.test_set_options(whole_options());
        require(
            !export_ui::tests::click(&ctx, dialog, "Review…"),
            "Settings unexpectedly requested destination",
        )?;
        require(
            !dialog.confirmed_whole_fixed_tail(),
            "Whole tail consent was prefilled",
        )?;
        require(
            !export_ui::tests::click(&ctx, dialog, "Choose destination and export…"),
            "unconfirmed destination button was enabled",
        )?;
        require(
            dialog.begin_destination(session, false).is_none(),
            "unconfirmed destination gate was bypassed",
        )?;
        require(
            !export_ui::tests::click(&ctx, dialog, "Confirm fixed VST tail window"),
            "checkbox unexpectedly requested destination",
        )?;
        require(
            dialog.confirmed_whole_fixed_tail(),
            "actual Review checkbox did not confirm the fixed tail",
        )?;
        require(
            export_ui::tests::click(&ctx, dialog, "Choose destination and export…"),
            "confirmed destination button was not accepted",
        )?;
        let options = dialog
            .begin_destination(session, false)
            .ok_or("actual destination gate was refused")?;
        let confirmed = dialog.confirmed_whole_fixed_tail();
        require(
            options == whole_options() && confirmed,
            "reviewed Whole options or permit changed",
        )?;
        let review = self
            .owner
            .app()
            .prepare_whole_song_export_review(options, confirmed)?;
        let path = self.owner.inputs.directory.join("whole-song-master.wav");
        require(!path.exists(), "private Whole output path already exists")?;
        self.owner.attempted_output_path = Some(path.clone());
        // Keep the Whole clone in its explicit observer stage; Conversion and
        // Undo retain their existing source and owner ordering guards.
        self.owner
            .observer
            .set_stage(Stage::WholeExport)
            .map_err(error)?;
        self.owner
            .app_mut()
            .begin_whole_song_export(review, path.clone())?;
        self.owner.app_mut().export_dialog.finish_destination(true);
        require(
            self.owner
                .app()
                .whole_song_export_job
                .as_ref()
                .is_some_and(|job| job.genuine_whole_production_factory_selected()),
            "genuine Whole export must retain render_starter=None from construction",
        )?;
        while self.owner.app().whole_song_export_fence_active() {
            self.tick(self.owner.deadline, true)?;
        }
        let job = self
            .owner
            .app()
            .whole_song_export_job
            .as_ref()
            .ok_or("original Whole controller owner disappeared")?;
        require(
            job.genuine_whole_terminal_owners_drained(),
            "Whole outcome preceded actual capture/render termination",
        )?;
        require(
            job.genuine_whole_path() == path.as_path(),
            "controller output path differs from reviewed destination",
        )?;
        self.report = Some(match job.genuine_whole_outcome() {
            Some(MasterSongRenderOutcome::Complete(report)) => report.clone(),
            Some(MasterSongRenderOutcome::Failed(failure)) => return Err(error(&failure.primary)),
            None => return Err("Whole controller returned no typed publication outcome".into()),
        });
        self.audit_stop_and_ownership()?;
        require(
            model_image(self.owner.app())?.as_slice()
                == self.before_model.as_ref().unwrap().as_slice(),
            "Whole export changed Project, history, dirty state or saved path",
        )?;
        let report = self.report.as_ref().unwrap();
        require(
            !report.master_output.tail_limit_reached
                || self.owner.app().whole_song_export_tail_result.is_some(),
            "unknown/long reported tail was not retained as a Whole App metadata warning",
        )?;
        let bytes = read_bounded(&path, 16 * 1024 * 1024)?;
        let (records, observer_failure) = self
            .owner
            .observer
            .with_snapshot(|snapshot| (snapshot.records.clone(), snapshot.failure));
        let captures = records
            .iter()
            .filter_map(|record| match &record.kind {
                RecordKind::Capture {
                    label,
                    identity,
                    receipt_identity,
                    state,
                } => Some((*label, *identity, *receipt_identity, state)),
                _ => None,
            })
            .collect::<Vec<_>>();
        require(
            captures.len() == 1 && captures[0].0 == CaptureLabel::Initial && captures[0].2 != 0,
            "Whole must consume exactly one actual ACKed private capture receipt",
        )?;
        require(
            captures[0].1 == report.dry_render.identity,
            "dry report is not bound to the real Whole capture receipt",
        )?;
        let metadata = &captures[0].3.metadata;
        require(
            metadata.class_uid == KONTAKT_UID
                && metadata.version == "8.4.1"
                && metadata.uses_selected_stereo_bounce()
                && metadata.output_buses.len() == 32,
            "actual NOIRE identity/version/complete stereo participation differs",
        )?;
        metadata.validate(WHOLE_RATE)?;
        let strict = records
            .iter()
            .filter_map(|record| {
                record
                    .instance
                    .filter(|instance| instance.phase == InstancePhase::StrictClone)
                    .map(|instance| (instance.ordinal, instance))
            })
            .collect::<BTreeMap<_, _>>();
        // Identify distinct actual incarnations by their run-owned ordinal,
        // never by a class UID/path shared with another loaded instance.
        require(
            strict.len() == 1,
            "actual strict clone identity is absent or duplicated",
        )?;
        let strict_instance = *strict.values().next().unwrap();
        let audit = whole_oracle::audit_whole_noire(whole_oracle::WholeNoireAuditInput {
            records: &records,
            observer_failure,
            strict_instance,
            metadata,
            report,
            source_channel_volume: self.owner.app().project.channels[0].volume,
            wav_bytes: &bytes,
        })?;
        self.owner.evidence["pcm_and_midi_audit"] = audit.evidence();
        self.owner.evidence["authoritative_capture"] = json!({
            "receipt_identity":captures[0].2,"identity":capture_identity_json(captures[0].1),
            "saved_state_envelope":state_envelope(&captures[0].3.opaque_state)?,
            "metadata":metadata_json(metadata),"generic_base_count":captures[0].3.parameter_bases.len(),
            "extra_source_capture_or_vendor_call":false,
        });
        self.owner.evidence["whole_export"] = json!({
            "real_production_controller":true,"test_render_starter_none":true,
            "actual_review_checkbox_and_destination_gate":true,"native_save_window_invoked":false,
            "fixed_tail_confirmed":confirmed,"tail_milliseconds":options.tail_milliseconds,
            "source_sample_rate":WHOLE_RATE,"tempo":WHOLE_TEMPO,"song_beats":2,
            "patterns":2,"pattern_placements":2,"global_master_volume_automation":true,
            "project_history_dirty_saved_path_unchanged":true,"original_source_remains_paused":true,
            "conversion_performed":false,"undo_or_redo_exercised":false,
            "wav_sha256":oracles::sha256_hex(&bytes),"wav_bytes":bytes.len(),
            "metadata_tail_limit_reached":report.master_output.tail_limit_reached,
            "metadata_limit_is_not_acoustic_silence":true,"physical_listening":false,
        });
        // Even a truncated/quiet render keeps its actual WAV and full measurements.
        audit.qualify_acoustics()?;
        self.owner.inputs.unchanged()
    }
    fn audit_stop_and_ownership(&mut self) -> Check {
        let app = self.owner.app();
        let job = app
            .whole_song_export_job
            .as_ref()
            .ok_or("Whole job absent")?;
        let binding = job
            .genuine_whole_binding()
            .ok_or("actual stopped binding absent")?;
        let stop = job
            .genuine_whole_stop_preparation()
            .ok_or("actual stop intent absent")?;
        let activation = stop.activation.ok_or("actual queue receipt absent")?;
        let target = job
            .genuine_whole_source_target()
            .ok_or("actual source settlement target absent")?;
        let settled = job
            .genuine_whole_settlement()
            .ok_or("actual settlement observation absent")?;
        let expected = self.expected_source.unwrap();
        require(
            binding.stamp.project_session == expected.stamp.project_session
                && binding.stamp.engine_session == expected.stamp.engine_session
                && binding.stamp.endpoint_id == expected.stamp.endpoint_id
                && binding.stamp.instance_id == expected.stamp.instance_id
                && binding.stamp.channel_id == expected.stamp.channel_id
                && binding.stamp.timeline_revision == expected.stamp.timeline_revision
                && binding.sample_rate == WHOLE_RATE,
            "Whole source or full Timeline binding changed",
        )?;
        require(
            !stop.superseded
                && activation.committed_epoch == Some(binding.stamp.epoch)
                && stop.generation == binding.stamp.timeline_revision
                && job.genuine_whole_old_epoch() != Some(binding.stamp.epoch)
                && settled.settlement == BounceSourceSettlement::Ready
                && settled.coherent
                && settled.callback_ready
                && settled.callback_identity == Some(target)
                && settled.worker_valid
                && !settled.exhausted
                && settled.busy == 0
                && settled.parameter_admission == 0
                && !settled.capture_busy,
            "Whole capture lacked actual queue/confirmation, source drain or worker settlement",
        )?;
        require(
            app.running_generator_chains.len() == 1
                && app.pending_generator_chains.is_empty()
                && app.pending_generator_removals.is_empty()
                && app.clip_render_job.is_none()
                && app.undo_stack.is_empty()
                && app.redo_stack.is_empty()
                && self.owner.paused(),
            "Whole did not preserve one paused original source without conversion",
        )?;
        let evidence = json!({
            "generation":stop.generation,"seek_serial":stop.seek_serial,"frame":stop.frame,
            "transport_request":stop.transport_request,"activation_request":activation.request_id,
            "minimum_epoch":activation.minimum_epoch,"committed_epoch":activation.committed_epoch,
            "old_epoch":job.genuine_whole_old_epoch(),"callback_identity_matches_target":true,
            "worker_submitted":settled.submitted,"worker_consumed":settled.consumed,"worker_terminal":settled.terminal,
            "worker_busy":settled.busy,"reset_completed_epoch":settled.reset_completed_epoch,
            "records_are_actual_notify_and_guard_results":true,
        });
        self.owner.evidence["actual_stop_receipts"] = evidence;
        Ok(())
    }
    fn cleanup(&mut self) -> Check {
        self.owner.app_mut().cancel_whole_song_export();
        let deadline = Instant::now() + Duration::from_secs(35);
        while self.owner.app().whole_song_export_fence_active() {
            self.tick(deadline, false)?;
        }
        require(
            self.owner
                .app()
                .whole_song_export_job
                .as_ref()
                .is_none_or(|job| job.genuine_whole_terminal_owners_drained()),
            "cleanup cannot discard a Whole capture/render/retired-source owner",
        )?;
        self.owner.cleanup()
    }
    fn initial_driver_snapshot(&self, moment: &str) -> Value {
        let app = self.owner.app();
        let row = |channel: u32,
                   session: u64,
                   endpoint: u64,
                   instance: u64,
                   control: &crate::plugins::plugin_runtime::PluginChainControl,
                   guard: &PluginWorkerGuard| {
            let stats = control.stats();
            let runtime = app
                .project
                .plugin_instances
                .iter()
                .find(|i| i.id == instance)
                .map(|i| initial_driver_text(&format!("{:?}", i.runtime_status)));
            json!({"channel":channel,"project_session":session,"endpoint_id":endpoint,
                "instance_id":instance,"guard_worker_id":guard.genuine_worker_id(),
                "runtime_status":runtime,"outstanding_admin_work":control.has_outstanding_admin_work(),
                "endpoint_snapshot":format!("{:?}",control.plugin_endpoint_snapshot()),
                "stats":format!("{stats:?}")})
        };
        let pending = app
            .pending_generator_chains
            .iter()
            .take(INITIAL_DRIVER_OWNER_LIMIT)
            .map(|(channel, c)| {
                row(
                    *channel,
                    c.project_session,
                    c.endpoint_id,
                    c.instance_id,
                    &c.control,
                    &c.guard,
                )
            })
            .collect::<Vec<_>>();
        let running = app
            .running_generator_chains
            .iter()
            .take(INITIAL_DRIVER_OWNER_LIMIT)
            .map(|(channel, c)| {
                row(
                    *channel,
                    c.project_session,
                    c.endpoint_id,
                    c.instance_id,
                    &c.control,
                    &c.guard,
                )
            })
            .collect::<Vec<_>>();
        let fresh_workers = app
            .retired_fresh_source_candidates
            .iter()
            .take(INITIAL_DRIVER_OWNER_LIMIT)
            .map(PluginWorkerGuard::genuine_worker_id)
            .collect::<Vec<_>>();
        let owners = json!({
            "pending_count":app.pending_generator_chains.len(),"pending":pending,
            "pending_truncated":app.pending_generator_chains.len()>INITIAL_DRIVER_OWNER_LIMIT,
            "running_count":app.running_generator_chains.len(),"running":running,
            "running_truncated":app.running_generator_chains.len()>INITIAL_DRIVER_OWNER_LIMIT,
            "retired_fresh_count":app.retired_fresh_source_candidates.len(),
            "retired_fresh_worker_ids":fresh_workers,
            "retired_fresh_truncated":app.retired_fresh_source_candidates.len()>INITIAL_DRIVER_OWNER_LIMIT,
            "deferred_rebuild_count":app.deferred_generator_rebuilds.len(),
            "pending_removal_count":app.pending_generator_removals.len(),
        });
        let (handles, observer_failure, total_records, total_eligible) =
            self.owner.observer.with_snapshot(|snapshot| {
                let eligible = |record: &&Arc<observe::Record>| {
                    record
                        .instance
                        .is_none_or(|i| i.phase == InstancePhase::InitialSource)
                };
                let matched = snapshot.records.iter().filter(eligible).count();
                let selected = snapshot
                    .records
                    .iter()
                    .filter(eligible)
                    .take(INITIAL_DRIVER_RECORD_LIMIT)
                    .cloned()
                    .collect::<Vec<_>>();
                (selected, snapshot.failure, snapshot.records.len(), matched)
            });
        let callback = self.owner.callback.as_ref().map(|c| {
            let s = c.genuine_snapshot();
            json!({"playing":s.playing,"generation":s.generation,"epoch":s.epoch,
                "active_epoch":s.active_epoch,"transport_request":s.request_id})
        });
        json!({"moment":moment,"read_only":true,"owners":owners,"callback":callback,
            "playing":app.playing,"project_session":app.project_session,
            "toast":app.toast.as_ref().map(|(message,_)|initial_driver_text(message)),
            "restoration":self.owner.restoration_diagnostics(moment),
            "observed_records":initial_driver_trace(&handles,total_eligible),"snapshot_record_count":total_records,
            "snapshot_failure":observer_failure,"no_sdk_or_poll_or_capture_added":true,
            "diagnostic_reconstruction_of_run03":false})
    }

    fn record_initial_driver_snapshot(&mut self, moment: &'static str) {
        let snapshot = catch_unwind(AssertUnwindSafe(|| self.initial_driver_snapshot(moment)))
            .unwrap_or_else(|payload|json!({"moment":moment,"diagnostic_error":panic_error(payload,"read-only initial driver snapshot"),"success_predicates_unchanged":true}));
        self.owner.evidence[moment] = snapshot;
    }

    fn final_receipt(&mut self, body: &Check, cleanup: &Check) -> Check {
        let records = self
            .owner
            .observer
            .with_snapshot(|snapshot| snapshot.records.clone());
        self.owner.evidence["actual_strict_process_timing"] = actual_strict_process_timing(
            &records,
            self.owner
                .observer
                .with_snapshot(|snapshot| snapshot.failure),
        );
        let terminal = TerminalFacts::from_records(&records);
        let terminal_result: Check = (|| {
            terminal.qualify(false)?;
            if body.is_ok() {
                require(
                    terminal.source_instances.len() == 1
                        && terminal.workers.len() == 1
                        && terminal.source_instances[0].0 == InstancePhase::InitialSource
                        && terminal.clone_instances == 1
                        && terminal.clone_reaps.len() == 1
                        && terminal.clone_reaps[0].1,
                    "Whole success needs one original source join and one naturally exited/reaped strict clone",
                )?;
            }
            Ok(())
        })();
        let observer_result = self.owner.observer.with_snapshot(|snapshot| {
            require(
                snapshot.failure.is_none(),
                snapshot.failure.unwrap_or("observer evidence incomplete"),
            )
        });
        let pins = self.owner.inputs.unchanged();
        let raw_pcm = persist_generated_raw_pcm(&records, &self.owner.inputs.directory);
        self.owner.evidence["retained_actual_raw_pcm"] = match &raw_pcm {
            Ok(evidence) => evidence.clone(),
            Err(reason) => json!({"write_or_readback_error":reason,"partial_files_preserved":true}),
        };
        let job = self.owner.app().whole_song_export_job.as_ref();
        let capture_failure = job
            .and_then(|job| job.genuine_whole_capture_failure())
            .map(error);
        let phase = job.map(|job| job.genuine_whole_phase());
        let render_failure = job
            .and_then(|job| job.genuine_whole_outcome())
            .and_then(|outcome| match outcome {
                MasterSongRenderOutcome::Failed(failure) => Some(json!({
                    "kind":format!("{:?}",failure.kind),"primary":error(&failure.primary),
                    "backend":failure.backend.as_deref().map(backend_failure_json),
                    "cleanup_backend":failure.cleanup_backend.as_deref().map(backend_failure_json),
                    "cleanup_grace_elapsed":failure.cleanup_grace_elapsed,
                })),
                _ => None,
            });
        attach_capture_diagnostics(
            &mut self.owner.evidence,
            &self.owner.inputs.directory,
            &records,
            cleanup.is_ok() && observer_result.is_ok(),
        );
        self.owner.evidence["result"] = json!({
            "body_passed":body.is_ok(),"body_error":body.as_ref().err(),
            "cleanup_terminal":cleanup.is_ok(),"cleanup_error":cleanup.as_ref().err(),
            "terminal_evidence_passed":terminal_result.is_ok(),"terminal_error":terminal_result.as_ref().err(),
            "terminal_ownership":terminal.evidence(),"observer_complete":observer_result.is_ok(),
            "observer_error":observer_result.as_ref().err(),"phase":phase,"capture_failure":capture_failure,"render_failure":render_failure,
            "original_input_and_binaries_unchanged":pins.is_ok(),"pin_error":pins.as_ref().err(),
            "attempted_output_path":self.owner.attempted_output_path,
            "typed_complete_publication_outcome":self.report.is_some(),
            "source_worker_join_is_not_independent_source_helper_reap_evidence":true,
            "no_conversion_or_undo_qualification":true,"partial_files_preserved":true,
            "success":body.is_ok() && cleanup.is_ok() && terminal_result.is_ok() && observer_result.is_ok() && pins.is_ok() && raw_pcm.is_ok(),
        });
        new_json(
            &self.owner.inputs.directory.join("whole-result.json"),
            &self.owner.evidence,
        )?;
        terminal_result?;
        observer_result?;
        raw_pcm?;
        pins
    }
}

thread_local! {
    static WHOLE_QUARANTINED: std::cell::RefCell<Option<ManuallyDrop<WholeOwner>>>
        = const { std::cell::RefCell::new(None) };
}

#[test]
#[ignore = "actual local NOIRE Whole master export; explicit pinned private env and root-only launch required"]
fn windows_noire_two_pattern_whole_master_export() {
    require(
        std::env::var("CITRUS_NOIRE_WHOLE_QA_OPT_IN").as_deref() == Ok(WHOLE_OPT_IN),
        "explicit Whole NOIRE opt-in is absent",
    )
    .unwrap();
    let args: Vec<_> = std::env::args().skip(1).collect();
    require(
        args == [
            WHOLE_SELECTOR,
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ],
        "only the exact once-only ignored Whole selector is admitted",
    )
    .unwrap();
    require(
        !ADMITTED.swap(true, Ordering::AcqRel),
        "actual NOIRE fixture already admitted in this process",
    )
    .unwrap();
    let directory = absolute_env("CITRUS_NOIRE_WHOLE_QA_DIR").unwrap();
    fs::create_dir(&directory)
        .expect("Whole QA directory must be newly created; preserve old evidence");
    let loaded = catch_unwind(AssertUnwindSafe(|| {
        let (inputs, project, descriptor, mut evidence) = Inputs::load(directory.clone())?;
        pinned_hash(&inputs.executable_hash, "CITRUS_NOIRE_WHOLE_QA_EXE_SHA256")?;
        pinned_hash(&inputs.helper_hash, "CITRUS_NOIRE_WHOLE_QA_HELPER_SHA256")?;
        let project = whole_scratch(project)?;
        evidence["fixture"] = json!(WHOLE_OPT_IN);
        evidence["scope"] = json!("Actual one-source/two-pattern Whole master export through production controller and StrictBackend; no conversion/Undo/native Save window/device/listening qualification");
        evidence["observer_stage_schema"] = json!("InitialSource then WholeExport/StrictClone then Cleanup; never Conversion/Undo");
        Ok::<_, String>((inputs, project, descriptor, evidence))
    })).unwrap_or_else(|payload| Err(panic_error(payload, "Whole preflight")));
    let (inputs, project, descriptor, evidence) = loaded.unwrap_or_else(|reason| {
        new_json(&directory.join("whole-result.json"), &json!({"schema":1,"fixture":WHOLE_OPT_IN,
            "stage":"preflight","success":false,"error":reason,"plugin_started":false,"partial_files_preserved":true})).unwrap();
        panic!("Whole NOIRE preflight failed; preserve {}", directory.display());
    });
    let initialized = catch_unwind(AssertUnwindSafe(|| Owner::new(inputs, evidence)))
        .unwrap_or_else(|payload| Err(panic_error(payload, "Whole owner construction")));
    let mut owner = ManuallyDrop::new(WholeOwner::new(initialized.unwrap_or_else(|reason| {
        new_json(
            &directory.join("whole-result.json"),
            &json!({"schema":1,"fixture":WHOLE_OPT_IN,
            "stage":"owner-construction","success":false,"error":reason,"plugin_started":false}),
        )
        .unwrap();
        panic!(
            "Whole NOIRE owner construction failed; preserve {}",
            directory.display()
        );
    })));
    let body = catch_unwind(AssertUnwindSafe(|| owner.exercise(project, descriptor)))
        .unwrap_or_else(|payload| Err(panic_error(payload, "actual Whole export")));
    owner.record_initial_driver_snapshot("initial_source_before_cleanup");
    let cleanup = catch_unwind(AssertUnwindSafe(|| owner.cleanup())).unwrap_or_else(|payload| {
        Err(panic_error(
            payload,
            "actual Whole cleanup; owners quarantined",
        ))
    });
    owner.record_initial_driver_snapshot("initial_source_after_cleanup");
    let receipt = catch_unwind(AssertUnwindSafe(|| owner.final_receipt(&body, &cleanup)))
        .unwrap_or_else(|payload| {
            let reason = panic_error(payload, "Whole final evidence writer");
            let _ = new_json(
                &directory.join("whole-result-writer-failure.json"),
                &json!({
                    "schema":1,"success":false,"error":reason,"body_error":body.as_ref().err(),
                    "cleanup_error":cleanup.as_ref().err(),"partial_files_preserved":true,
                }),
            );
            Err(reason)
        });
    let success = body.is_ok() && cleanup.is_ok() && receipt.is_ok();
    if cleanup.is_ok() {
        unsafe {
            ManuallyDrop::drop(&mut owner);
        }
    } else {
        WHOLE_QUARANTINED.with(|quarantine| *quarantine.borrow_mut() = Some(owner));
    }
    assert!(
        success,
        "actual Whole NOIRE qualification failed; preserve {}",
        directory.display()
    );
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    // Pure diagnostic inputs: these do not claim an actual SDK receipt or App export.
    fn clone_identity(worker: usize, source: u64) -> observe::InstanceIdentity {
        observe::InstanceIdentity {
            ordinal: 2,
            phase: InstancePhase::StrictClone,
            worker_id: Some(worker),
            source_instance_id: Some(source),
        }
    }

    fn call(
        ordinal: u64,
        ns: u64,
        identity: observe::InstanceIdentity,
        boundary: Boundary,
        outcome: Option<observe::CallOutcome>,
    ) -> Arc<observe::Record> {
        Arc::new(observe::Record {
            ordinal,
            monotonic_elapsed_ns: ns,
            stage: Stage::WholeExport,
            instance: Some(identity),
            kind: RecordKind::Call {
                operation: Operation::StrictProcess,
                boundary,
                frames: Some(128),
                transport: None,
                outcome,
            },
        })
    }

    fn complete_pairs() -> Vec<Arc<observe::Record>> {
        let identity = clone_identity(31, 73);
        vec![
            call(20, 100, identity, Boundary::Attempt, None),
            call(
                21,
                140,
                identity,
                Boundary::Returned,
                Some(observe::CallOutcome::Ok),
            ),
            call(40, 200, identity, Boundary::Attempt, None),
            call(
                41,
                290,
                identity,
                Boundary::Returned,
                Some(observe::CallOutcome::Ok),
            ),
        ]
    }

    #[test]
    fn complete_pairs_preserve_exact_clone_actual_points_and_duration_counts() {
        let value = actual_strict_process_timing(&complete_pairs(), None);
        assert_eq!(
            value["strict_instance"],
            json!({
                "ordinal":2,"phase":"StrictClone","worker_id":31,"source_instance_id":73,
            })
        );
        assert_eq!(
            value["points"],
            json!({
                "first_attempt":{"record_ordinal":20,"monotonic_elapsed_ns":100},
                "last_attempt":{"record_ordinal":40,"monotonic_elapsed_ns":200},
                "first_returned":{"record_ordinal":21,"monotonic_elapsed_ns":140},
                "last_returned":{"record_ordinal":41,"monotonic_elapsed_ns":290},
            })
        );
        assert_eq!(
            value["calls"],
            json!({
                "attempts":2,"returned":2,"returned_ok":2,"returned_error":0,
                "complete_pairs":2,"complete_success_pairs":true,
                "sum_paired_call_duration_ns":130,"minimum_paired_call_duration_ns":40,
                "maximum_paired_call_duration_ns":90,
            })
        );
        assert_eq!(value["first_attempt_to_last_returned_ns"], json!(190));
        assert!(value["diagnostic_error"].is_null());
        assert!(value["snapshot_failure"].is_null());
    }

    #[test]
    fn missing_return_keeps_measured_prior_pair_but_never_reports_complete() {
        let mut records = complete_pairs();
        records.pop();
        let value = actual_strict_process_timing(&records, None);
        assert_eq!(value["calls"]["attempts"], json!(2));
        assert_eq!(value["calls"]["returned"], json!(1));
        assert_eq!(value["calls"]["complete_pairs"], json!(1));
        assert_eq!(value["calls"]["sum_paired_call_duration_ns"], json!(40));
        assert_eq!(value["calls"]["complete_success_pairs"], json!(false));
        assert_eq!(
            value["diagnostic_error"],
            json!("StrictProcess still has an unreturned actual attempt")
        );
    }

    #[test]
    fn returned_error_or_missing_outcome_keeps_time_and_rejects_success_pairs() {
        let identity = clone_identity(31, 73);
        for outcome in [
            None,
            Some(observe::CallOutcome::Error("native failure".into())),
        ] {
            let records = [
                call(20, 100, identity, Boundary::Attempt, None),
                call(21, 140, identity, Boundary::Returned, outcome),
            ];
            let value = actual_strict_process_timing(&records, None);
            assert_eq!(value["calls"]["returned"], json!(1));
            assert_eq!(value["calls"]["returned_ok"], json!(0));
            assert_eq!(value["calls"]["returned_error"], json!(1));
            assert_eq!(value["calls"]["complete_pairs"], json!(1));
            assert_eq!(value["calls"]["sum_paired_call_duration_ns"], json!(40));
            assert_eq!(value["calls"]["complete_success_pairs"], json!(false));
        }
    }

    #[test]
    fn attempt_cannot_carry_a_returned_outcome() {
        let identity = clone_identity(31, 73);
        let records = [
            call(
                20,
                100,
                identity,
                Boundary::Attempt,
                Some(observe::CallOutcome::Ok),
            ),
            call(
                21,
                140,
                identity,
                Boundary::Returned,
                Some(observe::CallOutcome::Ok),
            ),
        ];
        let value = actual_strict_process_timing(&records, None);
        assert_eq!(value["calls"]["complete_pairs"], json!(1));
        assert_eq!(value["calls"]["complete_success_pairs"], json!(false));
        assert_eq!(
            value["diagnostic_error"],
            json!("StrictProcess attempt has a returned outcome")
        );
    }

    #[test]
    fn same_ordinal_foreign_worker_or_source_cannot_merge_into_exact_clone() {
        for foreign in [clone_identity(32, 73), clone_identity(31, 74)] {
            let mut records = complete_pairs();
            records.push(call(50, 300, foreign, Boundary::Attempt, None));
            records.push(call(
                51,
                340,
                foreign,
                Boundary::Returned,
                Some(observe::CallOutcome::Ok),
            ));
            let value = actual_strict_process_timing(&records, None);
            assert_eq!(value["strict_instance"]["worker_id"], json!(31));
            assert_eq!(value["strict_instance"]["source_instance_id"], json!(73));
            assert_eq!(value["calls"]["attempts"], json!(2));
            assert_eq!(value["calls"]["complete_pairs"], json!(2));
            assert_eq!(value["calls"]["complete_success_pairs"], json!(false));
            assert_eq!(
                value["diagnostic_error"],
                json!("StrictProcess belongs to more than one exact clone")
            );
        }
    }

    #[test]
    fn pair_internal_cross_pair_clock_and_duplicate_ordinals_fail_sticky() {
        let identity = clone_identity(31, 73);
        for points in [
            vec![(20, 100), (21, 90)],
            vec![(20, 100), (21, 200), (22, 150), (23, 190)],
            vec![(20, 100), (21, 140), (20, 200), (21, 290)],
        ] {
            let mut records = Vec::new();
            for (index, (ordinal, ns)) in points.into_iter().enumerate() {
                let boundary = if index % 2 == 0 {
                    Boundary::Attempt
                } else {
                    Boundary::Returned
                };
                let outcome = (boundary == Boundary::Returned).then_some(observe::CallOutcome::Ok);
                records.push(call(ordinal, ns, identity, boundary, outcome));
            }
            // Later valid data cannot clear the earliest clock/order diagnostic.
            records.push(call(60, 500, identity, Boundary::Attempt, None));
            records.push(call(
                61,
                550,
                identity,
                Boundary::Returned,
                Some(observe::CallOutcome::Ok),
            ));
            let value = actual_strict_process_timing(&records, None);
            assert_eq!(value["calls"]["complete_success_pairs"], json!(false));
            assert_eq!(
                value["diagnostic_error"],
                json!("StrictProcess record clock/order regressed")
            );
        }
    }

    #[test]
    fn equal_monotonic_timestamps_are_legal_zero_duration_measurements() {
        let identity = clone_identity(31, 73);
        let records = [
            call(20, 100, identity, Boundary::Attempt, None),
            call(
                21,
                100,
                identity,
                Boundary::Returned,
                Some(observe::CallOutcome::Ok),
            ),
        ];
        let value = actual_strict_process_timing(&records, None);
        assert_eq!(value["calls"]["complete_success_pairs"], json!(true));
        assert_eq!(value["calls"]["sum_paired_call_duration_ns"], json!(0));
        assert_eq!(value["calls"]["minimum_paired_call_duration_ns"], json!(0));
        assert_eq!(value["first_attempt_to_last_returned_ns"], json!(0));
    }

    #[test]
    fn sticky_snapshot_failure_survives_complete_pairs_and_later_diagnostic() {
        let failure = "observer record cap exceeded";
        let mut records = complete_pairs();
        let value = actual_strict_process_timing(&records, Some(failure));
        assert_eq!(value["snapshot_failure"], json!(failure));
        assert_eq!(value["calls"]["complete_pairs"], json!(2));
        assert_eq!(value["calls"]["complete_success_pairs"], json!(false));
        assert_eq!(value["first_attempt_to_last_returned_ns"], json!(190));
        let identity = clone_identity(31, 73);
        records.push(call(
            50,
            300,
            identity,
            Boundary::Attempt,
            Some(observe::CallOutcome::Ok),
        ));
        records.push(call(
            51,
            340,
            identity,
            Boundary::Returned,
            Some(observe::CallOutcome::Ok),
        ));
        let later = actual_strict_process_timing(&records, Some(failure));
        assert_eq!(later["snapshot_failure"], json!(failure));
        assert_eq!(later["calls"]["complete_pairs"], json!(3));
        assert_eq!(later["calls"]["complete_success_pairs"], json!(false));
        assert_eq!(
            later["diagnostic_error"],
            json!("StrictProcess attempt has a returned outcome")
        );
    }
}

#[cfg(test)]
mod initial_diagnostic_tests {
    use super::*;

    fn supplied(ordinal: u64, kind: RecordKind, source: bool) -> Arc<observe::Record> {
        Arc::new(observe::Record {
            ordinal,
            monotonic_elapsed_ns: ordinal * 100,
            stage: Stage::InitialSource,
            instance: source.then_some(observe::InstanceIdentity {
                ordinal: 1,
                phase: InstancePhase::InitialSource,
                worker_id: Some(31),
                source_instance_id: Some(73),
            }),
            kind,
        })
    }

    #[test]
    fn diagnostic_keeps_initial_call_fault_rejection_and_actual_header_fields() {
        use crate::plugins::plugin_runtime::{RuntimeEvent, StateCaptureRejectReason};
        let rows = [
            supplied(
                7,
                RecordKind::Call {
                    operation: Operation::SourceLoad,
                    boundary: Boundary::Returned,
                    frames: None,
                    transport: None,
                    outcome: Some(observe::CallOutcome::Error(
                        "exact initial load error".into(),
                    )),
                },
                true,
            ),
            supplied(
                8,
                RecordKind::RuntimeEvent {
                    endpoint_id: 5,
                    project_session: 2,
                    instance_id: 73,
                    event: RuntimeEvent::SlotFault {
                        slot: 0,
                        message: "exact slot fault".into(),
                    },
                },
                false,
            ),
            supplied(
                9,
                RecordKind::RuntimeEvent {
                    endpoint_id: 5,
                    project_session: 2,
                    instance_id: 73,
                    event: RuntimeEvent::StateRejected {
                        slot: 0,
                        request_id: 12,
                        epoch: 4,
                        reason: StateCaptureRejectReason::BackendFailure,
                    },
                },
                false,
            ),
        ];
        let trace = initial_driver_trace(&rows, 3);
        let records = trace["records"].as_array().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["ordinal"], json!(7));
        assert_eq!(records[0]["monotonic_elapsed_ns"], json!(700));
        assert_eq!(records[0]["instance"]["worker_id"], json!(31));
        assert_eq!(
            records[0]["detail"]["outcome"]["reason"]["text"],
            json!("exact initial load error")
        );
        assert_eq!(
            records[1]["detail"]["event"]["message"]["text"],
            json!("exact slot fault")
        );
        assert_eq!(
            records[2]["detail"]["event"]["kind"],
            json!("StateRejected")
        );
        assert_eq!(
            records[2]["detail"]["event"]["reason"],
            json!("BackendFailure")
        );
        assert_eq!(trace["truncated"], json!(false));
    }

    #[test]
    fn diagnostic_omits_private_state_bytes_in_both_observer_payload_forms() {
        let secret = "raw-state-sentinel-not-a-diagnostic-string";
        let rows = [
            supplied(
                1,
                RecordKind::StateLoad {
                    bytes: secret.as_bytes().to_vec(),
                    outcome: observe::CallOutcome::Ok,
                },
                true,
            ),
            supplied(
                2,
                RecordKind::RuntimeEvent {
                    endpoint_id: 5,
                    project_session: 2,
                    instance_id: 73,
                    event: crate::plugins::plugin_runtime::RuntimeEvent::State {
                        slot: 0,
                        request_id: 1,
                        epoch: Some(3),
                        bytes: secret.as_bytes().to_vec(),
                    },
                },
                false,
            ),
        ];
        let trace = initial_driver_trace(&rows, 2);
        let encoded = serde_json::to_string(&trace).unwrap();
        assert!(!encoded.contains(secret));
        assert_eq!(
            trace["records"][0]["detail"]["byte_len"],
            json!(secret.len())
        );
        assert_eq!(
            trace["records"][1]["detail"]["event"]["byte_len"],
            json!(secret.len())
        );
        assert_eq!(trace["raw_state_or_pcm_serialized"], json!(false));
    }

    #[test]
    fn diagnostic_row_limit_is_honest_and_excludes_strict_clone_incarnation() {
        let mut rows = (1..=65)
            .map(|n| supplied(n, RecordKind::Lifecycle("source marker"), true))
            .collect::<Vec<_>>();
        rows.push(Arc::new(observe::Record {
            ordinal: 66,
            monotonic_elapsed_ns: 6600,
            stage: Stage::WholeExport,
            instance: Some(observe::InstanceIdentity {
                ordinal: 2,
                phase: InstancePhase::StrictClone,
                worker_id: None,
                source_instance_id: None,
            }),
            kind: RecordKind::Lifecycle("clone excluded"),
        }));
        let trace = initial_driver_trace(&rows, 65);
        assert_eq!(trace["matched_records"], json!(65));
        assert_eq!(trace["records"].as_array().unwrap().len(), 64);
        assert_eq!(trace["records"][63]["ordinal"], json!(64));
        assert_eq!(trace["truncated"], json!(true));
        assert!(
            !serde_json::to_string(&trace)
                .unwrap()
                .contains("clone excluded")
        );
        let long = "a".repeat(4095) + "é";
        let text = initial_driver_text(&long);
        assert_eq!(text["utf8_bytes"], json!(4097));
        assert_eq!(text["truncated"], json!(true));
        assert_eq!(text["text"].as_str().unwrap().len(), 4095);
    }
}
