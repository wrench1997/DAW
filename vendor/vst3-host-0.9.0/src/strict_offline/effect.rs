//! True-PCM stereo Effect admission on the existing retained child.

use super::*;

/// Exact source identity expected by the caller. Category tokens must contain
/// `Fx` and must not contain `Instrument`; substring matches are not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIdentity {
    /// Exact current class identifier, compared case-insensitively as hex.
    pub class_id: String,
    /// Exact SDK-reported vendor.
    pub vendor: String,
    /// Exact SDK-reported name.
    pub name: String,
    /// Exact nonempty SDK-reported version.
    pub version: String,
    /// Complete, exact SDK-reported subcategory string.
    pub category: String,
}

impl EffectIdentity {
    fn is_effect(&self) -> bool {
        self.class_id.len() == 32
            && [&self.vendor, &self.name, &self.version, &self.category]
                .iter()
                .all(|text| !text.is_empty() && text.len() <= 64 * 1024)
            && self.class_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.category.split('|').any(|token| token == "Fx")
            && !self.category.split('|').any(|token| token == "Instrument")
    }

    fn matches(&self, actual: &LoadedPlugin) -> bool {
        self.class_id.eq_ignore_ascii_case(&actual.uid)
            && self.vendor == actual.vendor
            && self.name == actual.name
            && self.version == actual.version
            && self.category == actual.category
            && !actual.has_midi_input
            && !actual.has_midi_output
            && actual.audio_inputs == 1
            && actual.audio_outputs == 1
            && actual.output_channels == 2
    }
}

/// Fresh clone receipt, not a source-instance capture or proof of state-byte
/// equivalence. The client privately retains the corresponding admission.
#[derive(Debug, Clone, PartialEq)]
pub struct StereoEffectMetadata {
    /// Identity checked against this client's actual LoadPlugin reply.
    pub identity: EffectIdentity,
    /// Exact requested bundle path.
    pub path: String,
    /// Acknowledged load sample rate.
    pub sample_rate: f64,
    /// Exact checked configured maximum from the stopped owner.
    pub max_block_frames: u32,
    /// Acknowledged load transport tempo.
    pub tempo: f64,
    /// Acknowledged load meter numerator.
    pub time_sig_numerator: i32,
    /// Acknowledged load meter denominator.
    pub time_sig_denominator: i32,
    /// Exact acknowledged opaque input byte count, including zero if supplied.
    pub state_bytes: usize,
    /// Complete active main stereo input/output descriptors.
    pub audio: CheckedAudioBusLayout,
    /// Complete event layout; both directions must be empty.
    pub events: EventBusLayout,
    /// Actual DSP latency, not realtime bridge/lookahead latency.
    pub latency_samples: u32,
    /// Actual advertised tail, retaining u32::MAX without interpretation.
    pub tail_samples: u32,
    /// Quiet IProgress revision armed at the actual DSP boundary.
    pub async_started_revision: u64,
    /// Native edit revision surrounding all metadata getters.
    pub native_dirty_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeMetadata {
    audio: CheckedAudioBusLayout,
    events: EventBusLayout,
    latency_samples: u32,
    tail_samples: u32,
    async_started_revision: u64,
    native_dirty_revision: u64,
}

pub(super) struct EffectSession {
    identity: EffectIdentity,
    path: String,
    sample_rate: f64,
    tempo: f64,
    numerator: i32,
    denominator: i32,
    pub(super) offline: bool,
    pub(super) state_bytes: Option<usize>,
    pub(super) processing: bool,
    admitted: Option<RuntimeMetadata>,
}

fn stereo(layout: &CheckedAudioBusLayout) -> bool {
    let side = |buses: &[crate::plugin::CheckedAudioBusConfig]| {
        buses.len() == 1
            && buses[0].is_main
            && buses[0].channel_count == 2
            && buses[0].active == Some(true)
    };
    side(&layout.inputs) && side(&layout.outputs)
}

impl Client {
    pub(super) fn invalidate_effect_admission(&mut self) {
        if let Some(effect) = &mut self.effect {
            effect.admitted = None;
        }
    }

