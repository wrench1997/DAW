//! Short saved-state/source-capture/Whole qualification, not a full Saved13 or GUI run.
//! No separate loaded-default scan: this attempt's actual source worker must return
//! the Kontakt identity and processing metadata before its production capture/export.
use super::windows_noire_full_inputs::FullInputs;
use super::windows_noire_full_owner::{FullParent, RetainedFullParent};
use super::windows_noire_full_source_qa::{
    actual_source_identity, validate_metadata, validate_state_operands,
};
use super::windows_noire_whole_song_qa::{
    actual_strict_process_timing, audit_short_whole, persist_generated_raw_pcm, whole_scratch,
};
use super::*;
use crate::export_options::{WavExportOptions, WavLevelPolicy, WavSampleFormat};
use crate::instrument_bounce::{BouncePluginMetadata, render::MasterSongRenderReport};

const SHORT_SELECTOR: &str = "app::clip_render_job::windows_noire_bounce_qa::windows_noire_sdk_b_short_qa::windows_noire_saved_state_source_capture_strict_short_whole";
const SHORT_OPT_IN: &str = "actual-noire-saved-state-source-capture-strict-short-whole-v1";
const SHORT_SECONDS: u64 = 120;
const INITIALIZATION_SECONDS: u64 = 40;
const CLEANUP_SECONDS: u64 = 35;
const PRELUDE_LIMIT: usize = 64;
static SHORT_ADMITTED: AtomicBool = AtomicBool::new(false);

/// Private fixed two-note derivative only. No persisted user file is rewritten.
/// The independent existing Whole oracle specifies this exact score and mix.
fn short_project(saved: &Project, source_id: u64, channel_id: u32) -> Check<Project> {
    let source = saved
        .plugin_instances
        .iter()
        .find(|item| item.id == source_id)
        .ok_or("saved short source is absent")?;
    let channel = saved
        .channels
        .iter()
        .find(|item| item.id == channel_id)
        .ok_or("saved short source channel is absent")?;
    require(
        channel.instrument_plugin_instance_id == Some(source_id),
        "saved short assignment differs",
    )?;
    let mut project = Project::blank();
    project.channels[0].id = channel_id;
    project.channels[0].volume = channel.volume;
    project.channels[0].instrument_plugin_instance_id = Some(source_id);
    let mut instance = source.clone();
    instance.runtime_status = PluginRuntimeStatus::Unloaded;
    project.plugin_instances = vec![instance];
    for track in &mut project.mixer_tracks {
        track.volume = 1.0;
    }
    project.clips = vec![
        saved
            .clips
            .iter()
            .find(|clip| clip.kind == ClipKind::Pattern)
            .ok_or("saved short input has no pattern template")?
            .clone(),
    ];
    let project = whole_scratch(project)?;
    require(
        project.plugin_instances[0].opaque_state == source.opaque_state
            && project.plugin_instances[0].parameters == source.parameters
            && project.plugin_instances[0].midi_ports == source.midi_ports,
        "private short projection changed saved opaque, generic map or MIDI ports",
    )?;
    Ok(project)
}

fn actual_metadata(
    records: &[Arc<observe::Record>],
    identity: observe::InstanceIdentity,
) -> Check<BouncePluginMetadata> {
    let selected = records
        .iter()
        .filter(|record| {
            record.instance == Some(identity) && matches!(record.kind, RecordKind::Metadata { .. })
        })
        .take(PRELUDE_LIMIT + 1)
        .cloned()
        .collect::<Vec<_>>();
    validate_metadata(&selected, identity, RATE as u32)?;
    let metadata = selected
        .iter()
        .rev()
        .find_map(|record| match &record.kind {
            RecordKind::Metadata {
                result: Ok(metadata),
                ..
            } => Some(metadata.clone()),
            _ => None,
        })
        .ok_or("short source has no actual metadata return")?;
    require(
        metadata
            .category
            .split('|')
            .any(|token| token.trim() == "Instrument")
            && metadata
                .event_input_buses
                .iter()
                .any(|bus| bus.0 > 0 && bus.2 == Some(true)),
        "actual short source is not an Instrument with active MIDI input",
    )?;
    Ok(metadata)
}

