//! Explicit complete Saved13 production whole-song driver. The original score,
//! routes, automation, state, history and saved path are never normalized.
//! Native acceptance requires the concrete stream retirement, complete typed
//! replay and independent full PCM/WAV verification wired below; source gates
//! alone do not establish native sound acceptance.
#![cfg(all(test, windows, feature = "experimental-fresh-helper"))]

use super::windows_noire_full_inputs::FullInputs;
use super::windows_noire_full_owner::{FullParent, RetainedFullParent};
use super::windows_noire_full_source_qa::{
    actual_source_identity, validate_metadata, validate_state_operands,
};
use super::*;
use crate::app::stopped_bounce_capture::LiveBinding;
use crate::export_options::{WavExportOptions, WavLevelPolicy, WavSampleFormat};
use crate::instrument_bounce::BouncePluginMetadata;
use crate::instrument_bounce::render::MasterSongRenderReport;

const SELECTOR: &str = "app::clip_render_job::windows_noire_bounce_qa::windows_noire_full_song_qa::windows_noire_full_saved13_whole_song_offline";
const OPT_IN: &str = "actual-noire-pure-complete-saved13-whole-offline-v1";
const RUN_SECONDS: u64 = 280;
const INITIALIZATION_SECONDS: u64 = 40;
const CLEANUP_SECONDS: u64 = 35;
const PRELUDE_OPERAND_LIMIT: usize = 64;
static FULL_SONG_ADMITTED: AtomicBool = AtomicBool::new(false);

pub(super) struct FullSongEvidencePaths {
    pub directory: PathBuf,
    pub stream: PathBuf,
    pub states: PathBuf,
    pub wav: PathBuf,
}

/// Root supplies the actual observer storage and independent verification.
/// This contract separates native resource retirement from sound acceptance.
/// All methods retain their original evidence on failure; no timeout restart,
/// count-only replay, shortened score or heuristic opaque state equivalence.
pub(super) trait FullSongVerification {
    fn activate(
        &mut self,
        observer: &mut ObservationRun,
        source_instance: observe::InstanceIdentity,
        actual_metadata: &BouncePluginMetadata,
        paths: &FullSongEvidencePaths,
        deadline: Instant,
    ) -> Check;
    fn retire(
        &mut self,
        observer: &mut ObservationRun,
        actual_resources_joined: bool,
        deadline: Instant,
    ) -> Check<Value>;
    fn audit(&mut self, inputs: FullSongAuditInputs<'_>) -> Check<Value>;
}

pub(super) struct FullSongAuditInputs<'a> {
    pub paths: &'a FullSongEvidencePaths,
    pub project: &'a Project,
    pub original_project_path: &'a Path,
    pub original_project_sha256: &'a str,
    pub original_state_bytes: usize,
    pub original_state_sha256: &'a str,
    pub actual_source_instance: observe::InstanceIdentity,
    pub actual_source_binding: LiveBinding,
    pub actual_source_metadata: &'a BouncePluginMetadata,
    pub report: &'a MasterSongRenderReport,
}

// The concrete adapter retains the actual retirement owner and audit child.
// A production WAV alone cannot manufacture a successful native receipt.
fn native_verification() -> Check<Box<dyn FullSongVerification>> {
    Ok(Box::new(NativeVerification {
        retirement: None,
        audit_child: None,
    }))
}

