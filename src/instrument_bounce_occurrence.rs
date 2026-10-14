//! The occurrence capture bridge owns one bounded attestation, never a source endpoint.
//! Completed private receipt tokens distinguish recapture from reuse of the first reply.
use super::{
    BouncePluginMetadata, BounceSourceIdentity, CapturedInstrumentBounce, MAX_BOUNCE_STATE_BYTES,
};
use crate::{
    model::Project,
    occurrence_request::OccurrenceRenderRequest,
    timeline::{CompiledTimeline, NoteSourceDescriptor, TimelineEvent, TimelineEventKind},
};
use std::{io::Write, sync::Arc};

pub(super) fn check_project_snapshot(project: &Project, expected: &[u8]) -> Result<(), String> {
    struct Compare<'a> {
        expected: &'a [u8],
        offset: usize,
    }
    impl Write for Compare<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let end = self
                .offset
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("Snapshot size overflow"))?;
            if self.expected.get(self.offset..end) != Some(bytes) {
                return Err(std::io::Error::other(
                    "Project differs from the reviewed occurrence snapshot",
                ));
            }
            self.offset = end;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    if expected.is_empty() || expected.len() > 64 * 1024 * 1024 {
        return Err("Invalid occurrence snapshot size".into());
    }
    let mut compare = Compare {
        expected,
        offset: 0,
    };
    serde_json::to_writer(&mut compare, project)
        .map_err(|_| "Project differs from the reviewed occurrence snapshot".to_owned())?;
    if compare.offset != expected.len() {
        return Err("Project differs from the reviewed occurrence snapshot".into());
    }
    Ok(())
}

pub(super) fn check_compiled_request(
    request: &OccurrenceRenderRequest,
    timeline: &CompiledTimeline,
    instance_id: u64,
) -> Result<(), String> {
    if request.instance_id() != instance_id
        || request
            .end_frame()
            .checked_add(u64::from(request.tail_frames()))
            .is_none_or(|end| end > timeline.duration_frames())
        || request.tempo().to_bits() != timeline.plugin_transport_tempo().to_bits()
    {
        return Err("Occurrence request differs from the original compiled source".into());
    }
    let mut local = request.local_events().iter();
    for event in timeline.events() {
        let (channel, source) = match event.kind {
            TimelineEventKind::NoteOn {
                channel_id, source, ..
            }
            | TimelineEventKind::NoteOff {
                channel_id, source, ..
            } => (channel_id, source),
            _ => continue,
        };
        let (clip, pattern) = match source {
            NoteSourceDescriptor::ChannelStep {
                clip_id,
                pattern_id,
                ..
            }
            | NoteSourceDescriptor::LegacyPiano {
                clip_id,
                pattern_id,
                ..
            } => (clip_id, pattern_id),
        };
        if clip != request.clip_id() || pattern != request.pattern_id() {
            continue;
        }
        let frame = event
            .frame
            .checked_sub(request.start_frame())
            .ok_or("Selected source event precedes the exact occurrence")?;
        if channel != request.channel_id()
            || frame > request.song_frames()
            || local.next()
                != Some(&TimelineEvent {
                    frame,
                    kind: event.kind,
                })
        {
            return Err("Occurrence schedule omits or changes an original compiled event".into());
        }
    }
    if local.next().is_some() {
        return Err("Occurrence schedule adds an event absent from the original source".into());
    }
    Ok(())
}

struct CaptureProof {
    request: Arc<OccurrenceRenderRequest>,
    receipt_token: Arc<()>,
    identity: BounceSourceIdentity,
    metadata: BouncePluginMetadata,
    native_generation: u64,
    native_dirty_revision: u64,
}

