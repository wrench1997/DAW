//! Explicit init-only qualification of the complete pinned Saved13 source.
//! The real scan happens before observation and is a separate empty default
//! Kontakt instance. Its checked helper reap is independent of the source
//! worker join; that join still does not prove the source helper's strict reap.
//! No MIDI playback, device, editor, save, capture, clone or WAV is exercised.
#![cfg(all(test, windows, feature = "experimental-fresh-helper"))]

use super::windows_noire_full_inputs::FullInputs;
use super::windows_noire_full_owner::{FullParent, RetainedFullParent};
use super::*;
use crate::plugins::scan_qa::{CheckedScan, SingleModuleScan};

const FULL_SOURCE_SELECTOR: &str = "app::clip_render_job::windows_noire_bounce_qa::windows_noire_full_source_qa::windows_noire_full_saved13_source_initialization_only";
const FULL_SOURCE_OPT_IN: &str = "actual-noire-pure-complete-saved13-source-init-v1";
const RUN_SECONDS: u64 = 280;
const INITIALIZATION_SECONDS: u64 = 40;
const CLEANUP_SECONDS: u64 = 35;
const DIAGNOSTIC_RECORDS: usize = 64;
const DIAGNOSTIC_OWNERS: usize = 4;
const RAW_ERROR_BYTES: usize = 4096;
static FULL_SOURCE_ADMITTED: AtomicBool = AtomicBool::new(false);

struct BinaryPins {
    executable: PathBuf,
    executable_sha256: String,
    helper: PathBuf,
    helper_sha256: String,
}
impl BinaryPins {
    fn load() -> Check<Self> {
        let executable = std::env::current_exe()
            .map_err(error)?
            .canonicalize()
            .map_err(error)?;
        let helper = absolute_env("CITRUS_NOIRE_FULL_SOURCE_QA_HELPER")?
            .canonicalize()
            .map_err(error)?;
        let expected = executable
            .parent()
            .ok_or("private test executable has no parent")?
            .join("vst3-host-helper.exe")
            .canonicalize()
            .map_err(error)?;
        require(
            helper == expected,
            "full source helper must be the production helper beside this private test executable",
        )?;
        let executable_sha256 = oracles::sha256_hex(&read_bounded(&executable, 256 * 1024 * 1024)?);
        let helper_sha256 = oracles::sha256_hex(&read_bounded(&helper, 64 * 1024 * 1024)?);
        for (actual, variable) in [
            (
                &executable_sha256,
                "CITRUS_NOIRE_FULL_SOURCE_QA_EXECUTABLE_SHA256",
            ),
            (&helper_sha256, "CITRUS_NOIRE_FULL_SOURCE_QA_HELPER_SHA256"),
        ] {
            let supplied = std::env::var(variable).map_err(error)?;
            require(
                supplied.len() == 64
                    && supplied.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && supplied == *actual,
                "private binary differs from its explicit lowercase SHA256 admission pin",
            )?;
        }
        Ok(Self {
            executable,
            executable_sha256,
            helper,
            helper_sha256,
        })
    }
    fn unchanged(&self) -> Check {
        require(
            oracles::sha256_hex(&read_bounded(&self.executable, 256 * 1024 * 1024)?)
                == self.executable_sha256,
            "private test executable changed during source qualification",
        )?;
        require(
            oracles::sha256_hex(&read_bounded(&self.helper, 64 * 1024 * 1024)?)
                == self.helper_sha256,
            "private production helper changed during source qualification",
        )
    }
    fn evidence(&self) -> Value {
        json!({"test_executable":{"path":self.executable,"sha256":self.executable_sha256},
            "production_helper":{"path":self.helper,"sha256":self.helper_sha256},
            "hash_work_scope":"driver-only preflight/final; never callback or observer collector lock"})
    }
}