struct NativeVerification {
    retirement: Option<observe::StreamRetirement>,
    audit_child: Option<std::process::Child>,
}
impl FullSongVerification for NativeVerification {
    fn activate(
        &mut self,
        observer: &mut ObservationRun,
        source_instance: observe::InstanceIdentity,
        _actual_metadata: &BouncePluginMetadata,
        paths: &FullSongEvidencePaths,
        deadline: Instant,
    ) -> Check {
        observer
            .enable_stream(&paths.stream, &paths.states, source_instance, deadline)
            .map_err(|failure| format!("actual stream activation failed: {failure:?}"))
    }
    fn retire(
        &mut self,
        observer: &mut ObservationRun,
        actual_resources_joined: bool,
        deadline: Instant,
    ) -> Check<Value> {
        require(
            self.retirement.is_none(),
            "stream retirement may be transferred only once",
        )?;
        // Store the entire owner before any poll can fail or unwind.
        self.retirement = Some(
            observer
                .take_stream_after_taps_retired(actual_resources_joined)
                .map_err(|failure| {
                    format!("actual stream retirement transfer failed: {failure:?}")
                })?,
        );
        let retirement = self.retirement.as_mut().unwrap();
        loop {
            match retirement.poll_finish() {
                observe::StreamFinishPoll::Pending => {
                    if Instant::now() >= deadline {
                        retirement.cancel();
                        return Err("stream/state writer observation deadline elapsed; the same owner remains retained, not joined or restarted".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                observe::StreamFinishPoll::Joined { complete } => {
                    let evidence = retirement.writer_evidence();
                    require(
                        complete,
                        "actual transcript or state writer joined without successful complete linkage",
                    )?;
                    return Ok(evidence);
                }
            }
        }
    }
    fn audit(&mut self, inputs: FullSongAuditInputs<'_>) -> Check<Value> {
        let retirement = self
            .retirement
            .as_ref()
            .ok_or("complete sound audit has no retained joined stream")?;
        let replay: observe::StreamReplayEvidence = retirement
            .replay_to_raw(&inputs.paths.directory.join("complete-generated-raw.f32le"))
            .map_err(|failure| {
                format!("complete typed replay/raw extraction failed: {failure:?}")
            })?;
        let report = &inputs.report.dry_render;
        let expected_stamp = [
            report.identity.project_session,
            report.identity.project_fingerprint,
            report.identity.timeline_revision,
            report.identity.epoch,
            report.identity.endpoint_id,
            u64::from(report.identity.channel_id),
            report.identity.instance_id,
        ];
        require(
            replay.source_ordinal == inputs.actual_source_instance.ordinal
                && replay.source_worker_id == inputs.actual_source_instance.worker_id
                && replay.source_instance_id == inputs.actual_source_instance.source_instance_id
                && replay.report_identity == expected_stamp
                && replay.capture_identity == expected_stamp
                && replay.native_latency == report.native_latency
                && replay.reported_tail == report.reported_tail
                && replay.generated_frames == report.generated_frames
                && replay.musical_frames == report.musical_frames
                && replay.retained_frames == report.frames
                && replay.fixed_tail == report.fixed_tail,
            "typed stream and production full-song report disagree",
        )?;
        require(
            report.identity.project_session == inputs.actual_source_binding.stamp.project_session
                && report.identity.timeline_revision
                    == inputs.actual_source_binding.stamp.timeline_revision
                && report.identity.endpoint_id == inputs.actual_source_binding.stamp.endpoint_id
                && report.identity.channel_id == inputs.actual_source_binding.stamp.channel_id
                && report.identity.instance_id == inputs.actual_source_binding.stamp.instance_id,
            "full sound report differs from its actual source binding",
        )?;
        require(
            inputs.actual_source_metadata.class_uid == KONTAKT_UID
                && inputs.actual_source_metadata.version == "8.4.1"
                && inputs.original_project_sha256 == SOURCE_SHA256
                && inputs.original_state_bytes == 591_772
                && inputs.original_state_sha256
                    == "5d902e91f5eb267f4cf71bd3c39f12f8551076017708530a243015adc075502b"
                && inputs
                    .project
                    .patterns
                    .iter()
                    .map(|p| p.notes.len())
                    .sum::<usize>()
                    == 405,
            "complete sound audit did not retain original Saved13 source inputs",
        )?;
        let observation_path = inputs
            .paths
            .directory
            .join("complete-stream-projection.json");
        new_json(
            &observation_path,
            &serde_json::to_value(&replay).map_err(error)?,
        )?;
        let verifier =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/validate_full_observation.py");
        let output = inputs
            .paths
            .directory
            .join("independent-complete-sound-audit.json");
        let stdout_path = inputs
            .paths
            .directory
            .join("independent-complete-sound-audit.stdout");
        let stderr_path = inputs
            .paths
            .directory
            .join("independent-complete-sound-audit.stderr");
        // Child output goes directly to private create-new files. No complete
        // PCM, state or uncontrolled stdout is buffered into the App owner.
        let stdout = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stdout_path)
            .map_err(error)?;
        let stderr = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stderr_path)
            .map_err(error)?;
        require(
            self.audit_child.is_none(),
            "the independent audit child cannot be replaced while owned",
        )?;
        self.audit_child = Some(
            std::process::Command::new("python")
                .args(["-B"])
                .arg(&verifier)
                .arg("--project")
                .arg(inputs.original_project_path)
                .arg("--observation")
                .arg(&observation_path)
                .arg("--wav")
                .arg(&inputs.paths.wav)
                .arg("--out")
                .arg(&output)
                .stdout(stdout)
                .stderr(stderr)
                .spawn()
                .map_err(error)?,
        );
        let status = self.audit_child.as_mut().unwrap().wait().map_err(error)?;
        // A wait error retains this exact Child in the quarantined verifier.
        // Only a real joined terminal status releases its owner.
        self.audit_child.take();
        let result: Value =
            serde_json::from_slice(&read_bounded(&output, 1024 * 1024)?).map_err(error)?;
        require(
            status.success() && result["whole_sound_files_pass"].as_bool() == Some(true),
            "independent full MIDI/state/PCM/WAV audit rejected the output; original files and diagnostics preserved",
        )?;
        Ok(
            json!({"typed_replay":serde_json::to_value(&replay).map_err(error)?,
            "independent_full_audit":result,"actual_audit_child_joined":true,
            "stdout":stdout_path,"stderr":stderr_path,
            "SDK_or_source_helper_strict_reap_claim":false,"physical_listening":false}),
        )
    }
}

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
        let helper = absolute_env("CITRUS_NOIRE_FULL_SONG_QA_HELPER")?
            .canonicalize()
            .map_err(error)?;
        let beside = executable
            .parent()
            .ok_or("complete whole-song private executable has no parent")?
            .join("vst3-host-helper.exe")
            .canonicalize()
            .map_err(error)?;
        require(
            helper == beside,
            "complete whole-song helper must be the production helper beside the private executable",
        )?;
        let executable_sha256 = oracles::sha256_hex(&read_bounded(&executable, 256 * 1024 * 1024)?);
        let helper_sha256 = oracles::sha256_hex(&read_bounded(&helper, 64 * 1024 * 1024)?);
        for (actual, variable) in [
            (
                &executable_sha256,
                "CITRUS_NOIRE_FULL_SONG_QA_EXECUTABLE_SHA256",
            ),
            (&helper_sha256, "CITRUS_NOIRE_FULL_SONG_QA_HELPER_SHA256"),
        ] {
            let supplied = std::env::var(variable).map_err(error)?;
            require(
                supplied.len() == 64
                    && supplied.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && supplied == *actual,
                "complete whole-song binary differs from its explicit lowercase SHA256 admission pin",
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
                == self.executable_sha256
                && oracles::sha256_hex(&read_bounded(&self.helper, 64 * 1024 * 1024)?)
                    == self.helper_sha256,
            "complete whole-song executable or production helper changed during qualification",
        )
    }
    fn evidence(&self) -> Value {
        json!({"test_executable":{"path":self.executable,"sha256":self.executable_sha256},
            "production_helper":{"path":self.helper,"sha256":self.helper_sha256},
            "hash_work_scope":"driver-only preflight/final; never callback or observer collector lock"})
    }
}