    /// Load an explicitly identified Effect through the same one-child owner.
    /// Only the actual reply can establish the private role binding. A wrong
    /// identity/role/layout seals the child, which still requires checked reap.
    /// The successful role binding is sticky: neither load entry point may
    /// replace this instance. Dispose/reap it before loading another stage.
    /// Ordinary load/process and silent-generator entry points are unchanged.
    pub fn load_stereo_effect(
        &mut self,
        spec: LoadSpec<'_>,
        expected: &EffectIdentity,
        deadline: Instant,
    ) -> Result<LoadedPlugin, Failure> {
        if self.effect.is_some() {
            return Err(self.reject(fresh_helper::ErrorKind::UnsupportedRequest, deadline));
        }
        if !expected.is_effect() || !expected.class_id.eq_ignore_ascii_case(spec.class_id) {
            return Err(self.reject(fresh_helper::ErrorKind::InvalidRequest, deadline));
        }
        let loaded = self.load_plugin(spec, deadline)?;
        if !expected.matches(&loaded) {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        self.effect = Some(EffectSession {
            identity: expected.clone(),
            path: spec.path.to_owned(),
            sample_rate: spec.sample_rate,
            tempo: spec.tempo,
            numerator: spec.time_sig_numerator,
            denominator: spec.time_sig_denominator,
            offline: false,
            state_bytes: None,
            processing: false,
            admitted: None,
        });
        Ok(loaded)
    }

    fn effect_runtime_metadata(&mut self, deadline: Instant) -> Result<RuntimeMetadata, Failure> {
        let before = self.async_state_snapshot(deadline)?;
        let revision = self.native_dirty_revision(deadline)?;
        let audio = self.checked_audio_layout(deadline)?;
        let events = self.event_layout(deadline)?;
        let latency_samples = self.latency_samples(deadline)?;
        let tail_samples = self.tail_samples(deadline)?;
        let after_revision = self.native_dirty_revision(deadline)?;
        let after = self.async_state_snapshot(deadline)?;
        // UI-only progress may legitimately change; it does not enter the DSP
        // readiness identity. Async work that starts and finishes still changes
        // its revision and therefore cannot slip between these observations.
        if before.ensure_ready().is_err()
            || after.ensure_ready().is_err()
            || before.async_started_revision != after.async_started_revision
            || revision != after_revision
            || !stereo(&audio)
            || !events.inputs.is_empty()
            || !events.outputs.is_empty()
            || self.selected_audio_layout.as_ref() != Some(&audio)
        {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        Ok(RuntimeMetadata {
            audio,
            events,
            latency_samples,
            tail_samples,
            async_started_revision: after.async_started_revision,
            native_dirty_revision: after_revision,
        })
    }

    /// After explicit Offline, opaque state, generic bases/readback and stereo
    /// bus selection, admit the stopped clone and arm actual DSP readiness.
    /// Call start_processing only after comparing this receipt with the source
    /// capture. State/parameter/bus-selection changes invalidate admission.
    /// Exact latency/tail are retained; the renderer owns PDC and tail policy.
    pub fn admit_stereo_effect(
        &mut self,
        deadline: Instant,
    ) -> Result<StereoEffectMetadata, Failure> {
        self.invalidate_effect_admission();
        if !self.effect.as_ref().is_some_and(|effect| {
            effect.offline && effect.state_bytes.is_some() && !effect.processing
        }) || !self.audio_selection_required
            || self.selected_audio_layout.is_none()
        {
            return Err(self.reject(fresh_helper::ErrorKind::InvalidRequest, deadline));
        }
        let support = self.reset_origin_support(deadline)?;
        if support.contract_version != 1 || support.max_block_frames != 2048 || support.processing {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        let metadata = self.effect_runtime_metadata(deadline)?;
        self.arm_offline_state_readiness(metadata.async_started_revision, deadline)?;
        if self.effect_runtime_metadata(deadline)? != metadata {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        let effect = self.effect.as_mut().expect("checked effect role");
        let receipt = StereoEffectMetadata {
            identity: effect.identity.clone(),
            path: effect.path.clone(),
            sample_rate: effect.sample_rate,
            max_block_frames: support.max_block_frames,
            tempo: effect.tempo,
            time_sig_numerator: effect.numerator,
            time_sig_denominator: effect.denominator,
            state_bytes: effect.state_bytes.expect("acknowledged state"),
            audio: metadata.audio.clone(),
            events: metadata.events.clone(),
            latency_samples: metadata.latency_samples,
            tail_samples: metadata.tail_samples,
            async_started_revision: metadata.async_started_revision,
            native_dirty_revision: metadata.native_dirty_revision,
        };
        effect.admitted = Some(metadata);
        Ok(receipt)
    }

    pub(super) fn verify_effect_admission(&mut self, deadline: Instant) -> Result<(), Failure> {
        let Some(effect) = &self.effect else {
            return Ok(());
        };
        let Some(expected) = effect.admitted.clone() else {
            return Err(self.reject(fresh_helper::ErrorKind::InvalidRequest, deadline));
        };
        if self.effect_runtime_metadata(deadline)? != expected {
            return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
        }
        Ok(())
    }

    /// Process borrowed, genuine stereo Q128 PCM on an admitted, started Effect.
    /// No clipping, padding, dry fallback, MIDI injection or output publication.
    /// Finite samples outside [-1,1] are legitimate internal audio. Explicit
    /// 2x128 zero input drains tails; an empty input is a shape error, not silence.
    /// Every failure seals the same child; dispose it before another stage.
    pub fn process_stereo_effect(
        &mut self,
        inputs: &[Vec<f32>],
        transport: ProcessTransport,
        deadline: Instant,
    ) -> Result<AudioBlock, Failure> {
        if !self.effect.as_ref().is_some_and(|effect| effect.processing) {
            return Err(self.reject(fresh_helper::ErrorKind::InvalidRequest, deadline));
        }
        self.verify_effect_admission(deadline)?;
        let reply = self.exchange(
            RequestView::stereo_effect(inputs, transport),
            ReplyShape::Audio {
                channels: 2,
                frames: 128,
            },
            deadline,
        )?;
        self.verify_effect_admission(deadline)?;
        match reply {
            HostResponse::AudioOutput {
                outputs,
                output_events,
                output_events_lost,
                transport_applied,
            } => {
                if output_events_lost
                    || !transport_applied
                    || !output_events.is_empty()
                    || outputs.iter().flatten().any(|sample| !sample.is_finite())
                {
                    return Err(self.reject(fresh_helper::ErrorKind::Shape, deadline));
                }
                Ok(AudioBlock {
                    outputs,
                    output_event_count: 0,
                    output_events_lost,
                    transport_applied,
                })
            }
            _ => unreachable!("strict effect audio shape"),
        }
    }
}
