//! Driver-only evidence around the single actual review entry. No preparation,
//! admission replay, runtime query, callback work, or replacement App state.
use super::*;

const TEXT_BYTES: usize = 1024;
const SELECTION_IDS: usize = 16;

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