fn raw_text(value: &str) -> Value {
    let mut end = value.len().min(RAW_ERROR_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    json!({"text":&value[..end],"utf8_bytes":value.len(),"truncated":end != value.len()})
}
fn raw_panic(payload: Box<dyn std::any::Any + Send>, stage: &str) -> String {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-text panic payload");
    format!("{stage} unwound: {message}; complete run owner remains retained")
}
fn raw_outcome(value: &observe::CallOutcome) -> Value {
    match value {
        observe::CallOutcome::Ok => json!({"kind":"Ok"}),
        observe::CallOutcome::Error(message) => {
            json!({"kind":"Error","actual_error":raw_text(message)})
        }
        observe::CallOutcome::StrictError(failure) => {
            json!({"kind":"StrictError","backend":backend_failure_json(failure)})
        }
    }
}
fn metadata_evidence(metadata: &crate::instrument_bounce::BouncePluginMetadata) -> Value {
    json!({"class_uid":metadata.class_uid,"version":metadata.version,"name":raw_text(&metadata.name),
        "category":raw_text(&metadata.category),"latency_samples":metadata.latency_samples,
        "tail_samples":metadata.tail_samples,
        "audio_inputs":metadata.input_buses.iter().map(|bus|json!({"channel_count":bus.channel_count,"is_main":bus.is_main,"active":bus.active})).collect::<Vec<_>>(),
        "audio_outputs":metadata.output_buses.iter().map(|bus|json!({"channel_count":bus.channel_count,"is_main":bus.is_main,"active":bus.active})).collect::<Vec<_>>(),
        "event_inputs":metadata.event_input_buses,"event_outputs":metadata.event_output_buses})
}
pub(super) fn actual_source_identity(
    records: &[Arc<observe::Record>],
    worker: usize,
    source_id: u64,
) -> Check<observe::InstanceIdentity> {
    let mut births = records.iter().filter(|record| {
        matches!(
            record.kind,
            RecordKind::Lifecycle("instance construction observed")
        )
    });
    let first = births
        .next()
        .ok_or("actual full source construction observation is absent")?
        .instance
        .ok_or("source construction is missing its actual incarnation header")?;
    require(
        births.next().is_none()
            && first.phase == InstancePhase::InitialSource
            && first.worker_id == Some(worker)
            && worker != 0
            && first.source_instance_id == Some(source_id)
            && source_id != 0,
        "actual source incarnation is ambiguous or differs from its ready guard",
    )?;
    Ok(first)
}
pub(super) fn validate_state_operands(
    records: &[Arc<observe::Record>],
    identity: observe::InstanceIdentity,
    expected_bytes: usize,
    expected_sha256: &str,
) -> Check {
    require(
        !records.is_empty() && records.len() <= DIAGNOSTIC_RECORDS,
        "actual complete source load has no bounded state operand evidence",
    )?;
    for record in records {
        let RecordKind::StateLoad {
            bytes,
            outcome: observe::CallOutcome::Ok,
        } = &record.kind
        else {
            return Err("source state operand is not an actual acknowledged StateLoad".into());
        };
        require(
            record.instance == Some(identity)
                && bytes.len() == expected_bytes
                && oracles::sha256_hex(bytes) == expected_sha256,
            "actual source StateLoad differs from the complete original Saved13 bytes or incarnation",
        )?;
    }
    Ok(())
}
pub(super) fn validate_metadata(
    records: &[Arc<observe::Record>],
    identity: observe::InstanceIdentity,
    sample_rate: u32,
) -> Check {
    require(
        !records.is_empty() && records.len() <= DIAGNOSTIC_RECORDS,
        "existing actual source metadata observations are absent or exceed the diagnostic bound",
    )?;
    for record in records {
        let RecordKind::Metadata {
            result: Ok(metadata),
            ..
        } = &record.kind
        else {
            return Err(
                "actual source metadata query failed or was not an observed metadata record".into(),
            );
        };
        require(
            record.instance == Some(identity)
                && metadata.class_uid == KONTAKT_UID
                && metadata.version == "8.4.1",
            "actual source metadata is not the exact reviewed Kontakt 8.4.1 incarnation",
        )?;
    }
    let Some(RecordKind::Metadata {
        result: Ok(final_metadata),
        ..
    }) = records.last().map(|record| &record.kind)
    else {
        return Err("the final actual source metadata snapshot is absent".into());
    };
    // Early actual snapshots can precede acknowledged output selection. Their
    // declarations are retained, but only the final Ready snapshot must satisfy
    // the complete active-main-stereo/aux-inactive processing contract.
    final_metadata.validate(sample_rate)?;
    Ok(())
}
fn actual_failure(record: &observe::Record) -> bool {
    match &record.kind {
        RecordKind::Call {
            outcome: Some(observe::CallOutcome::Error(_) | observe::CallOutcome::StrictError(_)),
            ..
        }
        | RecordKind::StateLoad {
            outcome: observe::CallOutcome::Error(_) | observe::CallOutcome::StrictError(_),
            ..
        }
        | RecordKind::Metadata { result: Err(_), .. }
        | RecordKind::ResetReport {
            process_error: Some(_),
            ..
        }
        | RecordKind::Disposal { error: Some(_), .. }
        | RecordKind::WorkerJoin {
            outcome: WorkerJoinOutcome::WorkerPanicked,
            ..
        }
        | RecordKind::HookInventory {
            inactive: false, ..
        } => true,
        RecordKind::RuntimeEvent { event, .. } => matches!(
            event,
            crate::plugins::plugin_runtime::RuntimeEvent::SlotFault { .. }
                | crate::plugins::plugin_runtime::RuntimeEvent::ParameterCommandFailed { .. }
                | crate::plugins::plugin_runtime::RuntimeEvent::StateRejected { .. }
        ),
        _ => false,
    }
}
fn record_evidence(record: &observe::Record) -> Value {
    let detail = match &record.kind {
        RecordKind::Call {
            operation,
            boundary,
            frames,
            transport,
            outcome,
        } => json!({
            "kind":"Call","operation":format!("{operation:?}"),"boundary":format!("{boundary:?}"),
            "frames":frames,"transport":transport.map(|t|json!({"sample_position":t.sample_position,
                "quarter_note_bits":t.quarter_note_position.to_bits(),"tempo_bits":t.tempo.to_bits(),
                "playing":t.playing,"time_sig_numerator":t.time_sig_numerator,"time_sig_denominator":t.time_sig_denominator})),
            "outcome":outcome.as_ref().map(raw_outcome)}),
        RecordKind::StateLoad { bytes, outcome } => {
            json!({"kind":"StateLoad","byte_len":bytes.len(),
            "sha256":oracles::sha256_hex(bytes),"outcome":raw_outcome(outcome),"private_bytes_omitted":true})
        }
        RecordKind::Metadata { site, result } => match result {
            Ok(metadata) => {
                json!({"kind":"Metadata","site":site,"metadata":metadata_evidence(metadata)})
            }
            Err(message) => json!({"kind":"Metadata","site":site,"actual_error":raw_text(message)}),
        },
        RecordKind::RuntimeEvent {
            endpoint_id,
            project_session,
            instance_id,
            event,
        } => {
            use crate::plugins::plugin_runtime::RuntimeEvent;
            let value = match event {
                RuntimeEvent::SlotReady {
                    slot,
                    name,
                    latency_samples,
                    tail_samples,
                    ..
                } => {
                    json!({"kind":"SlotReady","slot":slot,"name":raw_text(name),"latency_samples":latency_samples,"tail_samples":tail_samples})
                }
                RuntimeEvent::SlotFault { slot, message } => {
                    json!({"kind":"SlotFault","slot":slot,"actual_error":raw_text(message)})
                }
                RuntimeEvent::ParameterCommandFailed {
                    slot,
                    request_id,
                    command,
                    id,
                    message,
                } => {
                    json!({"kind":"ParameterCommandFailed","slot":slot,"request_id":request_id,"command":format!("{command:?}"),"id":id,"actual_error":raw_text(message)})
                }
                RuntimeEvent::StateRejected {
                    slot,
                    request_id,
                    epoch,
                    reason,
                } => {
                    json!({"kind":"StateRejected","slot":slot,"request_id":request_id,"epoch":epoch,"reason":format!("{reason:?}")})
                }
                RuntimeEvent::ShutdownComplete => json!({"kind":"ShutdownComplete"}),
                _ => json!({"kind":"other actual runtime event","detail_omitted":true}),
            };
            json!({"kind":"RuntimeEvent","endpoint_id":endpoint_id,"project_session":project_session,"instance_id":instance_id,"event":value})
        }
        RecordKind::Worker {
            site,
            worker_id,
            manifest,
            epoch,
            sequence,
            stats,
        } => {
            json!({"kind":"Worker","site":site,"worker_id":worker_id,"manifest":format!("{manifest:?}"),"epoch":epoch,"sequence":sequence,"stats":format!("{stats:?}")})
        }
        RecordKind::WorkerJoin { worker_id, outcome } => {
            json!({"kind":"WorkerJoin","worker_id":worker_id,"outcome":format!("{outcome:?}")})
        }
        RecordKind::HookInventory { site, inactive } => {
            json!({"kind":"HookInventory","site":site,"inactive":inactive})
        }
        RecordKind::Lifecycle(site) => json!({"kind":"Lifecycle","site":site}),
        RecordKind::WorkerCaptureGuard {
            worker_id,
            operands,
        } => {
            json!({"kind":"WorkerCaptureGuard","worker_id":worker_id,"operands":format!("{operands:?}")})
        }
        RecordKind::ResetReport {
            frames,
            discarded_prior_midi_events,
            discarded_midi_events,
            discarded_prior_parameter_points,
            discarded_parameter_points,
            output_events_lost,
            parameter_output_fault,
            process_error,
        } => {
            json!({"kind":"ResetReport","frames":frames,"discarded_prior_midi_events":discarded_prior_midi_events,"discarded_midi_events":discarded_midi_events,"discarded_prior_parameter_points":discarded_prior_parameter_points,"discarded_parameter_points":discarded_parameter_points,"output_events_lost":output_events_lost,"parameter_output_fault":parameter_output_fault,"actual_error":process_error.as_deref().map(raw_text)})
        }
        _ => json!({"kind":"other actual record","detail_omitted":true}),
    };
    json!({"ordinal":record.ordinal,"monotonic_elapsed_ns":record.monotonic_elapsed_ns,
        "stage":format!("{:?}",record.stage),"instance":record.instance.map(|i|json!({
            "ordinal":i.ordinal,"phase":format!("{:?}",i.phase),"worker_id":i.worker_id,"source_instance_id":i.source_instance_id})),
        "detail":detail})
}

struct SourceRun {
    parent: RetainedFullParent,
    observer: ObservationRun,
    binaries: BinaryPins,
    scan: SingleModuleScan,
    evidence: Value,
    ready_worker: Option<usize>,
    ready_instance: Option<observe::InstanceIdentity>,
    deadline: Instant,
}
impl SourceRun {
    fn new(inputs: FullInputs, binaries: BinaryPins, scan: SingleModuleScan) -> Check<Self> {
        let pins = json!({"path":inputs.pins.project_path,"bytes":inputs.pins.project_bytes,
            "sha256":inputs.pins.project_sha256,"state_bytes":inputs.pins.saved_state_bytes,
            "state_sha256":inputs.pins.saved_state_sha256,"actual_bundle_path":inputs.pins.actual_bundle_path,
            "note_counts":inputs.project.patterns.iter().map(|p|p.notes.len()).collect::<Vec<_>>(),
            "placements":inputs.project.clips.len(),"song_length_beats":inputs.project.song_length_beats,
            "mixer_tracks":inputs.project.mixer_tracks.len(),"mixer_routes":inputs.project.mixer_routes.len()});
        let observer = ObservationRun::install().map_err(error)?;
        let parent = FullParent::new_retained(inputs, [0x46; 32])?;
        let evidence = json!({"schema":1,"fixture":FULL_SOURCE_OPT_IN,
            "scope":"complete original Saved13 initialization only; device-free App source and callback, no writing the original project",
            "original_project":pins,"binaries":binaries.evidence(),"separate_scan_preflight":scan.evidence(),
            "budgets":{"outer_driver_seconds":RUN_SECONDS,"initialization_seconds":INITIALIZATION_SECONDS,
                "cleanup_seconds":CLEANUP_SECONDS,"existing_render_budget_seconds":300,"render_started":false,
                "cooperative_deadlines_are_not_hard_OS_time_guarantees":true},
            "unverified":{"midi_audibility":true,"physical_monitoring":true,"plugin_editor":true,
                "NOIRE_preset_name_and_sound":true,"all_NOIRE_internal_parameters":true,"new_save_cold_reopen":true,"clip_conversion":true,
                "whole_song_export":true,"Felt":true,"all_native_process_owners_strictly_joined":true}});
        Ok(Self {
            parent,
            observer,
            binaries,
            scan,
            evidence,
            ready_worker: None,
            ready_instance: None,
            deadline: Instant::now() + Duration::from_secs(RUN_SECONDS),
        })
    }
    fn exercise(&mut self, descriptor: &PluginDescriptor) -> Check {
        let initialization_deadline = self
            .deadline
            .min(Instant::now() + Duration::from_secs(INITIALIZATION_SECONDS));
        self.parent.begin_source_initialization(
            descriptor,
            &mut self.observer,
            initialization_deadline,
        )?;
        let ready = self
            .parent
            .wait_source_initialization(&self.observer, initialization_deadline)?;
        self.parent.model_unchanged()?;
        self.parent.inputs_unchanged()?;
        require(
            self.parent.paused(),
            "actual full source callback is not paused and quiescent",
        )?;
        let source_instance = self.observer.with_snapshot(|snapshot| {
            actual_source_identity(
                &snapshot.records,
                ready.actual_worker_id,
                self.parent.source.plugin_instance_id,
            )
        })?;
        self.ready_worker = Some(ready.actual_worker_id);
        self.ready_instance = Some(source_instance);
        let state_records = self.observer.with_snapshot(|snapshot| {
            snapshot
                .records
                .iter()
                .filter(|record| {
                    record.instance == Some(source_instance)
                        && matches!(record.kind, RecordKind::StateLoad { .. })
                })
                .take(DIAGNOSTIC_RECORDS + 1)
                .cloned()
                .collect::<Vec<_>>()
        });
        validate_state_operands(
            &state_records,
            source_instance,
            self.parent.pins.saved_state_bytes,
            &self.parent.pins.saved_state_sha256,
        )?;
        let metadata_records = self.observer.with_snapshot(|snapshot| {
            snapshot
                .records
                .iter()
                .filter(|record| {
                    record.instance == Some(source_instance)
                        && matches!(record.kind, RecordKind::Metadata { .. })
                })
                .take(DIAGNOSTIC_RECORDS + 1)
                .cloned()
                .collect::<Vec<_>>()
        });
        validate_metadata(
            &metadata_records,
            source_instance,
            ready.binding.sample_rate,
        )?;
        self.observer.with_snapshot(|snapshot| {
            let mut prepared = snapshot
                .records
                .iter()
                .filter(|record| {
                    record.instance == Some(source_instance)
                        && matches!(
                            record.kind,
                            RecordKind::Call {
                                operation: Operation::SourcePrepare,
                                boundary: Boundary::Returned,
                                outcome: Some(observe::CallOutcome::Ok),
                                ..
                            }
                        )
                })
                .map(|record| record.ordinal);
            let first = prepared.next();
            require(
                first.is_some() && prepared.next().is_none(),
                "actual source does not have exactly one successful wrapper SourcePrepare return",
            )
        })?;
        let actual_metadata = metadata_records
            .iter()
            .filter_map(|record| match &record.kind {
                RecordKind::Metadata {
                    result: Ok(metadata),
                    ..
                } => Some(metadata),
                _ => None,
            })
            .collect::<Vec<_>>();
        let stamp = ready.binding.stamp;
        let callback = self.parent.callback()?.genuine_snapshot();
        self.evidence["source_ready"] = json!({"actual_instance":{"ordinal":source_instance.ordinal,
            "phase":"InitialSource","worker_id":source_instance.worker_id,"source_instance_id":source_instance.source_instance_id},
            "actual_guard_worker_id":ready.actual_worker_id,"stamp":{
                "project_session":stamp.project_session,"project_revision":stamp.project_revision,
                "timeline_revision":stamp.timeline_revision,"epoch":stamp.epoch,"engine_session":stamp.engine_session,
                "endpoint_id":stamp.endpoint_id,"channel_id":stamp.channel_id,"instance_id":stamp.instance_id,
                "mixer_track_id":stamp.mixer_track_id,"native_generation":stamp.native_generation,
                "app_native_dirty_revision":stamp.app_native_dirty_revision,"native_capture_serial":stamp.native_capture_serial,
                "parameter_capture_serial":stamp.parameter_capture_serial,"native_control_request":stamp.native_control_request,
                "parameter_control_request":stamp.parameter_control_request},
            "control_revision":ready.binding.control_revision,"asset_generation":ready.binding.asset_generation,
            "sample_rate":ready.binding.sample_rate,"coherent_endpoint":format!("{:?}",ready.binding.endpoint),
            "callback":{"playing":callback.playing,"generation":callback.generation,"epoch":callback.epoch,
                "active_epoch":callback.active_epoch,"request_id":callback.request_id},
            "actual_source_metadata":actual_metadata.iter().map(|m|metadata_evidence(m)).collect::<Vec<_>>(),
            "original_opaque_operand_acknowledged":true,"full_project_history_dirty_saved_path_unchanged":true,
            "midi_or_NOIRE_audibility_not_tested":true});
        Ok(())
    }
    fn diagnostic(&self, moment: &str) -> Value {
        let app = self.parent.app();
        let row = |channel: u32,
                   session: u64,
                   endpoint: u64,
                   instance: u64,
                   control: &crate::plugins::plugin_runtime::PluginChainControl,
                   guard: &PluginWorkerGuard| {
            json!({"channel":channel,"project_session":session,"endpoint_id":endpoint,"instance_id":instance,
                "guard_worker_id":guard.genuine_worker_id(),"stats":format!("{:?}",control.stats()),
                "has_outstanding_admin_work":control.has_outstanding_admin_work(),
                "coherent_endpoint":format!("{:?}",control.plugin_endpoint_snapshot())})
        };
        let pending = app
            .pending_generator_chains
            .iter()
            .take(DIAGNOSTIC_OWNERS)
            .map(|(c, s)| {
                row(
                    *c,
                    s.project_session,
                    s.endpoint_id,
                    s.instance_id,
                    &s.control,
                    &s.guard,
                )
            })
            .collect::<Vec<_>>();
        let running = app
            .running_generator_chains
            .iter()
            .take(DIAGNOSTIC_OWNERS)
            .map(|(c, s)| {
                row(
                    *c,
                    s.project_session,
                    s.endpoint_id,
                    s.instance_id,
                    &s.control,
                    &s.guard,
                )
            })
            .collect::<Vec<_>>();
        let (records, failures, count, failure) = self.observer.with_snapshot(|snapshot| {
            (
                snapshot
                    .records
                    .iter()
                    .take(DIAGNOSTIC_RECORDS)
                    .cloned()
                    .collect::<Vec<_>>(),
                snapshot
                    .records
                    .iter()
                    .rev()
                    .filter(|record| actual_failure(record))
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>(),
                snapshot.records.len(),
                snapshot.failure,
            )
        });
        json!({"moment":moment,"read_only":true,"project_session":app.project_session,
            "pending_count":app.pending_generator_chains.len(),"pending":pending,
            "running_count":app.running_generator_chains.len(),"running":running,
            "retired_fresh_count":app.retired_fresh_source_candidates.len(),
            "retired_fresh_worker_ids":app.retired_fresh_source_candidates.iter().take(DIAGNOSTIC_OWNERS)
                .map(PluginWorkerGuard::genuine_worker_id).collect::<Vec<_>>(),
            "pending_removals":app.pending_generator_removals.len(),"deferred_rebuilds":app.deferred_generator_rebuilds.len(),
            "toast":app.toast.as_ref().map(|(message,_)|raw_text(message)),
            "model_check_error":self.parent.model_unchanged().err(),"input_check_error":self.parent.inputs_unchanged().err(),
            "actual_record_count":count,"snapshot_failure":failure,"first_actual_records":records.iter().map(|r|record_evidence(r)).collect::<Vec<_>>(),
            "latest_actual_failure_records":failures.iter().rev().map(|r|record_evidence(r)).collect::<Vec<_>>(),
            "record_rows_truncated":count > records.len(),"no_additional_SDK_query_or_callback_pump":true})
    }
    fn cleanup(&mut self) -> Check {
        self.parent.cleanup(
            &mut self.observer,
            Instant::now() + Duration::from_secs(CLEANUP_SECONDS),
        )
    }
    fn final_receipt(&mut self, directory: &Path, body: &Check, cleanup: &Check) -> Check {
        let (terminal, observer_failure, all_record_count, forbidden_records, metadata) =
            self.observer.with_snapshot(|snapshot| {
                let terminal = TerminalFacts::from_records(&snapshot.records);
                let forbidden = snapshot
                    .records
                    .iter()
                    .filter(|r| {
                        matches!(
                            r.kind,
                            RecordKind::Capture { .. }
                                | RecordKind::StrictPcm { .. }
                                | RecordKind::StrictCaptureIdentity(_)
                                | RecordKind::RenderReport(_)
                        )
                    })
                    .count();
                (
                    terminal,
                    snapshot.failure,
                    snapshot.records.len(),
                    forbidden,
                    snapshot.source_positive_attempts,
                )
            });
        let terminal_result = (|| {
            require(
                self.scan.helper_reaped() && self.scan.metadata_admitted(),
                "actual separate scanner helper reap or single-module metadata admission is not proven",
            )?;
            require(
                self.parent.resources_joined(),
                "complete source cleanup has not finished its actual retained joins",
            )?;
            terminal.qualify(false)?;
            require(
                terminal.clone_instances == 0 && forbidden_records == 0,
                "init-only run unexpectedly created a capture, clone, renderer or output",
            )?;
            if body.is_ok() {
                let worker = self
                    .ready_worker
                    .ok_or("successful source readiness has no actual guard worker")?;
                let instance = self
                    .ready_instance
                    .ok_or("successful source readiness has no actual incarnation")?;
                require(
                    terminal.workers == BTreeSet::from([worker])
                        && terminal.source_instances.as_slice()
                            == [(
                                InstancePhase::InitialSource,
                                Some(worker),
                                instance.source_instance_id,
                            )],
                    "source readiness and terminal actual worker/incarnation identities disagree",
                )?;
            }
            require(
                observer_failure.is_none(),
                observer_failure.unwrap_or("observer incomplete"),
            )
        })();
        let pins = self
            .binaries
            .unchanged()
            .and_then(|()| self.parent.inputs_unchanged());
        let model = self.parent.model_unchanged();
        self.evidence["terminal"] = terminal.evidence();
        self.evidence["result"] = json!({"body_passed":body.is_ok(),"actual_body_error":body.as_ref().err(),
            "cleanup_passed":cleanup.is_ok(),"actual_cleanup_error":cleanup.as_ref().err(),
            "terminal_evidence_passed":terminal_result.is_ok(),"terminal_error":terminal_result.as_ref().err(),
            "original_project_and_binaries_unchanged":pins.is_ok(),"pin_error":pins.as_ref().err(),
            "full_model_history_unchanged":model.is_ok(),"model_error":model.as_ref().err(),
            "actual_record_count":all_record_count,"source_positive_process_attempts":metadata,
            "observer_error":observer_failure,"owner_wrapper_quarantined":body.is_err() || cleanup.is_err() || terminal_result.is_err() || pins.is_err() || model.is_err(),
            "source_resources_joined":self.parent.resources_joined(),
            "scope":"initialization and exact paused source/callback/worker only",
            "scan_instance_is_not_source_instance":true,
            "scan_helper_strict_reap_proven":self.scan.helper_reaped(),
            "scan_metadata_admitted":self.scan.metadata_admitted(),
            "scan_reap_evidence_source":"same retained checked single-module ScanReport",
            "source_helper_strict_reap_proven":false,
            "source_worker_join_is_not_independent_source_helper_reap_evidence":true,
            "wrapper_lifecycle_and_worker_join_do_not_count_internal_LoadPlugin_retries":true,
            "all_native_owners_strictly_joined_not_proven":true,"OS_process_absence_is_not_join_evidence":true,
            "no_MIDI_playback_or_monitoring_or_audio_export_claim":true,"partial_files_preserved":true,
            "success":body.is_ok() && cleanup.is_ok() && terminal_result.is_ok() && pins.is_ok() && model.is_ok()});
        new_json(&directory.join("source-result.json"), &self.evidence)?;
        terminal_result?;
        pins?;
        model
    }
}

thread_local! {
    // An unsuccessful complete run keeps both the source owner and its actual
    // observations; observer Drop must not erase a still-live worker's sink.
    static FULL_SOURCE_QUARANTINED: std::cell::RefCell<Option<ManuallyDrop<SourceRun>>>
        = const { std::cell::RefCell::new(None) };
}

#[test]
#[ignore = "actual complete local NOIRE Pure Saved13 source initialization only; explicit private pins and once-only exact selector required"]
fn windows_noire_full_saved13_source_initialization_only() {
    require(
        std::env::var("CITRUS_NOIRE_FULL_SOURCE_QA_OPT_IN").as_deref() == Ok(FULL_SOURCE_OPT_IN),
        "explicit full Saved13 source initialization opt-in is absent",
    )
    .unwrap();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    require(
        args == [
            FULL_SOURCE_SELECTOR,
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ],
        "only the exact once-only ignored full-source selector is admitted",
    )
    .unwrap();
    require(
        !FULL_SOURCE_ADMITTED.swap(true, Ordering::AcqRel),
        "complete source fixture was already admitted",
    )
    .unwrap();
    let directory = absolute_env("CITRUS_NOIRE_FULL_SOURCE_QA_DIR").unwrap();
    fs::create_dir(&directory)
        .expect("full-source QA directory must be new; old receipts are never overwritten");
    let mut scan_attempted = false;
    let mut scan_helper_reaped = None;
    let preflight = catch_unwind(AssertUnwindSafe(|| {
        let binaries = BinaryPins::load()?;
        let inputs = FullInputs::load_from_env()?;
        new_json(&directory.join("preflight-pins.json"), &json!({"schema":1,"binaries":binaries.evidence(),
            "project":{"path":inputs.pins.project_path,"sha256":inputs.pins.project_sha256,
                "bytes":inputs.pins.project_bytes,"state_bytes":inputs.pins.saved_state_bytes,"state_sha256":inputs.pins.saved_state_sha256},
            "source_owner_not_started":true,"original_project_writes":0}))?;
        // Production scan uses the exact saved spelling. Canonical paths are
        // input/binary pins only. No scan metadata is invented or substituted.
        scan_attempted = true;
        let checked = CheckedScan::perform(&inputs.source.descriptor.path);
        scan_helper_reaped = Some(checked.helper_reaped());
        new_json(&directory.join("separate-scan-preflight.json"), &checked.evidence())?;
        let scan = checked.admit(KONTAKT_UID)?;
        require(scan.helper_reaped() && scan.metadata_admitted(), "complete source scan helper or metadata admission is not proven")?;
        let descriptor = scan.descriptor().clone();
        binaries.unchanged()?;
        Ok((inputs, binaries, descriptor, scan))
    })).unwrap_or_else(|payload| Err(raw_panic(payload, "full source preflight including separate scan")));
    let (inputs, binaries, descriptor, scan) = preflight.unwrap_or_else(|reason| {
        let _ = new_json(
            &directory.join("source-result.json"),
            &json!({"schema":1,"stage":"preflight",
            "success":false,"actual_error":reason,"source_owner_started":false,
            "separate_scan_attempted":scan_attempted,"scan_helper_strict_reap_proven":scan_helper_reaped,
            "scan_outcome_observed":scan_helper_reaped.is_some(),
            "scan_receipt_path":directory.join("separate-scan-preflight.json"),
            "source_helper_strict_reap_proven":false,
            "partial_files_preserved":true}),
        );
        panic!(
            "complete NOIRE source preflight failed; preserve {}",
            directory.display()
        );
    });
    let constructed = catch_unwind(AssertUnwindSafe(|| SourceRun::new(inputs, binaries, scan)))
        .unwrap_or_else(|payload| Err(raw_panic(payload, "complete source owner construction")));
    let mut run = ManuallyDrop::new(constructed.unwrap_or_else(|reason| {
        let _ = new_json(
            &directory.join("source-result.json"),
            &json!({"schema":1,"stage":"owner-construction",
            "success":false,"actual_error":reason,"source_initialization_requested":false,
            "scan_helper_strict_reap_proven":scan_helper_reaped,
            "scan_receipt_path":directory.join("separate-scan-preflight.json"),
            "source_helper_strict_reap_proven":false,"partial_files_preserved":true}),
        );
        panic!(
            "complete NOIRE owner construction failed; preserve {}",
            directory.display()
        );
    }));
    let body = catch_unwind(AssertUnwindSafe(|| run.exercise(&descriptor)))
        .unwrap_or_else(|payload| Err(raw_panic(payload, "actual complete source initialization")));
    let before_cleanup = catch_unwind(AssertUnwindSafe(|| run.diagnostic("after_body_before_cleanup")))
        .unwrap_or_else(|payload| json!({"actual_error":raw_panic(payload,"read-only diagnostic"),"success_predicates_unchanged":true}));
    run.evidence["before_cleanup"] = before_cleanup;
    let cleanup = catch_unwind(AssertUnwindSafe(|| run.cleanup())).unwrap_or_else(|payload| {
        Err(raw_panic(
            payload,
            "actual complete source cleanup; wrapper quarantined",
        ))
    });
    let after_cleanup = catch_unwind(AssertUnwindSafe(|| run.diagnostic("after_cleanup_attempt")))
        .unwrap_or_else(|payload| json!({"actual_error":raw_panic(payload,"read-only post-cleanup diagnostic"),"success_predicates_unchanged":true}));
    run.evidence["after_cleanup"] = after_cleanup;
    let receipt = catch_unwind(AssertUnwindSafe(|| {
        run.final_receipt(&directory, &body, &cleanup)
    }))
    .unwrap_or_else(|payload| {
        Err(raw_panic(
            payload,
            "final source receipt; prior files preserved",
        ))
    });
    let success = body.is_ok() && cleanup.is_ok() && receipt.is_ok();
    if receipt.is_err() {
        let _ = new_json(
            &directory.join("result-writer-or-audit-failure.json"),
            &json!({"schema":1,"success":false,
            "body_error":body.as_ref().err(),"cleanup_error":cleanup.as_ref().err(),"receipt_error":receipt.as_ref().err(),
            "entire_run_wrapper_quarantined":true,"partial_files_preserved":true}),
        );
    }
    if success {
        unsafe { ManuallyDrop::drop(&mut run) };
    } else {
        // A failed run never replaces an earlier retained owner. ManuallyDrop
        // also keeps the whole run alive if TLS is busy or already unavailable
        // during thread exit; neither case is a successful resource release.
        let mut pending = Some(run);
        let _ = FULL_SOURCE_QUARANTINED.try_with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut()
                && slot.is_none()
            {
                *slot = pending.take();
            }
        });
    }
    assert!(
        success,
        "complete init-only NOIRE source qualification failed; preserve {}",
        directory.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument_bounce::{BounceAudioBus, BouncePluginMetadata};

    // Synthetic evidence predicates only. No observer installation, backend,
    // helper, SourceRun, FullParent, device or NI module is created here.
    fn identity() -> observe::InstanceIdentity {
        observe::InstanceIdentity {
            ordinal: 1,
            phase: InstancePhase::InitialSource,
            worker_id: Some(71),
            source_instance_id: Some(17),
        }
    }
    fn record(
        ordinal: u64,
        instance: Option<observe::InstanceIdentity>,
        kind: RecordKind,
    ) -> Arc<observe::Record> {
        Arc::new(observe::Record {
            ordinal,
            monotonic_elapsed_ns: ordinal,
            stage: Stage::InitialSource,
            instance,
            kind,
        })
    }
    fn metadata() -> BouncePluginMetadata {
        BouncePluginMetadata {
            class_uid: KONTAKT_UID.into(),
            version: "8.4.1".into(),
            name: "Synthetic evidence, not Kontakt".into(),
            category: "Instrument|Sampler".into(),
            input_buses: vec![],
            output_buses: vec![
                BounceAudioBus::new(2, true, Some(true)),
                BounceAudioBus::new(2, false, Some(false)),
            ],
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![],
            latency_samples: 0,
            tail_samples: 0,
        }
    }
    #[test]
    fn full_source_evidence_missing_duplicate_or_scan_identity_is_not_source_birth() {
        assert!(actual_source_identity(&[], 71, 17).is_err());
        let valid = record(
            1,
            Some(identity()),
            RecordKind::Lifecycle("instance construction observed"),
        );
        assert!(actual_source_identity(&[valid.clone(), valid.clone()], 71, 17).is_err());
        for case in 0..3 {
            let mut foreign = identity();
            match case {
                0 => foreign.phase = InstancePhase::StrictClone,
                1 => foreign.worker_id = None,
                _ => foreign.source_instance_id = None,
            }
            let foreign = record(
                1,
                Some(foreign),
                RecordKind::Lifecycle("instance construction observed"),
            );
            assert!(actual_source_identity(&[foreign], 71, 17).is_err());
        }
        assert!(actual_source_identity(std::slice::from_ref(&valid), 73, 17).is_err());
        let without_identity = record(
            2,
            None,
            RecordKind::Lifecycle("instance construction observed"),
        );
        assert!(actual_source_identity(&[valid.clone(), without_identity], 71, 17).is_err());
        assert!(actual_source_identity(&[valid], 71, 19).is_err());
    }
    #[test]
    fn full_source_evidence_ordinary_state_length_cannot_substitute_actual_bytes() {
        let ordinary = record(
            1,
            None,
            RecordKind::OrdinaryState {
                endpoint_id: 3,
                project_session: 4,
                instance_id: 17,
                slot: 0,
                request_id: 0,
                epoch: Some(5),
                byte_len: 591772,
            },
        );
        assert!(
            validate_state_operands(
                &[ordinary],
                identity(),
                591772,
                "5d902e91f5eb267f4cf71bd3c39f12f8551076017708530a243015adc075502b"
            )
            .is_err()
        );
    }
    #[test]
    fn full_source_evidence_failed_wrong_bytes_or_foreign_state_load_is_refused() {
        let bytes = vec![1, 2, 3];
        let hash = oracles::sha256_hex(&bytes);
        let failed = record(
            1,
            Some(identity()),
            RecordKind::StateLoad {
                bytes: bytes.clone(),
                outcome: observe::CallOutcome::Error("actual load failed".into()),
            },
        );
        assert!(validate_state_operands(&[failed], identity(), bytes.len(), &hash).is_err());
        let valid = record(
            2,
            Some(identity()),
            RecordKind::StateLoad {
                bytes: bytes.clone(),
                outcome: observe::CallOutcome::Ok,
            },
        );
        assert!(
            validate_state_operands(std::slice::from_ref(&valid), identity(), 591772, &hash)
                .is_err()
        );
        assert!(
            validate_state_operands(
                std::slice::from_ref(&valid),
                identity(),
                bytes.len(),
                &"0".repeat(64)
            )
            .is_err()
        );
        let mut foreign = identity();
        foreign.worker_id = Some(73);
        let wrong_identity = record(
            3,
            Some(foreign),
            RecordKind::StateLoad {
                bytes,
                outcome: observe::CallOutcome::Ok,
            },
        );
        assert!(validate_state_operands(&[valid, wrong_identity], identity(), 3, &hash).is_err());
    }
    #[test]
    fn full_source_evidence_one_valid_load_does_not_mask_a_later_different_operand() {
        let expected = vec![1, 2, 3];
        let hash = oracles::sha256_hex(&expected);
        let first = record(
            1,
            Some(identity()),
            RecordKind::StateLoad {
                bytes: expected,
                outcome: observe::CallOutcome::Ok,
            },
        );
        let second = record(
            2,
            Some(identity()),
            RecordKind::StateLoad {
                bytes: vec![1, 2, 4],
                outcome: observe::CallOutcome::Ok,
            },
        );
        assert!(validate_state_operands(&[first, second], identity(), 3, &hash).is_err());
    }
    #[test]
    fn full_source_evidence_missing_failed_or_wrong_version_metadata_is_refused() {
        assert!(validate_metadata(&[], identity(), 48000).is_err());
        let error = record(
            1,
            Some(identity()),
            RecordKind::Metadata {
                site: "synthetic evidence",
                result: Err("actual metadata failed".into()),
            },
        );
        assert!(validate_metadata(&[error], identity(), 48000).is_err());
        let mut wrong = metadata();
        wrong.version = "8.4.0".into();
        let wrong = record(
            2,
            Some(identity()),
            RecordKind::Metadata {
                site: "synthetic evidence",
                result: Ok(wrong),
            },
        );
        assert!(validate_metadata(&[wrong], identity(), 48000).is_err());
    }
    #[test]
    fn full_source_evidence_pre_start_unknown_activation_is_retained_but_final_stereo_required() {
        let mut early = metadata();
        early.output_buses[0].active = None;
        early.output_buses[1].active = None;
        let early = record(
            1,
            Some(identity()),
            RecordKind::Metadata {
                site: "synthetic pre-start",
                result: Ok(early),
            },
        );
        assert!(validate_metadata(std::slice::from_ref(&early), identity(), 48000).is_err());
        let final_record = record(
            2,
            Some(identity()),
            RecordKind::Metadata {
                site: "synthetic post-start",
                result: Ok(metadata()),
            },
        );
        validate_metadata(&[early, final_record], identity(), 48000).unwrap();
        let mut active_aux = metadata();
        active_aux.output_buses[1].active = Some(true);
        let active_aux = record(
            3,
            Some(identity()),
            RecordKind::Metadata {
                site: "synthetic final",
                result: Ok(active_aux),
            },
        );
        assert!(validate_metadata(&[active_aux], identity(), 48000).is_err());
    }
    #[test]
    fn full_source_evidence_metadata_receipt_keeps_complete_audio_and_event_descriptors() {
        let metadata = metadata();
        let value = metadata_evidence(&metadata);
        assert_eq!(value["audio_outputs"][0]["channel_count"], json!(2));
        assert_eq!(value["audio_outputs"][0]["is_main"], json!(true));
        assert_eq!(value["audio_outputs"][1]["active"], json!(false));
        assert_eq!(value["event_inputs"][0], json!([16, true, true]));
        assert_eq!(value["audio_outputs"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn full_source_evidence_actual_failure_selector_does_not_relabel_success() {
        let failed = record(
            1,
            Some(identity()),
            RecordKind::Call {
                operation: Operation::SourcePrepare,
                boundary: Boundary::Returned,
                frames: None,
                transport: None,
                outcome: Some(observe::CallOutcome::Error("literal actual failure".into())),
            },
        );
        assert!(actual_failure(&failed));
        let success = record(
            2,
            Some(identity()),
            RecordKind::WorkerJoin {
                worker_id: 71,
                outcome: WorkerJoinOutcome::Joined,
            },
        );
        assert!(!actual_failure(&success));
        let panic = record(
            3,
            Some(identity()),
            RecordKind::WorkerJoin {
                worker_id: 71,
                outcome: WorkerJoinOutcome::WorkerPanicked,
            },
        );
        assert!(actual_failure(&panic));
    }
}
