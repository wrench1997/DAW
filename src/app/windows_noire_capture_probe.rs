//! Explicit same-source capture diagnostic. No render, insertion, UI or audio device.
//! The three receipts use production capture admission and worker termination ACKs.
use super::*;
use crate::instrument_bounce::InstrumentBounceCapture;

const PROBE_SELECTOR: &str = "app::clip_render_job::windows_noire_bounce_qa::windows_noire_capture_probe::windows_noire_stopped_same_source_three_captures";
const PROBE_OPT_IN: &str = "actual-noire-stopped-same-source-three-captures-v1";
const PROBE_CAPTURE_COUNT: usize = 3;
const PROBE_STATE_TOTAL_LIMIT: usize = 8 * 1024 * 1024;
const PROBE_TRACE_LIMIT: usize = 64;

struct ProbeOwner {
    owner: Owner,
    capture: Option<InstrumentBounceCapture>,
    capture_identity: Option<BounceSourceIdentity>,
    // Retain every real receipt token, so pointer reuse cannot imitate distinct receipts.
    captures: Vec<CapturedInstrumentBounce>,
    rows: Vec<Value>,
}

fn expected_hash(actual: &str, expected: &str) -> Check {
    require(
        expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "the explicit executable/helper SHA256 pin is not 64 hexadecimal digits",
    )?;
    require(
        actual.eq_ignore_ascii_case(expected),
        "the actual executable/helper differs from the explicit probe SHA256 pin",
    )
}

fn probe_input_pins(inputs: &Inputs) -> Check {
    expected_hash(
        &inputs.executable_hash,
        &std::env::var("CITRUS_NOIRE_CAPTURE_PROBE_EXE_SHA256").map_err(error)?,
    )?;
    expected_hash(
        &inputs.helper_hash,
        &std::env::var("CITRUS_NOIRE_CAPTURE_PROBE_HELPER_SHA256").map_err(error)?,
    )
}

/// An attempt is evidence of a real host call boundary, not proof that SDK Process succeeded.
fn positive_calls(records: &[Arc<observe::Record>], start: u64, end: u64) -> Value {
    let calls = records
        .iter()
        .filter(|record| {
            record.ordinal > start
                && record.ordinal < end
                && record
                    .instance
                    .is_some_and(|instance| instance.phase == InstancePhase::InitialSource)
                && matches!(&record.kind, RecordKind::Call {
                operation: Operation::OrdinaryProcess | Operation::ResetOrigin,
                frames: Some(frames), ..
            } if *frames > 0)
        })
        .collect::<Vec<_>>();
    let attempt_count = calls
        .iter()
        .filter(|record| {
            matches!(
                &record.kind,
                RecordKind::Call {
                    boundary: Boundary::Attempt,
                    ..
                }
            )
        })
        .count();
    let returned_ok_count = calls
        .iter()
        .filter(|record| {
            matches!(
                &record.kind,
                RecordKind::Call {
                    boundary: Boundary::Returned,
                    outcome: Some(observe::CallOutcome::Ok),
                    ..
                }
            )
        })
        .count();
    let rows = calls
        .iter()
        .take(PROBE_TRACE_LIMIT)
        .filter_map(|record| {
            let RecordKind::Call {
                operation,
                boundary,
                frames,
                transport,
                outcome,
            } = &record.kind
            else {
                return None;
            };
            let instance = record
                .instance
                .expect("filtered actual original-source identity");
            Some(
                json!({"ordinal":record.ordinal,"monotonic_elapsed_ns":record.monotonic_elapsed_ns,
            "instance_ordinal":instance.ordinal,"worker_id":instance.worker_id,
            "source_instance_id":instance.source_instance_id,"operation":format!("{operation:?}"),
            "boundary":format!("{boundary:?}"),"frames":frames,"epoch":null,
            "transport_playing":transport.map(|transport|transport.playing),
            "sample_position":transport.map(|transport|transport.sample_position),
            "returned_ok":matches!(outcome,Some(observe::CallOutcome::Ok))}),
            )
        })
        .collect::<Vec<_>>();
    json!({"start_ordinal_exclusive":start,"end_ordinal_exclusive":end,
        "positive_attempt_count":attempt_count,"returned_ok_count":returned_ok_count,
        "row_limit":PROBE_TRACE_LIMIT,"row_count":rows.len(),"truncated":calls.len()>rows.len(),
        "rows":rows,"attempt_is_not_sdk_success":true,"epoch_absent_from_call_record":true})
}

