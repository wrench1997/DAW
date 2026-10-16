use super::*;
use vst3::{ComPtr, Interface};

fn prepared() -> Effect {
    prepared_mode(ProcessModes_::kOffline as i32)
}

fn prepared_mode(mode: i32) -> Effect {
    let effect = Effect::default();
    unsafe {
        let mut setup = ProcessSetup {
            processMode: mode,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            maxSamplesPerBlock: 2048,
            sampleRate: 48_000.0,
        };
        assert_eq!(effect.setupProcessing(&mut setup), kResultOk);
        assert_eq!(
            effect.activateBus(
                MediaTypes_::kAudio as i32,
                BusDirections_::kInput as i32,
                0,
                1
            ),
            kResultOk
        );
        assert_eq!(
            effect.activateBus(
                MediaTypes_::kAudio as i32,
                BusDirections_::kOutput as i32,
                0,
                1
            ),
            kResultOk
        );
        assert_eq!(effect.setActive(1), kResultOk);
        assert_eq!(effect.setProcessing(1), kResultOk);
    }
    effect
}

unsafe fn block(
    effect: &Effect,
    input: &mut [Vec<f32>; 2],
    start: i64,
    output_fill: f32,
) -> (tresult, [Vec<f32>; 2]) {
    block_mode(
        effect,
        input,
        start,
        output_fill,
        ProcessModes_::kOffline as i32,
    )
}

unsafe fn block_mode(
    effect: &Effect,
    input: &mut [Vec<f32>; 2],
    start: i64,
    output_fill: f32,
    mode: i32,
) -> (tresult, [Vec<f32>; 2]) {
    block_context(effect, input, start, output_fill, mode, 120.0)
}

unsafe fn block_context(
    effect: &Effect,
    input: &mut [Vec<f32>; 2],
    start: i64,
    output_fill: f32,
    mode: i32,
    tempo: f64,
) -> (tresult, [Vec<f32>; 2]) {
    let frames = input[0].len();
    let mut output = [vec![output_fill; frames], vec![output_fill; frames]];
    let mut input_ptrs = [input[0].as_mut_ptr(), input[1].as_mut_ptr()];
    let mut output_ptrs = [output[0].as_mut_ptr(), output[1].as_mut_ptr()];
    let mut input_bus: AudioBusBuffers = std::mem::zeroed();
    input_bus.numChannels = 2;
    input_bus.__field0.channelBuffers32 = input_ptrs.as_mut_ptr();
    let mut output_bus: AudioBusBuffers = std::mem::zeroed();
    output_bus.numChannels = 2;
    output_bus.__field0.channelBuffers32 = output_ptrs.as_mut_ptr();
    let mut context: ProcessContext = std::mem::zeroed();
    context.state = (ProcessContext_::StatesAndFlags_::kPlaying
        | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
        | ProcessContext_::StatesAndFlags_::kTempoValid
        | ProcessContext_::StatesAndFlags_::kTimeSigValid) as u32;
    context.sampleRate = 48_000.0;
    context.projectTimeSamples = start;
    context.projectTimeMusic = start as f64 / (48_000.0 * 60.0 / tempo);
    context.tempo = tempo;
    context.timeSigNumerator = 4;
    context.timeSigDenominator = 4;
    let mut data: ProcessData = std::mem::zeroed();
    data.processMode = mode;
    data.symbolicSampleSize = SymbolicSampleSizes_::kSample32 as i32;
    data.numSamples = frames as i32;
    data.numInputs = 1;
    data.numOutputs = 1;
    data.inputs = &mut input_bus;
    data.outputs = &mut output_bus;
    data.processContext = &mut context;
    (effect.process(&mut data), output)
}

