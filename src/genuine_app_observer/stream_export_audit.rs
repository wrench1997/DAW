//! Derive raw PCM and MIDI evidence from the complete, actually joined stream.
//! No mixing or musical compiler is used here. Independent Python audits the
//! original score and every final WAV sample after this bounded readback.
use super::{
    CaptureLabel, InstancePhase, stream_codec as native, stream_control as control,
    stream_live as live, stream_replay as replay, stream_state as state,
};
use crate::instrument_bounce::BounceSourceIdentity;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_FRAMES: u64 = 24_000_000;
const MAX_MIDI: usize = 4096;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct MidiEvidence {
    pub frame: u64,
    pub message: [u8; 3],
    pub block_offset: u16,
}

#[derive(Debug, Serialize)]
pub(crate) struct StreamReplayEvidence {
    pub raw_path: PathBuf,
    pub state_sidecar_path: PathBuf,
    pub state_ranges: Vec<StateRangeEvidence>,
    pub records: u64,
    pub transcript_bytes: u64,
    pub by_kind: [u64; 24],
    pub generated_frames: u64,
    pub pcm_blocks: u64,
    pub raw_bytes: u64,
    pub native_latency: u32,
    pub reported_tail: u32,
    pub source_ordinal: u64,
    pub source_worker_id: Option<usize>,
    pub source_instance_id: Option<u64>,
    pub clone_ordinal: u64,
    pub clone_worker_id: Option<usize>,
    pub clone_source_instance_id: Option<u64>,
    pub capture_identity: [u64; 7],
    pub capture_receipt_identity: usize,
    pub capture_slot: usize,
    pub source_load_slot: usize,
    pub clone_load_slot: usize,
    pub capture_parameter_bases: usize,
    pub capture_native_generation: u64,
    pub capture_native_dirty_revision: u64,
    pub report_identity: [u64; 7],
    pub musical_frames: u64,
    pub retained_frames: usize,
    pub fixed_tail: u32,
    pub disposal_sequence: u64,
    pub destruction_sequence: u64,
    pub report_sequence: u64,
    pub source_worker_joined_records: u64,
    pub actual_eof: bool,
    pub file_stamp_matches: bool,
    pub state_links_match: bool,
    pub instance_links_match: bool,
    pub raw_flush_sync_and_length_passed: bool,
    pub midi: Vec<MidiEvidence>,
    pub source_helper_strict_reap_proven: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct InstanceEvidence {
    ordinal: u64,
    phase: u32,
    worker_id: Option<usize>,
    source_instance_id: Option<u64>,
}
fn incarnation(value: super::InstanceIdentity) -> InstanceEvidence {
    InstanceEvidence {
        ordinal: value.ordinal,
        phase: match value.phase {
            InstancePhase::InitialSource => 1,
            InstancePhase::StrictClone => 2,
            InstancePhase::UndoSource => 3,
        },
        worker_id: value.worker_id,
        source_instance_id: value.source_instance_id,
    }
}
#[derive(Debug, Serialize)]
pub(crate) struct StateRangeEvidence {
    slot: usize,
    kind: u16,
    record_instance: Option<InstanceEvidence>,
    offset: u64,
    length: u64,
    original_owned_capacity: u64,
    unique_blob_ordinal: u64,
    phase: u32,
    worker_id: Option<u64>,
    source_id: Option<u64>,
    instance_ordinal: u64,
    record_ordinal: u64,
    monotonic_elapsed_ns: u64,
}

fn stamp(value: BounceSourceIdentity) -> [u64; 7] {
    [
        value.project_session,
        value.project_fingerprint,
        value.timeline_revision,
        value.epoch,
        value.endpoint_id,
        u64::from(value.channel_id),
        value.instance_id,
    ]
}
fn refusal(code: &'static str) -> replay::VisitError {
    replay::VisitError { code }
}

struct Projection {
    frames: u64,
    blocks: u64,
    midi: Vec<MidiEvidence>,
    capture: Option<CaptureProjection>,
    source_load_slot: Option<usize>,
    clone_load_slot: Option<usize>,
    disposal: Option<u64>,
    destruction: Option<u64>,
}
struct CaptureProjection {
    identity: [u64; 7],
    receipt: usize,
    slot: usize,
    latency: u32,
    tail: u32,
    bases: usize,
    native_generation: u64,
    native_dirty_revision: u64,
}
impl Projection {
    fn new() -> Result<Self, live::StreamError> {
        let mut midi = Vec::new();
        midi.try_reserve_exact(MAX_MIDI)
            .map_err(|_| live::StreamError::admission("raw replay MIDI reservation failed"))?;
        Ok(Self {
            frames: 0,
            blocks: 0,
            midi,
            capture: None,
            source_load_slot: None,
            clone_load_slot: None,
            disposal: None,
            destruction: None,
        })
    }
    fn visit(
        &mut self,
        record: replay::RecordView<'_>,
        raw: &mut impl Write,
    ) -> Result<(), replay::VisitError> {
        let strict = record
            .header
            .instance
            .is_some_and(|i| i.phase == InstancePhase::StrictClone);
        match record.payload {
            replay::Payload::Native(native::NativeBody::StrictPcm(pcm)) => {
                if !strict
                    || pcm.frames != 128
                    || pcm.channel_count() != 2
                    || !pcm.transport_applied
                    || pcm.output_events_lost
                    || pcm.output_events != 0
                    || pcm.transport.sample_position != self.frames as i64
                    || pcm.transport.quarter_note_bits
                        != (self.frames as f64 * 84. / (48_000. * 60.)).to_bits()
                    || pcm.transport.tempo_bits != 84_f64.to_bits()
                    || !pcm.transport.playing
                    || pcm.transport.time_sig_numerator != 4
                    || pcm.transport.time_sig_denominator != 4
                    || self
                        .frames
                        .checked_add(pcm.frames)
                        .is_none_or(|n| n > MAX_FRAMES)
                {
                    return Err(refusal(
                        "raw replay PCM shape, transport or bounded extent refused",
                    ));
                }
                let mut channels = pcm.channels();
                let left = channels
                    .next()
                    .ok_or_else(|| refusal("raw left channel absent"))?;
                let right = channels
                    .next()
                    .ok_or_else(|| refusal("raw right channel absent"))?;
                if left.len() != 128 || right.len() != 128 || channels.next().is_some() {
                    return Err(refusal("raw stereo channel lengths differ from Q128"));
                }
                // Preserve every IEEE bit, including nonfinite values. The
                // independent full-file audit reports those after readback.
                let mut block = [0_u8; 128 * 8];
                for (index, (left, right)) in
                    left.sample_bits().zip(right.sample_bits()).enumerate()
                {
                    block[index * 8..index * 8 + 4].copy_from_slice(&left.to_le_bytes());
                    block[index * 8 + 4..index * 8 + 8].copy_from_slice(&right.to_le_bytes());
                }
                raw.write_all(&block)
                    .map_err(|_| refusal("actual raw PCM write failed; partial file preserved"))?;
                self.frames += 128;
                self.blocks += 1;
            }
            replay::Payload::Native(native::NativeBody::Midi { data, offset }) if strict => {
                if self.midi.len() >= MAX_MIDI
                    || offset >= 128
                    || !matches!(data[0], 0x80 | 0x90)
                    || data[1] > 127
                    || data[2] > 127
                {
                    return Err(refusal("actual MIDI exceeds complete score wire admission"));
                }
                self.midi.push(MidiEvidence {
                    frame: self.frames + u64::from(offset),
                    message: data,
                    block_offset: offset,
                });
            }
            replay::Payload::State(state::StateBody::StateLoad { link, outcome }) => {
                if !matches!(outcome, control::Outcome::Ok) {
                    return Err(refusal("actual state load did not acknowledge success"));
                }
                match record.header.instance.map(|i| i.phase) {
                    Some(InstancePhase::InitialSource) if self.source_load_slot.is_none() => {
                        self.source_load_slot = Some(link.slot())
                    }
                    Some(InstancePhase::StrictClone) if self.clone_load_slot.is_none() => {
                        self.clone_load_slot = Some(link.slot())
                    }
                    _ => return Err(refusal("unexpected or repeated source/clone state load")),
                }
            }
            replay::Payload::State(state::StateBody::Capture {
                link,
                label,
                identity,
                receipt_identity,
                metadata,
                parameter_bases,
                native_generation,
                native_dirty_revision,
            }) => {
                if self.capture.is_some()
                    || label != CaptureLabel::Initial
                    || record.header.instance.is_some()
                {
                    return Err(refusal("complete-song capture attribution is ambiguous"));
                }
                self.capture = Some(CaptureProjection {
                    identity: stamp(identity),
                    receipt: receipt_identity,
                    slot: link.slot(),
                    latency: metadata.latency_samples,
                    tail: metadata.tail_samples,
                    bases: parameter_bases.len(),
                    native_generation,
                    native_dirty_revision,
                });
            }
            replay::Payload::Control(control::Control::Disposal {
                terminated, error, ..
            }) if strict => {
                if error.is_some() {
                    return Err(refusal("actual strict clone disposal reported an error"));
                }
                // Repeated nonterminal polls are possible; only a healthy
                // terminal poll supplies this sequence.
                if terminated && self.disposal.replace(record.header.sequence).is_some() {
                    return Err(refusal("duplicate clone terminal disposal"));
                }
            }
            replay::Payload::Control(control::Control::Lifecycle(
                "renderer backend destruction returned",
            )) => {
                if self.disposal.is_none_or(|n| n >= record.header.sequence)
                    || self.destruction.is_some()
                {
                    return Err(refusal(
                        "renderer destruction did not follow actual clone termination",
                    ));
                }
                self.destruction = Some(record.header.sequence);
            }
            _ => {}
        }
        Ok(())
    }
}

impl live::StreamRetirement {
    pub(crate) fn writer_evidence(&self) -> Value {
        let inspection = self.inspect();
        let actor = inspection.actor.as_ref().and_then(|a| a.terminal);
        json!({"transcript": inspection.transcript_path, "states": inspection.sidecar_path,
            "transcript_actual_writer_joined": actor.is_some_and(|t| t.writer.writer_joined),
            "transcript_complete": actor.is_some_and(|t| t.complete()),
            "transcript_records": actor.map(|t| t.expected.records),
            "transcript_bytes": actor.map(|t| t.expected.bytes),
            "sidecar_actual_writer_joined": inspection.state_terminal.is_some_and(|t| t.actual_writer_joined),
            "state_range_linkage": inspection.states.is_some(), "instance_dictionary_complete": inspection.instances.is_ok(),
            "stream_failure": inspection.failure.map(|f| f.code),
            "SDK_or_source_helper_reap_claim": false})
    }
    pub(crate) fn replay_to_raw(
        &self,
        raw_path: &Path,
    ) -> Result<StreamReplayEvidence, live::StreamError> {
        let reject = live::StreamError::admission;
        let inspection = self.inspect();
        if !raw_path.is_absolute()
            || raw_path == inspection.transcript_path
            || raw_path == inspection.sidecar_path
            || inspection.failure.is_some()
            || inspection.startup_failure.is_some()
        {
            return Err(reject(
                "raw replay requires a new separate file and healthy retained owners",
            ));
        }
        let actor = inspection
            .actor
            .ok_or_else(|| reject("raw replay has no retained transcript actor"))?;
        let terminal = actor
            .terminal
            .ok_or_else(|| reject("raw replay has no actual transcript writer terminal"))?;
        let states = inspection
            .states
            .ok_or_else(|| reject("raw replay has no joined state range linkage"))?;
        let mut state_ranges = Vec::with_capacity(state::MAX_BLOB_COUNT);
        for (slot, range) in states.ranges.iter().enumerate() {
            let link = states.links[slot]
                .ok_or_else(|| reject("state range has no retained original slot"))?;
            state_ranges.push(StateRangeEvidence {
                slot,
                kind: link.original_header().kind,
                record_instance: link.original_header().instance.map(incarnation),
                offset: range.offset,
                length: range.length,
                original_owned_capacity: range.original_owned_capacity,
                unique_blob_ordinal: range.unique_blob_ordinal,
                phase: range.identity.phase as u32,
                worker_id: range.identity.worker_id,
                source_id: range.identity.source_id,
                instance_ordinal: range.identity.instance_ordinal,
                record_ordinal: range.identity.record_ordinal,
                monotonic_elapsed_ns: range.identity.monotonic_elapsed_ns,
            });
        }
        let dictionary = inspection
            .instances
            .map_err(|_| reject("raw replay instance dictionary is incomplete"))?;
        let source = dictionary
            .entries()
            .find(|e| e.identity.phase == InstancePhase::InitialSource)
            .map(|e| e.identity)
            .ok_or_else(|| reject("raw replay actual source birth is absent"))?;
        let clone = dictionary
            .entries()
            .find(|e| e.identity.phase == InstancePhase::StrictClone)
            .map(|e| e.identity)
            .ok_or_else(|| reject("raw replay actual clone birth is absent"))?;
        if dictionary.entries().count() != 2 || !dictionary.links_complete() {
            return Err(reject(
                "complete song requires exactly its retained source and selected clone",
            ));
        }
        let mut reader = replay::ReplayReader::open_joined(
            inspection.transcript_path,
            &terminal,
            actor.dictionary,
        )
        .map_err(|_| reject("actual joined transcript could not be opened for typed replay"))?;
        reader
            .bind_instances(dictionary)
            .map_err(|_| reject("actual instance linkage could not be bound"))?;
        reader
            .bind_state_ranges(states)
            .map_err(|_| reject("actual joined state ranges could not be bound"))?;
        let mut projection = Projection::new()?;
        let mut raw = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(raw_path)
            .map_err(|_| reject("raw replay create-new failed; existing file preserved"))?;
        while let replay::ReadProgress::Record = reader
            .read_next(|record| projection.visit(record, &mut raw))
            .map_err(|_| reject("complete typed replay failed; transcript, sidecar and partial raw file preserved"))?
        {}
        let summary = reader.summary();
        let capture = projection
            .capture
            .ok_or_else(|| reject("complete replay has no original capture"))?;
        let report = summary.reports[0]
            .ok_or_else(|| reject("complete replay lacks the actual typed dry-render report"))?;
        let disposal = projection
            .disposal
            .ok_or_else(|| reject("complete replay lacks actual clone terminal disposal"))?;
        let destruction = projection
            .destruction
            .ok_or_else(|| reject("complete replay lacks actual renderer destruction"))?;
        let source_proof = summary
            .births
            .iter()
            .flatten()
            .find(|e| e.identity == source)
            .ok_or_else(|| reject("actual source birth differs during full replay"))?;
        if !summary.complete
            || summary.report_count != 1
            || summary.birth_count != 2
            || projection.frames == 0
            || projection.blocks != summary.strict_pcm_records
            || projection.frames != summary.strict_pcm_frames
            || report.generated_frames != projection.frames
            || report.native_latency != capture.latency
            || report.reported_tail != capture.tail
            || stamp(report.identity) != capture.identity
            || report.header.sequence <= destruction
            || source_proof.worker_joined_records != 1
            || source_proof.worker_panicked_records != 0
        {
            return Err(reject(
                "full raw/report/incarnation/actual source Join evidence disagrees",
            ));
        }
        raw.flush()
            .map_err(|_| reject("raw replay flush failed; output preserved"))?;
        raw.sync_all()
            .map_err(|_| reject("raw replay sync failed; output preserved"))?;
        let raw_bytes = projection
            .frames
            .checked_mul(8)
            .ok_or_else(|| reject("raw replay byte extent overflow"))?;
        if raw
            .metadata()
            .map_err(|_| reject("raw replay file length unavailable"))?
            .len()
            != raw_bytes
        {
            return Err(reject(
                "raw replay file length differs from every actual Q128 sample",
            ));
        }
        Ok(StreamReplayEvidence {
            raw_path: raw_path.to_owned(),
            state_sidecar_path: inspection.sidecar_path.to_owned(),
            state_ranges,
            records: summary.accepted_records,
            transcript_bytes: summary.accepted_bytes,
            by_kind: summary.by_kind,
            generated_frames: projection.frames,
            pcm_blocks: projection.blocks,
            raw_bytes,
            native_latency: capture.latency,
            reported_tail: capture.tail,
            source_ordinal: source.ordinal,
            source_worker_id: source.worker_id,
            source_instance_id: source.source_instance_id,
            clone_ordinal: clone.ordinal,
            clone_worker_id: clone.worker_id,
            clone_source_instance_id: clone.source_instance_id,
            capture_identity: capture.identity,
            capture_receipt_identity: capture.receipt,
            capture_slot: capture.slot,
            source_load_slot: projection
                .source_load_slot
                .ok_or_else(|| reject("source original state load absent"))?,
            clone_load_slot: projection
                .clone_load_slot
                .ok_or_else(|| reject("clone actual state load absent"))?,
            capture_parameter_bases: capture.bases,
            capture_native_generation: capture.native_generation,
            capture_native_dirty_revision: capture.native_dirty_revision,
            report_identity: stamp(report.identity),
            musical_frames: report.musical_frames,
            retained_frames: report.frames,
            fixed_tail: report.fixed_tail,
            disposal_sequence: disposal,
            destruction_sequence: destruction,
            report_sequence: report.header.sequence,
            source_worker_joined_records: source_proof.worker_joined_records,
            actual_eof: summary.actual_eof,
            file_stamp_matches: summary.file_stamp_matches,
            state_links_match: summary.state_links_match,
            instance_links_match: summary.instance_links_match,
            raw_flush_sync_and_length_passed: true,
            midi: projection.midi,
            source_helper_strict_reap_proven: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Stage, stream_envelope as envelope};
    use super::*;
    use crate::plugins::plugin_runtime::PluginTransport;

    fn header(sequence: u64, kind: u16) -> envelope::Header {
        envelope::Header {
            sequence,
            monotonic_ns: sequence,
            stage: Stage::WholeExport,
            instance: Some(super::super::InstanceIdentity {
                ordinal: 2,
                phase: InstancePhase::StrictClone,
                worker_id: None,
                source_instance_id: None,
            }),
            kind,
            lane: envelope::Lane::Native(12),
        }
    }
    #[test]
    fn raw_projection_keeps_ieee_bits_and_channel_order_without_mixing() {
        let dictionary = native::BaselineDictionary::new(
            1,
            native::CanonicalMetadata {
                class_uid: "synthetic".into(),
                version: "1".into(),
                name: "synthetic".into(),
                category: "Instrument".into(),
                audio: native::AudioLayout::default(),
                events: native::EventLayout::default(),
                latency_samples: 0,
                tail_samples: 0,
            },
        )
        .unwrap();
        let mut left = vec![0.; 128];
        let mut right = vec![0.; 128];
        left[0] = f32::from_bits(0x8000_0000);
        right[0] = f32::from_bits(0x7fc1_2345);
        left[127] = 7.;
        right[127] = -7.;
        let channels = [left, right];
        let transport = PluginTransport {
            sample_position: 0,
            quarter_note_position: 0.,
            tempo: 84.,
            playing: true,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
        };
        let mut bytes = Vec::with_capacity(envelope::MAX_OPERAND);
        native::encode_native_body(
            native::NativeView::StrictPcm {
                frames: 128,
                transport: &transport,
                channels: &channels,
                transport_applied: true,
                output_events: 0,
                output_events_lost: false,
            },
            &dictionary,
            &mut bytes,
        )
        .unwrap();
        let payload =
            native::decode_native_body_with_limit(12, &bytes, &dictionary, envelope::MAX_BODY)
                .unwrap();
        let mut projection = Projection::new().unwrap();
        let mut raw = Vec::new();
        projection
            .visit(
                replay::RecordView {
                    header: header(1, 7),
                    payload: replay::Payload::Native(payload),
                },
                &mut raw,
            )
            .unwrap();
        assert_eq!(raw.len(), 128 * 8);
        assert_eq!(&raw[..8], &[0, 0, 0, 0x80, 0x45, 0x23, 0xc1, 0x7f]);
        assert_eq!(&raw[127 * 8..127 * 8 + 4], &7_f32.to_le_bytes());
        assert_eq!(&raw[127 * 8 + 4..], &(-7_f32).to_le_bytes());
        assert_eq!((projection.frames, projection.blocks), (128, 1));
    }
    #[test]
    fn raw_projection_rejects_invalid_midi_without_count_or_output() {
        let mut projection = Projection::new().unwrap();
        let mut raw = Vec::new();
        for (data, offset) in [
            ([0x90, 60, 1], 128),
            ([0xb0, 60, 1], 0),
            ([0x90, 128, 1], 0),
        ] {
            assert!(
                projection
                    .visit(
                        replay::RecordView {
                            header: header(1, 8),
                            payload: replay::Payload::Native(native::NativeBody::Midi {
                                data,
                                offset
                            })
                        },
                        &mut raw
                    )
                    .is_err()
            );
        }
        assert!(projection.midi.is_empty());
        assert!(raw.is_empty());
    }
    #[test]
    fn destruction_cannot_be_accepted_without_actual_terminal_disposal() {
        let mut projection = Projection::new().unwrap();
        let mut raw = Vec::new();
        let destruction = |sequence| replay::RecordView {
            header: header(sequence, 23),
            payload: replay::Payload::Control(control::Control::Lifecycle(
                "renderer backend destruction returned",
            )),
        };
        assert!(projection.visit(destruction(2), &mut raw).is_err());
        let mut bytes = Vec::with_capacity(envelope::MAX_OPERAND);
        control::encode(
            control::View::Disposal {
                terminated: false,
                error: None,
                diagnostics: &[],
            },
            &mut bytes,
        )
        .unwrap();
        let pending = control::decode(&bytes, 20).unwrap();
        projection
            .visit(
                replay::RecordView {
                    header: header(3, 20),
                    payload: replay::Payload::Control(pending),
                },
                &mut raw,
            )
            .unwrap();
        assert!(projection.visit(destruction(4), &mut raw).is_err());
        bytes.clear();
        control::encode(
            control::View::Disposal {
                terminated: true,
                error: None,
                diagnostics: &[],
            },
            &mut bytes,
        )
        .unwrap();
        let joined = control::decode(&bytes, 20).unwrap();
        projection
            .visit(
                replay::RecordView {
                    header: header(5, 20),
                    payload: replay::Payload::Control(joined),
                },
                &mut raw,
            )
            .unwrap();
        projection.visit(destruction(6), &mut raw).unwrap();
        assert_eq!(projection.disposal, Some(5));
        assert_eq!(projection.destruction, Some(6));
        assert!(projection.visit(destruction(7), &mut raw).is_err());
    }
}