fn options() -> WavExportOptions {
    WavExportOptions {
        sample_rate: RATE as u32,
        sample_format: WavSampleFormat::Float32,
        level_policy: WavLevelPolicy::PreserveLevel,
        tail_milliseconds: 3_000,
    }
}

fn panic_error(payload: Box<dyn std::any::Any + Send>, stage: &str) -> String {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-text panic payload");
    format!("{stage} unwound: {message}; entire complete run remains retained")
}

struct FullSongRun {
    parent: RetainedFullParent,
    observer: ObservationRun,
    binaries: BinaryPins,
    verification: Box<dyn FullSongVerification>,
    paths: FullSongEvidencePaths,
    source_instance: Option<observe::InstanceIdentity>,
    source_binding: Option<LiveBinding>,
    source_metadata: Option<BouncePluginMetadata>,
    report: Option<MasterSongRenderReport>,
    stream_attempted: bool,
    evidence: Value,
    deadline: Instant,
}
impl FullSongRun {
    fn new(
        inputs: FullInputs,
        binaries: BinaryPins,
        verification: Box<dyn FullSongVerification>,
        directory: PathBuf,
        scan: Value,
    ) -> Check<Self> {
        let evidence = json!({"schema":1,"fixture":OPT_IN,
            "scope":"complete original Saved13 production stopped-source capture and whole-song offline export; device-free actual App callback",
            "original_project":{"path":inputs.pins.project_path,"bytes":inputs.pins.project_bytes,
                "sha256":inputs.pins.project_sha256,"state_bytes":inputs.pins.saved_state_bytes,
                "state_sha256":inputs.pins.saved_state_sha256,
                "note_counts":inputs.project.patterns.iter().map(|p|p.notes.len()).collect::<Vec<_>>(),
                "pattern_count":inputs.project.patterns.len(),"placements":inputs.project.clips.len(),
                "song_length_beats":inputs.project.song_length_beats,"tempo":inputs.project.tempo,
                "mixer_tracks":inputs.project.mixer_tracks.len(),"mixer_routes":inputs.project.mixer_routes.len()},
            "binaries":binaries.evidence(),"separate_scan_preflight":scan,
            "budgets":{"outer_driver_seconds":RUN_SECONDS,"initialization_seconds":INITIALIZATION_SECONDS,
                "cleanup_seconds":CLEANUP_SECONDS,"existing_renderer_budget_seconds":300,
                "cooperative_deadlines_are_not_hard_OS_time_guarantees":true},
            "unverified":{"physical_monitoring":true,"plugin_editor":true,"NOIRE_preset_name":true,
                "all_NOIRE_internal_parameters":true,"new_save_cold_reopen":true,
                "realtime_recording":true,"clip_conversion":true,"undo_redo":true,"Felt":true,
                "scan_and_source_helper_independent_strict_reap":true},
            "original_project_writes":0,"conversion_performed":false,"source_normalizer_used":false,
            "fixed_tail_explicitly_confirmed":true,"tail_milliseconds":3_000,
            "empty_coda_beats":[128,160],"tail_nonzero_required":false,
            "native_checkbox_or_destination_UI_exercised":false});
        let paths = FullSongEvidencePaths {
            stream: directory.join("full-native-observations.nobs"),
            states: directory.join("full-native-state-sidecar.bin"),
            wav: directory.join("complete-saved13-master.wav"),
            directory,
        };
        let observer = ObservationRun::install().map_err(error)?;
        let parent = FullParent::new_retained(inputs, [0x57; 32])?;
        Ok(Self {
            parent,
            observer,
            binaries,
            verification,
            paths,
            source_instance: None,
            source_binding: None,
            source_metadata: None,
            report: None,
            stream_attempted: false,
            evidence,
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
        require(
            ready.binding.sample_rate == 48_000,
            "complete whole-song actual source rate is not 48 kHz",
        )?;
        let source_instance = self.observer.with_snapshot(|snapshot| {
            actual_source_identity(
                &snapshot.records,
                ready.actual_worker_id,
                self.parent.source.plugin_instance_id,
            )
        })?;
        let select_prelude = |want_state: bool| {
            self.observer.with_snapshot(|snapshot| {
                snapshot
                    .records
                    .iter()
                    .filter(|record| {
                        record.instance == Some(source_instance)
                            && if want_state {
                                matches!(record.kind, RecordKind::StateLoad { .. })
                            } else {
                                matches!(record.kind, RecordKind::Metadata { .. })
                            }
                    })
                    .take(PRELUDE_OPERAND_LIMIT + 1)
                    .cloned()
                    .collect::<Vec<_>>()
            })
        };
        validate_state_operands(
            &select_prelude(true),
            source_instance,
            self.parent.pins.saved_state_bytes,
            &self.parent.pins.saved_state_sha256,
        )?;
        let metadata_records = select_prelude(false);
        validate_metadata(
            &metadata_records,
            source_instance,
            ready.binding.sample_rate,
        )?;
        self.observer.with_snapshot(|snapshot| {
            let returned = snapshot.records.iter().filter(|record| {
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
            });
            require(
                returned.count() == 1,
                "complete source lacks exactly one successful actual wrapper SourcePrepare return",
            )
        })?;
        let actual_metadata = metadata_records
            .iter()
            .rev()
            .find_map(|record| match &record.kind {
                RecordKind::Metadata {
                    result: Ok(metadata),
                    ..
                } => Some(metadata.clone()),
                _ => None,
            })
            .ok_or("complete source has no actual ready metadata")?;
        self.parent.model_unchanged()?;
        self.parent.inputs_unchanged()?;
        self.source_instance = Some(source_instance);
        self.source_metadata = Some(actual_metadata);
        // The memory prelude remains intact. The verifier must append all its
        // original records and preserve ordinal/clock/incarnation continuity.
        // A failed activation can already own a partial spool. Keep that
        // concrete actor and ask its retirement path after native cleanup.
        self.stream_attempted = true;
        self.verification.activate(
            &mut self.observer,
            source_instance,
            self.source_metadata.as_ref().unwrap(),
            &self.paths,
            self.deadline,
        )?;
        self.source_binding = Some(self.parent.begin_full_export(
            &mut self.observer,
            self.paths.wav.clone(),
            options(),
            true,
        )?);
        self.report = Some(self.parent.wait_full_export(
            &self.observer,
            self.source_binding.unwrap(),
            &self.paths.wav,
            self.deadline,
        )?);
        self.evidence["production_export"] = json!({"typed_complete_outcome":true,
            "test_render_starter_none":true,"full_project_history_dirty_saved_path_unchanged":true,
            "original_source_remains_paused":true,"WAV_path":self.paths.wav,
            "all_PCM_WAV_sound_verification_pending":true});
        Ok(())
    }
    fn cleanup(&mut self) -> Check {
        self.parent.cleanup(
            &mut self.observer,
            Instant::now() + Duration::from_secs(CLEANUP_SECONDS),
        )
    }
    fn final_receipt(&mut self, body: &Check, cleanup: &Check) -> Check {
        let resources_joined = self.parent.resources_joined();
        let retirement = if self.stream_attempted {
            self.verification.retire(
                &mut self.observer,
                resources_joined,
                Instant::now() + Duration::from_secs(CLEANUP_SECONDS),
            )
        } else {
            Err(
                "complete native stream was not activated; no stream or sidecar completion claim"
                    .into(),
            )
        };
        self.evidence["stream_retirement"] = match &retirement {
            Ok(value) => value.clone(),
            Err(reason) => json!({"passed":false,"actual_error":reason}),
        };
        let audit = if body.is_ok() && cleanup.is_ok() && retirement.is_ok() && resources_joined {
            (|| {
                self.verification.audit(FullSongAuditInputs {
                    paths: &self.paths,
                    project: &self.parent.app().project,
                    original_project_path: &self.parent.pins.project_path,
                    original_project_sha256: &self.parent.pins.project_sha256,
                    original_state_bytes: self.parent.pins.saved_state_bytes,
                    original_state_sha256: &self.parent.pins.saved_state_sha256,
                    actual_source_instance: self
                        .source_instance
                        .ok_or("full audit has no actual source incarnation")?,
                    actual_source_binding: self
                        .source_binding
                        .ok_or("full audit has no actual stopped source binding")?,
                    actual_source_metadata: self
                        .source_metadata
                        .as_ref()
                        .ok_or("full audit has no actual source metadata")?,
                    report: self
                        .report
                        .as_ref()
                        .ok_or("full audit has no production typed Complete report")?,
                })
            })()
        } else {
            Err("full sound acceptance requires actual body, native cleanup and stream/sidecar joins first".into())
        };
        self.evidence["complete_pcm_and_midi_audit"] = match &audit {
            Ok(value) => value.clone(),
            Err(reason) => json!({"passed":false,"actual_error":reason}),
        };
        let pins = self
            .binaries
            .unchanged()
            .and_then(|()| self.parent.inputs_unchanged());
        let model = self.parent.model_unchanged();
        let observer_failure = self.observer.with_snapshot(|snapshot| snapshot.failure);
        let success = body.is_ok()
            && cleanup.is_ok()
            && retirement.is_ok()
            && audit.is_ok()
            && pins.is_ok()
            && model.is_ok()
            && resources_joined
            && observer_failure.is_none();
        self.evidence["result"] = json!({"success":success,"body_passed":body.is_ok(),
            "actual_body_error":body.as_ref().err(),"cleanup_passed":cleanup.is_ok(),
            "actual_cleanup_error":cleanup.as_ref().err(),"source_resources_joined":resources_joined,
            "stream_and_sidecar_retirement_passed":retirement.is_ok(),"sound_audit_passed":audit.is_ok(),
            "original_project_and_binaries_unchanged":pins.is_ok(),"actual_pin_error":pins.as_ref().err(),
            "full_model_history_unchanged":model.is_ok(),"actual_model_error":model.as_ref().err(),
            "observer_error":observer_failure,"entire_run_quarantined":!success,
            "OS_process_absence_is_not_join_evidence":true,"partial_files_preserved":true});
        new_json(
            &self.paths.directory.join("full-song-result.json"),
            &self.evidence,
        )?;
        require(
            success,
            "complete Saved13 whole-song qualification did not satisfy every native, ownership, model and full sound gate",
        )
    }
}

thread_local! {
    static FULL_SONG_QUARANTINED: std::cell::RefCell<Option<ManuallyDrop<FullSongRun>>>
        = const { std::cell::RefCell::new(None) };
}

#[test]
#[ignore = "actual complete local NOIRE Pure Saved13 whole-song offline export; exact private pins, complete native verifier and once-only selector required"]
fn windows_noire_full_saved13_whole_song_offline() {
    require(
        std::env::var("CITRUS_NOIRE_FULL_SONG_QA_OPT_IN").as_deref() == Ok(OPT_IN),
        "explicit complete Saved13 whole-song opt-in is absent",
    )
    .unwrap();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    require(
        args == [
            SELECTOR,
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ],
        "only the exact once-only ignored complete whole-song selector is admitted",
    )
    .unwrap();
    require(
        !FULL_SONG_ADMITTED.swap(true, Ordering::AcqRel),
        "complete whole-song fixture was already admitted",
    )
    .unwrap();
    // Refuse an unwired verifier before scan, state load, or a native owner.
    let verification = native_verification().unwrap();
    let directory = absolute_env("CITRUS_NOIRE_FULL_SONG_QA_DIR").unwrap();
    fs::create_dir(&directory)
        .expect("complete whole-song directory must be new; prior evidence is never overwritten");
    let mut scan_attempted = false;
    let preflight = catch_unwind(AssertUnwindSafe(|| {
        let binaries = BinaryPins::load()?;
        let inputs = FullInputs::load_from_env()?;
        new_json(&directory.join("preflight-pins.json"), &json!({"schema":1,
            "binaries":binaries.evidence(),"project":{"path":inputs.pins.project_path,
                "sha256":inputs.pins.project_sha256,"bytes":inputs.pins.project_bytes,
                "state_bytes":inputs.pins.saved_state_bytes,"state_sha256":inputs.pins.saved_state_sha256},
            "native_source_owner_not_started":true,"original_project_writes":0}))?;
        scan_attempted = true;
        let mut scanned = plugins::scan(std::slice::from_ref(&inputs.source.descriptor.path));
        let scan = json!({"source":"production plugins::scan plus isolated default-class LoadPlugin",
            "requested_path":inputs.source.descriptor.path,"result_count":scanned.len(),"results":scanned,
            "scope":"separate empty scan instance before source observer installation",
            "source_worker_identity":Value::Null,"legacy_scan_Drop_is_not_strict_helper_reap":true});
        new_json(&directory.join("separate-scan-preflight.json"), &scan)?;
        require(scanned.len() == 1, "production scan did not return exactly one complete source descriptor")?;
        let descriptor = scanned.pop().ok_or("production scan descriptor disappeared")?;
        binaries.unchanged()?;
        Ok((inputs, binaries, descriptor, scan))
    }))
    .unwrap_or_else(|payload| Err(panic_error(payload, "complete whole-song preflight")));
    let (inputs, binaries, descriptor, scan) = preflight.unwrap_or_else(|reason| {
        let _ = new_json(&directory.join("full-song-result.json"), &json!({"schema":1,
            "stage":"preflight","success":false,"actual_error":reason,"source_owner_started":false,
            "separate_scan_attempted":scan_attempted,"scan_cleanup_not_strictly_proven":scan_attempted,
            "partial_files_preserved":true}));
        panic!("complete NOIRE whole-song preflight failed; preserve {}", directory.display());
    });
    let constructed = catch_unwind(AssertUnwindSafe(|| {
        FullSongRun::new(inputs, binaries, verification, directory.clone(), scan)
    }))
    .unwrap_or_else(|payload| {
        Err(panic_error(
            payload,
            "complete whole-song owner construction",
        ))
    });
    let mut run = ManuallyDrop::new(constructed.unwrap_or_else(|reason| {
        let _ = new_json(
            &directory.join("full-song-result.json"),
            &json!({"schema":1,
            "stage":"owner-construction","success":false,"actual_error":reason,
            "source_initialization_requested":false,"partial_files_preserved":true}),
        );
        panic!(
            "complete NOIRE whole-song owner construction failed; preserve {}",
            directory.display()
        );
    }));
    let body =
        catch_unwind(AssertUnwindSafe(|| run.exercise(&descriptor))).unwrap_or_else(|payload| {
            Err(panic_error(
                payload,
                "complete production whole-song export",
            ))
        });
    let cleanup = catch_unwind(AssertUnwindSafe(|| run.cleanup()))
        .unwrap_or_else(|payload| Err(panic_error(payload, "complete actual native cleanup")));
    let receipt = catch_unwind(AssertUnwindSafe(|| run.final_receipt(&body, &cleanup)))
        .unwrap_or_else(|payload| {
            Err(panic_error(
                payload,
                "complete stream retirement and full sound audit",
            ))
        });
    let success = body.is_ok() && cleanup.is_ok() && receipt.is_ok();
    if receipt.is_err() {
        let _ = new_json(
            &directory.join("result-writer-or-audit-failure.json"),
            &json!({"schema":1,
            "success":false,"body_error":body.as_ref().err(),"cleanup_error":cleanup.as_ref().err(),
            "receipt_error":receipt.as_ref().err(),"entire_run_quarantined":true,"partial_files_preserved":true}),
        );
    }
    if success {
        // Both the concrete verifier and parent have completed actual joins.
        unsafe { ManuallyDrop::drop(&mut run) };
    } else {
        let mut pending = Some(run);
        let _ = FULL_SONG_QUARANTINED.try_with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut()
                && slot.is_none()
            {
                *slot = pending.take();
            }
        });
    }
    assert!(
        success,
        "complete NOIRE whole-song qualification failed; preserve {}",
        directory.display()
    );
}
