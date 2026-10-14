//! Independent complete-frame oracle for the private, actual Whole NOIRE QA.
//!
//! Expected MIDI, transport and master gain come from the fixed QA score below,
//! never from CapturedMasterSongMix, compiled controls or the observed ledger.
//! The only reused helpers are the independent RIFF parser and SHA-256 routine.
//! This module does not mutate a Project, observer record, raw sample or WAV.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::{
    export::StereoFrame,
    export_options::WavSampleFormat,
    genuine_app_observer::{
        self as observe, Boundary, CallOutcome, InstancePhase, Operation, RecordKind,
    },
    instrument_bounce::{BouncePluginMetadata, render::MasterSongRenderReport},
    plugins::plugin_runtime::PluginTransport,
};

use super::super::oracles as wav_oracle;

const RATE: usize = 48_000;
const TEMPO: f64 = 84.0;
const QUANTUM: usize = 128;
const TAIL: usize = RATE * 3;
const CLONE_REPLY_LIMIT: usize = 2_048;
// Independent beat-to-frame analysis and production's bounded f32 automation
// endpoint interpolation can differ slightly. This fixed, predeclared absolute
// tolerance is never fitted to observed PCM. Every sample is still compared.
const PCM_ABSOLUTE_TOLERANCE: f32 = 2.0e-6;

pub(super) struct WholeNoireAuditInput<'a> {
    pub(super) records: &'a [Arc<observe::Record>],
    pub(super) observer_failure: Option<&'a str>,
    pub(super) strict_instance: observe::InstanceIdentity,
    pub(super) metadata: &'a BouncePluginMetadata,
    pub(super) report: &'a MasterSongRenderReport,
    /// The immutable Saved13 source-channel value, checked by the QA caller.
    /// Both channel pan and every source/master route must be center/unity.
    pub(super) source_channel_volume: f32,
    pub(super) wav_bytes: &'a [u8],
}

pub(super) struct WholeNoireAudit {
    evidence: Value,
    output_metrics: PcmMetrics,
    held_note_metrics: [PcmMetrics; 2],
}

impl WholeNoireAudit {
    pub(super) fn evidence(&self) -> Value {
        self.evidence.clone()
    }

    /// Call only after saving evidence. Failure keeps the already published WAV
    /// and distinguishes measured nonquiet release from metadata truncation.
    pub(super) fn qualify_acoustics(&self) -> Result<(), String> {
        let output = &self.output_metrics;
        require(
            output.peak > 1.0e-7 && output.rms > 1.0e-8 && output.nonzero_frames > 50,
            "whole NOIRE WAV is silent or insufficiently audible",
        )?;
        for (index, note) in self.held_note_metrics.iter().enumerate() {
            require(
                note.peak > 1.0e-7 && note.rms > 1.0e-8,
                &format!("held-note window {index} is silent or insufficiently audible"),
            )?;
        }
        require(
            output.clipping_samples == 0,
            "whole NOIRE WAV reaches full scale",
        )?;
        require(
            output.tail_nonzero_frames > 0,
            "whole NOIRE fixed tail contains no nonzero release PCM",
        )?;
        require(
            output.last_100ms_peak <= output.tail_end_threshold,
            "whole NOIRE release is nonquiet in the final 100 ms; retain the WAV and audit, but acoustic tail qualification fails",
        )
    }
}

fn require(condition: bool, reason: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(reason.to_owned())
    }
}

fn beat_frame(beat: f64) -> usize {
    (beat * 60.0 / TEMPO * RATE as f64).round() as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    music: usize,
    retained: usize,
    latency: usize,
    generated: usize,
    calls: usize,
}

