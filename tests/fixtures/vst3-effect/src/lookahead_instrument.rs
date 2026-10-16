//! The second factory class: sample-accurate MIDI followed by a real stereo FIFO.
use super::*;

pub(super) struct VoiceState {
    notes: [[f32; 128]; 16],
    level: f32,
    history: [[f32; LATENCY]; 2],
    cursor: usize,
}

impl Default for VoiceState {
    fn default() -> Self {
        Self {
            notes: [[0.0; 128]; 16],
            level: 0.0,
            history: [[0.0; LATENCY]; 2],
            cursor: 0,
        }
    }
}

#[derive(Clone, Copy)]
enum Change {
    Note {
        channel: usize,
        pitch: usize,
        velocity: f32,
    },
    SilenceChannel(usize),
}

impl Change {
    unsafe fn from_event(event: &Event) -> Option<Self> {
        let note = |channel: i16, pitch: i16, velocity: f32| {
            if !(0..16).contains(&channel)
                || !(0..128).contains(&pitch)
                || !velocity.is_finite()
                || !(0.0..=1.0).contains(&velocity)
            {
                return None;
            }
            Some(Self::Note {
                channel: channel as usize,
                pitch: pitch as usize,
                velocity,
            })
        };
        match event.r#type as u32 {
            t if t == Event_::EventTypes_::kNoteOnEvent as u32 => {
                let n = event.__field0.noteOn;
                if n.tuning != 0.0 {
                    return None;
                }
                note(n.channel, n.pitch, n.velocity)
            }
            t if t == Event_::EventTypes_::kNoteOffEvent as u32 => {
                let n = event.__field0.noteOff;
                if n.tuning != 0.0 {
                    return None;
                }
                note(n.channel, n.pitch, 0.0)
            }
            t if t == Event_::EventTypes_::kLegacyMIDICCOutEvent as u32 => {
                let cc = event.__field0.midiCCOut;
                if !(0..16).contains(&cc.channel) || !matches!(cc.controlNumber, 120 | 123) {
                    return None;
                }
                Some(Self::SilenceChannel(cc.channel as usize))
            }
            _ => None,
        }
    }
}

impl VoiceState {
    fn apply(&mut self, change: Change) {
        match change {
            Change::Note {
                channel,
                pitch,
                velocity,
            } => {
                self.notes[channel][pitch] = velocity * ((pitch + 1) as f32 / 128.0);
            }
            Change::SilenceChannel(channel) => self.notes[channel].fill(0.0),
        }
        // Fixed channel/pitch order makes the polyphonic fixture's oracle reproducible.
        self.level = self.notes.iter().flatten().copied().sum();
    }

    fn tick(&mut self, gain: f32) -> [f32; 2] {
        let output = [self.history[0][self.cursor], self.history[1][self.cursor]];
        let left = gain * self.level;
        self.history[0][self.cursor] = left;
        self.history[1][self.cursor] = -0.5 * left;
        self.cursor = (self.cursor + 1) % LATENCY;
        output
    }
}

pub(super) unsafe fn process(effect: &Effect, data: *mut ProcessData) -> tresult {
    let Some(data) = data.as_mut() else {
        return kInvalidArgument;
    };
    let Ok(mut engine) = effect.engine.lock() else {
        return kResultFalse;
    };
    if data.numSamples == 0 {
        if ComRef::from_raw(data.inputEvents).is_some_and(|events| events.getEventCount() != 0) {
            return kResultFalse;
        }
        return apply_static_changes(&mut engine, data.inputParameterChanges);
    }
    if !engine.active
        || !engine.processing
        || !engine.input_active
        || !engine.output_active
        || !matches!(data.processMode, mode if mode == ProcessModes_::kRealtime as i32 || mode == ProcessModes_::kOffline as i32)
        || engine.mode != data.processMode
        || data.symbolicSampleSize != SymbolicSampleSizes_::kSample32 as i32
        || !(1..=engine.max_frames).contains(&data.numSamples)
        || data.numInputs != 0
        || data.numOutputs != 1
        || data.outputs.is_null()
    {
        return kResultFalse;
    }
    let Some(context) = data.processContext.as_ref() else {
        return kResultFalse;
    };
    let required = (ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
        | ProcessContext_::StatesAndFlags_::kTempoValid
        | ProcessContext_::StatesAndFlags_::kTimeSigValid) as u32;
    if context.state & required != required
        || !context.tempo.is_finite()
        || context.tempo <= 0.0
        || !context.projectTimeMusic.is_finite()
        || context.sampleRate != engine.sample_rate
        || context.timeSigNumerator <= 0
        || !matches!(context.timeSigDenominator, 1 | 2 | 4 | 8 | 16)
    {
        return kResultFalse;
    }
    let output_bus = &mut *data.outputs;
    if output_bus.numChannels != 2 || output_bus.__field0.channelBuffers32.is_null() {
        return kResultFalse;
    }
    let pointers = slice::from_raw_parts(output_bus.__field0.channelBuffers32, 2);
    if pointers.iter().any(|p| p.is_null()) {
        return kResultFalse;
    }
    let frames = data.numSamples as usize;
    let mut changes = Vec::new();
    if let Some(events) = ComRef::from_raw(data.inputEvents) {
        let count = events.getEventCount();
        if !(0..=4096).contains(&count) || changes.try_reserve_exact(count as usize).is_err() {
            return kResultFalse;
        }
        let mut previous = 0;
        for index in 0..count {
            let mut event: Event = std::mem::zeroed();
            if events.getEvent(index, &mut event) != kResultOk
                || event.busIndex != 0
                || event.sampleOffset < previous
                || !(0..data.numSamples).contains(&event.sampleOffset)
            {
                return kResultFalse;
            }
            let Some(change) = Change::from_event(&event) else {
                return kResultFalse;
            };
            previous = event.sampleOffset;
            changes.push((event.sampleOffset as usize, change));
        }
    }
    if apply_static_changes(&mut engine, data.inputParameterChanges) != kResultOk {
        return kResultFalse;
    }
    engine.outputs_initially_clear = pointers
        .iter()
        .all(|p| (0..frames).all(|f| *p.add(f) == 0.0));
    let gain = engine.gain as f32;
    let mut changes = changes.into_iter().peekable();
    for frame in 0..frames {
        while changes.peek().is_some_and(|(offset, _)| *offset == frame) {
            engine.voice.apply(changes.next().unwrap().1);
        }
        let output = engine.voice.tick(gain);
        *pointers[0].add(frame) = output[0];
        *pointers[1].add(frame) = output[1];
    }
    output_bus.silenceFlags = pointers.iter().enumerate().fold(0, |flags, (channel, p)| {
        flags
            | if (0..frames).all(|f| *p.add(f) == 0.0) {
                1 << channel
            } else {
                0
            }
    });
    engine.process_calls += 1;
    engine.last_sample = context.projectTimeSamples;
    engine.last_quarter = context.projectTimeMusic;
    engine.last_tempo = context.tempo;
    engine.last_playing = context.state & ProcessContext_::StatesAndFlags_::kPlaying as u32 != 0;
    kResultOk
}
