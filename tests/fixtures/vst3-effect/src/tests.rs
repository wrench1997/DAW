use super::*;

fn prepared() -> Effect {
    let effect = Effect::default();
    unsafe {
        let mut setup = ProcessSetup {
            processMode: ProcessModes_::kOffline as i32,
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
    context.projectTimeMusic = start as f64 / 24_000.0;
    context.tempo = 120.0;
    context.timeSigNumerator = 4;
    context.timeSigDenominator = 4;
    let mut data: ProcessData = std::mem::zeroed();
    data.processMode = ProcessModes_::kOffline as i32;
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
        assert_eq!(factory.getClassInfo2(0, &mut info), kResultOk);
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
        assert_eq!(factory.getClassInfo2(1, &mut info), kResultOk);
        assert_eq!(info.cid, INSTRUMENT_CID);
        assert_eq!(text(&info.name), INSTRUMENT_NAME);
        assert_eq!(text(&info.subCategories), INSTRUMENT_CATEGORY);
        assert_eq!(factory.getClassInfo2(2, &mut info), kInvalidArgument);
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