#[test]
fn sdk_factory2_preserves_default_fx_and_separately_advertises_lookahead_instrument() {
    unsafe {
        let factory = ComWrapper::new(Factory)
            .to_com_ptr::<IPluginFactory2>()
            .unwrap();
        assert_eq!(factory.countClasses(), 2);
        let mut info: PClassInfo2 = std::mem::zeroed();
        let effect_index = i32::from(cfg!(feature = "instrument-first"));
        let instrument_index = 1 - effect_index;
        assert_eq!(factory.getClassInfo2(effect_index, &mut info), kResultOk);
        let text = |value: &[c_char]| {
            String::from_utf8(
                value
                    .iter()
                    .take_while(|c| **c != 0)
                    .map(|c| *c as u8)
                    .collect(),
            )
            .unwrap()
        };
        assert_eq!(info.cid, CID);
        assert_eq!(text(&info.subCategories), CATEGORY);
        assert_eq!(text(&info.vendor), VENDOR);
        assert_eq!(text(&info.version), VERSION);
        assert_eq!(text(&info.name), NAME);
        assert_eq!(
            factory.getClassInfo2(instrument_index, &mut info),
            kResultOk
        );
        assert_eq!(info.cid, INSTRUMENT_CID);
        assert_eq!(text(&info.name), INSTRUMENT_NAME);
        assert_eq!(text(&info.subCategories), INSTRUMENT_CATEGORY);
        assert_eq!(factory.getClassInfo2(2, &mut info), kInvalidArgument);

        let factory1 = factory.cast::<IPluginFactory>().unwrap();
        assert_eq!(factory1.countClasses(), 2);
        let mut expected = [
            (CID, NAME, CATEGORY),
            (INSTRUMENT_CID, INSTRUMENT_NAME, INSTRUMENT_CATEGORY),
        ];
        if cfg!(feature = "instrument-first") {
            expected.reverse();
        }
        for (index, (cid, name, category)) in expected.into_iter().enumerate() {
            let mut basic: PClassInfo = std::mem::zeroed();
            let mut extended: PClassInfo2 = std::mem::zeroed();
            assert_eq!(factory1.getClassInfo(index as i32, &mut basic), kResultOk);
            assert_eq!(
                factory.getClassInfo2(index as i32, &mut extended),
                kResultOk
            );
            assert_eq!(basic.cid, cid);
            assert_eq!(extended.cid, cid);
            assert_eq!(text(&basic.name), name);
            assert_eq!(text(&extended.name), name);
            assert_eq!(text(&basic.category), "Audio Module Class");
            assert_eq!(text(&extended.category), "Audio Module Class");
            assert_eq!(text(&extended.subCategories), category);
            assert_eq!(basic.cardinality, extended.cardinality);
        }
        for invalid in [-1, 2] {
            let mut basic: PClassInfo = std::mem::zeroed();
            assert_eq!(factory1.getClassInfo(invalid, &mut basic), kInvalidArgument);
            assert_eq!(factory.getClassInfo2(invalid, &mut info), kInvalidArgument);
        }
        // Class selection stays UID-based, regardless of declaration order.
        for (cid, audio_inputs, event_inputs, tail) in
            [(CID, 1, 0, 160), (INSTRUMENT_CID, 0, 1, 32)]
        {
            let mut raw: *mut IComponent = ptr::null_mut();
            assert_eq!(
                factory1.createInstance(
                    cid.as_ptr().cast(),
                    IComponent::IID.as_ptr().cast(),
                    (&mut raw as *mut *mut IComponent).cast(),
                ),
                kResultOk
            );
            let component = ComPtr::from_raw(raw).expect("factory-created component");
            let processor = component.cast::<IAudioProcessor>().unwrap();
            let controller = component.cast::<IEditController>().unwrap();
            assert_eq!(
                component.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32),
                audio_inputs
            );
            assert_eq!(
                component.getBusCount(MediaTypes_::kEvent as i32, BusDirections_::kInput as i32),
                event_inputs
            );
            assert_eq!(
                component.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kOutput as i32),
                1
            );
            assert_eq!(
                component.getBusCount(MediaTypes_::kEvent as i32, BusDirections_::kOutput as i32),
                0
            );
            assert_eq!(processor.getLatencySamples(), 32);
            assert_eq!(processor.getTailSamples(), tail);
            assert!(controller.createView(ptr::null()).is_null());
        }
        let effect = ComWrapper::new(Effect::default())
            .to_com_ptr::<IComponent>()
            .unwrap();
        assert!(effect.cast::<IAudioProcessor>().is_some());
        assert!(effect.cast::<IEditController>().is_some());
        assert_eq!(
            effect.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32),
            1
        );
        assert_eq!(
            effect.getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kOutput as i32),
            1
        );
        assert_eq!(
            effect.getBusCount(MediaTypes_::kEvent as i32, BusDirections_::kInput as i32),
            0
        );
        assert_eq!(
            effect.getBusCount(MediaTypes_::kEvent as i32, BusDirections_::kOutput as i32),
            0
        );
        assert!(effect
            .cast::<IEditController>()
            .unwrap()
            .createView(ptr::null())
            .is_null());
    }
}