impl ProbeOwner {
    fn new(owner: Owner) -> Self {
        Self {
            owner,
            capture: None,
            capture_identity: None,
            captures: Vec::with_capacity(PROBE_CAPTURE_COUNT),
            rows: Vec::with_capacity(PROBE_CAPTURE_COUNT),
        }
    }

    fn settled_without_pump(&self, binding: LiveBinding, deadline: Instant) -> Check<Value> {
        loop {
            require(
                Instant::now() < deadline,
                "actual stopped source settlement deadline exceeded",
            )?;
            let app = self.owner.app();
            require(
                app.stopped_bounce_observe(binding.stamp.channel_id)? == binding,
                "the exact source binding changed during the probe",
            )?;
            let audio = app
                .audio
                .as_ref()
                .ok_or("the retained device-free audio engine is absent")?;
            require(
                !app.playing && !audio.snapshot().transport_playing,
                "the actual source transport is no longer stopped",
            )?;
            let target = BounceSourceCallbackIdentity {
                engine_session: binding.stamp.engine_session,
                project_session: binding.stamp.project_session,
                timeline_generation: binding.stamp.timeline_revision,
                epoch: binding.stamp.epoch,
                transport_request: audio.transport_request_id(),
                channel_id: binding.stamp.channel_id,
                endpoint_id: binding.stamp.endpoint_id,
                instance_id: binding.stamp.instance_id,
            };
            let source = app
                .running_generator_chains
                .get(&binding.stamp.channel_id)
                .ok_or("the actual original source is absent")?;
            let observed = source.control.observe_bounce_source_settlement(target);
            match observed.settlement {
                BounceSourceSettlement::Invalid => {
                    return Err("the actual source settlement became invalid".into());
                }
                BounceSourceSettlement::Ready if self.owner.paused() => {
                    return Ok(json!({
                    "actual_ready":true,"coherent":observed.coherent,"callback_ready":observed.callback_ready,
                    "callback_identity_matches_target":observed.callback_identity==Some(target),
                    "worker_valid":observed.worker_valid,"exhausted":observed.exhausted,
                    "submitted":observed.submitted,"consumed":observed.consumed,"terminal":observed.terminal,
                    "worker_busy":observed.busy,"requested_epoch":observed.requested_epoch,
                    "reset_completed_epoch":observed.reset_completed_epoch,"capture_busy":observed.capture_busy,
                    "parameter_admission":observed.parameter_admission,
                    "transport_request":target.transport_request}));
                }
                BounceSourceSettlement::Waiting | BounceSourceSettlement::Ready => {
                    std::thread::sleep(Duration::from_millis(1))
                }
            }
        }
    }

