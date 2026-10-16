use super::*;

struct Notes(Vec<Event>);
impl Class for Notes {
    type Interfaces = (IEventList,);
}
impl IEventListTrait for Notes {
    unsafe fn getEventCount(&self) -> i32 {
        self.0.len() as i32
    }
    unsafe fn getEvent(&self, index: i32, event: *mut Event) -> tresult {
        if event.is_null() || index < 0 {
            return kInvalidArgument;
        }
        match self.0.get(index as usize) {
            Some(value) => {
                *event = *value;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, _event: *mut Event) -> tresult {
        kNotImplemented
    }
}

fn note(offset: i32, on: bool) -> Event {
    let mut event: Event = unsafe { std::mem::zeroed() };
    event.sampleOffset = offset;
    if on {
        event.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
        event.__field0.noteOn = NoteOnEvent {
            channel: 0,
            pitch: 63,
            tuning: 0.0,
            velocity: 1.0,
            length: 0,
            noteId: -1,
        };
    } else {
        event.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
        event.__field0.noteOff = NoteOffEvent {
            channel: 0,
            pitch: 63,
            tuning: 0.0,
            velocity: 0.0,
            noteId: -1,
        };
    }
    event
}

fn prepared(mode: i32) -> Effect {
    let instrument = Effect::instrument();
    unsafe {
        let mut setup = ProcessSetup {
            processMode: mode,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            maxSamplesPerBlock: 2048,
            sampleRate: 48_000.0,
        };
        assert_eq!(instrument.setupProcessing(&mut setup), kResultOk);
        assert_eq!(
            instrument.activateBus(
                MediaTypes_::kEvent as i32,
                BusDirections_::kInput as i32,
                0,
                1
            ),
            kResultOk
        );
        assert_eq!(
            instrument.activateBus(
                MediaTypes_::kAudio as i32,
                BusDirections_::kOutput as i32,
                0,
                1
            ),
            kResultOk
        );
        assert_eq!(instrument.setActive(1), kResultOk);
        assert_eq!(instrument.setProcessing(1), kResultOk);
    }
    instrument
}

unsafe fn block(
    instrument: &Effect,
    mode: i32,
    start: i64,
    notes: Vec<Event>,
) -> (tresult, [Vec<f32>; 2]) {
    let events = ComWrapper::new(Notes(notes))
        .to_com_ptr::<IEventList>()
        .unwrap();
    let mut output = [vec![0.0f32; 128], vec![0.0f32; 128]];
    let mut pointers = [output[0].as_mut_ptr(), output[1].as_mut_ptr()];
    let mut bus: AudioBusBuffers = std::mem::zeroed();
    bus.numChannels = 2;
    bus.__field0.channelBuffers32 = pointers.as_mut_ptr();
    let mut context: ProcessContext = std::mem::zeroed();
    context.state = (ProcessContext_::StatesAndFlags_::kPlaying
        | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
        | ProcessContext_::StatesAndFlags_::kTempoValid
        | ProcessContext_::StatesAndFlags_::kTimeSigValid) as u32;
    context.sampleRate = 48_000.0;
    context.projectTimeSamples = start;
    context.projectTimeMusic = start as f64 / 24_000.0;
    context.tempo = 120.0;
    context.timeSigNumerator = 4;
    context.timeSigDenominator = 4;
    let mut data: ProcessData = std::mem::zeroed();
    data.processMode = mode;
    data.symbolicSampleSize = SymbolicSampleSizes_::kSample32 as i32;
    data.numSamples = 128;
    data.numOutputs = 1;
    data.outputs = &mut bus;
    data.inputEvents = events.as_ptr();
    data.processContext = &mut context;
    (instrument.process(&mut data), output)
}

#[test]
fn actual_instrument_sdk_declares_midi_stereo_and_real_fifo_crosses_q128() {
    for mode in [
        ProcessModes_::kRealtime as i32,
        ProcessModes_::kOffline as i32,
    ] {
        let instrument = prepared(mode);
        unsafe {
            assert_eq!(
                instrument.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32),
                0
            );
            assert_eq!(
                instrument.getBusCount(MediaTypes_::kEvent as i32, BusDirections_::kInput as i32),
                1
            );
            let mut info: BusInfo = std::mem::zeroed();
            assert_eq!(
                instrument.getBusInfo(
                    MediaTypes_::kEvent as i32,
                    BusDirections_::kInput as i32,
                    0,
                    &mut info
                ),
                kResultOk
            );
            assert_eq!(info.channelCount, 16);
            assert_eq!(instrument.getLatencySamples(), 32);
            assert_eq!(instrument.getTailSamples(), 32);
            let mut observed = [Vec::new(), Vec::new()];
            for block_index in 0..3 {
                let notes = match block_index {
                    0 => vec![note(0, true), note(1, false), note(127, true)],
                    1 => vec![note(1, false)],
                    _ => vec![],
                };
                let (result, audio) = block(&instrument, mode, block_index * 128, notes);
                assert_eq!(result, kResultOk);
                for (dest, source) in observed.iter_mut().zip(audio) {
                    dest.extend(source);
                }
            }
            // The undelayed voice exists only at raw frames0,127,128. The
            // independent oracle checks all stereo samples, not just metadata.
            for (channel, audio) in observed.iter().enumerate() {
                for (frame, &actual) in audio.iter().enumerate() {
                    let expected = if matches!(frame, 32 | 159 | 160) {
                        if channel == 0 {
                            0.25
                        } else {
                            -0.125
                        }
                    } else {
                        0.0
                    };
                    assert_eq!(actual, expected, "channel{channel} rawframe{frame}");
                }
            }
            assert_eq!(instrument.getParamNormalized(1000), 3.0 / PROBE_SCALE);
            assert_eq!(
                instrument.getParamNormalized(1001),
                256.0 / SAMPLE_POSITION_SCALE
            );
            assert_eq!(
                instrument.getParamNormalized(1005),
                f64::from(u8::from(mode == ProcessModes_::kOffline as i32))
            );
            assert_eq!(instrument.getParamNormalized(1006), 1.0);
        }
    }
}

#[test]
fn actual_instrument_rejects_invalid_midi_and_reset_removes_voices_and_fifo() {
    let mode = ProcessModes_::kOffline as i32;
    let instrument = prepared(mode);
    unsafe {
        assert_eq!(
            block(&instrument, mode, 0, vec![note(2, true), note(1, false)]).0,
            kResultFalse
        );
        assert_eq!(
            block(&instrument, mode, 0, vec![note(128, true)]).0,
            kResultFalse
        );
        assert_eq!(instrument.getParamNormalized(1000), 0.0);
        let (_, audio) = block(&instrument, mode, 0, vec![note(0, true)]);
        assert_eq!(audio[0][31], 0.0);
        assert_eq!(audio[0][32], 0.25);
        assert_eq!(instrument.setActive(0), kResultOk);
        assert_eq!(instrument.setActive(1), kResultOk);
        assert_eq!(instrument.setProcessing(1), kResultOk);
        let (result, audio) = block(&instrument, mode, 128, vec![]);
        assert_eq!(result, kResultOk);
        assert!(audio.iter().flatten().all(|&sample| sample == 0.0));
    }
}