#[test]
fn actual_sdk_process_carries_delay_and_echo_across_q128_boundaries() {
    let effect = prepared();
    let mut source = [vec![0.0f32; 512], vec![0.0f32; 512]];
    source[0][0] = 2.0;
    source[0][127] = -0.5;
    source[1][63] = -1.0;
    source[1][128] = 0.75;
    let mut received = [Vec::new(), Vec::new()];
    for start in (0..512).step_by(128) {
        let mut input = [
            source[0][start..start + 128].to_vec(),
            source[1][start..start + 128].to_vec(),
        ];
        let (result, output) = unsafe { block(&effect, &mut input, start as i64, 0.0) };
        assert_eq!(result, kResultOk);
        for channel in 0..2 {
            received[channel].extend_from_slice(&output[channel]);
        }
    }
    for channel in 0..2 {
        for (sample, actual) in received[channel].iter().enumerate() {
            let direct = sample.checked_sub(32).map_or(0.0, |i| source[channel][i]);
            let echo = sample.checked_sub(160).map_or(0.0, |i| source[channel][i]);
            assert_eq!(
                *actual,
                0.5 * (direct + 0.5 * echo),
                "channel {channel}, sample {sample}"
            );
        }
    }
    assert_eq!(received[0][32], 1.0);
    assert_eq!(received[0][160], 0.5);
    assert_eq!(received[1][288], 0.1875);
    assert!(received
        .iter()
        .all(|channel| channel[289..].iter().all(|sample| *sample == 0.0)));
    unsafe {
        assert_eq!(effect.getLatencySamples(), 32);
        assert_eq!(effect.getTailSamples(), 160);
        assert_eq!(effect.getParamNormalized(1000), 4.0 / PROBE_SCALE);
        assert_eq!(
            effect.getParamNormalized(1001),
            384.0 / SAMPLE_POSITION_SCALE
        );
        assert_eq!(effect.getParamNormalized(1005), 1.0);
        assert_eq!(effect.getParamNormalized(1006), 1.0);
    }
}

#[test]
fn actual_process_requires_active_input_output_and_processing_and_detects_clear_buffers() {
    let effect = prepared();
    let mut input = [vec![0.0; 128], vec![0.0; 128]];
    unsafe {
        assert_eq!(effect.setProcessing(0), kResultOk);
        assert_eq!(block(&effect, &mut input, 0, 0.0).0, kResultFalse);
        assert_eq!(effect.setProcessing(1), kResultOk);
        assert_eq!(
            effect.activateBus(
                MediaTypes_::kAudio as i32,
                BusDirections_::kInput as i32,
                0,
                0
            ),
            kResultOk
        );
        assert_eq!(block(&effect, &mut input, 0, 0.0).0, kResultFalse);
        assert_eq!(
            effect.activateBus(
                MediaTypes_::kAudio as i32,
                BusDirections_::kInput as i32,
                0,
                1
            ),
            kResultOk
        );
        assert_eq!(block(&effect, &mut input, 0, 0.25).0, kResultOk);
        assert_eq!(effect.getParamNormalized(1006), 0.0);
        assert_eq!(block(&effect, &mut input, 128, 0.0).0, kResultOk);
        assert_eq!(effect.getParamNormalized(1006), 1.0);
    }
}

#[test]
fn gain_state_codec_rejects_invalid_without_clamping_and_preserves_exact_values() {
    for gain in [0.0, 0.25, 0.5, 0.75, 1.0] {
        assert_eq!(decode_state(&state_bytes(gain).unwrap()), Some(gain));
    }
    for gain in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
        assert!(state_bytes(gain).is_none());
        let mut bytes = [0u8; 12];
        bytes[..4].copy_from_slice(MAGIC);
        bytes[4..].copy_from_slice(&gain.to_le_bytes());
        assert!(decode_state(&bytes).is_none());
    }
    let mut bad_magic = state_bytes(0.25).unwrap();
    bad_magic[0] = b'?';
    assert!(decode_state(&bad_magic).is_none());
    let effect = Effect::default();
    unsafe {
        assert_eq!(effect.setParamNormalized(0, 0.75), kResultOk);
        assert_eq!(effect.setParamNormalized(0, f64::NAN), kInvalidArgument);
        assert_eq!(effect.setParamNormalized(1000, 1.0), kInvalidArgument);
        assert_eq!(effect.getParamNormalized(0), 0.75);
    }
}