    fn exercise(&mut self, candidate: Project, descriptor: PluginDescriptor) -> Check {
        self.owner.initialize_source(candidate, descriptor)?;
        require(
            self.owner.app().clip_render_job.is_none(),
            "the diagnostic must not own a conversion job",
        )?;
        let channel = self.owner.app().project.channels[0].id;
        let binding = self.owner.app().stopped_bounce_observe(channel)?;
        let original = clip_render::project_snapshot(&self.owner.app().project).map_err(error)?;
        let original_pumps = self.owner.pumps;
        let original_callback_segments = self.owner.callback_segments;
        let baseline_ordinal = self
            .owner
            .observer
            .with_snapshot(|snapshot| snapshot.records.last().map_or(0, |record| record.ordinal));
        let mut total_state_bytes = 0_usize;
        for index in 0..PROBE_CAPTURE_COUNT {
            let deadline = self
                .owner
                .deadline
                .min(Instant::now() + crate::instrument_bounce::BOUNCE_CAPTURE_TIMEOUT);
            let ready = self.settled_without_pump(binding, deadline)?;
            require(
                clip_render::project_matches_snapshot(&self.owner.app().project, &original)
                    .map_err(error)?,
                "the scratch Project changed between same-source captures",
            )?;
            let plan = InstrumentBouncePlan::prepare(
                &self.owner.app().project,
                channel,
                binding.sample_rate,
                0,
            )?;
            let identity = BounceSourceIdentity {
                project_session: binding.stamp.project_session,
                project_fingerprint: plan.project_fingerprint(),
                timeline_revision: binding.stamp.timeline_revision,
                epoch: binding.stamp.epoch,
                endpoint_id: binding.stamp.endpoint_id,
                channel_id: channel,
                instance_id: binding.stamp.instance_id,
            };
            let app = self.owner.app();
            let source = app
                .running_generator_chains
                .get(&channel)
                .ok_or("the original source disappeared")?;
            self.capture = Some(InstrumentBounceCapture::begin(
                plan,
                identity,
                app.stopped_bounce_status(binding),
                &source.control,
                Instant::now(),
            )?);
            self.capture_identity = Some(identity);
            let captured = loop {
                require(
                    Instant::now() < deadline,
                    "the actual private capture remains pending at the probe deadline",
                )?;
                let status = self.owner.app().stopped_bounce_status(binding);
                let capture = self
                    .capture
                    .as_mut()
                    .expect("retained actual capture receiver");
                if !capture.termination_acknowledged() {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                // The ACK above is the actual worker release publication, never synthesized.
                match capture
                    .poll_typed(identity, status, Instant::now())
                    .map_err(error)?
                {
                    Some(captured) => break captured,
                    None => std::thread::sleep(Duration::from_millis(1)),
                }
            };
            self.capture = None;
            self.capture_identity = None;
            total_state_bytes = total_state_bytes
                .checked_add(captured.state.opaque_state.len())
                .ok_or("probe retained state accounting overflow")?;
            // This QA-local limit is narrower than the unchanged production 64 MiB/state cap.
            require(
                total_state_bytes <= PROBE_STATE_TOTAL_LIMIT,
                "probe raw state total exceeds its 8 MiB diagnostic bound",
            )?;
            require(
                captured.state.metadata.class_uid == KONTAKT_UID
                    && captured.state.metadata.version == "8.4.1",
                "the captured actual class/version differs from pinned Kontakt",
            )?;
            let receipt = captured.genuine_receipt_identity();
            require(
                self.captures
                    .iter()
                    .all(|previous| previous.genuine_receipt_identity() != receipt),
                "actual completed capture receipt identities were conflated",
            )?;
            observe::record_capture(
                if index == 0 {
                    CaptureLabel::Initial
                } else {
                    CaptureLabel::Final
                },
                &captured,
            );
            let capture_ordinal = self
                .owner
                .observer
                .with_snapshot(|snapshot| {
                    snapshot
                        .records
                        .iter()
                        .find_map(|record| match &record.kind {
                            RecordKind::Capture {
                                receipt_identity, ..
                            } if *receipt_identity == receipt => Some(record.ordinal),
                            _ => None,
                        })
                })
                .ok_or("the actual completed capture observation is absent")?;
            self.rows.push(json!({"probe_index":index+1,"label":format!("A{}",index+1),
                "ordinal":capture_ordinal,"actual_capture_termination_acknowledged":true,
                "receipt_identity":receipt,"source_identity":{"project_session":identity.project_session,
                    "project_fingerprint":identity.project_fingerprint,"timeline_revision":identity.timeline_revision,
                    "epoch":identity.epoch,"endpoint_id":identity.endpoint_id,"channel_id":identity.channel_id,
                    "instance_id":identity.instance_id},"actual_settlement_before_capture":ready,
                "native_generation":captured.state.native_generation,"native_dirty_revision":captured.state.native_dirty_revision,
                "metadata":metadata_json(&captured.state.metadata),"generic_base_count":captured.state.parameter_bases.len(),
                "opaque_envelope":capture_envelope_diagnostics(&captured.state.opaque_state)?}));
            self.captures.push(captured);
        }
        self.owner.evidence["settlement_after_consecutive_captures"] =
            self.settled_without_pump(binding, self.owner.deadline)?;
        require(
            self.owner.pumps == original_pumps
                && self.owner.callback_segments == original_callback_segments,
            "the diagnostic pumped a callback during consecutive captures",
        )?;
        let records = self
            .owner
            .observer
            .with_snapshot(|snapshot| snapshot.records.clone());
        let final_ordinal = self
            .rows
            .last()
            .and_then(|row| row["ordinal"].as_u64())
            .ok_or("final capture ordinal absent")?;
        let calls = positive_calls(&records, baseline_ordinal, final_ordinal);
        self.owner.evidence["consecutive_capture_source_calls"] = calls.clone();
        require(
            calls["positive_attempt_count"] == 0 && calls["returned_ok_count"] == 0,
            "actual source positive Process/ResetOrigin calls occurred during the no-pump probe",
        )?;
        require(
            clip_render::project_matches_snapshot(&self.owner.app().project, &original)
                .map_err(error)?
                && self.owner.app().undo_stack.is_empty()
                && self.owner.app().redo_stack.is_empty()
                && !self.owner.app().dirty
                && self.owner.paused(),
            "the diagnostic changed Project/history or left stopped source work unsettled",
        )?;
        self.owner.evidence["probe"] = json!({"capture_count":self.captures.len(),"rows":self.rows,
            "scope":"consecutive captures after actual source initialization and input settlement",
            "raw_state_total_bytes":total_state_bytes,"raw_state_total_limit":PROBE_STATE_TOTAL_LIMIT,
            "callback_pumps_during_consecutive_captures":0,"callback_segments_during_consecutive_captures":0,
            "scratch_project_and_history_unchanged":true,"render_started":false,"insertion_started":false,
            "native_editor_opened":false,"audio_device_opened":false,
            "sdk_suspend_not_requested_during_consecutive_captures":true,
            "capture_lease_released_at_each_actual_termination_ack":true,
            "source_feedback_polling_and_plugin_background_threads_may_remain_active":true,
            "state_byte_equality_is_diagnostic_only":true,"all_ni_internal_parameters_restoration_proven":false});
        Ok(())
    }

    fn cleanup(&mut self) -> Check {
        if let Some(capture) = &mut self.capture {
            capture.cancel();
        }
        let deadline = Instant::now() + crate::instrument_bounce::BOUNCE_CAPTURE_TIMEOUT;
        while let Some(capture) = &mut self.capture {
            require(
                Instant::now() < deadline,
                "cancelled probe capture has not acknowledged actual worker termination",
            )?;
            let identity = self
                .capture_identity
                .ok_or("retained capture lost its exact identity")?;
            let result =
                capture.poll_typed(identity, BounceSourceStatus::default(), Instant::now());
            if capture.termination_acknowledged() && !matches!(result, Ok(None)) {
                self.capture = None;
                self.capture_identity = None;
            } else {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        self.owner.cleanup()
    }

    fn final_receipt(&mut self, body: &Check, cleanup: &Check) -> Check {
        let records = self
            .owner
            .observer
            .with_snapshot(|snapshot| snapshot.records.clone());
        self.owner.evidence["actual_source_captures"] = capture_diagnostics(&records);
        let retained = retain_capture_bytes(&self.owner.inputs.directory, &records);
        let retention = require(
            retained["diagnostic_complete"] == true,
            "raw capture retention/geometry/diff incomplete; preserve detailed diagnostic errors and partial files",
        );
        self.owner.evidence["raw_capture_retention"] = retained;
        let mut pairs = Vec::new();
        for index in 1..self.captures.len() {
            let before = &self.captures[index - 1];
            let after = &self.captures[index];
            pairs.push(json!({"before_probe_index":index,"after_probe_index":index+1,
                "actual_positive_source_calls":positive_calls(&records,
                    self.rows[index-1]["ordinal"].as_u64().ok_or("first actual pair ordinal absent")?,
                    self.rows[index]["ordinal"].as_u64().ok_or("second actual pair ordinal absent")?),
                "same_source_identity":before.identity()==after.identity(),
                "same_metadata":before.state.metadata==after.state.metadata,
                "same_native_generation":before.state.native_generation==after.state.native_generation,
                "same_native_dirty_revision":before.state.native_dirty_revision==after.state.native_dirty_revision,
                "same_generic_base_bits":before.state.parameter_bases.len()==after.state.parameter_bases.len()
                    && before.state.parameter_bases.iter().zip(&after.state.parameter_bases)
                        .all(|((id,value),(other_id,other))|id==other_id && value.to_bits()==other.to_bits()),
                "opaque_diff":opaque_byte_diff(&before.state.opaque_state,&after.state.opaque_state)?}));
        }
        self.owner.evidence["capture_pairs"] = json!(pairs);
        // Preserve partial completed capture rows even if a later capture failed.
        self.owner.evidence["completed_capture_rows"] = json!(self.rows);
        let terminal = TerminalFacts::from_records(&records);
        let terminal_result = terminal.qualify(false).and_then(|()| {
            require(
                terminal.clone_instances == 0
                    && terminal.source_instances.len() == 1
                    && terminal.source_instances[0].0 == InstancePhase::InitialSource,
                "diagnostic cleanup constructed another source or a render clone",
            )
        });
        let unchanged = self.owner.inputs.unchanged();
        let observation = self.owner.observer.with_snapshot(|snapshot| {
            require(
                snapshot.failure.is_none(),
                snapshot.failure.unwrap_or("observer failure"),
            )
        });
        let success = body.is_ok()
            && cleanup.is_ok()
            && terminal_result.is_ok()
            && unchanged.is_ok()
            && observation.is_ok()
            && retention.is_ok();
        self.owner.evidence["result"] = json!({"success":success,"diagnostic_completed":body.is_ok(),
            "body_error":body.as_ref().err(),"cleanup_terminal":cleanup.is_ok(),"cleanup_error":cleanup.as_ref().err(),
            "terminal_evidence":terminal.evidence(),"terminal_evidence_error":terminal_result.as_ref().err(),
            "original_input_and_binaries_unchanged":unchanged.is_ok(),"pin_error":unchanged.as_ref().err(),
            "observer_error":observation.as_ref().err(),"partial_files_preserved":true,
            "raw_capture_retention_complete":retention.is_ok(),"raw_capture_retention_error":retention.as_ref().err(),
            "state_bytes_equal_is_not_required_for_diagnostic_success":true,"conversion_qualified":false,
            "whole_song_export_qualified":false,"sound_auditioned":false});
        new_json(
            &self.owner.inputs.directory.join("result.json"),
            &self.owner.evidence,
        )?;
        terminal_result?;
        unchanged?;
        observation?;
        retention
    }
}

thread_local! {
    static PROBE_QUARANTINED: std::cell::RefCell<Option<ManuallyDrop<ProbeOwner>>> = const { std::cell::RefCell::new(None) };
}

#[test]
#[ignore = "actual same-source NOIRE capture diagnostic; explicit private directory, SHA pins and root-only launch required"]
fn windows_noire_stopped_same_source_three_captures() {
    require(
        std::env::var("CITRUS_NOIRE_CAPTURE_PROBE_OPT_IN").as_deref() == Ok(PROBE_OPT_IN),
        "explicit stopped-source capture probe opt-in is absent",
    )
    .unwrap();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    require(
        args == [
            PROBE_SELECTOR,
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture",
        ],
        "only the exact once-only ignored probe selector is admitted",
    )
    .unwrap();
    require(
        !ADMITTED.swap(true, Ordering::AcqRel),
        "actual NOIRE fixture already admitted in this process",
    )
    .unwrap();
    let directory = absolute_env("CITRUS_NOIRE_CAPTURE_PROBE_DIR").unwrap();
    fs::create_dir(&directory)
        .expect("PROBE_DIR must be a new private directory; never overwrite evidence");
    let loaded=catch_unwind(AssertUnwindSafe(|| {
        let (inputs,project,descriptor,mut evidence)=Inputs::load(directory.clone())?;
        probe_input_pins(&inputs)?;
        evidence["fixture"]=json!(PROBE_OPT_IN);
        evidence["scope"]=json!("actual device-free paused same-source A1/A2/A3 private captures; no render/insertion/UI/listening or state canonicalization");
        evidence["selector"]=json!(PROBE_SELECTOR);
        evidence["source_input_pin"]=json!(SOURCE_SHA256);
        Ok((inputs,project,descriptor,evidence))
    })).unwrap_or_else(|payload|Err(panic_error(payload,"probe preflight")));
    let (inputs, project, descriptor, evidence) = loaded.unwrap_or_else(|reason| {
        new_json(
            &directory.join("result.json"),
            &json!({"schema":1,"stage":"probe-preflight",
            "success":false,"plugin_started":false,"error":reason,"partial_files_preserved":true}),
        )
        .unwrap();
        panic!(
            "NOIRE capture probe preflight failed; preserve {}",
            directory.display()
        )
    });
    let initialized = catch_unwind(AssertUnwindSafe(|| Owner::new(inputs, evidence)))
        .unwrap_or_else(|payload| {
            Err(panic_error(
                payload,
                "probe owner construction before plugin admission",
            ))
        });
    let mut probe = ManuallyDrop::new(ProbeOwner::new(initialized.unwrap_or_else(|reason| {
        new_json(
            &directory.join("result.json"),
            &json!({"schema":1,"stage":"probe-owner-construction",
            "success":false,"plugin_started":false,"error":reason}),
        )
        .unwrap();
        panic!(
            "NOIRE capture probe owner construction failed; preserve {}",
            directory.display()
        )
    })));
    let body = catch_unwind(AssertUnwindSafe(|| probe.exercise(project, descriptor)))
        .unwrap_or_else(|payload| Err(panic_error(payload, "actual same-source captures")));
    let cleanup = catch_unwind(AssertUnwindSafe(|| probe.cleanup())).unwrap_or_else(|payload| {
        Err(panic_error(
            payload,
            "probe cleanup; exact owners quarantined",
        ))
    });
    let receipt=catch_unwind(AssertUnwindSafe(||probe.final_receipt(&body,&cleanup)))
        .unwrap_or_else(|payload| {
            let reason=panic_error(payload,"probe final evidence writer");
            let _=new_json(&directory.join("result-writer-failure.json"),&json!({"schema":1,"success":false,
                "error":reason,"body_error":body.as_ref().err(),"cleanup_error":cleanup.as_ref().err(),
                "owners_quarantined":cleanup.is_err(),"partial_files_preserved":true}));
            Err(reason)
        });
    let success = body.is_ok() && cleanup.is_ok() && receipt.is_ok();
    if cleanup.is_ok() {
        unsafe {
            ManuallyDrop::drop(&mut probe);
        }
    } else {
        PROBE_QUARANTINED.with(|quarantine| *quarantine.borrow_mut() = Some(probe));
    }
    assert!(
        success,
        "actual NOIRE capture probe failed; preserve {}",
        directory.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn executable_and_helper_pins_reject_missing_malformed_or_different_hashes() {
        let actual = "a".repeat(64);
        for expected in [
            String::new(),
            "a".repeat(63),
            "g".repeat(64),
            "b".repeat(64),
        ] {
            assert!(expected_hash(&actual, &expected).is_err());
        }
        assert!(expected_hash(&actual, &"A".repeat(64)).is_ok());
    }
    #[test]
    fn positive_call_windows_distinguish_attempts_returns_zero_samples_and_clone_calls() {
        let call = |ordinal, phase, operation, boundary, frames, outcome| {
            Arc::new(observe::Record {
                ordinal,
                monotonic_elapsed_ns: ordinal,
                stage: Stage::InitialSource,
                instance: Some(observe::InstanceIdentity {
                    ordinal: 1,
                    phase,
                    worker_id: Some(2),
                    source_instance_id: Some(3),
                }),
                kind: RecordKind::Call {
                    operation,
                    boundary,
                    frames: Some(frames),
                    transport: None,
                    outcome,
                },
            })
        };
        let records = vec![
            call(
                10,
                InstancePhase::InitialSource,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            call(
                11,
                InstancePhase::InitialSource,
                Operation::OrdinaryProcess,
                Boundary::Returned,
                128,
                Some(observe::CallOutcome::Ok),
            ),
            call(
                12,
                InstancePhase::InitialSource,
                Operation::ResetOrigin,
                Boundary::Attempt,
                64,
                None,
            ),
            call(
                13,
                InstancePhase::InitialSource,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                0,
                None,
            ),
            call(
                14,
                InstancePhase::StrictClone,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
            call(
                15,
                InstancePhase::InitialSource,
                Operation::OrdinaryProcess,
                Boundary::Attempt,
                128,
                None,
            ),
        ];
        let result = positive_calls(&records, 9, 15);
        assert_eq!(result["positive_attempt_count"], 2);
        assert_eq!(result["returned_ok_count"], 1);
        assert_eq!(result["row_count"], 3);
        assert_eq!(result["rows"][0]["epoch"], Value::Null);
        assert_eq!(
            positive_calls(&records, 12, 15)["positive_attempt_count"],
            0
        );
    }
}