fn geometry(latency: u32) -> Result<Geometry, String> {
    let music = beat_frame(2.0);
    let retained = music.checked_add(TAIL).ok_or("retained frame overflow")?;
    let latency = latency as usize;
    // Include a processing quantum containing the song-end NoteOff even when
    // a future zero-tail fixture has zero latency. Padding is never published.
    let minimum = latency
        .checked_add(retained)
        .ok_or("generated frame overflow")?
        .max(music.checked_add(1).ok_or("song-end frame overflow")?);
    let generated = minimum
        .checked_add(QUANTUM - 1)
        .ok_or("Q128 frame overflow")?
        / QUANTUM
        * QUANTUM;
    let calls = generated / QUANTUM;
    require(
        calls <= CLONE_REPLY_LIMIT,
        "native latency exceeds the explicit 2048-reply private observation bound",
    )?;
    Ok(Geometry {
        music,
        retained,
        latency,
        generated,
        calls,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MidiEntry {
    block: usize,
    offset: u16,
    frame: usize,
    data: [u8; 3],
}

impl MidiEntry {
    fn at(frame: usize, data: [u8; 3]) -> Self {
        Self {
            block: frame / QUANTUM,
            offset: (frame % QUANTUM) as u16,
            frame,
            data,
        }
    }

    fn evidence(self) -> Value {
        json!({"block":self.block,"offset":self.offset,"frame":self.frame,"data":self.data})
    }
}

fn expected_midi() -> [MidiEntry; 4] {
    // Independent score: C4 [0,1), E4 [1,2), ordinary MIDI channel 0.
    // Equal-frame Timeline ordering is NoteOff before NoteOn; final NoteOff
    // is legal at N and is not moved to N-1 or withheld from the tail quantum.
    let velocity = (0.2_f64 * 127.0).round() as u8;
    [
        MidiEntry::at(0, [0x90, 60, velocity]),
        MidiEntry::at(beat_frame(1.0), [0x80, 60, 0]),
        MidiEntry::at(beat_frame(1.0), [0x90, 64, velocity]),
        MidiEntry::at(beat_frame(2.0), [0x80, 64, 0]),
    ]
}

fn check_transport(actual: PluginTransport, block: usize) -> Result<(), String> {
    let frame = block
        .checked_mul(QUANTUM)
        .ok_or("transport position overflow")?;
    let ppq = frame as f64 / RATE as f64 * TEMPO / 60.0;
    require(
        actual.sample_position == frame as i64
            && actual.tempo == TEMPO
            && actual.playing
            && actual.time_sig_numerator == 4
            && actual.time_sig_denominator == 4
            && actual.quarter_note_position.is_finite()
            && (actual.quarter_note_position - ppq).abs()
                <= 16.0 * f64::EPSILON * ppq.abs().max(1.0),
        &format!(
            "strict Q128 transport differs at block {block}: actual {actual:?}; expected frame {frame}, PPQ {ppq}, 84 BPM, playing, 4/4"
        ),
    )
}

fn analytical_master_volume(output_frame: usize, music: usize) -> f32 {
    let musical_frame = output_frame.min(music - 1);
    let beat = musical_frame as f64 / RATE as f64 * TEMPO / 60.0;
    // Tail holds the last actual musical sample. It does not evaluate the
    // endpoint at beat 2 again, loop, or extend the automation into the tail.
    (0.85_f64 + (0.35_f64 - 0.85_f64) * (beat / 2.0).clamp(0.0, 1.0)) as f32
}

fn expected_frame(
    raw: StereoFrame,
    output_frame: usize,
    music: usize,
    channel_volume: f32,
) -> StereoFrame {
    let volume = analytical_master_volume(output_frame, music);
    // The fixed QA graph has center pan and unity source/master routing.
    // Deliberate ordering: endpoint headroom clamp, channel volume, analytic
    // MasterVolume, one tanh, Float32 PreserveLevel with export gain exactly 1.
    StereoFrame {
        left: (raw.left.clamp(-8.0, 8.0) * channel_volume * volume).tanh(),
        right: (raw.right.clamp(-8.0, 8.0) * channel_volume * volume).tanh(),
    }
}

#[derive(Default)]
struct CallCounts {
    attempts: usize,
    returned: usize,
}

fn count_call(
    counts: &mut CallCounts,
    boundary: Boundary,
    outcome: &Option<CallOutcome>,
) -> Result<(), String> {
    match boundary {
        Boundary::Attempt => {
            require(outcome.is_none(), "attempt unexpectedly carries a result")?;
            counts.attempts += 1;
        }
        Boundary::Returned => {
            require(
                matches!(outcome, Some(CallOutcome::Ok)),
                &format!("strict call did not return Ok: {outcome:?}"),
            )?;
            counts.returned += 1;
            require(
                counts.returned <= counts.attempts,
                "strict return has no matching earlier attempt",
            )?;
        }
    }
    Ok(())
}

pub(super) fn audit_whole_noire(
    input: WholeNoireAuditInput<'_>,
) -> Result<WholeNoireAudit, String> {
    if let Some(reason) = input.observer_failure {
        return Err(format!(
            "observer evidence is incomplete: {reason}; no complete-frame claim is permitted"
        ));
    }
    require(
        input.strict_instance.phase == InstancePhase::StrictClone,
        "supplied identity is not a strict clone",
    )?;
    require(
        input.source_channel_volume.is_finite() && input.source_channel_volume >= 0.0,
        "Saved13 source-channel volume must be finite and nonnegative",
    )?;
    let shape = geometry(input.metadata.latency_samples)?;
    let dry = &input.report.dry_render;
    let output = input.report.master_output;
    let metadata_truncation = u64::from(input.metadata.tail_samples) > TAIL as u64;
    require(
        dry.origin_frame == 0
            && dry.musical_frames == shape.music as u64
            && dry.frames == shape.retained
            && dry.native_latency == input.metadata.latency_samples
            && dry.reported_tail == input.metadata.tail_samples
            && dry.fixed_tail == TAIL as u32
            && dry.generated_frames == shape.generated as u64
            && input.report.sample_format == WavSampleFormat::Float32
            && output.sample_rate == RATE as u32
            && output.music_frames == shape.music
            && output.tail_frames == TAIL
            && output.frames == shape.retained
            && output.tail_limit_reached == metadata_truncation
            && output.applied_gain.to_bits() == 1.0_f32.to_bits(),
        "typed Whole report differs from independent two-beat/48k/Q128/latency/three-second-tail/Float32/unity-gain geometry or metadata truncation",
    )?;
    let captures = input
        .records
        .iter()
        .filter_map(|record| match &record.kind {
            RecordKind::Capture {
                label,
                identity,
                receipt_identity,
                state,
            } => Some((record.ordinal, label, identity, receipt_identity, state)),
            _ => None,
        })
        .collect::<Vec<_>>();
    require(
        captures.len() == 1,
        "actual Whole private capture is absent or duplicated",
    )?;
    let (capture_ordinal, capture_label, capture_identity, capture_receipt_identity, capture_state) =
        captures[0];
    require(
        *capture_label == observe::CaptureLabel::Initial
            && *capture_identity == dry.identity
            && *capture_receipt_identity != 0
            && &capture_state.metadata == input.metadata,
        "actual acknowledged Whole capture metadata/source identity differs from supplied metadata or typed report",
    )?;

    let mut generated = Vec::with_capacity(shape.generated);
    let mut midi = Vec::new();
    let mut process = CallCounts::default();
    let mut metadata_calls = CallCounts::default();
    let mut midi_calls = CallCounts::default();
    let mut prepare_calls = CallCounts::default();
    let mut metadata_records = 0;
    let mut prepare_metadata_records = 0;
    let mut assembled_metadata_records = 0;
    let mut capture_identity_records = 0;
    let mut open_process: Option<(PluginTransport, bool)> = None;
    let mut previous_ordinal = None;
    let mut first_process_ordinal = None;
    let mut last_pcm_ordinal = None;
    let mut process_ordinals = Vec::with_capacity(shape.calls);
    for record in input.records {
        if let Some(previous) = previous_ordinal {
            require(
                record.ordinal > previous,
                "observer records are duplicated, missing their order, or reversed",
            )?;
        }
        previous_ordinal = Some(record.ordinal);
        if record
            .instance
            .is_some_and(|identity| identity.phase == InstancePhase::StrictClone)
        {
            require(
                record.instance == Some(input.strict_instance),
                "another strict clone appears in the supplied Whole observation scope",
            )?;
        }
        if record.instance != Some(input.strict_instance) {
            continue;
        }
        match &record.kind {
            RecordKind::StrictCaptureIdentity(identity) => {
                require(
                    *identity == dry.identity,
                    "actual strict clone source identity differs from typed Whole report",
                )?;
                capture_identity_records += 1;
            }
            RecordKind::Metadata { site, result } => {
                let observed = result.as_ref().map_err(|reason| {
                    format!("metadata failed at ordinal {}: {reason}", record.ordinal)
                })?;
                require(
                    observed == input.metadata,
                    &format!(
                        "strict metadata changed at ordinal {} ({site})",
                        record.ordinal
                    ),
                )?;
                match *site {
                    "StrictBackend assembled actual metadata" => assembled_metadata_records += 1,
                    "StrictBackend prepare result" => prepare_metadata_records += 1,
                    "StrictBackend metadata result" => metadata_records += 1,
                    _ => {
                        return Err(format!(
                            "unexpected strict metadata site at ordinal {}: {site}",
                            record.ordinal
                        ));
                    }
                }
            }
            RecordKind::Call {
                operation: Operation::StrictProcess,
                boundary,
                frames,
                transport,
                outcome,
            } => {
                require(
                    *frames == Some(QUANTUM),
                    "strict process attempt/return is not exactly Q128",
                )?;
                let transport = transport.ok_or("strict process transport is absent")?;
                let block = match boundary {
                    Boundary::Attempt => process.attempts,
                    Boundary::Returned => process.returned,
                };
                check_transport(transport, block)?;
                match boundary {
                    Boundary::Attempt => {
                        require(
                            open_process.is_none(),
                            "strict process starts before its predecessor has a complete PCM reply",
                        )?;
                        open_process = Some((transport, false));
                        first_process_ordinal.get_or_insert(record.ordinal);
                        process_ordinals.push(record.ordinal);
                    }
                    Boundary::Returned => {
                        let (attempt_transport, returned) = open_process
                            .as_mut()
                            .ok_or("strict process return lacks its attempt")?;
                        require(
                            !*returned && *attempt_transport == transport,
                            "strict process return duplicates or differs from its attempt transport",
                        )?;
                        *returned = true;
                    }
                }
                count_call(&mut process, *boundary, outcome)?;
            }
            RecordKind::StrictPcm {
                frames,
                transport,
                channels,
                transport_applied,
                output_events,
                output_events_lost,
            } => {
                let (attempt_transport, returned) = open_process
                    .take()
                    .ok_or("strict PCM has no matching process attempt")?;
                require(
                    returned && attempt_transport == *transport,
                    "strict PCM precedes its successful return or has different transport",
                )?;
                check_transport(*transport, generated.len() / QUANTUM)?;
                require(
                    *frames == QUANTUM
                        && channels.len() == 2
                        && channels.iter().all(|channel| channel.len() == QUANTUM)
                        && *transport_applied
                        && *output_events == 0
                        && !*output_events_lost,
                    &format!(
                        "strict PCM shape/transport/output-MIDI acknowledgement differs at ordinal {}",
                        record.ordinal
                    ),
                )?;
                for (&left, &right) in channels[0].iter().zip(&channels[1]) {
                    generated.push(StereoFrame { left, right });
                }
                last_pcm_ordinal = Some(record.ordinal);
            }
            RecordKind::Midi { data, offset } => {
                require(
                    open_process.is_none() && usize::from(*offset) < QUANTUM,
                    "MIDI is out of Q128 range or arrived inside an already attempted Process",
                )?;
                let frame = process
                    .attempts
                    .checked_mul(QUANTUM)
                    .and_then(|start| start.checked_add(usize::from(*offset)))
                    .ok_or("MIDI frame overflow")?;
                midi.push(MidiEntry {
                    block: process.attempts,
                    offset: *offset,
                    frame,
                    data: *data,
                });
            }
            RecordKind::Call {
                operation,
                boundary,
                outcome,
                ..
            } => match operation {
                Operation::StrictMetadata => count_call(&mut metadata_calls, *boundary, outcome)?,
                Operation::StrictMidi => count_call(&mut midi_calls, *boundary, outcome)?,
                Operation::StrictPrepare => count_call(&mut prepare_calls, *boundary, outcome)?,
                _ => {
                    if matches!(boundary, Boundary::Returned) {
                        require(
                            matches!(outcome, Some(CallOutcome::Ok)),
                            &format!(
                                "strict clone operation {operation:?} did not succeed: {outcome:?}"
                            ),
                        )?;
                    }
                }
            },
            _ => {}
        }
    }
    require(
        open_process.is_none()
            && process.attempts == shape.calls
            && process.returned == shape.calls
            && generated.len() == shape.generated
            && prepare_calls.attempts == 1
            && prepare_calls.returned == 1
            && prepare_metadata_records == 1
            && assembled_metadata_records == 1 + shape.calls * 2
            && metadata_calls.attempts == shape.calls * 2
            && metadata_calls.returned == shape.calls * 2
            && metadata_records == shape.calls * 2
            && midi_calls.attempts == 4
            && midi_calls.returned == 4
            && capture_identity_records == 1,
        &format!(
            "incomplete strict Whole evidence: process {}/{}, PCM {}, metadata calls {}/{}, metadata records prepare {} / assembled {} / wrappers {}, MIDI calls {}/{}, prepare {}/{}, capture identities {}; expected {} complete blocks",
            process.attempts,
            process.returned,
            generated.len(),
            metadata_calls.attempts,
            metadata_calls.returned,
            prepare_metadata_records,
            assembled_metadata_records,
            metadata_records,
            midi_calls.attempts,
            midi_calls.returned,
            prepare_calls.attempts,
            prepare_calls.returned,
            capture_identity_records,
            shape.calls
        ),
    )?;
    let expected_midi = expected_midi();
    require(
        midi == expected_midi,
        &format!(
            "complete Whole MIDI ledger differs: actual {midi:?}; expected {expected_midi:?}; no new notes are allowed in the fixed tail"
        ),
    )?;

    // Check EVERY generated sample, including discarded latency and Q128
    // padding. Invalid raw PCM is evidence; it is never zeroed or sanitized.
    let raw_nonfinite = generated
        .iter()
        .enumerate()
        .flat_map(|(frame, sample)| [(frame, 0, sample.left), (frame, 1, sample.right)])
        .filter(|(_, _, value)| !value.is_finite())
        .map(|(frame, channel, value)| (frame, channel, value.to_bits()))
        .collect::<Vec<_>>();
    require(
        raw_nonfinite.is_empty(),
        &format!(
            "generated raw PCM contains {} nonfinite samples; first {:?}",
            raw_nonfinite.len(),
            raw_nonfinite.first()
        ),
    )?;
    let retained_end = shape
        .latency
        .checked_add(shape.retained)
        .ok_or("retained range overflow")?;
    let retained = generated
        .get(shape.latency..retained_end)
        .ok_or("complete latency-trimmed range is missing")?;
    let generated_metrics = PcmMetrics::measure(&generated, 0, 0)?;
    let raw_metrics = PcmMetrics::measure(retained, shape.music, TAIL)?;
    require(
        dry.sample_peak.is_finite() && dry.sample_peak.to_bits() == raw_metrics.peak.to_bits(),
        "typed dry sample peak differs from all actual latency-trimmed raw samples",
    )?;

    let parsed = wav_oracle::parse_float32_stereo_48k_wav(input.wav_bytes)?;
    require(
        parsed.frames == shape.retained && parsed.sample_bits.len() == shape.retained * 2,
        "independently decoded WAV omits or adds musical/tail frames",
    )?;
    let actual = parsed
        .sample_bits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bits| StereoFrame {
            left: f32::from_bits(bits[0]),
            right: f32::from_bits(bits[1]),
        })
        .collect::<Vec<_>>();
    let comparison =
        compare_every_frame(&actual, retained, shape.music, input.source_channel_volume)?;
    let output_metrics = PcmMetrics::measure(&actual, shape.music, TAIL)?;
    require(
        output.sample_peak.is_finite()
            && output.sample_peak.to_bits() == output_metrics.peak.to_bits(),
        "typed master peak differs from all independently decoded WAV samples",
    )?;
    let held_note_ranges = [(RATE / 10, RATE / 2), (RATE * 9 / 10, RATE * 13 / 10)];
    let held_note_metrics = [
        PcmMetrics::measure(&actual[held_note_ranges[0].0..held_note_ranges[0].1], 0, 0)?,
        PcmMetrics::measure(&actual[held_note_ranges[1].0..held_note_ranges[1].1], 0, 0)?,
    ];
    let latency_metrics = PcmMetrics::measure(&generated[..shape.latency], 0, 0)?;
    let padding_metrics = PcmMetrics::measure(&generated[retained_end..], 0, 0)?;
    require(
        first_process_ordinal.is_some_and(|ordinal| capture_ordinal < ordinal),
        "actual capture acknowledgement does not precede strict processing",
    )?;
    let held_note_windows = held_note_ranges
        .iter()
        .zip(&held_note_metrics)
        .map(|(&(start, end), metric)| {
            json!({"start_frame":start,"end_frame_exclusive":end,"metrics":metric.evidence()})
        })
        .collect::<Vec<_>>();
    let mut evidence = json!({
        "oracle":"independent fixed score and analytical master controls; no production mix/control compiler",
        "music_beats":2.0,"tempo_bpm":TEMPO,"sample_rate":RATE,"channels":2,"process_quantum":QUANTUM,
        "music_frames":shape.music,"fixed_tail_frames":TAIL,"native_latency_frames":shape.latency,
        "retained_frames":shape.retained,"generated_frames":shape.generated,"process_calls":shape.calls,
        "discarded_latency_frames":shape.latency,"discarded_padding_frames":shape.generated-retained_end,
        "duration_seconds":shape.retained as f64/RATE as f64,
        "source_channel_volume":input.source_channel_volume,"channel_pan":0.0,"source_and_master_route_gain":1.0,
        "master_volume_points":[[0.0,0.85],[2.0,0.35]],
        "tail_control_policy":"last actual music sample held",
        "last_music_frame":shape.music-1,"last_music_beat":(shape.music-1) as f64/RATE as f64*TEMPO/60.0,
        "tail_held_master_volume":analytical_master_volume(shape.music,shape.music),
        "operation_order":["raw clamp +/-8","Saved13 channel volume","analytical MasterVolume","one tanh","PreserveLevel unity export gain"],
        "applied_gain":output.applied_gain,"sample_format":"IEEE Float32",
        "complete_actual_midi":midi.iter().copied().map(MidiEntry::evidence).collect::<Vec<_>>(),
        "no_new_tail_notes":true,"all_process_transports_checked":true,
        "strict_metadata_records":metadata_records+prepare_metadata_records+assembled_metadata_records,
    });
    let details = json!({
        "strict_metadata_sites":{"prepare_result":prepare_metadata_records,
            "assembled_actual":assembled_metadata_records,"metadata_result":metadata_records},
        "metadata":{"class_uid":input.metadata.class_uid,"version":input.metadata.version,"category":input.metadata.category,
            "native_latency_frames":input.metadata.latency_samples,"reported_tail_frames":input.metadata.tail_samples,
            "reported_tail_unknown":input.metadata.tail_samples==u32::MAX,"fixed_window_metadata_truncation":metadata_truncation},
        "generated_raw":generated_metrics.evidence(),"discarded_latency_raw":latency_metrics.evidence(),
        "discarded_padding_raw":padding_metrics.evidence(),"retained_raw":raw_metrics.evidence(),
        "published_master":output_metrics.evidence(),
        "held_note_windows":held_note_windows,
        "wav_comparison":comparison.evidence(),"wav_bytes":input.wav_bytes.len(),"wav_sha256":wav_oracle::sha256_hex(input.wav_bytes),
        "generated_raw_pcm_le_sha256":pcm_hash(&generated),"retained_raw_pcm_le_sha256":pcm_hash(retained),
        "published_master_pcm_le_sha256":pcm_hash(&actual),"capture_ordinal":capture_ordinal,
        "transport_first_process_ordinal":first_process_ordinal,"last_pcm_ordinal":last_pcm_ordinal,
        "process_attempt_ordinals":process_ordinals,
        "physical_listening":false,"full_natural_tail_guarantee":false,
        "acoustic_tail_qualification_separate_from_metadata_truncation":true,
        "original_observer_pcm_and_wav_are_unmodified":true,
    });
    let Value::Object(details) = details else {
        unreachable!("oracle detail literal is a JSON object")
    };
    evidence
        .as_object_mut()
        .expect("oracle evidence literal is a JSON object")
        .extend(details);
    require(
        comparison.mismatches == 0,
        &format!(
            "complete WAV/master PCM comparison failed; preserve observer raw data, WAV and this diagnostic: {}",
            comparison.evidence()
        ),
    )?;
    Ok(WholeNoireAudit {
        evidence,
        output_metrics,
        held_note_metrics,
    })
}

fn pcm_hash(samples: &[StereoFrame]) -> String {
    let mut bytes = Vec::with_capacity(samples.len() * 8);
    for sample in samples {
        bytes.extend_from_slice(&sample.left.to_le_bytes());
        bytes.extend_from_slice(&sample.right.to_le_bytes());
    }
    wav_oracle::sha256_hex(&bytes)
}

struct Comparison {
    samples: usize,
    exact_bits: usize,
    mismatches: usize,
    maximum_absolute_error: f32,
    rms_error: f64,
    first_mismatch: Option<Value>,
}

impl Comparison {
    fn evidence(&self) -> Value {
        json!({"compared_samples":self.samples,"exact_bits_equal_samples":self.exact_bits,"outside_tolerance_samples":self.mismatches,
            "absolute_tolerance":PCM_ABSOLUTE_TOLERANCE,"maximum_absolute_error":self.maximum_absolute_error,"rms_error":self.rms_error,
            "first_mismatch":self.first_mismatch,"every_frame_and_channel_compared":true})
    }
}

fn compare_every_frame(
    actual: &[StereoFrame],
    raw: &[StereoFrame],
    music: usize,
    channel_volume: f32,
) -> Result<Comparison, String> {
    require(
        actual.len() == raw.len() && music > 0,
        "full-frame comparison shape differs",
    )?;
    let mut result = Comparison {
        samples: 0,
        exact_bits: 0,
        mismatches: 0,
        maximum_absolute_error: 0.0,
        rms_error: 0.0,
        first_mismatch: None,
    };
    let mut sum_squares = 0.0_f64;
    for (frame, (&actual, &raw)) in actual.iter().zip(raw).enumerate() {
        let expected = expected_frame(raw, frame, music, channel_volume);
        for (channel, (actual, expected)) in
            [(actual.left, expected.left), (actual.right, expected.right)]
                .into_iter()
                .enumerate()
        {
            require(
                actual.is_finite() && expected.is_finite(),
                &format!("nonfinite master comparison at frame {frame}, channel {channel}"),
            )?;
            result.samples += 1;
            result.exact_bits += usize::from(actual.to_bits() == expected.to_bits());
            let difference = (actual - expected).abs();
            result.maximum_absolute_error = result.maximum_absolute_error.max(difference);
            sum_squares += f64::from(difference).powi(2);
            if difference > PCM_ABSOLUTE_TOLERANCE {
                result.mismatches += 1;
                result.first_mismatch.get_or_insert_with(|| json!({"frame":frame,"channel":channel,"actual":actual,"expected":expected,"actual_bits":actual.to_bits(),"expected_bits":expected.to_bits(),"absolute_error":difference}));
            }
        }
    }
    result.rms_error = if result.samples == 0 {
        0.0
    } else {
        (sum_squares / result.samples as f64).sqrt()
    };
    Ok(result)
}

struct PcmMetrics {
    frames: usize,
    peak: f32,
    rms: f64,
    nonzero_frames: usize,
    quiet_frames: usize,
    clipping_samples: usize,
    first_nonzero_frame: Option<usize>,
    last_nonzero_frame: Option<usize>,
    maximum_internal_zero_run: usize,
    maximum_quiet_run: usize,
    maximum_adjacent_channel_delta: f64,
    tail_nonzero_frames: usize,
    tail_peak: f32,
    tail_rms: f64,
    last_100ms_peak: f32,
    tail_end_threshold: f32,
}

impl PcmMetrics {
    fn measure(samples: &[StereoFrame], music: usize, tail: usize) -> Result<Self, String> {
        require(
            (music == 0 && tail == 0) || music.checked_add(tail) == Some(samples.len()),
            "PCM measurement shape differs from complete retained interval",
        )?;
        let mut result = Self {
            frames: samples.len(),
            peak: 0.0,
            rms: 0.0,
            nonzero_frames: 0,
            quiet_frames: 0,
            clipping_samples: 0,
            first_nonzero_frame: None,
            last_nonzero_frame: None,
            maximum_internal_zero_run: 0,
            maximum_quiet_run: 0,
            maximum_adjacent_channel_delta: 0.0,
            tail_nonzero_frames: 0,
            tail_peak: 0.0,
            tail_rms: 0.0,
            last_100ms_peak: 0.0,
            tail_end_threshold: 0.0,
        };
        let mut zero_run = 0;
        let mut quiet_run = 0;
        let mut sum_squares = 0.0_f64;
        let mut tail_sum_squares = 0.0_f64;
        let last_window_start = samples.len().saturating_sub(RATE / 10);
        for (frame, sample) in samples.iter().enumerate() {
            require(
                sample.left.is_finite() && sample.right.is_finite(),
                &format!("nonfinite PCM measurement at frame {frame}"),
            )?;
            let peak = sample.left.abs().max(sample.right.abs());
            result.peak = result.peak.max(peak);
            sum_squares += f64::from(sample.left).powi(2) + f64::from(sample.right).powi(2);
            result.clipping_samples +=
                usize::from(sample.left.abs() >= 1.0) + usize::from(sample.right.abs() >= 1.0);
            if peak > 0.0 {
                result.nonzero_frames += 1;
                if result.last_nonzero_frame.is_some() {
                    result.maximum_internal_zero_run =
                        result.maximum_internal_zero_run.max(zero_run);
                }
                result.first_nonzero_frame.get_or_insert(frame);
                result.last_nonzero_frame = Some(frame);
                zero_run = 0;
            } else {
                zero_run += 1;
            }
            if peak <= 1.0e-7 {
                result.quiet_frames += 1;
                quiet_run += 1;
                result.maximum_quiet_run = result.maximum_quiet_run.max(quiet_run);
            } else {
                quiet_run = 0;
            }
            if frame > 0 {
                let previous = samples[frame - 1];
                result.maximum_adjacent_channel_delta = result
                    .maximum_adjacent_channel_delta
                    .max((f64::from(sample.left) - f64::from(previous.left)).abs())
                    .max((f64::from(sample.right) - f64::from(previous.right)).abs());
            }
            if tail > 0 && frame >= music {
                result.tail_nonzero_frames += usize::from(peak > 0.0);
                result.tail_peak = result.tail_peak.max(peak);
                tail_sum_squares +=
                    f64::from(sample.left).powi(2) + f64::from(sample.right).powi(2);
            }
            if frame >= last_window_start {
                result.last_100ms_peak = result.last_100ms_peak.max(peak);
            }
        }
        if !samples.is_empty() {
            result.rms = (sum_squares / (samples.len() * 2) as f64).sqrt();
        }
        if tail > 0 {
            result.tail_rms = (tail_sum_squares / (tail * 2) as f64).sqrt();
        }
        result.tail_end_threshold = 1.0e-6_f32.max(result.peak * 1.0e-4);
        Ok(result)
    }

    fn evidence(&self) -> Value {
        json!({"frames":self.frames,"samples":self.frames*2,"nonfinite_samples":0,"peak":self.peak,"rms":self.rms,
            "nonzero_frames":self.nonzero_frames,"quiet_threshold":1.0e-7,"quiet_frames":self.quiet_frames,
            "clipping_samples":self.clipping_samples,"first_nonzero_frame":self.first_nonzero_frame,"last_nonzero_frame":self.last_nonzero_frame,
            "maximum_internal_zero_run_frames":self.maximum_internal_zero_run,"maximum_quiet_run_frames":self.maximum_quiet_run,
            "maximum_adjacent_channel_delta":self.maximum_adjacent_channel_delta,"tail_nonzero_frames":self.tail_nonzero_frames,
            "tail_peak":self.tail_peak,"tail_rms":self.tail_rms,"last_100ms_peak":self.last_100ms_peak,
            "tail_end_threshold":self.tail_end_threshold,"last_100ms_nonquiet":self.last_100ms_peak>self.tail_end_threshold,
            "adjacent_delta_is_measurement_not_listening_or_click_free_guarantee":true})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_geometry_keeps_full_latency_and_three_second_tail_without_changing_notes() {
        let shape = geometry(257).unwrap();
        assert_eq!(shape.music, 68_571);
        assert_eq!(shape.retained, 212_571);
        assert_eq!(shape.generated, 212_864);
        assert_eq!(shape.calls, 1_663);
        assert_eq!(shape.generated - shape.latency - shape.retained, 36);
        let ledger = expected_midi();
        assert_eq!(ledger[0], MidiEntry::at(0, [0x90, 60, 25]));
        assert_eq!(ledger[1], MidiEntry::at(34_286, [0x80, 60, 0]));
        assert_eq!(ledger[2], MidiEntry::at(34_286, [0x90, 64, 25]));
        assert_eq!(ledger[3], MidiEntry::at(68_571, [0x80, 64, 0]));
        assert!(geometry(u32::MAX).is_err());
    }

    #[test]
    fn analytical_control_holds_last_sample_and_protects_headroom_once() {
        let music = beat_frame(2.0);
        assert_eq!(analytical_master_volume(0, music), 0.85_f32);
        let last = analytical_master_volume(music - 1, music);
        assert!(last > 0.35 && last < 0.35002);
        assert_eq!(analytical_master_volume(music, music), last);
        assert_eq!(analytical_master_volume(music + TAIL - 1, music), last);
        let value = expected_frame(
            StereoFrame {
                left: 16.0,
                right: -16.0,
            },
            0,
            music,
            0.5,
        );
        assert_eq!(value.left, (8.0_f32 * 0.5 * 0.85).tanh());
        assert_eq!(value.right, (-8.0_f32 * 0.5 * 0.85).tanh());
        assert_ne!(value.left, ((16.0_f32 * 0.5).tanh() * 0.85).tanh());
    }

    #[test]
    fn complete_comparison_detects_changes_in_last_frame_and_both_channels() {
        let music = beat_frame(2.0);
        let raw = vec![
            StereoFrame {
                left: 0.2,
                right: -0.1
            };
            music + TAIL
        ];
        let mut actual = raw
            .iter()
            .enumerate()
            .map(|(frame, &raw)| expected_frame(raw, frame, music, 0.75))
            .collect::<Vec<_>>();
        let exact = compare_every_frame(&actual, &raw, music, 0.75).unwrap();
        assert_eq!(exact.samples, (music + TAIL) * 2);
        assert_eq!(exact.exact_bits, exact.samples);
        assert_eq!(exact.mismatches, 0);
        actual[0].left += 0.0001;
        actual.last_mut().unwrap().right -= 0.0001;
        let damaged = compare_every_frame(&actual, &raw, music, 0.75).unwrap();
        assert_eq!(damaged.samples, exact.samples);
        assert_eq!(damaged.mismatches, 2);
    }

    #[test]
    fn measurement_exposes_nonquiet_tail_and_exact_internal_zero_run() {
        let music = 10;
        let mut pcm = vec![
            StereoFrame {
                left: 0.1,
                right: -0.1
            };
            music + TAIL
        ];
        pcm[2..5].fill(StereoFrame::default());
        let metrics = PcmMetrics::measure(&pcm, music, TAIL).unwrap();
        assert_eq!(metrics.maximum_internal_zero_run, 3);
        assert_eq!(metrics.tail_nonzero_frames, TAIL);
        assert!(metrics.last_100ms_peak > metrics.tail_end_threshold);
        pcm[music..].fill(StereoFrame::default());
        let ended = PcmMetrics::measure(&pcm, music, TAIL).unwrap();
        assert_eq!(ended.last_100ms_peak, 0.0);
        assert_eq!(ended.tail_nonzero_frames, 0);
        pcm[1].right = f32::NAN;
        assert!(PcmMetrics::measure(&pcm, music, TAIL).is_err());
    }
}