#[test]
fn actual_sdk_realtime_fx_matches_fir_and_rejects_mode_mismatch_without_advance() {
    let realtime = ProcessModes_::kRealtime as i32;
    let offline = ProcessModes_::kOffline as i32;
    for (mode, wrong_mode) in [(realtime, offline), (offline, realtime)] {
        let effect = prepared_mode(mode);
        let mut rejected_input = [vec![0.75; 128], vec![-0.25; 128]];
        unsafe {
            for refused_mode in [wrong_mode, -1, 12_345] {
                let (result, output) =
                    block_mode(&effect, &mut rejected_input, 896, 0.125, refused_mode);
                assert_eq!(result, kResultFalse);
                assert!(output.iter().flatten().all(|&sample| sample == 0.125));
                assert_eq!(effect.getParamNormalized(1000), 0.0);
                assert_eq!(effect.getParamNormalized(1001), 0.0);
            }
        }

        let mut source = [vec![0.0f32; 512], vec![0.0f32; 512]];
        source[0][0] = 2.0;
        source[0][127] = -0.5;
        source[0][255] = 0.125;
        source[1][63] = -1.0;
        source[1][128] = 0.75;
        let mut received = [Vec::new(), Vec::new()];
        for start in (0..512).step_by(128) {
            let mut input = [
                source[0][start..start + 128].to_vec(),
                source[1][start..start + 128].to_vec(),
            ];
            let (result, output) =
                unsafe { block_mode(&effect, &mut input, start as i64, 0.0, mode) };
            assert_eq!(result, kResultOk);
            for (destination, channel) in received.iter_mut().zip(output) {
                destination.extend(channel);
            }
        }
        // Independent direct/echo indexing also detects any history/cursor advance
        // or rejected-input contamination before the first accepted block.
        for (channel, (actual, input)) in received.iter().zip(&source).enumerate() {
            for (frame, &sample) in actual.iter().enumerate() {
                let direct = frame.checked_sub(32).map_or(0.0, |index| input[index]);
                let echo = frame.checked_sub(160).map_or(0.0, |index| input[index]);
                assert_eq!(
                    sample,
                    0.5 * (direct + 0.5 * echo),
                    "mode {mode}, channel {channel}, frame {frame}"
                );
            }
        }
        unsafe {
            assert_eq!(effect.getParamNormalized(1000), 4.0 / PROBE_SCALE);
            assert_eq!(
                effect.getParamNormalized(1001),
                384.0 / SAMPLE_POSITION_SCALE
            );
            assert_eq!(
                effect.getParamNormalized(1005),
                f64::from(u8::from(mode == offline))
            );
            let mut info: ParameterInfo = std::mem::zeroed();
            assert_eq!(effect.getParameterInfo(6, &mut info), kResultOk);
            assert_eq!(info.id, 1005);
            assert_eq!(
                info.flags,
                ParameterInfo_::ParameterFlags_::kIsReadOnly as i32
            );
            assert_eq!(effect.setParamNormalized(1005, 1.0), kInvalidArgument);
        }
    }

    // Matching an unrecognized setup/data mode is still not an admitted process.
    let unknown = prepared_mode(12_345);
    let mut input = [vec![1.0; 128], vec![-1.0; 128]];
    unsafe {
        let (result, output) = block_mode(&unknown, &mut input, 0, 0.25, 12_345);
        assert_eq!(result, kResultFalse);
        assert!(output.iter().flatten().all(|&sample| sample == 0.25));
        assert_eq!(unknown.getParamNormalized(1000), 0.0);
    }
}

