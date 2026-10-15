#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        automation::{AutomationLane, AutomationTarget},
        instrument_bounce::{BounceAudioBus, BouncePluginMetadata, BouncePluginState},
        mixer_graph::MASTER_MIXER_TRACK_ID,
        model::{Clip, Pattern, PluginInstance, ProjectAutomation},
        plugins::plugin_runtime::{
            LoadedVst3Identity, PluginPrepareConfig, test_capture_source::TestCaptureSource,
        },
    };

    fn synthetic_state() -> Vec<u8> {
        let mut bytes = b"VST3HOST_STATE\0\0".to_vec();
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0x11, 0x22, 0x33]);
        bytes
    }
    /// Synthetic whole-song geometry only. These model/hook tests never claim
    /// that fake scan metadata is from Kontakt, nor construct a native worker.
    fn fixture() -> (Box<CitrusApp>, ValidatedSource, PluginDescriptor) {
        let mut project = Project::blank();
        project.name = "Synthetic full-source preservation".into();
        project.tempo = 84.0;
        project.song_length_beats = 160.0;
        let mut dormant_channel = project.channels[0].clone();
        dormant_channel.id = 9;
        dormant_channel.name = "Other dormant Channel".into();
        dormant_channel.volume = 0.37;
        dormant_channel.steps[9] = true;
        project.channels.insert(0, dormant_channel);
        let channel = &mut project.channels[1];
        channel.instrument_plugin_instance_id = Some(17);
        channel.volume = 0.55;
        channel.pan = 0.0;
        channel.mixer_track = MASTER_MIXER_TRACK_ID;
        project.mixer_tracks[0].volume = 0.7;
        project.mixer_tracks[1].volume = 0.74;
        project.mixer_tracks[2].volume = 0.8;
        let source_channel = channel.id;
        project.plugin_instances.push(PluginInstance {
            id: 17,
            format: ModelPluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: std::env::temp_dir().join("Synthetic full-source Kontakt.vst3"),
            uid: KONTAKT_UID.into(),
            vendor: "Synthetic".into(),
            name: "Synthetic source".into(),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: BTreeMap::from([(7, 0.31)]),
            opaque_state: synthetic_state(),
            runtime_status: PluginRuntimeStatus::Unloaded,
            midi_ports: Default::default(),
        });
        let mut other = project.plugin_instances[0].clone();
        other.id = 23;
        other.uid = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into();
        other.name = "Other dormant plug-in".into();
        other.role = PluginRole::Effect;
        other.enabled = false;
        other.parameters.insert(29, 0.61);
        other.runtime_status = PluginRuntimeStatus::Crashed;
        project.plugin_instances.insert(0, other);
        let mut note_id = 1;
        project.patterns = [43, 106, 106, 107, 43]
            .into_iter()
            .enumerate()
            .map(|(index, count)| {
                let notes = (0..count)
                    .map(|n| {
                        let note = PianoNote {
                            id: note_id,
                            channel_id: Some(source_channel),
                            group_id: None,
                            note: 60 + (n % 8) as u8,
                            start: n as f32 / 4.0,
                            length: 0.25,
                            velocity: 0.63,
                            selected: n == 0,
                            muted: false,
                        };
                        note_id += 1;
                        note
                    })
                    .collect();
                Pattern {
                    id: index as u32 + 1,
                    name: format!("Full pattern {}", index + 1),
                    length_steps: 16,
                    channel_steps: vec![[false; 16]; 2],
                    notes,
                }
            })
            .collect();
        project.clips = [
            (0.0, 16.0),
            (16.0, 32.0),
            (48.0, 32.0),
            (80.0, 32.0),
            (112.0, 16.0),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (start, length))| Clip {
            id: index as u32 + 1,
            track: index,
            start,
            length,
            name: format!("Full placement {}", index + 1),
            color: [1, 2, 3],
            kind: ClipKind::Pattern,
            group_id: None,
            pattern_length_policy: None,
            pattern_id: index as u32 + 1,
            automation_id: None,
            audio_asset_id: None,
            source_offset: 0.0,
            audio_source_offset_frame: None,
            audio_source_reference: None,
            audio_length_reference: None,
            gain: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            fade_in_reference: None,
            fade_out_reference: None,
            muted: false,
        })
        .collect();
        let mut lane = AutomationLane::new(AutomationTarget::MasterVolume);
        for (beat, gain) in [(0.0, 0.7), (120.0, 0.7), (128.0, 0.56)] {
            lane.insert_point_at(beat, gain, 0.0).unwrap();
        }
        project.automation_lanes = vec![ProjectAutomation {
            id: 1,
            name: "Original Master".into(),
            lane,
        }];
        let instance = project
            .plugin_instances
            .iter()
            .find(|instance| instance.id == 17)
            .unwrap();
        let descriptor = plugins::plugin_load_identity_from_instance(instance).descriptor;
        let source = ValidatedSource {
            plugin_instance_id: 17,
            channel_id: source_channel,
            descriptor: descriptor.clone(),
            state_bytes: instance.opaque_state.len(),
            state_sha256: oracles::sha256_hex(&instance.opaque_state),
        };
        let mut scanned = descriptor;
        scanned.id = "synthetic scanner cache ID; not historical persisted UID".into();
        scanned.verified = true;
        scanned.scan_error = None;
        scanned.vst3_metadata = Some(plugins::Vst3ScanMetadata {
            class_uid: KONTAKT_UID.into(),
            category: "Instrument|Synth".into(),
            has_midi_input: true,
            has_midi_output: false,
        });
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        app.project = project;
        app.history_snapshot = app.project.clone();
        app.history_fingerprint = project_fingerprint(&app.project);
        app.project_fingerprint = app.history_fingerprint;
        let mut prior = app.project.clone();
        prior.name = "Prior full score".into();
        app.undo_stack = vec![prior.clone(), app.project.clone()];
        prior.name = "Future full score".into();
        app.redo_stack = vec![prior];
        app.saved_path = Some(std::env::temp_dir().join("Synthetic full score.citrus"));
        app.dirty = true;
        (app, source, scanned)
    }

    #[test]
    fn full_owner_enqueue_retains_405_notes_five_placements_coda_routes_and_history() {
        let (mut app, source, scanned) = fixture();
        let before = FullHistoryImage::take(&app, source.plugin_instance_id).unwrap();
        let original = FullProjectImage::take(&app.project, None).unwrap();
        register_and_enqueue_complete_source(
            &mut app,
            &source,
            &scanned,
            |app, channel, candidate, previous| {
                assert_eq!(channel, source.channel_id);
                assert!(FullProjectImage::take(&candidate, None).unwrap() == original);
                assert_eq!(
                    candidate
                        .patterns
                        .iter()
                        .map(|pattern| pattern.notes.len())
                        .sum::<usize>(),
                    405
                );
                assert_eq!(candidate.clips.len(), 5);
                assert_eq!(
                    candidate.clips.last().unwrap().start + candidate.clips.last().unwrap().length,
                    128.0
                );
                assert_eq!(candidate.song_length_beats, 160.0);
                assert_eq!(candidate.mixer_routes.len(), 31);
                assert_eq!(candidate.mixer_tracks.len(), 32);
                assert_eq!(candidate.automation_lanes[0].lane.points()[2].value, 0.56);
                assert_eq!(previous.unwrap().instance_id, Some(17));
                assert_eq!(app.fresh_bounce_sources[&17].0, KONTAKT_UID);
                Ok(())
            },
        )
        .unwrap();
        before.unchanged(&app, 17).unwrap();
        assert_eq!(app.project.plugin_instances.len(), 2);
        assert!(app.running_generator_chains.is_empty());
        assert!(app.pending_generator_chains.is_empty());
    }

    #[test]
    fn full_owner_binding_survives_channel_and_plugin_vector_reorder() {
        let (mut app, source, scanned) = fixture();
        app.project.channels.reverse();
        app.project.plugin_instances.reverse();
        let before = FullHistoryImage::take(&app, 17).unwrap();
        register_and_enqueue_complete_source(
            &mut app,
            &source,
            &scanned,
            |_, channel, candidate, previous| {
                assert_eq!(channel, source.channel_id);
                assert_eq!(previous.unwrap().instance_id, Some(17));
                assert_eq!(candidate.plugin_instances.len(), 2);
                Ok(())
            },
        )
        .unwrap();
        before.unchanged(&app, 17).unwrap();
    }

    #[test]
    fn full_owner_wrong_stable_binding_is_refused_before_hook_or_registry_write() {
        for case in 0..5 {
            let (mut app, mut source, scanned) = fixture();
            let before = FullHistoryImage::take(&app, 17).unwrap();
            match case {
                0 => source.plugin_instance_id = 23,
                1 => source.channel_id = 9,
                2 => source.plugin_instance_id = 0,
                3 => source.descriptor.id = "foreign historical identity".into(),
                _ => source.state_sha256 = "0".repeat(64),
            }
            let mut called = false;
            assert!(
                register_and_enqueue_complete_source(&mut app, &source, &scanned, |_, _, _, _| {
                    called = true;
                    Ok(())
                })
                .is_err()
            );
            assert!(!called);
            assert!(app.fresh_bounce_sources.is_empty());
            before.unchanged(&app, 17).unwrap();
        }
    }

    #[test]
    fn full_owner_scan_metadata_is_required_and_must_match_persisted_source() {
        for case in 0..12 {
            let (mut app, source, mut scanned) = fixture();
            let before = FullHistoryImage::take(&app, 17).unwrap();
            match case {
                0 => scanned.vst3_metadata = None,
                1 => scanned.verified = false,
                2 => scanned.scan_error = Some("actual scanner failed".into()),
                3 => scanned.path = std::env::temp_dir().join("Foreign.vst3"),
                4 => scanned.format = plugins::PluginFormat::Vst2,
                5 => scanned.is_instrument = false,
                6 => scanned.vst3_metadata.as_mut().unwrap().class_uid = "A".repeat(32),
                7 => scanned.vst3_metadata.as_mut().unwrap().category = "Fx".into(),
                8 => scanned.vst3_metadata.as_mut().unwrap().has_midi_input = false,
                9 => scanned.vst3_metadata.as_mut().unwrap().category = "NotInstrument".into(),
                10 => scanned.vst3_metadata.as_mut().unwrap().category = "Instrumental".into(),
                _ => {
                    let uid = scanned.vst3_metadata.take().unwrap().class_uid;
                    scanned.verified = false;
                    scanned.is_instrument = false;
                    scanned.category = "Unknown".into();
                    scanned.vst3_catalog = Some(plugins::Vst3FactoryCatalog {
                        factory: plugins::Vst3FactoryInfo {
                            vendor: "Declared vendor".into(),
                            url: String::new(),
                            email: String::new(),
                            flags: 0,
                        },
                        classes: vec![plugins::Vst3FactoryClass {
                            class_id: uid,
                            name: "Expected UID declaration only".into(),
                            category: "Audio Module Class".into(),
                            cardinality: i32::MAX,
                            vendor: None,
                            version: None,
                            sdk_version: None,
                            sub_categories: Some(vec!["Instrument".into()]),
                            class_flags: None,
                        }],
                    });
                    assert!(scanned.is_catalog_only());
                }
            }
            assert!(
                register_and_enqueue_complete_source(
                    &mut app,
                    &source,
                    &scanned,
                    |_, _, _, _| panic!("invalid scan may not reach a worker hook")
                )
                .is_err()
            );
            assert!(app.fresh_bounce_sources.is_empty());
            before.unchanged(&app, 17).unwrap();
        }
    }

    #[test]
    fn full_owner_duplicate_plugin_channel_and_assignment_are_refused() {
        for case in 0..3 {
            let (mut app, source, scanned) = fixture();
            match case {
                0 => app
                    .project
                    .plugin_instances
                    .push(app.project.plugin_instances[1].clone()),
                1 => app.project.channels.push(app.project.channels[1].clone()),
                _ => app.project.channels[0].instrument_plugin_instance_id = Some(17),
            }
            assert!(complete_source_intent(&app.project, &source, &scanned).is_err());
        }
    }

    #[test]
    fn full_owner_unsupported_wet_enabled_or_bypass_never_rewrites_source() {
        for case in 0..3 {
            let (mut app, source, scanned) = fixture();
            let instance = app
                .project
                .plugin_instances
                .iter_mut()
                .find(|instance| instance.id == 17)
                .unwrap();
            match case {
                0 => instance.wet = 0.9,
                1 => instance.enabled = false,
                _ => instance.bypass = true,
            }
            let before = FullHistoryImage::take(&app, 17).unwrap();
            assert!(complete_source_intent(&app.project, &source, &scanned).is_err());
            before.unchanged(&app, 17).unwrap();
        }
    }

    #[test]
    fn full_owner_failed_enqueue_preserves_full_history_and_retains_session_intent() {
        let (mut app, source, scanned) = fixture();
        let before = FullHistoryImage::take(&app, 17).unwrap();
        let result =
            register_and_enqueue_complete_source(&mut app, &source, &scanned, |_, _, _, _| {
                Err("actual test hook refuses before any worker construction".into())
            });
        assert_eq!(
            result,
            Err("actual test hook refuses before any worker construction".into())
        );
        before.unchanged(&app, 17).unwrap();
        assert_eq!(
            app.fresh_bounce_sources[&17].2,
            FreshSourceClass::Exact(KONTAKT_UID.into())
        );
        assert!(app.running_generator_chains.is_empty());
        assert!(app.pending_generator_chains.is_empty());
    }

    #[test]
    fn full_owner_second_initialization_intent_cannot_replace_the_first() {
        let (mut app, source, scanned) = fixture();
        register_and_enqueue_complete_source(&mut app, &source, &scanned, |_, _, _, _| Ok(()))
            .unwrap();
        let before = FullHistoryImage::take(&app, 17).unwrap();
        let original = app.fresh_bounce_sources[&17].clone();
        assert!(
            register_and_enqueue_complete_source(&mut app, &source, &scanned, |_, _, _, _| panic!(
                "second initialization must not be submitted"
            ))
            .is_err()
        );
        assert_eq!(app.fresh_bounce_sources[&17], original);
        before.unchanged(&app, 17).unwrap();
    }

    #[test]
    fn full_owner_registry_validation_requires_exact_callback_session_channel_and_worker() {
        let (_, source, _) = fixture();
        let exact = (4, source.channel_id, 88, 17, 0, 31);
        exact_complete_registry(&source, 4, 0, exact).unwrap();
        for wrong in [
            (5, source.channel_id, 88, 17, 0, 31),
            (4, 9, 88, 17, 0, 31),
            (4, source.channel_id, 0, 17, 0, 31),
            (4, source.channel_id, 88, 23, 0, 31),
            (4, source.channel_id, 88, 17, 1, 31),
            (4, source.channel_id, 88, 17, 0, 0),
        ] {
            assert!(exact_complete_registry(&source, 4, 0, wrong).is_err());
        }
    }

    #[test]
    fn full_owner_image_allows_only_source_session_status_changes() {
        let (mut app, _, _) = fixture();
        let before = FullHistoryImage::take(&app, 17).unwrap();
        app.project
            .plugin_instances
            .iter_mut()
            .find(|instance| instance.id == 17)
            .unwrap()
            .runtime_status = PluginRuntimeStatus::Loaded;
        before.unchanged(&app, 17).unwrap();
        app.project
            .plugin_instances
            .iter_mut()
            .find(|instance| instance.id == 23)
            .unwrap()
            .runtime_status = PluginRuntimeStatus::Loaded;
        assert!(before.unchanged(&app, 17).is_err());
    }

    #[test]
    fn full_owner_image_detects_state_notes_routes_automation_and_history_mutation() {
        for case in 0..10 {
            let (mut app, _, _) = fixture();
            let before = FullHistoryImage::take(&app, 17).unwrap();
            match case {
                0 => app.project.patterns[4].notes[0].velocity = 0.2,
                1 => app.project.patterns[4].notes[0].selected = false,
                2 => app.project.clips.pop().map(|_| ()).unwrap(),
                3 => app.project.mixer_routes[0].gain = 0.31,
                4 => app.project.automation_lanes[0].lane.set_enabled(false),
                5 => app.project.plugin_instances[1].opaque_state.push(0),
                6 => app.undo_stack.clear(),
                7 => app.redo_stack.clear(),
                8 => app.dirty = false,
                _ => app.saved_path = None,
            }
            assert!(before.unchanged(&app, 17).is_err());
        }
    }

    fn model_owner() -> FullParent {
        let (app, source, _) = fixture();
        let original = FullHistoryImage::take(&app, 17).unwrap();
        FullParent {
            pins: FullInputPins {
                project_path: app.saved_path.clone().unwrap(),
                project_bytes: 0,
                project_sha256: String::new(),
                saved_state_bytes: source.state_bytes,
                saved_state_sha256: source.state_sha256.clone(),
                actual_bundle_path: source.descriptor.path.clone(),
            },
            app: Some(app),
            callback: None,
            ctx: egui::Context::default(),
            source,
            original,
            initialization_requested: false,
            joins_complete: false,
            pumps: 0,
            callback_segments: 0,
            lifecycle_receipts: 0,
        }
    }

    fn compiled_assigned_model_owner(token: [u8; 32]) -> FullParent {
        let mut owner = model_owner();
        let (engine, mut callback) = AudioEngine::test_engine_with_callback();
        callback.enable_genuine_observation(token).unwrap();
        owner.app_mut().audio = Some(engine);
        owner.callback = Some(callback);
        let session = owner.app().project_session;
        assert!(
            owner
                .app_mut()
                .audio
                .as_mut()
                .unwrap()
                .set_project_session(session)
        );
        owner.app_mut().transport_mode = TransportMode::Song;
        owner.app_mut().rebuild_tempo_map();
        owner.app_mut().request_immediate_timeline_compile();
        let deadline = Instant::now() + Duration::from_secs(5);
        // Receive a real compile result without issuing any Timeline/runtime
        // poll yet. This deterministically exposes the former pre-enqueue race.
        while owner.app().timeline_compiled_generation
            != Some(owner.app().timeline_desired_generation)
        {
            assert!(
                Instant::now() < deadline,
                "{}",
                owner.source_readiness_evidence()
            );
            let ctx = owner.ctx.clone();
            owner.app_mut().poll_timeline_compile(&ctx);
            std::thread::yield_now();
        }
        assert_eq!(
            owner.app().timeline_audio_sync.phase,
            TimelineAudioSyncPhase::InstallReady
        );
        assert!(owner.app().pending_generator_chains.is_empty());
        assert!(owner.app().running_generator_chains.is_empty());
        owner
    }

    // Own the actual deterministic worker through assertion unwind as well.
    // This is the existing FullParent retained-guard shutdown policy, not an
    // alternate native host or a fake Pending/Running/Join receipt.
    struct PreparedModelOwner {
        parent: RetainedFullParent,
        source: Option<TestCaptureSource>,
    }
    impl Drop for PreparedModelOwner {
        fn drop(&mut self) {
            if let Some(source) = &self.source {
                source.release_gates();
            }
            self.parent.callback.take();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut failure = None;
            loop {
                let app = self.parent.app_mut();
                let mut pending = false;
                for chain in app.pending_generator_chains.values_mut() {
                    pending |= full_poll_guard(&mut chain.guard, &mut failure);
                }
                for chain in app.running_generator_chains.values_mut() {
                    pending |= full_poll_guard(&mut chain.guard, &mut failure);
                }
                for guard in &mut app.retired_fresh_source_candidates {
                    pending |= full_poll_guard(guard, &mut failure);
                }
                if !pending {
                    self.parent.joins_complete = true;
                    break;
                }
                if Instant::now() >= deadline {
                    // RetainedFullParent::drop keeps the same unjoined owner.
                    // Do not detach a real worker even during assertion unwind.
                    eprintln!("model source shutdown remains pending; retaining FullParent");
                    CitrusApp::mark_fixture_cleanup_failed_for_test();
                    assert!(
                        std::thread::panicking(),
                        "model source did not complete its actual retained join"
                    );
                    return;
                }
                std::thread::yield_now();
            }
            if let Some(reason) = failure {
                CitrusApp::mark_fixture_cleanup_failed_for_test();
                assert!(
                    std::thread::panicking(),
                    "actual model source shutdown failed: {reason}"
                );
                eprintln!("actual model source shutdown failed: {reason}");
            }
        }
    }

    #[test]
    fn full_owner_timeline_poll_before_source_admission_reproduces_sticky_route_fault() {
        let _admission = crate::app::clip_render_job::tests::fixture_admission();
        let mut owner = compiled_assigned_model_owner([0x53; 32]);
        let generation = owner.app().timeline_desired_generation;
        // This is the old full mixer tick: a compiled assigned source has no
        // actual Pending owner yet. The production missing-owner gate MUST fail.
        owner.tick(Instant::now() + Duration::from_secs(5)).unwrap();
        assert_eq!(
            owner.app().timeline_audio_sync.phase,
            TimelineAudioSyncPhase::Faulted
        );
        assert!(
            owner
                .app()
                .timeline_audio_sync
                .last_error
                .as_deref()
                .unwrap()
                .contains("has no callback-confirmed Running endpoint")
        );
        assert!(
            owner
                .app()
                .audio
                .as_ref()
                .unwrap()
                .confirmed_timeline_identity()
                .is_none()
        );
        // Session status changes are not a musical fingerprint change and
        // cannot silently retry the already faulted graph.
        let instance = owner.source.plugin_instance_id;
        owner
            .app_mut()
            .project
            .plugin_instances
            .iter_mut()
            .find(|item| item.id == instance)
            .unwrap()
            .runtime_status = PluginRuntimeStatus::Loaded;
        owner.app_mut().request_immediate_timeline_compile();
        owner.tick(Instant::now() + Duration::from_secs(5)).unwrap();
        assert_eq!(owner.app().timeline_desired_generation, generation);
        assert_eq!(
            owner.app().timeline_audio_sync.phase,
            TimelineAudioSyncPhase::Faulted
        );
        assert!(!owner.paused());
    }

    #[test]
    fn full_owner_mixer_prelude_then_real_pending_source_preserves_delayed_exact_activation() {
        let _admission = crate::app::clip_render_job::tests::fixture_admission();
        let mut owner = compiled_assigned_model_owner([0x54; 32]);
        let deadline = Instant::now() + Duration::from_secs(5);
        let command_count = mixer_control_commands(&owner.app().project).len();
        assert!(
            command_count > 64,
            "the fixture must exceed the callback ring capacity"
        );
        let pumps = owner.pumps;
        // Actual initializer uses this same mixer-only prelude. No source has
        // been constructed, so these unchanged callback pumps cannot warm it up.
        owner.service_initial_mixer_commands(deadline).unwrap();
        assert_eq!(owner.pumps - pumps, command_count);
        assert_eq!(
            owner.app().timeline_audio_sync.phase,
            TimelineAudioSyncPhase::InstallReady
        );
        assert_eq!(
            owner
                .callback()
                .unwrap()
                .genuine_snapshot()
                .pending_audio_commands,
            0
        );
        assert!(
            owner
                .callback()
                .unwrap()
                .genuine_snapshot()
                .endpoints
                .is_empty()
        );
        assert!(!owner.paused());
        let before = FullHistoryImage::take(owner.app(), owner.source.plugin_instance_id).unwrap();
        owner.initialization_requested = true;
        let mut fixture = PreparedModelOwner {
            parent: RetainedFullParent {
                owner: Some(ManuallyDrop::new(owner)),
            },
            source: None,
        };
        let id = fixture.parent.source.plugin_instance_id;
        let instance = fixture
            .parent
            .app()
            .project
            .plugin_instances
            .iter()
            .find(|item| item.id == id)
            .unwrap();
        let state = BouncePluginState {
            metadata: BouncePluginMetadata {
                class_uid: KONTAKT_UID.into(),
                version: "8.4.1".into(),
                name: "Synthetic held source, never an SDK plug-in".into(),
                category: "Instrument".into(),
                event_input_buses: vec![(16, true, Some(true))],
                event_output_buses: vec![],
                input_buses: vec![],
                output_buses: vec![BounceAudioBus::new(2, true, Some(true))],
                latency_samples: 0,
                tail_samples: 0,
            },
            opaque_state: instance.opaque_state.clone(),
            parameter_bases: instance.parameters.clone(),
            native_generation: 2,
            native_dirty_revision: 0,
        };
        let identity = LoadedVst3Identity::test_fixture(
            instance.path.clone(),
            KONTAKT_UID,
            "8.4.1",
            "Instrument",
            true,
            false,
        );
        let (chain, source) = TestCaptureSource::new_with_loaded_identity(
            id,
            state,
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: MAX_MIXER_BLOCK_FRAMES,
            },
            identity,
            true,
        )
        .unwrap();
        let worker = chain.guard.genuine_worker_id();
        fixture.source = Some(source);
        let intent =
            requested_saved_source_intent(&fixture.parent.app().project, &fixture.parent.source)
                .unwrap();
        let parent = &mut *fixture.parent;
        register_and_enqueue_source_intent(
            parent.app.as_deref_mut().unwrap(),
            &parent.source,
            intent,
            |app, channel, candidate, previous| {
                app.queue_channel_generator_candidate_prepared(
                    channel,
                    candidate,
                    previous,
                    Some(chain),
                )
            },
        )
        .unwrap();
        let channel = fixture.parent.source.channel_id;
        assert_eq!(
            fixture.parent.app().pending_generator_chains[&channel]
                .guard
                .genuine_worker_id(),
            worker
        );
        let timeline = Arc::clone(
            &fixture
                .parent
                .app()
                .timeline_audio_source
                .as_ref()
                .unwrap()
                .timeline,
        );
        for _ in 0..3 {
            fixture.parent.tick(deadline).unwrap();
            assert_eq!(
                fixture
                    .parent
                    .app()
                    .timeline_generator_routes_state(&timeline),
                RequiredGeneratorRouteState::Pending {
                    channel_id: channel,
                    plugin_instance_id: id
                }
            );
            assert!(fixture.parent.app().running_generator_chains.is_empty());
            assert_eq!(
                fixture.parent.app().timeline_audio_sync.phase,
                TimelineAudioSyncPhase::InstallReady
            );
            assert!(!fixture.parent.paused(), "Pending is not ready");
            assert_eq!(fixture.source.as_ref().unwrap().process_count(), 0);
        }
        fixture.source.as_ref().unwrap().release_prepare();
        while !fixture.parent.paused() {
            fixture.parent.tick(deadline).unwrap_or_else(|error| {
                panic!("{error}: {}", fixture.parent.source_readiness_evidence())
            });
            std::thread::yield_now();
        }
        assert!(fixture.parent.app().pending_generator_chains.is_empty());
        assert_eq!(
            fixture.parent.app().running_generator_chains[&channel]
                .guard
                .genuine_worker_id(),
            worker
        );
        assert_eq!(
            fixture
                .parent
                .app()
                .timeline_generator_routes_state(&timeline),
            RequiredGeneratorRouteState::Ready
        );
        assert!(
            fixture
                .parent
                .callback()
                .unwrap()
                .genuine_snapshot()
                .graph_binding_exact
        );
        before.unchanged(fixture.parent.app(), id).unwrap();
    }

    #[test]
    fn full_owner_device_free_tick_reaches_real_exact_paused_timeline_and_late_commands() {
        let _admission = crate::app::clip_render_job::tests::fixture_admission();
        // No source/native/helper is constructed. Exercise the real App
        // compiler, Timeline receipt state machine and callback, not a setter
        // for generation, epoch, compiled graph or paused acceptance.
        let mut owner = model_owner();
        owner.app_mut().project = Project::blank();
        owner.app_mut().transport_mode = TransportMode::Song;
        let (engine, mut callback) = AudioEngine::test_engine_with_callback();
        callback.enable_genuine_observation([0x51; 32]).unwrap();
        owner.app_mut().audio = Some(engine);
        owner.callback = Some(callback);
        let session = owner.app().project_session;
        assert!(
            owner
                .app_mut()
                .audio
                .as_mut()
                .unwrap()
                .set_project_session(session)
        );
        owner.app_mut().rebuild_tempo_map();
        owner.app_mut().request_immediate_timeline_compile();
        assert!(
            !owner.paused(),
            "an empty fallback is not an exact paused Timeline"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !owner.paused() {
            owner
                .tick(deadline)
                .unwrap_or_else(|error| panic!("{error}: {}", owner.source_readiness_evidence()));
            std::thread::yield_now();
        }
        let ready = owner.source_readiness_evidence();
        assert_eq!(ready["exact_paused"], true);
        assert!(ready["confirmed_identity"].is_array());
        assert_eq!(ready["callback"]["paused_work_quiescent"], true);
        assert_eq!(ready["timeline_compile_error"], Value::Null);
        assert!(owner.app().running_generator_chains.is_empty());
        assert!(owner.app().pending_generator_chains.is_empty());

        // A later queued command is visible to the live snapshot even though
        // no legacy transport barrier changes. Keep the existing pump gate.
        let barrier = owner
            .app()
            .audio
            .as_ref()
            .unwrap()
            .test_transport_request_id();
        let command = mixer_control_commands(&owner.app().project)
            .into_iter()
            .next()
            .unwrap();
        assert!(owner.app_mut().audio.as_mut().unwrap().command(command));
        assert_eq!(
            owner
                .app()
                .audio
                .as_ref()
                .unwrap()
                .test_transport_request_id(),
            barrier
        );
        assert!(
            owner
                .callback()
                .unwrap()
                .genuine_snapshot()
                .callback_work_pending()
        );
        let pumps = owner.pumps;
        owner.tick(deadline).unwrap();
        assert_eq!(owner.pumps, pumps + 1);
        assert!(owner.paused());
    }

    #[test]
    fn full_owner_readiness_diagnostics_keep_missing_identity_and_actual_errors_without_pumping() {
        let mut owner = model_owner();
        owner.app_mut().timeline_compile_error = Some("original compile failure".into());
        owner.app_mut().timeline_audio_sync.last_error = Some("original sync failure".into());
        let before = owner.pumps;
        let absent = owner.source_readiness_evidence();
        assert_eq!(absent["callback"], Value::Null);
        assert_eq!(absent["confirmed_identity"], Value::Null);
        assert_eq!(
            absent["timeline_compile_error"]["text"],
            "original compile failure"
        );
        assert_eq!(
            absent["timeline_sync_error"]["text"],
            "original sync failure"
        );
        let (engine, mut callback) = AudioEngine::test_engine_with_callback();
        callback.enable_genuine_observation([0x52; 32]).unwrap();
        owner.app_mut().audio = Some(engine);
        owner.callback = Some(callback);
        let observed = owner.source_readiness_evidence();
        assert!(observed["callback"].is_object());
        assert_eq!(observed["callback"]["generation"], Value::Null);
        assert_eq!(observed["callback"]["paused_work_quiescent"], Value::Null);
        assert_eq!(observed["exact_paused"], false);
        assert_eq!(owner.pumps, before);
    }

    #[test]
    fn full_owner_pausing_keeps_user_history_dirty_flag_and_saved_path() {
        let mut owner = model_owner();
        let app = owner.app_mut();
        app.playing = true;
        app.transport_clock_playing = true;
        owner.keep_paused().unwrap();
        assert!(!owner.app().playing && !owner.app().transport_clock_playing);
        assert_eq!(owner.app().undo_stack.len(), 2);
        assert_eq!(owner.app().redo_stack.len(), 1);
        assert!(owner.app().dirty);
        assert!(owner.app().saved_path.is_some());
        assert!(owner.source_ready().is_err());
    }

    #[test]
    fn full_owner_pending_drop_retains_full_synthetic_owner_and_actual_callback() {
        // No native source is spawned here. This tests the failure ownership
        // policy with a real device-free callback, not a fabricated join.
        let mut owner = model_owner();
        owner.initialization_requested = true;
        let (engine, callback) = AudioEngine::test_engine_with_callback();
        owner.app_mut().audio = Some(engine);
        owner.callback = Some(callback);
        FULL_OWNER_QUARANTINED.with(|slot| assert!(slot.borrow().is_none()));
        drop(RetainedFullParent {
            owner: Some(ManuallyDrop::new(owner)),
        });
        FULL_OWNER_QUARANTINED.with(|slot| {
            let mut slot = slot.borrow_mut();
            let retained = slot.as_ref().expect("pending complete owner retained");
            assert!(retained.callback.is_some());
            assert!(!retained.joins_complete);
            retained.model_unchanged().unwrap();
            assert_eq!(retained.app().undo_stack.len(), 2);
            assert_eq!(retained.app().redo_stack.len(), 1);
            // This fixture never constructed a native worker or render job.
            // Release only these known synthetic test resources explicitly.
            let mut retained = ManuallyDrop::into_inner(slot.take().unwrap());
            assert!(retained.app().running_generator_chains.is_empty());
            assert!(retained.app().pending_generator_chains.is_empty());
            retained.callback.take();
            drop(retained);
        });
    }

    #[test]
    fn full_owner_completed_resource_drop_does_not_quarantine_failed_model() {
        // The model may fail after the resource-completion boundary. Simulate
        // that boundary without claiming a native worker join in this test.
        let mut owner = model_owner();
        owner.initialization_requested = true;
        owner.joins_complete = true;
        owner.app_mut().project.patterns[0].notes[0].velocity = 0.01;
        assert!(owner.model_unchanged().is_err());
        drop(RetainedFullParent {
            owner: Some(ManuallyDrop::new(owner)),
        });
        FULL_OWNER_QUARANTINED.with(|slot| assert!(slot.borrow().is_none()));
    }

    #[test]
    fn sdk_b_short_saved_request_does_not_manufacture_scan_or_loaded_metadata() {
        let (mut app, source, _) = fixture();
        let before = FullHistoryImage::take(&app, 17).unwrap();
        assert!(source.descriptor.vst3_metadata.is_none());
        let intent = requested_saved_source_intent(&app.project, &source).unwrap();
        assert_eq!(intent.2, KONTAKT_UID);
        assert!(complete_source_intent(&app.project, &source, &source.descriptor).is_err());
        register_and_enqueue_source_intent(
            &mut app,
            &source,
            intent,
            |app, channel, candidate, previous| {
                assert_eq!(channel, source.channel_id);
                assert_eq!(previous.unwrap().instance_id, Some(17));
                let original = app
                    .project
                    .plugin_instances
                    .iter()
                    .find(|item| item.id == 17)
                    .unwrap();
                let queued = candidate
                    .plugin_instances
                    .iter()
                    .find(|item| item.id == 17)
                    .unwrap();
                assert_eq!(queued.opaque_state, original.opaque_state);
                assert_eq!(queued.parameters, original.parameters);
                assert!(app.running_generator_chains.is_empty());
                assert_eq!(
                    app.fresh_bounce_sources[&17].2,
                    FreshSourceClass::Exact(KONTAKT_UID.into())
                );
                Ok(())
            },
        )
        .unwrap();
        before.unchanged(&app, 17).unwrap();
    }

    #[test]
    fn sdk_b_short_saved_request_still_refuses_changed_binding_before_enqueue() {
        for case in 0..3 {
            let (app, mut source, _) = fixture();
            match case {
                0 => source.state_sha256 = "0".repeat(64),
                1 => source.channel_id = 9,
                _ => source.descriptor.path = std::env::temp_dir().join("foreign.vst3"),
            }
            assert!(requested_saved_source_intent(&app.project, &source).is_err());
            assert!(app.fresh_bounce_sources.is_empty());
        }
    }
}