/// Fixed threshold, original samples and frame origin. No alignment fit or trim.
fn onset_metrics(bits: &[u32]) -> Check<Value> {
    require(
        !bits.is_empty() && bits.len().is_multiple_of(2),
        "short onset needs complete stereo frames",
    )?;
    require(
        bits.iter().all(|bits| f32::from_bits(*bits).is_finite()),
        "short onset contains nonfinite PCM",
    )?;
    let first = |threshold: f32| {
        bits.chunks_exact(2).position(|frame| {
            frame
                .iter()
                .any(|bits| f32::from_bits(*bits).abs() > threshold)
        })
    };
    Ok(
        json!({"sample_rate":RATE,"frames":bits.len()/2,"threshold_dbfs":-100,
        "threshold_linear":1.0e-5,"first_above_threshold_frame":first(1.0e-5),
        "first_nonzero_frame":first(0.0),"frame_origin":"unaltered published WAV frame zero",
        "alignment_fitted":false,"extra_trim_or_delay":false,"repeatability_claim":false}),
    )
}

struct ShortPins {
    executable: PathBuf,
    helper: PathBuf,
    executable_hash: String,
    helper_hash: String,
}
impl ShortPins {
    fn load() -> Check<Self> {
        let executable = std::env::current_exe()
            .map_err(error)?
            .canonicalize()
            .map_err(error)?;
        let helper = absolute_env("CITRUS_NOIRE_SDK_B_QA_HELPER")?
            .canonicalize()
            .map_err(error)?;
        require(
            helper
                == executable
                    .parent()
                    .ok_or("short test binary has no parent")?
                    .join("vst3-host-helper.exe")
                    .canonicalize()
                    .map_err(error)?,
            "short probe requires its pinned adjacent production helper",
        )?;
        let executable_hash = oracles::sha256_hex(&read_bounded(&executable, 256 * 1024 * 1024)?);
        let helper_hash = oracles::sha256_hex(&read_bounded(&helper, 64 * 1024 * 1024)?);
        for (actual, variable) in [
            (&executable_hash, "CITRUS_NOIRE_SDK_B_QA_EXECUTABLE_SHA256"),
            (&helper_hash, "CITRUS_NOIRE_SDK_B_QA_HELPER_SHA256"),
        ] {
            let expected = std::env::var(variable).map_err(error)?;
            require(
                expected.len() == 64
                    && expected.bytes().all(|b| b.is_ascii_hexdigit())
                    && expected == *actual,
                "short probe binary differs from its explicit lowercase SHA256 pin",
            )?;
        }
        Ok(Self {
            executable,
            helper,
            executable_hash,
            helper_hash,
        })
    }
    fn evidence(&self) -> Value {
        json!({"test_executable":{"path":self.executable,"sha256":self.executable_hash},
            "production_helper":{"path":self.helper,"sha256":self.helper_hash}})
    }
    fn unchanged(&self) -> Check {
        require(
            oracles::sha256_hex(&read_bounded(&self.executable, 256 * 1024 * 1024)?)
                == self.executable_hash
                && oracles::sha256_hex(&read_bounded(&self.helper, 64 * 1024 * 1024)?)
                    == self.helper_hash,
            "short probe binary binding changed during the same attempt",
        )
    }
}