#[test]
fn actual_sdk_scaled_readonly_probes_saturate_without_changing_raw_context_or_counters() {
    let effect = prepared();
    for (start, tempo) in [(-2336_i64, 120.0), (128, 120.0), (3_145_728_000, 2048.0)] {
        let mut input = [vec![0.0; 128], vec![0.0; 128]];
        let (result, _) = unsafe {
            block_context(
                &effect,
                &mut input,
                start,
                0.0,
                ProcessModes_::kOffline as i32,
                tempo,
            )
        };
        assert_eq!(result, kResultOk);
        let raw = {
            let engine = effect.engine.lock().unwrap();
            assert_eq!(engine.last_sample, start);
            assert_eq!(
                engine.last_quarter,
                start as f64 / (48_000.0 * 60.0 / tempo)
            );
            assert_eq!(engine.last_tempo, tempo);
            [
                engine.last_sample as f64 / SAMPLE_POSITION_SCALE,
                engine.last_quarter / PROBE_SCALE,
                engine.last_tempo / 1024.0,
            ]
        };
        let actual = unsafe {
            [
                effect.getParamNormalized(1001),
                effect.getParamNormalized(1002),
                effect.getParamNormalized(1003),
            ]
        };
        assert_eq!(actual, raw.map(|value| value.clamp(0.0, 1.0)));
        if start < 0 {
            assert!(raw[0] < 0.0 && raw[1] < 0.0);
            assert_eq!(&actual[..2], &[0.0, 0.0]);
            // These are values from this genuine SDK negative-context case,
            // not recovered literals from a previous App failure log.
            eprintln!(
                "SDK_NEGATIVE_POSITION_PROBES raw1001={}; raw1002={}; normalized1001={}; normalized1002={}; retained_raw_sample={start}",
                raw[0], raw[1], actual[0], actual[1]
            );
        } else if start == 128 {
            assert_eq!(actual, raw, "Existing in-range probe values remain exact");
        } else {
            assert!(raw.iter().all(|&value| value > 1.0));
            assert_eq!(actual, [1.0; 3]);
        }
        for &(id, _) in PROBES {
            let value = unsafe { effect.getParamNormalized(id) };
            assert!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "id={id}: {value}"
            );
        }
    }
    for (index, &(id, _)) in PROBES.iter().enumerate() {
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        unsafe {
            assert_eq!(
                effect.getParameterInfo(index as i32 + 1, &mut info),
                kResultOk
            );
            assert_eq!(info.id, id);
            assert_eq!(
                info.flags,
                ParameterInfo_::ParameterFlags_::kIsReadOnly as i32
            );
            assert_eq!(effect.setParamNormalized(id, 0.25), kInvalidArgument);
        }
    }
    // Seed only these private observation counters for a normalization boundary
    // unit. They are not fabricated Process or state-load acceptance receipts.
    for count in [65_535, 65_536, u32::MAX] {
        {
            let mut engine = effect.engine.lock().unwrap();
            engine.process_calls = count;
            #[cfg(feature = "instrument-first")]
            {
                engine.accepted_state_loads = count;
            }
        }
        let expected = (f64::from(count) / PROBE_SCALE).clamp(0.0, 1.0);
        unsafe {
            assert_eq!(effect.getParamNormalized(1000), expected);
            #[cfg(feature = "instrument-first")]
            assert_eq!(effect.getParamNormalized(1007), expected);
        }
        let engine = effect.engine.lock().unwrap();
        assert_eq!(engine.process_calls, count);
        #[cfg(feature = "instrument-first")]
        assert_eq!(engine.accepted_state_loads, count);
    }
}

