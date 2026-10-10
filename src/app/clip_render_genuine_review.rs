//! Private bounded review evidence and full-schema Mixer setup helpers. Review
//! observations never replay admission or query a runtime. Genuine setup drains
//! remain in the existing driver; the separate device-free command test has no
//! plugin endpoint and provides no genuine source or App execution evidence.
use super::*;

const TEXT_BYTES: usize = 1024;
const SELECTION_IDS: usize = 16;
const MIXER_BATCH_COMMANDS: usize = 32;
const MIXER_NODES: usize = 32;
const MIXER_ROUTES: usize = 128;

pub(super) fn mixer_command_batches(
    commands: Vec<AudioCommand>,
) -> impl Iterator<Item = Vec<AudioCommand>> {
    let mut commands = commands.into_iter();
    std::iter::from_fn(move || {
        let batch = commands
            .by_ref()
            .take(MIXER_BATCH_COMMANDS)
            .collect::<Vec<_>>();
        (!batch.is_empty()).then_some(batch)
    })
}
pub(super) fn admit_mixer_batch(
    batch: Vec<AudioCommand>,
    mut admit: impl FnMut(AudioCommand) -> bool,
) -> Check {
    require(
        !batch.is_empty() && batch.len() <= MIXER_BATCH_COMMANDS,
        "initial mixer command batch is empty or exceeds 32",
    )?;
    for (index, command) in batch.into_iter().enumerate() {
        require(
            admit(command),
            &format!("initial mixer command {index} refused; no retry"),
        )?;
    }
    Ok(())
}
pub(super) fn require_fixture_graph(
    nodes: &[crate::mixer_graph::CompiledMixerNode],
    routes: &[crate::mixer_graph::CompiledMixerRoute],
) -> Check {
    require(
        nodes.len() == MIXER_NODES
            && (0..MIXER_NODES).all(|slot| {
                nodes
                    .iter()
                    .filter(|node| {
                        node.id == crate::model::mixer_track_id_for_runtime_slot(slot as u8)
                            && usize::from(node.runtime_slot) == slot
                            && node.project_index == slot
                    })
                    .count()
                    == 1
            }),
        "actual full32 unique stable Mixer node/slot mapping differs",
    )?;
    require(
        routes.len() == 1,
        "actual sole audible Mixer route count differs",
    )?;
    let route = &routes[0];
    require(
        route.id == 1
            && route.runtime_slot == 0
            && route.source_id == 1
            && route.destination_id == MASTER_MIXER_TRACK_ID
            && route.source_runtime_slot == 1
            && route.destination_runtime_slot == 0
            && route.source_dense == 1
            && route.destination_dense == 0
            && route.tap == MixerRouteTap::PostFader
            && route.gain.to_bits() == 1.0_f32.to_bits(),
        "actual unity source-to-master route differs",
    )
}
pub(super) fn require_mixer_controls(
    project: &Project,
    snapshot: &GenuineCallbackSnapshot,
) -> Check {
    require(
        project.mixer_tracks.len() == MIXER_NODES,
        "full32 Mixer Project controls absent",
    )?;
    for (slot, track) in project.mixer_tracks.iter().enumerate() {
        require(
            usize::from(track.runtime_slot) == slot
                && track.id == crate::model::mixer_track_id_for_runtime_slot(slot as u8)
                && snapshot.track_gains[slot].to_bits() == track.volume.to_bits()
                && snapshot.track_pans[slot].to_bits() == track.pan.to_bits()
                && snapshot.track_muted[slot] == track.muted
                && snapshot.track_solo[slot] == track.solo,
            &format!("actual Mixer slot {slot} gain/pan/mute/solo differs from Project"),
        )?;
    }
    require(
        snapshot.master_gain.to_bits() == project.mixer_tracks[0].volume.to_bits()
            && snapshot.master_pan.to_bits() == project.mixer_tracks[0].pan.to_bits(),
        "actual Master gain/pan differs from Project",
    )
}
pub(super) fn mixer_evidence(snapshot: &GenuineCallbackSnapshot) -> Value {
    let nodes = snapshot.nodes.iter().take(MIXER_NODES).map(|node| {
        json!({"id":node.id,"runtime_slot":node.runtime_slot,"project_index":node.project_index})
    }).collect::<Vec<_>>();
    let routes = snapshot
        .routes
        .iter()
        .take(MIXER_ROUTES)
        .map(|route| {
            json!({"id":route.id,"runtime_slot":route.runtime_slot,
            "source_id":route.source_id,"destination_id":route.destination_id,
            "source_dense":route.source_dense,"destination_dense":route.destination_dense,
            "source_runtime_slot":route.source_runtime_slot,
            "destination_runtime_slot":route.destination_runtime_slot,
            "tap":format!("{:?}",route.tap),"gain_bits":route.gain.to_bits()})
        })
        .collect::<Vec<_>>();
    let controls = (0..MIXER_NODES)
        .map(|slot| {
            json!({"runtime_slot":slot,"gain_bits":snapshot.track_gains[slot].to_bits(),
            "pan_bits":snapshot.track_pans[slot].to_bits(),
            "muted":snapshot.track_muted[slot],"solo":snapshot.track_solo[slot]})
        })
        .collect::<Vec<_>>();
    json!({"node_count":snapshot.nodes.len(),"nodes":nodes,
        "node_overflow":snapshot.nodes.len().saturating_sub(MIXER_NODES),
        "route_count":snapshot.routes.len(),"routes":routes,
        "route_overflow":snapshot.routes.len().saturating_sub(MIXER_ROUTES),
        "controls":controls,"master_gain_bits":snapshot.master_gain.to_bits(),
        "master_pan_bits":snapshot.master_pan.to_bits(),
        "pending_audio_commands":snapshot.pending_audio_commands})
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextObservation {
    text: String,
    original_bytes: usize,
}
impl TextObservation {
    fn new(text: &str) -> Self {
        let mut end = text.len().min(TEXT_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            text: text[..end].to_owned(),
            original_bytes: text.len(),
        }
    }
    fn truncated(&self) -> bool {
        self.text.len() != self.original_bytes
    }
    fn evidence(&self) -> Value {
        json!({"text":self.text,"original_bytes":self.original_bytes,
            "retained_bytes":self.text.len(),"truncated":self.truncated()})
    }
}
fn optional_text(text: Option<&str>) -> Value {
    text.map(|text| TextObservation::new(text).evidence())
        .unwrap_or(Value::Null)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ToastObservation {
    text: TextObservation,
    created: Instant,
}
fn toast_observation(toast: Option<&(String, Instant)>) -> Option<ToastObservation> {
    toast.map(|(text, created)| ToastObservation {
        text: TextObservation::new(text),
        created: *created,
    })
}
fn toast_evidence(toast: Option<&ToastObservation>) -> Value {
    match toast {
        Some(toast) => json!({"present":true,"message":toast.text.evidence(),
            "created":TextObservation::new(&format!("{:?}",toast.created)).evidence()}),
        None => json!({"present":false,"message":null,"created":null}),
    }
}
fn toast_changed(
    before: Option<&ToastObservation>,
    after: Option<&ToastObservation>,
) -> Option<bool> {
    match (before, after) {
        (None, None) => Some(false),
        (Some(_), None) | (None, Some(_)) => Some(true),
        (Some(before), Some(after)) if before != after => Some(true),
        // Equal bounded excerpts cannot prove equality of omitted bytes. This
        // remains unknown even if the App creation Instant was also unchanged.
        (Some(before), Some(_)) if before.text.truncated() => None,
        (Some(_), Some(_)) => Some(false),
    }
}
fn selection_evidence(selected: Option<u32>, ids: &HashSet<u32>) -> Value {
    let mut retained = ids.iter().copied().take(SELECTION_IDS).collect::<Vec<_>>();
    retained.sort_unstable();
    json!({"inspector_clip_id":selected,"playlist_ids":retained,
        "playlist_count":ids.len(),"id_limit":SELECTION_IDS,
        "overflow":ids.len().saturating_sub(SELECTION_IDS),
        "truncated":ids.len()>SELECTION_IDS,
        "inspector_in_playlist":selected.map(|id|ids.contains(&id))})
}

pub(super) struct ReviewObservation {
    pub evidence: Value,
    toast: Option<ToastObservation>,
}
impl ReviewObservation {
    pub fn missing_job_error(&self) -> String {
        format!(
            "actual selected review refused; bounded actual App toast observation: {}",
            json!({"toast":self.evidence["toast"],
                "toast_changed":self.evidence["toast_changed"],
                "toast_comparison":self.evidence["toast_comparison"]})
        )
    }
}

pub(super) fn observe(app: &CitrusApp, before: Option<&ReviewObservation>) -> ReviewObservation {
    let toast = toast_observation(app.toast.as_ref());
    let changed = before.and_then(|before| toast_changed(before.toast.as_ref(), toast.as_ref()));
    let comparison = match (before.is_some(), changed) {
        (false, _) => "pre_call_baseline",
        (true, None) => "indeterminate_equal_truncated_excerpts_and_creation_instant",
        (true, Some(_)) => "presence_creation_instant_and_bounded_text",
    };
    let selected = app
        .selected_clip
        .and_then(|id| app.project.clips.iter().find(|clip| clip.id == id));
    let selection = selection_evidence(app.selected_clip, &app.playlist_selection_ids);
    let selected_clip = selected.map(|clip| {
        json!({"id":clip.id,"kind":format!("{:?}",clip.kind),"muted":clip.muted,
            "pattern_id":clip.pattern_id,"track":clip.track,
            "clip_match_count":app.project.clips.iter().filter(|other|other.id==clip.id).count(),
            "pattern_match_count":app.project.patterns.iter().filter(|p|p.id==clip.pattern_id).count()})
    });
    let editing = json!({"legacy_piano_notes":app.project.piano_notes.len(),
        "parameter_edits_reserved":app.plugin_parameter_edits.reserved_len(),
        "parameter_drafts":app.plugin_parameter_edit_draft_desired.len(),
        "playlist_gesture":app.playlist_gesture_before.is_some(),
        "piano_gesture":app.piano_roll_gesture_before.is_some(),
        "piano_transform":app.piano_roll_transform.is_some(),
        "piano_note_properties":app.piano_note_properties.is_some(),
        "automation_point_drag":app.automation_point_drag_active,
        "project_media_open":app.project_media.open,"recovery_available":app.recovery_available});
    let pending = json!({"audio_import":app.audio_import_receiver.is_some(),
        "midi_import":app.pending_midi_import.is_some(),"midi_export":app.pending_midi_export.is_some(),
        "save_barrier":app.save_barrier.is_some(),"queued_save":app.queued_save_request.is_some(),
        "queued_save_after_midi_recording":app.queued_save_after_midi_recording.is_some(),
        "plugin_slot_config":app.pending_plugin_slot_config.is_some(),
        "loading_assets":app.loading_audio_assets.len(),
        "pending_asset_registrations":app.pending_audio_asset_registrations.len(),
        "prepared_asset_registrations":app.prepared_audio_asset_registrations.len(),
        "pending_asset_unregistrations":app.pending_audio_asset_unregistrations.len(),
        "pending_asset_clear":app.pending_audio_asset_clear.is_some(),
        "pending_generators":app.pending_generator_chains.len(),
        "pending_generator_removals":app.pending_generator_removals.len(),
        "pending_generator_routes":app.pending_generator_routes.len(),
        "pending_clip_restorations":app.pending_clip_restorations.len()});
    let recording = json!({"recording":app.recording,
        "midi_disconnected":app.midi_input.state.is_disconnected(),
        "midi_recording_idle":app.midi_recording.is_idle(),
        "input_recorder":app.input_recorder.is_some(),"pending_recording":app.pending_recording.is_some(),
        "master_capture":app.master_capture.is_some(),"pending_master_capture":app.pending_master_capture.is_some()});
    let timeline = json!({"desired_generation":app.timeline_desired_generation,
        "pending_generation":app.timeline_pending_generation,
        "compiled_generation":app.timeline_compiled_generation,
        "compile_error":optional_text(app.timeline_compile_error.as_deref()),
        "tempo_map_error":optional_text(app.tempo_map_error.as_deref())});
    let job = app.clip_render_job.as_ref().map(|job| {
        json!({"present":true,"phase":format!("{:?}",job.phase),
            "status_visible":job.status_visible,"status":TextObservation::new(job.label()).evidence(),
            "failure":optional_text(job.failure.as_deref()),
            "secondary_failure":optional_text(job.secondary_failure.as_deref())})
    }).unwrap_or_else(||json!({"present":false,"phase":null,"status_visible":null,
        "status":null,"failure":null,"secondary_failure":null}));
    let evidence = json!({"evidence_scope":"direct_App_fields_only_not_full_admission_or_runtime_readiness",
        "selection":selection,"selected_clip":selected_clip,
        "eligibility_inputs":{"song_mode":app.transport_mode==TransportMode::Song,
            "playing":app.playing,"audio_present":app.audio.is_some(),
            "model_revision":app.clip_render_model_revision,"control_revision":app.clip_render_control_revision,
            "editing":editing,"pending":pending,"recording":recording},
        "timeline":timeline,"job":job,"toast":toast_evidence(toast.as_ref()),
        "toast_changed":changed,"toast_comparison":comparison});
    ReviewObservation { evidence, toast }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{NoteSourceDescriptor, TimelineEvent, TimelineEventKind};

    fn command_key(command: &AudioCommand) -> (u8, usize, u32) {
        match command {
            AudioCommand::SetTrackGain { track, gain } => (0, *track, gain.to_bits()),
            AudioCommand::SetTrackPan { track, pan } => (1, *track, pan.to_bits()),
            AudioCommand::SetTrackMuted { track, muted } => (2, *track, u32::from(*muted)),
            AudioCommand::SetTrackSolo { track, solo } => (3, *track, u32::from(*solo)),
            AudioCommand::SetMaster(gain) => (4, 0, gain.to_bits()),
            AudioCommand::SetMasterPan(pan) => (5, 0, pan.to_bits()),
            _ => panic!("unexpected non-Mixer production command"),
        }
    }

    #[test]
    fn full_mixer_commands_use_five_checked_batches_without_retry() -> Check {
        let project = fixture_project();
        let commands = mixer_control_commands(&project);
        assert_eq!(commands.len(), 130);
        let mut expected = Vec::new();
        for (slot, track) in project.mixer_tracks.iter().enumerate() {
            expected.extend([
                (0, slot, track.volume.to_bits()),
                (1, slot, track.pan.to_bits()),
                (2, slot, u32::from(track.muted)),
                (3, slot, u32::from(track.solo)),
            ]);
        }
        expected.extend([(4, 0, 1.0_f32.to_bits()), (5, 0, 0.0_f32.to_bits())]);
        let mut admitted = Vec::new();
        let mut sizes = Vec::new();
        for batch in mixer_command_batches(commands) {
            sizes.push(batch.len());
            admit_mixer_batch(batch, |command| {
                admitted.push(command_key(&command));
                true
            })?;
        }
        assert_eq!(sizes, [32, 32, 32, 32, 2]);
        assert_eq!(admitted, expected);
        let mut attempts = 0;
        let batch = mixer_command_batches(mixer_control_commands(&project))
            .next()
            .unwrap();
        let failed = admit_mixer_batch(batch, |_| {
            attempts += 1;
            attempts != 3
        });
        assert_eq!(
            attempts, 3,
            "a refusal must not retry or submit later commands"
        );
        assert_eq!(
            failed.unwrap_err(),
            "initial mixer command 2 refused; no retry"
        );
        assert!(admit_mixer_batch(Vec::new(), |_| panic!("empty batch admission")).is_err());
        assert!(
            admit_mixer_batch(mixer_control_commands(&project), |_| panic!(
                "oversized batch admission"
            ))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn full_mixer_graph_keeps_32_unique_nodes_and_one_audible_route() -> Check {
        let project = fixture_project();
        project.validate_mixer_graph().map_err(error)?;
        let graph = crate::mixer_graph::compile_mixer_graph(&project).map_err(error)?;
        require_fixture_graph(graph.nodes(), graph.routes())?;
        assert!(require_fixture_graph(&graph.nodes()[..31], graph.routes()).is_err());
        let mut nodes = graph.nodes().to_vec();
        nodes[31] = nodes[30];
        assert!(require_fixture_graph(&nodes, graph.routes()).is_err());
        nodes = graph.nodes().to_vec();
        nodes.push(nodes[0]);
        assert!(require_fixture_graph(&nodes, graph.routes()).is_err());
        let mut routes = graph.routes().to_vec();
        routes.push(routes[0]);
        assert!(require_fixture_graph(graph.nodes(), &routes).is_err());
        routes.pop();
        routes[0].gain = 0.5;
        assert!(require_fixture_graph(graph.nodes(), &routes).is_err());
        routes[0] = graph.routes()[0];
        routes[0].source_id = 2;
        assert!(require_fixture_graph(graph.nodes(), &routes).is_err());
        Ok(())
    }

    #[test]
    fn full_mixer_real_callback_drains_commands_and_reads_all_controls_without_plugins() -> Check {
        // Device-free command-path evidence only, not a genuine App or source run.
        let project = fixture_project();
        let (mut engine, mut callback) = AudioEngine::test_engine_with_callback();
        callback
            .enable_genuine_observation([41; 32])
            .map_err(error)?;
        assert!(engine.command(AudioCommand::SetTrackGain {
            track: 1,
            gain: 0.5
        }));
        let mut pumps = 0;
        let mut admitted = 0;
        for batch in mixer_command_batches(mixer_control_commands(&project)) {
            if callback.genuine_snapshot().pending_audio_commands != 0 {
                callback.pump(&engine, 128);
                pumps += 1;
                assert_eq!(callback.genuine_snapshot().pending_audio_commands, 0);
            }
            admitted += batch.len();
            admit_mixer_batch(batch, |command| engine.command(command))?;
            assert!(callback.genuine_snapshot().pending_audio_commands <= MIXER_BATCH_COMMANDS);
        }
        assert_eq!(callback.genuine_snapshot().pending_audio_commands, 2);
        callback.pump(&engine, 128);
        pumps += 1;
        let snapshot = callback.genuine_snapshot();
        assert_eq!(
            pumps, 6,
            "one pre-existing command drain plus five production batches"
        );
        assert_eq!(admitted, 130);
        assert_eq!(engine.snapshot().command_queue_full, 0);
        assert_eq!(snapshot.pending_audio_commands, 0);
        assert!(snapshot.endpoints.is_empty() && snapshot.native_voices.is_empty());
        assert!(!snapshot.playing);
        let segments = callback.take_genuine_segments();
        assert_eq!(segments.len(), pumps);
        assert!(segments.iter().all(|segment| !segment.playing
            && segment.frames == 128
            && segment.disposition == GenuineSegmentDisposition::Rendered
            && segment.before.fault.is_none()
            && segment.after.fault.is_none()));
        require_mixer_controls(&project, &snapshot)?;
        let evidence = mixer_evidence(&snapshot);
        assert_eq!(evidence["controls"].as_array().unwrap().len(), 32);
        let mutations: [fn(&mut GenuineCallbackSnapshot); 6] = [
            |s| s.track_gains[31] = 0.0,
            |s| s.track_pans[31] = 0.5,
            |s| s.track_muted[31] = true,
            |s| s.track_solo[31] = true,
            |s| s.master_gain = 0.0,
            |s| s.master_pan = 0.5,
        ];
        for mutate in mutations {
            let mut changed = snapshot.clone();
            mutate(&mut changed);
            assert!(require_mixer_controls(&project, &changed).is_err());
        }
        Ok(())
    }

    #[test]
    fn review_text_observation_is_utf8_bounded_with_exact_length() {
        let short = TextObservation::new("exact refusal");
        assert_eq!(short.evidence()["text"], "exact refusal");
        assert!(!short.truncated());
        let long = format!("{}é refusal", "x".repeat(TEXT_BYTES - 1));
        let observed = TextObservation::new(&long);
        assert_eq!(observed.text.len(), TEXT_BYTES - 1);
        assert_eq!(observed.original_bytes, long.len());
        assert!(observed.truncated());
        assert_eq!(observed.evidence()["retained_bytes"], TEXT_BYTES - 1);
        let at = Instant::now();
        let before = toast_observation(Some(&(long.clone(), at)));
        let after = toast_observation(Some(&(format!("{long}other omitted bytes"), at)));
        assert_eq!(toast_changed(before.as_ref(), before.as_ref()), None);
        assert_eq!(toast_changed(before.as_ref(), after.as_ref()), Some(true));
    }

    #[test]
    fn review_missing_toast_stays_missing() {
        assert_eq!(
            toast_evidence(None),
            json!({"present":false,"message":null,"created":null})
        );
        assert_eq!(toast_changed(None, None), Some(false));
        let before = toast_observation(Some(&("older toast".into(), Instant::now())));
        assert_eq!(toast_changed(before.as_ref(), None), Some(true));
    }

    #[test]
    fn review_stale_toast_is_retained_without_new_refusal_claim() {
        let toast = toast_observation(Some(&("unrelated earlier toast".into(), Instant::now())));
        let changed = toast_changed(toast.as_ref(), toast.as_ref());
        assert_eq!(changed, Some(false));
        let observed = ReviewObservation {
            evidence: json!({"toast":toast_evidence(toast.as_ref()),"toast_changed":changed,
                "toast_comparison":"presence_creation_instant_and_bounded_text"}),
            toast,
        };
        let failure = observed.missing_job_error();
        assert!(failure.contains("unrelated earlier toast"));
        assert!(failure.contains("\"toast_changed\":false"));
    }

    #[test]
    fn review_changed_toast_includes_reissued_identical_text() {
        let at = Instant::now();
        let before = toast_observation(Some(&("same text".into(), at)));
        let reissued = toast_observation(Some(&("same text".into(), at + Duration::from_nanos(1))));
        let changed_text = toast_observation(Some(&("new exact refusal".into(), at)));
        assert_eq!(
            toast_changed(before.as_ref(), reissued.as_ref()),
            Some(true)
        );
        assert_eq!(
            toast_changed(before.as_ref(), changed_text.as_ref()),
            Some(true)
        );
        assert_eq!(toast_changed(None, before.as_ref()), Some(true));
    }

    #[test]
    fn review_selection_ids_are_bounded_without_repairing_mismatch() {
        let ids = (1..=20).collect::<HashSet<_>>();
        let observed = selection_evidence(Some(99), &ids);
        assert_eq!(observed["inspector_clip_id"], 99);
        assert_eq!(observed["playlist_count"], 20);
        assert_eq!(
            observed["playlist_ids"].as_array().unwrap().len(),
            SELECTION_IDS
        );
        assert_eq!(observed["overflow"], 4);
        assert_eq!(observed["truncated"], true);
        assert_eq!(observed["inspector_in_playlist"], false);
        assert_eq!(
            selection_evidence(None, &HashSet::new())["inspector_in_playlist"],
            Value::Null
        );
    }

    #[test]
    fn genuine_fixture_pure_review_admission() -> Check {
        // This is synthetic pure-planner evidence, never actual runtime evidence.
        // The primary case is the exact provisional zero-tail review boundary;
        // the second case is the existing later acknowledged 0.25-second choice.
        for (case, tail_frames) in [("provisional_review", 0), ("acknowledged_choice", 12_000)] {
            let mut project = fixture_project();
            let instance_id = commit_loaded_plugin(
                &mut project,
                PluginPickerTarget::ChannelDevice { channel: 0 },
                &descriptor(),
            )
            .map_err(|reason| format!("{case}: pure assignment: {reason}"))?;
            let plugin = project
                .plugin_instances
                .iter_mut()
                .find(|p| p.id == instance_id)
                .ok_or("pure assignment did not retain its source instance")?;
            plugin.parameters.insert(GLOBAL_VOLUME, 0.25);
            plugin.opaque_state.clear();
            assert_eq!(plugin.runtime_status, PluginRuntimeStatus::Unloaded);
            let clip_id = project.clips[0].id;
            let pattern_id = project.patterns[0].id;
            let channel_id = project.channels[0].id;
            let synthetic_stamp = OccurrenceStamp {
                project_session: 1,
                project_revision: 2,
                timeline_revision: 3,
                epoch: 4,
                engine_session: 5,
                endpoint_id: 6,
                channel_id,
                instance_id,
                mixer_track_id: project.channels[0].mixer_track,
                native_generation: 7,
                app_native_dirty_revision: 8,
                native_capture_serial: 9,
                parameter_capture_serial: 10,
                native_control_request: 11,
                parameter_control_request: 12,
            };
            let snapshot = clip_render::project_snapshot(&project)
                .map_err(|reason| format!("{case}: exact Project snapshot: {reason}"))?;
            let acknowledgment = Arc::new(
                FixedTailAcknowledgment::review(
                    snapshot,
                    synthetic_stamp,
                    clip_id,
                    48_000,
                    tail_frames,
                )
                .map_err(|reason| {
                    format!("{case}: synthetic fixed-tail acknowledgment: {reason}")
                })?,
            );
            let plan = OccurrencePlan::prepare(
                &project,
                synthetic_stamp,
                clip_id,
                48_000,
                tail_frames,
                Some(acknowledgment),
            )
            .map_err(|reason| format!("{case}: exact fixture pure admission: {reason}"))?;
            let request = plan.request();
            assert_eq!(
                (
                    request.start_frame(),
                    request.end_frame(),
                    request.tail_frames()
                ),
                (30_000, 54_000, tail_frames),
                "{case}"
            );
            assert_eq!(request.sample_rate(), 48_000, "{case}");
            assert_eq!(request.tempo(), 120.0, "{case}");
            assert_eq!(request.stamp(), synthetic_stamp, "{case}");
            let source = NoteSourceDescriptor::LegacyPiano {
                clip_id,
                pattern_id,
                persistent_note_id: 1,
                note_index: 0,
                repetition: 0,
            };
            assert_eq!(
                request.local_events(),
                &[
                    TimelineEvent {
                        frame: 0,
                        kind: TimelineEventKind::NoteOn {
                            note_id: 1,
                            channel_id,
                            note: 60,
                            velocity: 100.0 / 127.0,
                            gain: 1.0,
                            mixer_track: 1,
                            source,
                        }
                    },
                    TimelineEvent {
                        frame: 12_000,
                        kind: TimelineEventKind::NoteOff {
                            note_id: 1,
                            channel_id,
                            note: 60,
                            mixer_track: 1,
                            source,
                        }
                    },
                ],
                "{case}"
            );
        }
        Ok(())
    }
}