struct ShortRun {
    parent: RetainedFullParent,
    observer: ObservationRun,
    pins: ShortPins,
    directory: PathBuf,
    evidence: Value,
    deadline: Instant,
    report: Option<MasterSongRenderReport>,
}
impl ShortRun {
    fn new(
        mut inputs: FullInputs,
        pins: ShortPins,
        directory: PathBuf,
        run_label: String,
    ) -> Check<Self> {
        let project = short_project(
            &inputs.project,
            inputs.source.plugin_instance_id,
            inputs.source.channel_id,
        )?;
        let projection = serde_json::to_vec(&project).map_err(error)?;
        new_write(&directory.join("private-short-project.citrus"), &projection)?;
        let evidence = json!({"schema":1,"fixture":SHORT_OPT_IN,"run_label":run_label,
            "scope":"private two-note saved opaque/generic map -> actual source -> stopped capture -> production Whole/Strict; not complete Saved13 or ordinary GUI acceptance",
            "original_project":{"path":inputs.pins.project_path,"bytes":inputs.pins.project_bytes,
                "sha256":inputs.pins.project_sha256,"opaque_bytes":inputs.pins.saved_state_bytes,
                "opaque_sha256":inputs.pins.saved_state_sha256,"writes":0},
            "short_projection":{"path":directory.join("private-short-project.citrus"),"sha256":oracles::sha256_hex(&projection),
                "patterns":2,"placements":2,"beats":2,"tempo":84,"tail_ms":3000,
                "generic_base_count":project.plugin_instances[0].parameters.len(),"original_score_replaced_only_in_private_derivative":true},
            "binaries":pins.evidence(),"separate_loaded_default_scan_performed":false,
            "actual_source":{"instance":Value::Null,"helper_pid":Value::Null,"metadata":Value::Null},
            "scan_clean_reap_qualification":false,"metadata_source":"this attempt's actual source worker returns, never a catalog or saved declaration",
            "budgets":{"outer_driver_seconds":SHORT_SECONDS,"initialization_seconds":INITIALIZATION_SECONDS,
                "cleanup_seconds":CLEANUP_SECONDS,"original_renderer_budget_unchanged":true},
            "unverified":{"ordinary_GUI_workflow":true,"native_Save_close_reopen":true,"physical_monitoring":true,
                "preset_name_and_all_NI_internal_parameters":true,"full_Saved13":true,"repeatability":true,
                "independent_source_helper_strict_reap":true}});
        inputs.project = project;
        let observer =
            ObservationRun::install_with_clone_reply_limit(CLONE_REPLY_LIMIT).map_err(error)?;
        let parent = FullParent::new_retained(inputs, [0x42; 32])?;
        Ok(Self {
            parent,
            observer,
            pins,
            directory,
            evidence,
            deadline: Instant::now() + Duration::from_secs(SHORT_SECONDS),
            report: None,
        })
    }
    fn exercise(&mut self) -> Check {
        let initialization_deadline = self
            .deadline
            .min(Instant::now() + Duration::from_secs(INITIALIZATION_SECONDS));
        self.parent
            .begin_saved_source_initialization(&mut self.observer, initialization_deadline)?;
        let ready = self
            .parent
            .wait_source_initialization(&self.observer, initialization_deadline)?;
        let records = self.observer.with_snapshot(|state| state.records.clone());
        let source = actual_source_identity(
            &records,
            ready.actual_worker_id,
            self.parent.source.plugin_instance_id,
        )?;
        let state_records = records
            .iter()
            .filter(|record| {
                record.instance == Some(source)
                    && matches!(record.kind, RecordKind::StateLoad { .. })
            })
            .cloned()
            .collect::<Vec<_>>();
        validate_state_operands(
            &state_records,
            source,
            self.parent.pins.saved_state_bytes,
            &self.parent.pins.saved_state_sha256,
        )?;
        let metadata = actual_metadata(&records, source)?;
        require(
            records
                .iter()
                .filter(|record| {
                    record.instance == Some(source)
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
                .count()
                == 1,
            "short source lacks one successful actual prepare return",
        )?;
        self.evidence["actual_source"] = json!({"instance":{"ordinal":source.ordinal,"worker_id":source.worker_id,
            "source_instance_id":source.source_instance_id},"helper_pid":Value::Null,
            "helper_pid_evidence":"not exposed by the existing ordinary source guard; outer OS observer must bind its actual child handle",
            "metadata":metadata_json(&metadata),"original_opaque_operand_acknowledged":true});
        let wav = self.directory.join("short-whole-master.wav");
        let binding = self.parent.begin_full_export(
            &mut self.observer,
            wav.clone(),
            WavExportOptions {
                sample_rate: RATE as u32,
                sample_format: WavSampleFormat::Float32,
                level_policy: WavLevelPolicy::PreserveLevel,
                tail_milliseconds: 3000,
            },
            true,
        )?;
        self.report =
            Some(
                self.parent
                    .wait_full_export(&self.observer, binding, &wav, self.deadline)?,
            );
        self.audit()
    }
    fn audit(&mut self) -> Check {
        let (records, failure) = self
            .observer
            .with_snapshot(|state| (state.records.clone(), state.failure));
        let capture = records
            .iter()
            .filter_map(|record| match &record.kind {
                RecordKind::Capture {
                    label: CaptureLabel::Initial,
                    identity,
                    receipt_identity,
                    state,
                } => Some((*identity, *receipt_identity, state)),
                _ => None,
            })
            .collect::<Vec<_>>();
        require(
            capture.len() == 1 && capture[0].1 != 0,
            "short Whole needs one actual ACKed capture",
        )?;
        let report = self
            .report
            .as_ref()
            .ok_or("short Whole has no typed report")?;
        require(
            capture[0].0 == report.dry_render.identity,
            "short capture differs from the production report",
        )?;
        let clones = records
            .iter()
            .filter_map(|record| {
                record
                    .instance
                    .filter(|id| id.phase == InstancePhase::StrictClone)
            })
            .map(|id| (id.ordinal, id))
            .collect::<BTreeMap<_, _>>();
        require(
            clones.len() == 1,
            "short audit has absent or duplicate clone identities",
        )?;
        let wav = read_bounded(
            &self.directory.join("short-whole-master.wav"),
            16 * 1024 * 1024,
        )?;
        let (audit, acoustics) = audit_short_whole(
            &records,
            failure,
            *clones.values().next().unwrap(),
            &capture[0].2.metadata,
            report,
            self.parent.app().project.channels[0].volume,
            &wav,
        )?;
        self.evidence["first_actual_midi_quantum"] = audit["complete_actual_midi"]
            .as_array()
            .and_then(|rows| {
                rows.iter().find(|row| {
                    row["data"][0].as_u64().is_some_and(|v| v & 0xf0 == 0x90)
                        && row["data"][2].as_u64().is_some_and(|v| v > 0)
                })
            })
            .cloned()
            .unwrap_or(Value::Null);
        self.evidence["complete_pcm_midi_audit"] = audit;
        self.evidence["published_onset"] =
            onset_metrics(&oracles::parse_float32_stereo_48k_wav(&wav)?.sample_bits)?;
        self.evidence["acoustic_qualification"] =
            json!({"passed":acoustics.is_ok(),"actual_error":acoustics.as_ref().err()});
        acoustics
    }
    fn cleanup(&mut self) -> Check {
        self.parent.cleanup(
            &mut self.observer,
            Instant::now() + Duration::from_secs(CLEANUP_SECONDS),
        )
    }
    fn receipt(&mut self, body: &Check, cleanup: &Check) -> Check {
        let (records, failure) = self
            .observer
            .with_snapshot(|state| (state.records.clone(), state.failure));
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
                    "short success requires one joined source and one naturally reaped clone",
                )?;
            }
            Ok(())
        })();
        let raw = persist_generated_raw_pcm(&records, &self.directory);
        self.evidence["raw_generated_pcm"] = raw
            .as_ref()
            .cloned()
            .unwrap_or_else(|reason| json!({"error":reason}));
        self.evidence["actual_strict_process_timing"] =
            actual_strict_process_timing(&records, failure);
        let async_summary = self.observer.async_observation_summary();
        let async_evidence = async_summary.evidence();
        let async_observed = async_evidence["complete"].as_bool() == Some(true);
        self.evidence["reported_async_readiness"] = async_evidence;
        self.evidence["source_terminal"] = json!({
            "helper_pid":Value::Null,"helper_OS_exit_code":Value::Null,
            "worker_incarnations":terminal.source_instances.iter().map(|(phase,worker,instance)|
                json!({"phase":format!("{phase:?}"),"worker_id":worker,"source_instance_id":instance})).collect::<Vec<_>>(),
            "thread_joins":terminal.evidence()["joins"],
            "independent_helper_clean_exit":Value::Null,
            "OS_exit_evidence_owner":"Root's retained actual process handles; no inference from test exit or worker join"});
        attach_capture_diagnostics(
            &mut self.evidence,
            &self.directory,
            &records,
            cleanup.is_ok() && failure.is_none(),
        );
        let pins = self
            .pins
            .unchanged()
            .and_then(|()| self.parent.inputs_unchanged());
        let model = self.parent.model_unchanged();
        let success = body.is_ok()
            && cleanup.is_ok()
            && self.parent.resources_joined()
            && terminal_result.is_ok()
            && failure.is_none()
            && pins.is_ok()
            && model.is_ok()
            && async_observed
            && raw.is_ok();
        self.evidence["result"] = json!({"success":success,"body_passed":body.is_ok(),"actual_body_error":body.as_ref().err(),
            "cleanup_passed":cleanup.is_ok(),"actual_cleanup_error":cleanup.as_ref().err(),
            "owned_resources_joined":self.parent.resources_joined(),"terminal_evidence":terminal.evidence(),
            "terminal_error":terminal_result.as_ref().err(),"observer_failure":failure,
            "original_input_and_binaries_unchanged":pins.is_ok(),"pin_error":pins.as_ref().err(),
            "private_project_history_unchanged":model.is_ok(),"model_error":model.as_ref().err(),
            "typed_complete_publication":self.report.is_some(),"partial_evidence_preserved":true,
            "both_snapshot_phases_observed":async_observed,
            "source_worker_join_is_not_independent_source_helper_reap":true});
        new_json(&self.directory.join("short-result.json"), &self.evidence)?;
        require(
            success,
            "short saved-state/source-capture/strict qualification failed; retain the same owners and evidence",
        )
    }
}

