#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_state() -> Vec<u8> {
        let mut bytes = b"VST3HOST_STATE\0\0".to_vec();
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0x11, 0x22, 0x33]);
        bytes
    }

    /// Ordinary default/demo model with synthetic state. It is deliberately
    /// not Saved13 and requires no installed plugin, helper or user artwork.
    fn synthetic_project() -> Project {
        let mut project = Project::default();
        project.plugin_instances.push(PluginInstance {
            id: 17,
            format: ModelPluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: std::env::temp_dir().join("Synthetic Kontakt 8.vst3"),
            uid: KONTAKT_UID.into(),
            vendor: "Synthetic vendor".into(),
            name: "Synthetic identity only".into(),
            enabled: true,
            bypass: false,
            wet: 0.83,
            parameters: BTreeMap::from([(7, 0.31), (29, 0.78)]),
            opaque_state: synthetic_state(),
            runtime_status: PluginRuntimeStatus::Loaded,
            midi_ports: Default::default(),
        });
        // Locate a real stable ID away from element zero. No positional
        // assumptions may survive into source admission or descriptor binding.
        project.channels[2].instrument_plugin_instance_id = Some(17);
        project.channels[2].volume = 0.55;
        project.channels[2].pan = -0.2;
        project
    }

    /// Use the production Project serializer, with only fields absent from the
    /// original v12 schema omitted. No expected-model normalization is allowed.
    fn synthetic_legacy_document() -> Value {
        let mut project = synthetic_project();
        project.format_version = SAVED13_FORMAT_VERSION;
        project.audio_assets.push(crate::model::AudioAsset {
            id: 9201,
            name: "Synthetic dormant asset".into(),
            path: std::env::temp_dir().join("unopened-synthetic.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
            frames: 19,
            waveform_peaks: Vec::new(),
        });
        let mut document = serde_json::to_value(&project).unwrap();
        document.as_object_mut().unwrap().remove("auto_song_length");
        assert!(document["clips"].as_array().unwrap().iter().all(|clip| {
            !clip
                .as_object()
                .unwrap()
                .contains_key("pattern_length_policy")
        }));
        // This legacy mirror is read but not written by the production model.
        // v12 must retain it, including its separately snapshotted selection.
        document["piano_notes"] = serde_json::to_value(&project.patterns[0].notes).unwrap();
        document
    }

    struct SyntheticLegacyFile {
        directory: PathBuf,
        path: PathBuf,
    }

    impl SyntheticLegacyFile {
        fn new(document: &Value) -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "citrus-noire-full-inputs-{}-{timestamp}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let file = Self {
                path: directory.join("synthetic-v12.citrus"),
                directory,
            };
            fs::write(&file.path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
            file
        }

        fn load(&self) -> (Project, Project) {
            let bytes = fs::read(&self.path).unwrap();
            let decoded = serde_json::from_slice(&bytes).unwrap();
            let loaded = Project::load(&self.path).expect("production legacy disk load");
            assert_eq!(fs::read(&self.path).unwrap(), bytes);
            (decoded, loaded)
        }
    }

    impl Drop for SyntheticLegacyFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
            let _ = fs::remove_dir(&self.directory);
        }
    }

    #[test]
    fn full_saved13_inputs_disk_load_allows_only_v12_version_upgrade_with_legacy_defaults() {
        let file = SyntheticLegacyFile::new(&synthetic_legacy_document());
        let (decoded, loaded) = file.load();
        assert_eq!(decoded.format_version, SAVED13_FORMAT_VERSION);
        assert_eq!(loaded.format_version, CURRENT_PROJECT_FORMAT_VERSION);
        // This is the old preflight's false failure, using an actual disk load.
        assert!(model_snapshot(&decoded).unwrap() != model_snapshot(&loaded).unwrap());
        require_unchanged_saved13_load(&decoded, &loaded).unwrap();
        for project in [&decoded, &loaded] {
            assert!(!project.auto_song_length);
            assert!(
                project
                    .clips
                    .iter()
                    .all(|clip| clip.pattern_length_policy.is_none())
            );
            assert!(!project.piano_notes.is_empty());
        }
    }

    #[test]
    fn full_saved13_inputs_version_allowance_rejects_other_source_or_loaded_versions() {
        let file = SyntheticLegacyFile::new(&synthetic_legacy_document());
        let (decoded, loaded) = file.load();
        for version in [1, 11, 13, CURRENT_PROJECT_FORMAT_VERSION, 15] {
            let mut wrong_source = decoded.clone();
            wrong_source.format_version = version;
            assert!(require_unchanged_saved13_load(&wrong_source, &loaded).is_err());
        }
        for version in [
            SAVED13_FORMAT_VERSION,
            13,
            CURRENT_PROJECT_FORMAT_VERSION + 1,
        ] {
            let mut wrong_loaded = loaded.clone();
            wrong_loaded.format_version = version;
            assert!(require_unchanged_saved13_load(&decoded, &wrong_loaded).is_err());
        }
    }

    #[test]
    fn full_saved13_inputs_version_allowance_rejects_all_other_model_changes() {
        let file = SyntheticLegacyFile::new(&synthetic_legacy_document());
        let (decoded, loaded) = file.load();
        type ModelChange = (&'static str, fn(&mut Project));
        let changes: [ModelChange; 13] = [
            ("notes", |project| project.patterns[0].notes[0].note += 1),
            ("opaque state", |project| {
                project.plugin_instances[0].opaque_state[28] ^= 1
            }),
            ("parameter base", |project| {
                project.plugin_instances[0].parameters.insert(7, 0.32);
            }),
            ("channel base", |project| project.channels[2].volume = 0.56),
            ("routing", |project| project.mixer_routes[0].gain = 0.43),
            ("AUTO default", |project| project.auto_song_length = true),
            ("placement policy", |project| {
                project.clips[0].pattern_length_policy =
                    Some(crate::model::PatternLengthPolicy::Manual)
            }),
            ("pattern selection", |project| {
                project.patterns[0].notes[0].selected = true
            }),
            ("legacy mirror", |project| {
                project.piano_notes[0].start += 0.25
            }),
            ("legacy selection", |project| {
                project.piano_notes[0].selected = true
            }),
            ("runtime status", |project| {
                project.plugin_instances[0].runtime_status = PluginRuntimeStatus::Loaded
            }),
            ("waveform", |project| {
                project.audio_assets[0].waveform_peaks.push(-0.0)
            }),
            ("migration diagnostics", |project| {
                project
                    .migration_diagnostics
                    .push(ProjectMigrationDiagnostic::LegacyPianoMirrorIgnored { note_count: 91 })
            }),
        ];
        for (label, change) in changes {
            let mut changed = loaded.clone();
            change(&mut changed);
            assert!(
                require_unchanged_saved13_load(&decoded, &changed).is_err(),
                "{label}"
            );
        }
    }

    #[test]
    fn full_saved13_inputs_version_allowance_rejects_finite_value_repaired_by_disk_load() {
        let mut document = synthetic_legacy_document();
        document["channels"][2]["volume"] = json!(1.25);
        let file = SyntheticLegacyFile::new(&document);
        let (decoded, loaded) = file.load();
        assert_eq!(decoded.channels[2].volume, 1.25);
        assert_eq!(loaded.channels[2].volume, 1.0);
        assert!(require_unchanged_saved13_load(&decoded, &loaded).is_err());
    }

    #[test]
    fn full_saved13_inputs_retains_complete_synthetic_model_and_skipped_session_fields() {
        let mut project = synthetic_project();
        project.name = "Synthetic complete-model preservation".into();
        project.active_pattern = 0;
        project.channels[0].muted = true;
        project.mixer_tracks[1].volume = 0.39;
        project.mixer_routes[0].gain = 0.43;
        project.patterns[0].notes[0].selected = true;
        project.piano_notes = vec![PianoNote {
            id: 8001,
            channel_id: Some(project.channels[2].id),
            group_id: None,
            note: 73,
            start: 0.5,
            length: 0.75,
            velocity: 0.63,
            selected: true,
            muted: false,
        }];
        project
            .migration_diagnostics
            .push(ProjectMigrationDiagnostic::LegacyPianoMirrorIgnored { note_count: 1 });
        project.audio_assets.push(crate::model::AudioAsset {
            id: 9201,
            name: "Synthetic dormant asset".into(),
            path: std::env::temp_dir().join("unopened-synthetic.wav"),
            sample_rate: 48_000,
            channels: 2,
            bits_per_sample: 32,
            frames: 19,
            waveform_peaks: vec![0.125, -0.0, 0.75],
        });
        let before = model_snapshot(&project).expect("synthetic before snapshot");
        let (retained, source) = retain_validated_project(project).expect("generic source model");
        assert!(model_snapshot(&retained).expect("after snapshot") == before);
        assert_eq!(source.plugin_instance_id, 17);
        assert_eq!(source.channel_id, retained.channels[2].id);
        assert_eq!(source.descriptor.id, KONTAKT_UID);
        assert_eq!(source.descriptor.path, retained.plugin_instances[0].path);
        assert!(source.descriptor.vst3_metadata.is_none());
    }

    #[test]
    fn full_saved13_inputs_locates_assignment_by_id_after_channel_reorder() {
        let mut project = synthetic_project();
        let expected_channel = project.channels[2].id;
        project.channels.rotate_left(1);
        let before = model_snapshot(&project).unwrap();
        let (retained, source) = retain_validated_project(project).unwrap();
        assert_eq!(source.channel_id, expected_channel);
        assert_ne!(source.channel_id, retained.channels[0].id);
        assert!(model_snapshot(&retained).unwrap() == before);
    }

    #[test]
    fn full_saved13_inputs_descriptor_preserves_historical_saved_uid_and_path() {
        let project = synthetic_project();
        let source = validate_source_model(&project).unwrap();
        let instance = &project.plugin_instances[0];
        assert_eq!(source.descriptor.id, instance.uid);
        assert_eq!(source.descriptor.path, instance.path);
        assert_eq!(source.descriptor.vendor, instance.vendor);
        assert_eq!(source.descriptor.name, instance.name);
        assert_eq!(source.state_bytes, instance.opaque_state.len());
        assert_eq!(
            source.state_sha256,
            oracles::sha256_hex(&instance.opaque_state)
        );
        assert!(source.descriptor.vst3_metadata.is_none());
    }

    #[test]
    fn full_saved13_inputs_rejects_missing_or_ambiguous_uid_and_assignment() {
        let original = synthetic_project();
        let mut wrong_uid = original.clone();
        wrong_uid.plugin_instances[0].uid.push('0');
        assert!(validate_source_model(&wrong_uid).is_err());
        let mut duplicate_class = original.clone();
        let mut second = duplicate_class.plugin_instances[0].clone();
        second.id = 19;
        duplicate_class.plugin_instances.push(second);
        assert!(validate_source_model(&duplicate_class).is_err());
        let mut unassigned = original.clone();
        unassigned.channels[2].instrument_plugin_instance_id = None;
        assert!(validate_source_model(&unassigned).is_err());
        let mut ambiguous = original;
        ambiguous.channels[1].instrument_plugin_instance_id = Some(17);
        assert!(validate_source_model(&ambiguous).is_err());
    }

    #[test]
    fn full_saved13_inputs_rejects_zero_and_duplicate_stable_ids() {
        for channel_id in [0, 1] {
            let mut project = synthetic_project();
            project.channels[2].id = channel_id;
            assert!(validate_source_model(&project).is_err());
        }
        let mut project = synthetic_project();
        project.plugin_instances[0].id = 0;
        project.channels[2].instrument_plugin_instance_id = Some(0);
        assert!(validate_source_model(&project).is_err());
        let mut project = synthetic_project();
        let mut other = project.plugin_instances[0].clone();
        other.uid = "other synthetic class".into();
        project.plugin_instances.push(other);
        assert!(validate_source_model(&project).is_err());
    }

    #[test]
    fn full_saved13_inputs_rejects_disabled_bypassed_wrong_role_or_format() {
        for case in 0..4 {
            let mut project = synthetic_project();
            let source = &mut project.plugin_instances[0];
            match case {
                0 => source.enabled = false,
                1 => source.bypass = true,
                2 => source.role = PluginRole::Effect,
                _ => source.format = ModelPluginFormat::Vst2,
            }
            assert!(validate_source_model(&project).is_err());
        }
    }

    #[test]
    fn full_saved13_inputs_rejects_state_header_version_geometry_and_trailing_bytes() {
        let valid = synthetic_state();
        let mut bad_magic = valid.clone();
        bad_magic[0] ^= 1;
        let mut bad_version = valid.clone();
        bad_version[16..20].copy_from_slice(&2_u32.to_le_bytes());
        let mut bad_component = valid.clone();
        bad_component[20..24].copy_from_slice(&0_u32.to_le_bytes());
        let mut bad_controller = valid.clone();
        bad_controller[24..28].copy_from_slice(&1_u32.to_le_bytes());
        let mut extra = valid.clone();
        extra.push(0);
        for bytes in [
            vec![],
            valid[..27].to_vec(),
            bad_magic,
            bad_version,
            bad_component,
            bad_controller,
            extra,
        ] {
            let mut project = synthetic_project();
            project.plugin_instances[0].opaque_state = bytes;
            assert!(validate_source_model(&project).is_err());
        }
    }

    #[test]
    fn full_saved13_inputs_rejects_invalid_bases_without_normalizing_model() {
        for base in [f32::NAN, f32::INFINITY, -0.001, 1.001] {
            let mut project = synthetic_project();
            project.plugin_instances[0].parameters.insert(31, base);
            assert!(validate_source_model(&project).is_err());
            let mut project = synthetic_project();
            project.plugin_instances[0].wet = base;
            assert!(validate_source_model(&project).is_err());
            let mut project = synthetic_project();
            project.channels[2].volume = base;
            assert!(validate_source_model(&project).is_err());
        }
        let mut project = synthetic_project();
        project.plugin_instances[0].parameters = (0..4097).map(|id| (id, 0.5)).collect();
        assert!(validate_source_model(&project).is_err());
        for pan in [f32::NAN, -1.001, 1.001] {
            let mut project = synthetic_project();
            project.channels[2].pan = pan;
            assert!(validate_source_model(&project).is_err());
        }
        let mut project = synthetic_project();
        project.mixer_tracks[0].volume = f32::NAN;
        assert!(validate_source_model(&project).is_err());
    }

    #[test]
    fn full_saved13_inputs_rejects_invalid_transport_path_and_mixer_destination() {
        for case in 0..5 {
            let mut project = synthetic_project();
            match case {
                0 => project.tempo = 0.0,
                1 => project.swing = 1.001,
                2 => project.song_length_beats = f32::NAN,
                3 => project.plugin_instances[0].path = PathBuf::from("relative-plugin.vst3"),
                _ => project.channels[2].mixer_track = 99999,
            }
            assert!(validate_source_model(&project).is_err());
        }
    }

    #[test]
    fn full_saved13_inputs_snapshot_detects_persisted_and_skipped_field_changes() {
        let original = synthetic_project();
        let before = model_snapshot(&original).unwrap();
        for case in 0..5 {
            let mut changed = original.clone();
            match case {
                0 => changed.mixer_routes[0].gain = 0.13,
                1 => changed.patterns[0].notes[0].selected = true,
                2 => changed.plugin_instances[0].runtime_status = PluginRuntimeStatus::Missing,
                3 => changed.plugin_instances[0].opaque_state[28] ^= 1,
                _ => changed
                    .migration_diagnostics
                    .push(ProjectMigrationDiagnostic::LegacyPianoMirrorIgnored { note_count: 91 }),
            }
            assert!(model_snapshot(&changed).unwrap() != before);
        }
    }

    #[test]
    fn full_saved13_inputs_synthetic_validation_never_satisfies_real_file_pin() {
        let project = synthetic_project();
        assert!(validate_source_model(&project).is_ok());
        assert!(require_saved13_file(&serde_json::to_vec(&project).unwrap()).is_err());
        // Correct length cannot impersonate the entire original SHA binding.
        assert!(require_saved13_file(&vec![0; SAVED13_BYTES]).is_err());
        let source = validate_source_model(&project).unwrap();
        assert!(require_saved13_model(&project, &source).is_err());
    }

    #[test]
    fn full_saved13_inputs_short_loader_and_scratch_bodies_remain_byte_identical() {
        const EXPECTED_SHORT_SECTION_SHA256: &str =
            "40168dfdca3a8a4d9dab6dfdc838fcccafa557c01ef1d7dc625efe3dacf7562a";
        let source = include_bytes!("windows_noire_bounce_qa.rs");
        let find = |needle: &[u8]| {
            source
                .windows(needle.len())
                .position(|part| part == needle)
                .unwrap()
        };
        let start = find(b"fn scratch(saved: &Project)");
        let end = find(b"struct Owner {");
        assert_eq!(
            oracles::sha256_hex(&source[start..end]),
            EXPECTED_SHORT_SECTION_SHA256
        );
    }

    #[test]
    #[ignore = "explicit local Saved13 opt-in; never reads user projects on ordinary CI"]
    fn full_saved13_inputs_actual_pinned_project_load_only_no_native_instance() {
        let inputs = FullInputs::load_from_env().expect("explicit complete Saved13 loader");
        assert_eq!(inputs.pins.project_sha256, SOURCE_SHA256);
        assert_eq!(inputs.pins.saved_state_sha256, SAVED13_STATE_SHA256);
        assert_eq!(inputs.pins.project_bytes, SAVED13_BYTES);
        assert_eq!(inputs.pins.saved_state_bytes, SAVED13_STATE_BYTES);
        assert_eq!(
            inputs
                .project
                .patterns
                .iter()
                .map(|pattern| pattern.notes.len())
                .sum::<usize>(),
            405
        );
        assert_eq!(inputs.project.clips.len(), 5);
        assert_eq!(
            inputs.project.format_version,
            CURRENT_PROJECT_FORMAT_VERSION
        );
        assert!(!inputs.project.auto_song_length);
        assert!(
            inputs
                .project
                .clips
                .iter()
                .all(|clip| clip.pattern_length_policy.is_none())
        );
        assert_eq!(inputs.project.song_length_beats, 160.0);
        assert_eq!(inputs.source.channel_id, 1);
        assert_eq!(inputs.source.plugin_instance_id, 1);
        // Never print opaque state, full model snapshots, or user composition.
        assert!(inputs.pins.project_path.is_absolute());
        assert!(inputs.pins.actual_bundle_path.exists());
    }
}
