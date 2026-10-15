#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument_bounce::BounceAudioBus;
    use crate::model::PluginInstance;

    fn saved() -> Project {
        let mut project = Project::blank();
        project.channels[0].instrument_plugin_instance_id = Some(17);
        project.channels[0].volume = 0.55;
        project.plugin_instances.push(PluginInstance {
            id: 17,
            format: ModelPluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: std::env::temp_dir().join("synthetic-short-source.vst3"),
            uid: KONTAKT_UID.into(),
            vendor: "synthetic".into(),
            name: "synthetic short source".into(),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: BTreeMap::from([(7, 0.31)]),
            opaque_state: vec![11, 22, 33],
            runtime_status: PluginRuntimeStatus::Loaded,
            midi_ports: Default::default(),
        });
        project.clips = Project::default()
            .clips
            .into_iter()
            .filter(|clip| clip.kind == ClipKind::Pattern)
            .take(1)
            .collect();
        project
    }

    fn identity() -> observe::InstanceIdentity {
        observe::InstanceIdentity {
            ordinal: 1,
            phase: InstancePhase::InitialSource,
            worker_id: Some(31),
            source_instance_id: Some(17),
        }
    }
    fn metadata() -> BouncePluginMetadata {
        BouncePluginMetadata {
            class_uid: KONTAKT_UID.into(),
            version: "8.4.1".into(),
            name: "synthetic actual-return fixture".into(),
            category: "Instrument|Sampler".into(),
            input_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            output_buses: (0..32)
                .map(|index| BounceAudioBus::new(2, true, Some(index == 0)))
                .collect(),
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![(16, true, Some(true))],
            latency_samples: 0,
            tail_samples: u32::MAX,
        }
    }
    fn metadata_record(
        identity: observe::InstanceIdentity,
        metadata: BouncePluginMetadata,
    ) -> Arc<observe::Record> {
        Arc::new(observe::Record {
            ordinal: 1,
            monotonic_elapsed_ns: 0,
            stage: Stage::InitialSource,
            instance: Some(identity),
            kind: RecordKind::Metadata {
                site: "synthetic returned metadata",
                result: Ok(metadata),
            },
        })
    }

    #[test]
    fn sdk_b_short_projection_preserves_saved_state_generic_map_and_original_model() {
        let saved = saved();
        let before = serde_json::to_vec(&saved).unwrap();
        let projected = short_project(&saved, 17, 1).unwrap();
        assert_eq!(serde_json::to_vec(&saved).unwrap(), before);
        assert_eq!(
            projected.plugin_instances[0].opaque_state,
            saved.plugin_instances[0].opaque_state
        );
        assert_eq!(
            projected.plugin_instances[0].parameters,
            saved.plugin_instances[0].parameters
        );
        assert_eq!(
            projected.plugin_instances[0].midi_ports,
            saved.plugin_instances[0].midi_ports
        );
        assert_eq!(projected.song_length_beats, 2.0);
        assert_eq!(projected.tempo, 84.0);
        assert_eq!(
            projected
                .patterns
                .iter()
                .map(|p| p.notes[0].note)
                .collect::<Vec<_>>(),
            [60, 64]
        );
        assert_eq!(
            projected
                .clips
                .iter()
                .map(|c| (c.start, c.length))
                .collect::<Vec<_>>(),
            [(0.0, 1.0), (1.0, 1.0)]
        );
        assert_eq!(
            projected.automation_lanes[0]
                .lane
                .points()
                .iter()
                .map(|p| (p.position, p.value))
                .collect::<Vec<_>>(),
            [(0.0, 0.85), (2.0, 0.35)]
        );
        assert!(
            plugins::plugin_load_identity_from_instance(&projected.plugin_instances[0])
                .descriptor
                .vst3_metadata
                .is_none()
        );
    }

    #[test]
    fn sdk_b_short_source_identity_needs_this_incarnation_actual_metadata() {
        let id = identity();
        metadata()
            .validate(RATE as u32)
            .expect("synthetic valid fixture must satisfy the unchanged real metadata policy");
        assert!(actual_metadata(&[], id).is_err());
        let actual = actual_metadata(&[metadata_record(identity(), metadata())], id);
        assert!(actual.is_ok(), "{actual:?}");
        let foreign = observe::InstanceIdentity {
            worker_id: Some(32),
            ..id
        };
        assert!(actual_metadata(&[metadata_record(foreign, metadata())], id).is_err());
        for case in 0..5 {
            let mut bad = metadata();
            match case {
                0 => bad.class_uid = "A".repeat(32),
                1 => bad.version = "other".into(),
                2 => bad.category = "NotInstrument".into(),
                3 => bad.event_input_buses[0].2 = Some(false),
                _ => bad.input_buses[0].is_main = false,
            }
            assert!(actual_metadata(&[metadata_record(id, bad)], id).is_err());
        }
    }

    #[test]
    fn sdk_b_short_onset_keeps_original_frame_origin_and_frozen_threshold() {
        let mut bits = vec![0.0_f32.to_bits(); 128 * 2];
        bits[17 * 2] = 1.0e-5_f32.to_bits(); // Equal is not above the frozen threshold.
        bits[18 * 2 + 1] = (-2.0e-5_f32).to_bits();
        let onset = onset_metrics(&bits).unwrap();
        assert_eq!(onset["first_nonzero_frame"], 17);
        assert_eq!(onset["first_above_threshold_frame"], 18);
        assert_eq!(onset["alignment_fitted"], false);
        assert_eq!(onset["extra_trim_or_delay"], false);
        for frame_zero in [[2.0e-5_f32, 0.0], [0.0, -2.0e-5_f32]] {
            let onset = onset_metrics(&frame_zero.map(f32::to_bits)).unwrap();
            assert_eq!(onset["first_nonzero_frame"], 0);
            assert_eq!(onset["first_above_threshold_frame"], 0);
            assert_eq!(onset["frames"], 1);
        }
        assert_eq!(
            onset_metrics(&[0; 256]).unwrap()["first_above_threshold_frame"],
            Value::Null
        );
    }

    #[test]
    fn sdk_b_short_onset_refuses_incomplete_or_nonfinite_audio() {
        for bits in [
            vec![],
            vec![0],
            vec![0, 0, 1.0_f32.to_bits()], // A complete frame plus an odd tail is not truncated.
            vec![f32::NAN.to_bits(), 0],
            vec![0, f32::INFINITY.to_bits()],
        ] {
            assert!(onset_metrics(&bits).is_err());
        }
    }

    #[test]
    fn sdk_b_short_initial_failure_diagnostics_keep_actual_source_errors_with_bounded_whitelist() {
        let mut records = (1..=75)
            .map(|ordinal| {
                Arc::new(observe::Record {
                    ordinal,
                    monotonic_elapsed_ns: 0,
                    stage: Stage::InitialSource,
                    instance: Some(identity()),
                    kind: if ordinal == 5 {
                        RecordKind::StateLoad {
                            bytes: vec![0x51; 127],
                            outcome: observe::CallOutcome::Ok,
                        }
                    } else {
                        RecordKind::Lifecycle("successful prelude before a later source failure")
                    },
                })
            })
            .collect::<Vec<_>>();
        // All four real error forms arrive after the unchanged first-64 prelude.
        for (index, operation) in [Operation::SourceLoad, Operation::SourcePrepare]
            .into_iter()
            .enumerate()
        {
            records.push(Arc::new(observe::Record {
                ordinal: index as u64 + 76,
                monotonic_elapsed_ns: 0,
                stage: Stage::InitialSource,
                instance: Some(identity()),
                kind: RecordKind::Call {
                    operation,
                    boundary: Boundary::Returned,
                    frames: None,
                    transport: None,
                    outcome: Some(observe::CallOutcome::Error(format!(
                        "original {operation:?} failure"
                    ))),
                },
            }));
        }
        records.push(Arc::new(observe::Record {
            ordinal: 78,
            monotonic_elapsed_ns: 0,
            stage: Stage::InitialSource,
            instance: Some(identity()),
            kind: RecordKind::Metadata {
                site: "actual failed source metadata",
                result: Err("original metadata failure".into()),
            },
        }));
        records.push(Arc::new(observe::Record {
            ordinal: 79,
            monotonic_elapsed_ns: 0,
            stage: Stage::InitialSource,
            instance: None,
            kind: RecordKind::RuntimeEvent {
                endpoint_id: 19,
                project_session: 21,
                instance_id: 17,
                event: crate::plugins::plugin_runtime::RuntimeEvent::SlotFault {
                    slot: 0,
                    message: "original SlotFault failure".into(),
                },
            },
        }));
        records.push(Arc::new(observe::Record {
            ordinal: 80,
            monotonic_elapsed_ns: 0,
            stage: Stage::WholeExport,
            instance: Some(observe::InstanceIdentity {
                phase: InstancePhase::StrictClone,
                ..identity()
            }),
            kind: RecordKind::Metadata {
                site: "foreign strict record must not be included",
                result: Err("foreign strict failure must not be included".into()),
            },
        }));
        let diagnostics = initial_source_diagnostics(&records);
        assert_eq!(diagnostics["matched_records"], 79);
        assert_eq!(diagnostics["row_limit"], 64);
        assert_eq!(diagnostics["records"].as_array().unwrap().len(), 64);
        assert_eq!(diagnostics["truncated"], true);
        assert_eq!(diagnostics["raw_state_or_pcm_serialized"], false);
        let failures = &diagnostics["latest_actual_failures"];
        assert_eq!(failures["matched_records"], 4);
        assert_eq!(failures["row_limit"], 8);
        assert_eq!(failures["truncated"], false);
        assert_eq!(failures["records"].as_array().unwrap().len(), 4);
        assert_eq!(
            failures["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|record| record["ordinal"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [76, 77, 78, 79]
        );
        let head = diagnostics["records"].to_string();
        let latest = failures.to_string();
        for original in [
            "original SourceLoad failure",
            "original SourcePrepare failure",
            "original metadata failure",
            "original SlotFault failure",
        ] {
            assert!(
                !head.contains(original),
                "failure must be beyond the head: {original}"
            );
            assert!(latest.contains(original), "{original}");
        }
        assert!(!diagnostics.to_string().contains("foreign strict"));
        assert_eq!(diagnostics["records"][4]["detail"]["byte_len"], 127);
        assert!(diagnostics["records"][4]["detail"].get("bytes").is_none());

        // More than eight failures retain only the latest eight, not the first eight.
        for ordinal in 81..=89 {
            records.push(Arc::new(observe::Record {
                ordinal,
                monotonic_elapsed_ns: 0,
                stage: Stage::InitialSource,
                instance: Some(identity()),
                kind: RecordKind::Metadata {
                    site: "later failure overflow fixture",
                    result: Err(format!("actual later error {ordinal}")),
                },
            }));
        }
        let overflow = initial_source_diagnostics(&records);
        assert_eq!(overflow["records"], diagnostics["records"]);
        assert_eq!(overflow["matched_records"], 88);
        let latest = &overflow["latest_actual_failures"];
        assert_eq!(latest["matched_records"], 13);
        assert_eq!(latest["row_limit"], 8);
        assert_eq!(latest["truncated"], true);
        assert_eq!(
            latest["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|record| record["ordinal"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            (82..=89).collect::<Vec<_>>()
        );
        assert!(!latest.to_string().contains("actual later error 81"));
        assert!(latest.to_string().contains("actual later error 89"));
    }

    #[test]
    fn sdk_b_short_receipt_fallback_keeps_distinct_body_cleanup_and_actual_panic() {
        let body = Err("original body failure".into());
        let cleanup = Err("original cleanup failure".into());
        let receipt: Check =
            checked_short_phase("short NI receipt", || panic!("original receipt panic"));
        assert!(
            receipt
                .as_ref()
                .unwrap_err()
                .contains("original receipt panic")
        );
        let diagnostics = json!({"records":[{"actual_error":"original source metadata failure"}]});
        let mut saved = None;
        retain_receipt_failure(
            Path::new("private-unused-fixture"),
            &body,
            &cleanup,
            &receipt,
            &diagnostics,
            |path, value| {
                assert_eq!(path.file_name().unwrap(), "short-receipt-failure.json");
                saved = Some(value.clone());
                Ok(())
            },
        )
        .unwrap();
        let value = saved.unwrap();
        assert_eq!(value["actual_body_error"], "original body failure");
        assert_eq!(value["actual_cleanup_error"], "original cleanup failure");
        assert!(
            value["actual_receipt_error"]
                .as_str()
                .unwrap()
                .contains("original receipt panic")
        );
        assert_eq!(value["actual_initial_source_diagnostics"], diagnostics);
        assert_eq!(value["success"], false);
    }

    #[test]
    fn sdk_b_short_existing_primary_and_fallback_files_are_preserved_and_errors_reach_stderr_payload()
     {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "noire-short-receipt-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let primary = directory.join("short-result.json");
        let sentinel = b"preserve primary evidence";
        fs::write(&primary, sentinel).unwrap();
        let body = Err("original body refusal".into());
        let cleanup = Err("original cleanup refusal".into());
        let receipt = new_json(&primary, &json!({"must_not_overwrite":true}));
        assert!(receipt.is_err());
        let diagnostics = json!({"actual_error":"original SourceLoad reason"});
        retain_receipt_failure(
            &directory,
            &body,
            &cleanup,
            &receipt,
            &diagnostics,
            new_json,
        )
        .unwrap();
        let fallback = directory.join("short-receipt-failure.json");
        let fallback_bytes = fs::read(&fallback).unwrap();
        let retained: Value = serde_json::from_slice(&fallback_bytes).unwrap();
        assert_eq!(
            retained["actual_receipt_error"],
            receipt.as_ref().unwrap_err().as_str()
        );
        let stderr = retain_receipt_failure(
            &directory,
            &body,
            &cleanup,
            &receipt,
            &diagnostics,
            new_json,
        )
        .unwrap_err();
        for cause in [
            "original body refusal",
            "original cleanup refusal",
            "original SourceLoad reason",
            receipt.as_ref().unwrap_err().as_str(),
        ] {
            assert!(stderr.contains(cause), "{stderr}");
        }
        assert!(stderr.contains("short fallback receipt could not be written"));
        assert_eq!(fs::read(&primary).unwrap(), sentinel);
        assert_eq!(fs::read(&fallback).unwrap(), fallback_bytes);
        fs::remove_file(primary).unwrap();
        fs::remove_file(fallback).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