/// Clone duplicates only a small immutable proof handle, never state or PCM.
#[derive(Clone)]
pub(crate) struct OccurrenceRenderProvenance(Arc<CaptureProof>);
impl std::fmt::Debug for OccurrenceRenderProvenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OccurrenceRenderProvenance")
            .field("identity", &self.0.identity)
            .field("clip_id", &self.request().clip_id())
            .field("state", &"redacted")
            .finish()
    }
}
impl PartialEq for OccurrenceRenderProvenance {
    fn eq(&self, other: &Self) -> bool {
        self.same_capture(other)
    }
}
impl Eq for OccurrenceRenderProvenance {}
impl OccurrenceRenderProvenance {
    pub(crate) fn request(&self) -> &OccurrenceRenderRequest {
        &self.0.request
    }
    pub(crate) fn request_handle(&self) -> &Arc<OccurrenceRenderRequest> {
        &self.0.request
    }
    pub(crate) fn identity(&self) -> BounceSourceIdentity {
        self.0.identity
    }
    pub(crate) fn metadata(&self) -> &BouncePluginMetadata {
        &self.0.metadata
    }
    pub(crate) fn native_generation(&self) -> u64 {
        self.0.native_generation
    }
    pub(crate) fn native_dirty_revision(&self) -> u64 {
        self.0.native_dirty_revision
    }
    pub(crate) fn same_capture(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0.receipt_token, &other.0.receipt_token)
            && Arc::ptr_eq(&self.0.request, &other.0.request)
    }
}

fn copy_bytes(bytes: &[u8], max: usize) -> Result<Vec<u8>, String> {
    if bytes.len() > max {
        return Err("Occurrence attestation exceeds its payload limit".into());
    }
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len())
        .map_err(|_| "Occurrence attestation allocation failed")?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}
fn copy_text(value: &str) -> Result<String, String> {
    String::from_utf8(copy_bytes(value.as_bytes(), 64 * 1024)?)
        .map_err(|_| "Invalid capture metadata UTF-8".into())
}
fn copy_list<T: Copy>(values: &[T], maximum: usize) -> Result<Vec<T>, String> {
    if values.len() > maximum {
        return Err("Occurrence metadata list exceeds its bound".into());
    }
    let mut copy = Vec::new();
    copy.try_reserve_exact(values.len())
        .map_err(|_| "Occurrence metadata allocation failed")?;
    copy.extend_from_slice(values);
    Ok(copy)
}
fn copy_metadata(
    value: &BouncePluginMetadata,
    sample_rate: u32,
) -> Result<BouncePluginMetadata, String> {
    // Select limits only after the existing loaded-identity and complete bus
    // acknowledgment policy validates. Copy every declaration without projection.
    value.validate(sample_rate)?;
    let (output_bound, event_output_bound) = if value.uses_selected_stereo_bounce() {
        (32, 1)
    } else {
        (3, 0)
    };
    Ok(BouncePluginMetadata {
        class_uid: copy_text(&value.class_uid)?,
        version: copy_text(&value.version)?,
        name: copy_text(&value.name)?,
        category: copy_text(&value.category)?,
        event_input_buses: copy_list(&value.event_input_buses, 1)?,
        event_output_buses: copy_list(&value.event_output_buses, event_output_bound)?,
        input_buses: copy_list(&value.input_buses, 1)?,
        output_buses: copy_list(&value.output_buses, output_bound)?,
        latency_samples: value.latency_samples,
        tail_samples: value.tail_samples,
    })
}

