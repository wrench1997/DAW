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
            input_buses: vec![BounceAudioBus::new(2, false, Some(true))],
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
        assert!(actual_metadata(&[], id).is_err());
        assert!(actual_metadata(&[metadata_record(identity(), metadata())], id).is_ok());
        let foreign = observe::InstanceIdentity {
            worker_id: Some(32),
            ..id
        };
        assert!(actual_metadata(&[metadata_record(foreign, metadata())], id).is_err());
        for case in 0..4 {
            let mut bad = metadata();
            match case {
                0 => bad.class_uid = "A".repeat(32),
                1 => bad.version = "other".into(),
                2 => bad.category = "NotInstrument".into(),
                _ => bad.event_input_buses[0].2 = Some(false),
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
            vec![f32::NAN.to_bits(), 0],
            vec![0, f32::INFINITY.to_bits()],
        ] {
            assert!(onset_metrics(&bits).is_err());
        }
    }
}