#[cfg(feature = "instrument-first")]
#[test]
fn accepted_state_probes_require_actual_sdk_state_load_and_ignore_generic_gain_replay() {
    struct StateStream(Mutex<std::io::Cursor<Vec<u8>>>);
    impl Class for StateStream {
        type Interfaces = (IBStream,);
    }
    impl IBStreamTrait for StateStream {
        unsafe fn read(&self, buffer: *mut c_void, bytes: i32, read: *mut i32) -> tresult {
            if buffer.is_null() || bytes < 0 {
                return kInvalidArgument;
            }
            let Ok(mut cursor) = self.0.lock() else {
                return kResultFalse;
            };
            let output = slice::from_raw_parts_mut(buffer.cast(), bytes as usize);
            let Ok(count) = std::io::Read::read(&mut *cursor, output) else {
                return kResultFalse;
            };
            if !read.is_null() {
                *read = count as i32;
            }
            kResultOk
        }
        unsafe fn write(&self, _: *mut c_void, _: i32, _: *mut i32) -> tresult {
            kNotImplemented
        }
        unsafe fn seek(&self, _: i64, _: i32, _: *mut i64) -> tresult {
            kNotImplemented
        }
        unsafe fn tell(&self, _: *mut i64) -> tresult {
            kNotImplemented
        }
    }

    let stream = |bytes: &[u8]| {
        ComWrapper::new(StateStream(Mutex::new(std::io::Cursor::new(
            bytes.to_vec(),
        ))))
        .to_com_ptr::<IBStream>()
        .unwrap()
    };
    let component = ComWrapper::new(Effect::instrument())
        .to_com_ptr::<IComponent>()
        .unwrap();
    let controller = component.cast::<IEditController>().unwrap();
    unsafe {
        assert_eq!(controller.getParameterCount(), 10);
        for (index, id, title) in [
            (8, 1007, "Accepted State Loads"),
            (9, 1008, "Last Accepted State Gain"),
        ] {
            let mut info: ParameterInfo = std::mem::zeroed();
            assert_eq!(controller.getParameterInfo(index, &mut info), kResultOk);
            assert_eq!(info.id, id);
            assert_eq!(info.defaultNormalizedValue, 0.0);
            assert_eq!(
                info.flags,
                ParameterInfo_::ParameterFlags_::kIsReadOnly as i32
            );
            let text: Vec<_> = info
                .title
                .into_iter()
                .take_while(|unit| *unit != 0)
                .collect();
            assert_eq!(String::from_utf16(&text).unwrap(), title);
        }

        // Matching generic Gain alone must not masquerade as restored opaque state.
        assert_eq!(controller.setParamNormalized(0, 0.625), kResultOk);
        assert_eq!(controller.getParamNormalized(0), 0.625);
        assert_eq!(controller.getParamNormalized(1007), 0.0);
        assert_eq!(controller.getParamNormalized(1008), 0.0);
        let valid = stream(&state_bytes(0.625).unwrap());
        assert_eq!(component.setState(valid.as_ptr()), kResultOk);
        assert_eq!(controller.getParamNormalized(1007), 1.0 / PROBE_SCALE);
        assert_eq!(controller.getParamNormalized(1008), 0.625);

        assert_eq!(controller.setParamNormalized(0, 0.8125), kResultOk);
        assert_eq!(controller.getParamNormalized(0), 0.8125);
        assert_eq!(controller.getParamNormalized(1007), 1.0 / PROBE_SCALE);
        assert_eq!(controller.getParamNormalized(1008), 0.625);

        let mut bad_magic = state_bytes(0.875).unwrap();
        bad_magic[0] = b'?';
        let mut bad_gain = state_bytes(0.875).unwrap();
        bad_gain[4..].copy_from_slice(&f64::NAN.to_le_bytes());
        for bytes in [&bad_magic[..], &bad_gain[..], &bad_magic[..11]] {
            assert_eq!(component.setState(stream(bytes).as_ptr()), kResultFalse);
            assert_eq!(controller.getParamNormalized(0), 0.8125);
            assert_eq!(controller.getParamNormalized(1007), 1.0 / PROBE_SCALE);
            assert_eq!(controller.getParamNormalized(1008), 0.625);
        }
        for id in [1007, 1008] {
            assert_eq!(controller.setParamNormalized(id, 0.25), kInvalidArgument);
        }
        assert_eq!(controller.getParamNormalized(1007), 1.0 / PROBE_SCALE);
        assert_eq!(controller.getParamNormalized(1008), 0.625);

        // Controller component-state delegation reaches the same accepted-load counter.
        let delegated = stream(&state_bytes(0.375).unwrap());
        assert_eq!(controller.setComponentState(delegated.as_ptr()), kResultOk);
        assert_eq!(controller.getParamNormalized(0), 0.375);
        assert_eq!(controller.getParamNormalized(1007), 2.0 / PROBE_SCALE);
        assert_eq!(controller.getParamNormalized(1008), 0.375);
    }
}
