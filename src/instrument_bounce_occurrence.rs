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
fn copy_metadata(value: &BouncePluginMetadata) -> Result<BouncePluginMetadata, String> {
    Ok(BouncePluginMetadata {
        class_uid: copy_text(&value.class_uid)?,
        version: copy_text(&value.version)?,
        name: copy_text(&value.name)?,
        category: copy_text(&value.category)?,
        event_input_buses: copy_list(&value.event_input_buses, 1)?,
        event_output_buses: copy_list(&value.event_output_buses, 0)?,
        input_buses: copy_list(&value.input_buses, 1)?,
        output_buses: copy_list(&value.output_buses, 3)?,
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
            metadata: copy_metadata(&self.state.metadata)?,
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