impl CapturedInstrumentBounce {
    pub(crate) fn occurrence_provenance(&self) -> Result<OccurrenceRenderProvenance, String> {
        let request = self
            .plan
            .occurrence_request()
            .ok_or("Capture was not prepared for a selected occurrence")?;
        let stamp = request.stamp();
        if self.identity.project_session != stamp.project_session
            || self.identity.timeline_revision != stamp.timeline_revision
            || self.identity.epoch != stamp.epoch
            || self.identity.endpoint_id != stamp.endpoint_id
            || self.identity.channel_id != stamp.channel_id
            || self.identity.instance_id != stamp.instance_id
            || self.state.native_generation != stamp.native_generation
        {
            return Err(
                "Actual occurrence capture differs from its reviewed source/runtime stamp".into(),
            );
        }
        self.state
            .validate(&self.plan.parameter_ids, request.sample_rate())?;
        Ok(OccurrenceRenderProvenance(Arc::new(CaptureProof {
            request: Arc::clone(request),
            receipt_token: Arc::clone(&self.receipt_token),
            identity: self.identity,
            metadata: copy_metadata(&self.state.metadata, request.sample_rate())?,
            native_generation: self.state.native_generation,
            native_dirty_revision: self.state.native_dirty_revision,
        })))
    }

    pub(crate) fn retain_occurrence_attestation(
        &self,
    ) -> Result<OccurrenceCaptureAttestation, String> {
        let provenance = self.occurrence_provenance()?;
        let opaque_state = copy_bytes(&self.state.opaque_state, MAX_BOUNCE_STATE_BYTES)?;
        let mut parameter_bases = Vec::new();
        parameter_bases
            .try_reserve_exact(self.state.parameter_bases.len())
            .map_err(|_| "Occurrence base attestation allocation failed")?;
        parameter_bases.extend(
            self.state
                .parameter_bases
                .iter()
                .map(|(&id, &value)| (id, value)),
        );
        Ok(OccurrenceCaptureAttestation {
            provenance,
            opaque_state,
            parameter_bases,
        })
    }
}

pub(crate) struct OccurrenceCaptureAttestation {
    provenance: OccurrenceRenderProvenance,
    opaque_state: Vec<u8>,
    parameter_bases: Vec<(u32, f32)>,
}
impl OccurrenceCaptureAttestation {
    pub(crate) fn provenance(&self) -> &OccurrenceRenderProvenance {
        &self.provenance
    }
    /// The caller obtains this SECOND completed private receipt after render and
    /// media verification while the source is still stopped and fenced. This
    /// method cannot manufacture an additional capture or retry a failed one.
    pub(crate) fn match_recapture(
        self,
        recapture: CapturedInstrumentBounce,
    ) -> Result<MatchedOccurrenceSource, String> {
        let current = recapture.occurrence_provenance()?;
        if Arc::ptr_eq(&self.provenance.0.receipt_token, &current.0.receipt_token)
            || !Arc::ptr_eq(&self.provenance.0.request, &current.0.request)
            || self.provenance.identity() != current.identity()
            || self.provenance.metadata() != current.metadata()
            || self.provenance.native_generation() != current.native_generation()
            || self.provenance.native_dirty_revision() != current.native_dirty_revision()
            || self.opaque_state != recapture.state.opaque_state
            || self.parameter_bases.len() != recapture.state.parameter_bases.len()
            || !self
                .parameter_bases
                .iter()
                .zip(recapture.state.parameter_bases.iter())
                .all(|(&(id, value), (&other_id, &other))| {
                    id == other_id && value.to_bits() == other.to_bits()
                })
        {
            return Err(
                "The second actual source capture differs from the consumed render capture".into(),
            );
        }
        // The second opaque payload is released here before Project image preparation.
        drop(recapture);
        Ok(MatchedOccurrenceSource { attestation: self })
    }
}

/// A sealed exact comparison result; no caller-provided state can construct it.
pub(crate) struct MatchedOccurrenceSource {
    attestation: OccurrenceCaptureAttestation,
}
impl std::fmt::Debug for MatchedOccurrenceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchedOccurrenceSource")
            .field("provenance", self.provenance())
            .field("state_bytes", &self.opaque_state().len())
            .finish()
    }
}
impl MatchedOccurrenceSource {
    pub(crate) fn provenance(&self) -> &OccurrenceRenderProvenance {
        &self.attestation.provenance
    }
    pub(crate) fn opaque_state(&self) -> &[u8] {
        &self.attestation.opaque_state
    }
    pub(crate) fn parameter_bases(&self) -> &[(u32, f32)] {
        &self.attestation.parameter_bases
    }
    pub(crate) fn metadata(&self) -> &BouncePluginMetadata {
        self.provenance().metadata()
    }
    pub(crate) fn identity(&self) -> BounceSourceIdentity {
        self.provenance().identity()
    }
}