thread_local! { static SHORT_QUARANTINED:std::cell::RefCell<Option<ManuallyDrop<ShortRun>>> = const {std::cell::RefCell::new(None)}; }

#[test]
#[ignore = "actual saved NOIRE source/capture/strict short Whole; explicit private pins and exclusive native window, not full Saved13/GUI acceptance"]
fn windows_noire_saved_state_source_capture_strict_short_whole() {
    require(
        std::env::var("CITRUS_NOIRE_SDK_B_QA_OPT_IN").as_deref() == Ok(SHORT_OPT_IN),
        "explicit short NI opt-in missing",
    )
    .unwrap();
    require(
        std::env::args().skip(1).collect::<Vec<_>>()
            == [
                SHORT_SELECTOR,
                "--ignored",
                "--exact",
                "--test-threads=1",
                "--nocapture",
            ],
        "only the exact once-only short NI selector is admitted",
    )
    .unwrap();
    require(
        !SHORT_ADMITTED.swap(true, Ordering::AcqRel),
        "short NI selector already admitted",
    )
    .unwrap();
    let directory = absolute_env("CITRUS_NOIRE_SDK_B_QA_DIR").unwrap();
    fs::create_dir(&directory).expect("short NI output directory must be new");
    let prepared = catch_unwind(AssertUnwindSafe(|| {
        let run_label = std::env::var("CITRUS_NOIRE_SDK_B_QA_RUN_LABEL").map_err(error)?;
        require(
            !run_label.is_empty()
                && run_label.len() <= 64
                && run_label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "short run label needs 1..64 ASCII letters, digits or hyphens",
        )?;
        ShortRun::new(
            FullInputs::load_from_env()?,
            ShortPins::load()?,
            directory.clone(),
            run_label,
        )
    }))
    .unwrap_or_else(|p| Err(panic_error(p, "short NI preflight")));
    let mut run = ManuallyDrop::new(prepared.unwrap_or_else(|reason| {
        let _=new_json(&directory.join("short-result.json"),&json!({"success":false,"stage":"preflight","actual_error":reason,"source_owner_started":false}));
        panic!("short NI preflight failed; preserve {}",directory.display());
    }));
    let body = catch_unwind(AssertUnwindSafe(|| run.exercise()))
        .unwrap_or_else(|p| Err(panic_error(p, "short NI body")));
    let cleanup = catch_unwind(AssertUnwindSafe(|| run.cleanup()))
        .unwrap_or_else(|p| Err(panic_error(p, "short NI cleanup")));
    let receipt = catch_unwind(AssertUnwindSafe(|| run.receipt(&body, &cleanup)))
        .unwrap_or_else(|p| Err(panic_error(p, "short NI receipt")));
    let success = body.is_ok() && cleanup.is_ok() && receipt.is_ok();
    if success {
        unsafe { ManuallyDrop::drop(&mut run) };
    } else {
        SHORT_QUARANTINED.with(|slot| *slot.borrow_mut() = Some(run));
    }
    assert!(
        success,
        "short NI qualification failed; preserve {}",
        directory.display()
    );
}

#[cfg(test)]
include!("windows_noire_sdk_b_short_tests.rs");