#[cfg(test)]
pub(crate) fn test_capture_pair(
    project: &Project,
    request: Arc<OccurrenceRenderRequest>,
    opaque_state: Vec<u8>,
    parameter_bases: Vec<(u32, f32)>,
) -> (OccurrenceRenderProvenance, MatchedOccurrenceSource) {
    let first = test_capture(
        project,
        Arc::clone(&request),
        opaque_state.clone(),
        parameter_bases.clone(),
    );
    let attestation = first
        .retain_occurrence_attestation()
        .expect("fixture attestation");
    let provenance = first.occurrence_provenance().expect("fixture provenance");
    drop(first);
    let second = test_capture(project, request, opaque_state, parameter_bases);
    (
        provenance,
        attestation
            .match_recapture(second)
            .expect("two distinct fixture receipts match"),
    )
}

#[cfg(test)]
pub(crate) fn test_capture(
    project: &Project,
    request: Arc<OccurrenceRenderRequest>,
    opaque_state: Vec<u8>,
    parameter_bases: Vec<(u32, f32)>,
) -> CapturedInstrumentBounce {
    let stamp = request.stamp();
    let plan = super::InstrumentBouncePlan::prepare_occurrence(project, request)
        .expect("fixture occurrence preparation");
    let identity = BounceSourceIdentity {
        project_session: stamp.project_session,
        project_fingerprint: plan.project_fingerprint(),
        timeline_revision: stamp.timeline_revision,
        epoch: stamp.epoch,
        endpoint_id: stamp.endpoint_id,
        channel_id: stamp.channel_id,
        instance_id: stamp.instance_id,
    };
    CapturedInstrumentBounce {
        identity,
        plan,
        state: super::BouncePluginState {
            metadata: BouncePluginMetadata {
                class_uid: "00112233445566778899AABBCCDDEEFF".into(),
                version: "1".into(),
                name: "Fixture generator".into(),
                category: "Instrument".into(),
                event_input_buses: vec![(16, true, Some(true))],
                event_output_buses: vec![],
                input_buses: vec![],
                output_buses: vec![super::BounceAudioBus {
                    channel_count: 2,
                    is_main: true,
                    active: Some(true),
                }],
                latency_samples: 0,
                tail_samples: 0,
            },
            opaque_state,
            parameter_bases: parameter_bases.into_iter().collect(),
            native_generation: stamp.native_generation,
            native_dirty_revision: 1,
        },
        receipt_token: Arc::new(()),
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    use crate::{
        app::clip_render::test_prepare_occurrence_request,
        instrument_bounce::BounceAudioBus,
        model::{PluginFormat, PluginInstance, PluginRole, PluginRuntimeStatus},
        occurrence_request::OccurrenceStamp,
    };
    use std::collections::BTreeMap;

    fn selected_metadata() -> BouncePluginMetadata {
        BouncePluginMetadata {
            class_uid: "5653544E694B386B6F6E74616B742038".into(),
            version: "8.4.1".into(),
            name: "Selected policy fixture".into(),
            category: "Instrument|Sampler".into(),
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![(16, true, Some(true))],
            input_buses: vec![BounceAudioBus::new(2, true, Some(true))],
            output_buses: (0..32)
                .map(|index| BounceAudioBus::new(2, index % 3 == 0, Some(index == 0)))
                .collect(),
            latency_samples: 1,
            tail_samples: 7,
        }
    }

    fn source() -> (Project, Arc<OccurrenceRenderRequest>) {
        let mut project = Project {
            tempo: 120.0,
            swing: 0.0,
            song_length_beats: 4.0,
            ..Project::default()
        };
        project.automation_lanes.clear();
        project.channels.truncate(1);
        project.channels[0].instrument_plugin_instance_id = Some(41);
        project.patterns.truncate(1);
        project.patterns[0].notes.clear();
        project.patterns[0].length_steps = 4;
        project.patterns[0].channel_steps = vec![[false; 16]];
        project.patterns[0].channel_steps[0][0] = true;
        project.clips.truncate(1);
        project.clips[0].start = 0.0;
        project.clips[0].length = 1.0;
        project.clips[0].pattern_id = project.patterns[0].id;
        project.plugin_instances.push(PluginInstance {
            id: 41,
            format: PluginFormat::Vst3,
            role: PluginRole::Instrument,
            path: "fixture.vst3".into(),
            uid: "historical-path-id".into(),
            vendor: "Fixture".into(),
            name: "Fixture".into(),
            enabled: true,
            bypass: false,
            wet: 1.0,
            parameters: BTreeMap::from([(17, 0.25)]),
            opaque_state: vec![9],
            runtime_status: PluginRuntimeStatus::Loaded,
            midi_ports: Default::default(),
        });
        let stamp = OccurrenceStamp {
            project_session: 2,
            project_revision: 3,
            timeline_revision: 4,
            epoch: 7,
            engine_session: 5,
            endpoint_id: 19,
            channel_id: project.channels[0].id,
            instance_id: 41,
            mixer_track_id: project.channels[0].mixer_track,
            native_generation: 2,
            app_native_dirty_revision: 1,
            native_capture_serial: 0,
            parameter_capture_serial: 0,
            native_control_request: 10,
            parameter_control_request: 11,
        };
        let request =
            test_prepare_occurrence_request(&project, stamp, project.clips[0].id, 8000, 7).unwrap();
        (project, request)
    }

    fn capture(
        project: &Project,
        request: Arc<OccurrenceRenderRequest>,
        metadata: BouncePluginMetadata,
    ) -> CapturedInstrumentBounce {
        let mut captured = test_capture(project, request, vec![1, 2, 3], vec![(17, 0.25)]);
        captured.state.metadata = metadata;
        captured
    }

    #[test]
    fn occurrence_selected_copy_and_provenance_preserve_32_bus_roles_and_midi() {
        let (project, request) = source();
        let before = serde_json::to_vec(&project).unwrap();
        for midi_active in [false, true] {
            let mut metadata = selected_metadata();
            metadata.event_output_buses[0].2 = Some(midi_active);
            metadata.validate(8000).unwrap();
            assert_eq!(copy_metadata(&metadata, 8000).unwrap(), metadata);
            let first = capture(&project, Arc::clone(&request), metadata.clone());
            let provenance = first.occurrence_provenance().unwrap();
            let attestation = first.retain_occurrence_attestation().unwrap();
            assert_eq!(provenance.metadata(), &metadata);
            assert_eq!(attestation.provenance().metadata(), &metadata);
            assert_eq!(provenance.metadata().output_buses.len(), 32);
            assert_eq!(
                provenance.metadata().event_output_buses,
                [(16, true, Some(midi_active))]
            );
            let second = capture(&project, Arc::clone(&request), metadata.clone());
            let matched = attestation.match_recapture(second).unwrap();
            assert_eq!(matched.metadata(), &metadata);
            assert_eq!(matched.opaque_state(), &[1, 2, 3]);
            assert_eq!(matched.parameter_bases(), &[(17, 0.25)]);
            assert_eq!(first.metadata(), &metadata);
        }
        assert_eq!(serde_json::to_vec(&project).unwrap(), before);
    }

    #[test]
    fn occurrence_selected_copy_rejects_generic_fallback_and_malformed_policy_before_copy() {
        let selected = selected_metadata();
        let mut generic = selected.clone();
        generic.class_uid = "00112233445566778899AABBCCDDEEFF".into();
        generic.version = "1".into();
        generic.output_buses.truncate(3);
        generic.event_output_buses.clear();
        assert_eq!(copy_metadata(&generic, 8000).unwrap(), generic);
        for invalid in 0..16 {
            let mut metadata = selected.clone();
            match invalid {
                0 => metadata.class_uid = generic.class_uid.clone(),
                1 => metadata.version = "8.4.2".into(),
                2 => metadata.category = "Fx|Sampler".into(),
                3 => metadata.output_buses[31].active = None,
                4 => metadata.output_buses[31].active = Some(true),
                5 => metadata.output_buses[0].channel_count = 1,
                6 => metadata
                    .output_buses
                    .push(BounceAudioBus::new(1, false, Some(false))),
                7 => metadata.output_buses[31].channel_count = 3,
                8 => metadata.output_buses[31].channel_count = usize::MAX,
                9 => metadata.event_output_buses[0].2 = None,
                10 => metadata.event_output_buses.push((16, true, Some(false))),
                11 => metadata.event_output_buses[0].0 = 17,
                12 => metadata.event_input_buses[0].2 = None,
                13 => metadata.input_buses[0].active = Some(false),
                14 => {
                    metadata = generic.clone();
                    metadata
                        .output_buses
                        .push(BounceAudioBus::new(2, false, Some(false)));
                }
                _ => {
                    metadata = generic.clone();
                    metadata.event_output_buses = selected.event_output_buses.clone();
                }
            }
            let before = metadata.clone();
            assert!(
                copy_metadata(&metadata, 8000).is_err(),
                "invalid case {invalid}"
            );
            assert_eq!(metadata, before);
        }
    }

    #[test]
    fn occurrence_selected_attestation_rejects_bus_midi_loss_and_policy_drift() {
        let (project, request) = source();
        let before = serde_json::to_vec(&project).unwrap();
        for drift in 0..6 {
            let metadata = selected_metadata();
            let first = capture(&project, Arc::clone(&request), metadata.clone());
            let attestation = first.retain_occurrence_attestation().unwrap();
            let mut changed = metadata.clone();
            match drift {
                0 => changed.output_buses[31].is_main = !changed.output_buses[31].is_main,
                1 => changed.output_buses[31].channel_count = 1,
                2 => changed.event_output_buses[0].2 = Some(false),
                3 => changed.event_output_buses.clear(),
                4 => changed.version = "8.4.2".into(),
                _ => changed.output_buses[31].active = None,
            }
            if drift < 4 {
                changed.validate(8000).unwrap();
            }
            let second = capture(&project, Arc::clone(&request), changed);
            assert!(
                attestation.match_recapture(second).is_err(),
                "drift case {drift}"
            );
            assert_eq!(first.metadata(), &metadata);
            assert_eq!(serde_json::to_vec(&project).unwrap(), before);
        }
    }

    #[test]
    fn occurrence_selected_invalid_capture_never_returns_partial_provenance_or_attestation() {
        let (project, request) = source();
        let before = serde_json::to_vec(&project).unwrap();
        for invalid in 0..5 {
            let mut captured = capture(&project, Arc::clone(&request), selected_metadata());
            match invalid {
                0 => captured.identity.epoch += 1,
                1 => captured.state.native_generation += 1,
                2 => captured.state.metadata.output_buses[31].active = None,
                3 => captured.state.opaque_state.clear(),
                _ => {
                    captured.state.parameter_bases.remove(&17);
                }
            }
            assert!(
                captured.occurrence_provenance().is_err(),
                "invalid case {invalid}"
            );
            assert!(
                captured.retain_occurrence_attestation().is_err(),
                "invalid case {invalid}"
            );
            assert_eq!(serde_json::to_vec(&project).unwrap(), before);
        }
    }
}
