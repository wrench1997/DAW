use citrus_vst3_effect_fixture::{CATEGORY, CLASS_ID, LAST_TAP, LATENCY, NAME, VENDOR, VERSION};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use vst3_host::strict_offline::{
    CleanupObservation, CleanupState, Client, EffectIdentity, LoadSpec,
};
use vst3_host::{ProcessMode, ProcessTransport, Vst3Host};

fn checked_cleanup(observation: CleanupObservation) -> Result<(), String> {
    let reaped_success =
        matches!(observation.state, CleanupState::Reaped(status) if status.success());
    if !reaped_success
        || observation.shutdown_failure.is_some()
        || observation.kill_requested
        || observation.kill_error.is_some()
        || observation.wait_error.is_some()
        || observation.pipe_error.is_some()
    {
        return Err(format!(
            "helper did not naturally exit and reap: {observation:?}"
        ));
    }
    Ok(())
}

fn with_child<F>(helper: &PathBuf, body: F) -> Result<(), String>
where
    F: FnOnce(&mut Client, Instant) -> Result<(), String>,
{
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut command = Command::new(helper);
    let mut client = match Client::spawn(&mut command, deadline) {
        Ok(client) => client,
        Err(mut failure) => {
            if let Some(owner) = failure.cleanup.as_mut() {
                let observation = owner.dispose(
                    Instant::now() + Duration::from_secs(2),
                    Instant::now() + Duration::from_secs(6),
                );
                return Err(format!(
                    "spawn failed: {:?}; cleanup: {observation:?}",
                    failure.failure
                ));
            }
            return Err(format!(
                "spawn failed before child ownership: {:?}",
                failure.failure
            ));
        }
    };
    let result = body(&mut client, deadline);
    let cleanup = checked_cleanup(client.dispose(
        Instant::now() + Duration::from_secs(2),
        Instant::now() + Duration::from_secs(6),
    ));
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(format!("{primary}; cleanup: {cleanup}")),
    }
}

#[test]
#[ignore = "requires explicitly source-built CITRUS_EFFECT_FIXTURE and matching CITRUS_EFFECT_HELPER; run the documented exact target"]
fn real_sdk_effect_pcm_state_transport_delay_tail_and_natural_reap() {
    let fixture = PathBuf::from(
        std::env::var_os("CITRUS_EFFECT_FIXTURE").expect("set CITRUS_EFFECT_FIXTURE explicitly"),
    )
    .canonicalize()
    .expect("source-built fixture bundle exists");
    let expected = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/bundles/CitrusStereoDelayFixture.vst3")
        .canonicalize()
        .unwrap();
    assert_eq!(
        fixture, expected,
        "only this source-built fixture is accepted; no installed-plugin fallback"
    );
    let helper = PathBuf::from(
        std::env::var_os("CITRUS_EFFECT_HELPER").expect("set CITRUS_EFFECT_HELPER explicitly"),
    )
    .canonicalize()
    .expect("matching production helper exists");
    assert!(helper.is_file());
    let fixture_text = fixture.to_str().unwrap();
    let identity = EffectIdentity {
        class_id: CLASS_ID.into(),
        vendor: VENDOR.into(),
        name: NAME.into(),
        version: VERSION.into(),
        category: CATEGORY.into(),
    };

    // Capture actual IComponent state through the existing public SDK owner;
    // neither a fabricated helper reply nor a new renderer supplies these bytes.
    let saved = {
        let mut host = Vst3Host::builder()
            .sample_rate(48_000.0)
            .block_size(2048)
            .input_channels(2)
            .tempo(120.0)
            .time_signature(4, 4)
            .with_process_isolation(false)
            .build()
            .unwrap();
        let mut source = host
            .load_main_thread_plugin_class(&fixture, CLASS_ID)
            .unwrap();
        assert_eq!(source.info().name, NAME);
        assert_eq!(source.info().category, CATEGORY);
        source.set_process_mode(ProcessMode::Offline).unwrap();
        source.set_parameter(0, 0.25).unwrap();
        assert_eq!(source.get_parameter(0).unwrap(), 0.25);
        let saved = source.save_state().unwrap();
        assert!(!saved.is_empty());
        saved
    };
    // Cold source instance: actual load/readback/save must restore the same blob.
    {
        let mut host = Vst3Host::builder()
            .sample_rate(48_000.0)
            .block_size(2048)
            .with_process_isolation(false)
            .build()
            .unwrap();
        let mut cold = host
            .load_main_thread_plugin_class(&fixture, CLASS_ID)
            .unwrap();
        assert_eq!(cold.get_parameter(0).unwrap(), 0.5);
        cold.load_state(&saved).unwrap();
        assert_eq!(cold.get_parameter(0).unwrap(), 0.25);
        assert_eq!(cold.save_state().unwrap(), saved);
    }

    with_child(&helper, |client, deadline| {
        let captured_base = 0.75000004_f64;
        if f64::from(captured_base as f32).to_bits() == captured_base.to_bits() {
            return Err("fixture base must exercise genuine f64 restoration".into());
        }
        let loaded = client.load_stereo_effect(LoadSpec { path: fixture_text, sample_rate: 48_000.0, tempo: 120.0,
            time_sig_numerator: 4, time_sig_denominator: 4, class_id: CLASS_ID }, &identity, deadline).map_err(|e| format!("Load: {e:?}"))?;
        if loaded.has_gui || loaded.has_midi_input || loaded.has_midi_output { return Err(format!("wrong real fixture capabilities: {loaded:?}")); }
        client.set_offline(deadline).map_err(|e| format!("Offline: {e:?}"))?;
        client.load_state(&saved, deadline).map_err(|e| format!("State: {e:?}"))?;
        if client.parameter(0, deadline).map_err(|e| format!("state readback: {e:?}"))? != 0.25 { return Err("opaque state gain not restored".into()); }
        // Captured bases override opaque state before admission and actual DSP.
        client.set_parameter_f64(0, captured_base, deadline).map_err(|e| format!("base: {e:?}"))?;
        if client.parameter(0, deadline).map_err(|e| format!("base readback: {e:?}"))?.to_bits() != captured_base.to_bits() { return Err("captured f64 gain base not restored exactly".into()); }
        let layout = client.checked_audio_layout(deadline).map_err(|e| format!("layout: {e:?}"))?;
        client.select_main_stereo_output(&layout, deadline).map_err(|e| format!("selection: {e:?}"))?;
        let receipt = client.admit_stereo_effect(deadline).map_err(|e| format!("admission: {e:?}"))?;
        if receipt.identity != identity || receipt.state_bytes != saved.len() || receipt.max_block_frames != 2048
            || receipt.latency_samples != LATENCY as u32 || receipt.tail_samples != LAST_TAP as u32
            || !receipt.events.inputs.is_empty() || !receipt.events.outputs.is_empty()
        { return Err(format!("wrong actual SDK receipt: {receipt:?}")); }
        client.start_processing(deadline).map_err(|e| format!("start: {e:?}"))?;
        let mut input = [vec![0.0f32; 640], vec![0.0f32; 640]];
        input[0][0] = 2.0;
        input[0][127] = -0.5;
        input[0][255] = 1.25;
        input[1][63] = -0.75;
        input[1][128] = 0.5;
        input[1][255] = -1.5;
        let mut observed = [Vec::new(), Vec::new()];
        for start in (0..640).step_by(128) {
            let pcm = [input[0][start..start + 128].to_vec(), input[1][start..start + 128].to_vec()];
            let quarter = 8.0 + start as f64 / 48_000.0 * 2.0;
            let transport = ProcessTransport { sample_position: 192_000 + start as i64, quarter_note_position: quarter,
                tempo: 120.0, playing: start < 256, time_sig_numerator: 4, time_sig_denominator: 4 };
            let result = client.process_stereo_effect(&pcm, transport, deadline).map_err(|e| format!("Process {start}: {e:?}"))?;
            if !result.transport_applied || result.output_event_count != 0 || result.output_events_lost { return Err(format!("bad actual Process flags at {start}")); }
            for channel in 0..2 { observed[channel].extend_from_slice(&result.outputs[channel]); }
            let expected_probes = [(1000, (start / 128 + 1) as f64 / 65_536.0),
                (1001, (192_000 + start) as f64 / 16_777_216.0), (1002, quarter / 65_536.0),
                (1003, 120.0 / 1024.0), (1004, if start < 256 { 1.0 } else { 0.0 }), (1005, 1.0), (1006, 1.0)];
            for (id, expected) in expected_probes {
                let actual = client.parameter(id, deadline).map_err(|e| format!("probe {id}: {e:?}"))?;
                if actual != expected { return Err(format!("actual SDK probe {id}: {actual} != {expected}, block {start}")); }
            }
        }
        // This fixture intentionally renders f32 audio with a f32 gain. Its
        // controller nevertheless retains the exact captured f64 above. Model
        // that actual DSP conversion, not an early transport-side truncation.
        let dsp_gain = captured_base as f32;
        for channel in 0..2 {
            for frame in 0usize..640 {
                let direct = frame.checked_sub(32).map_or(0.0, |i| input[channel][i]);
                let echo = frame.checked_sub(160).map_or(0.0, |i| input[channel][i]);
                let expected = dsp_gain * (direct + 0.5 * echo);
                if observed[channel][frame] != expected { return Err(format!("actual PCM differs channel {channel} frame {frame}: {} != {expected}", observed[channel][frame])); }
            }
        }
        if observed[0][32] != dsp_gain * 2.0 || observed[1][287] != dsp_gain * -1.5
            || observed.iter().any(|channel| channel[416..].iter().any(|value| *value != 0.0))
        { return Err("unclamped PCM or finite tail end differs".into()); }
        client.stop_processing(deadline).map_err(|e| format!("stop: {e:?}"))?;
        eprintln!("REAL_SDK_EFFECT_PASS metadata={receipt:?}; exact_stereo_samples=1280; cross_q128_delay=32; last_tap=160; source_last_frame=255; final_nonzero_frame=415; silence_after=416; transport_probes=35; opaque_state_bytes={}; captured_base_f64_bits={}; dsp_gain_f32_bits={}", saved.len(), captured_base.to_bits(), dsp_gain.to_bits());
        Ok(())
    }).unwrap();
}

#[test]
#[ignore = "requires explicit source-built fixture and matching production helper; no GUI/device or installed fallback"]
fn real_sdk_instrument_midi_has_actual_q128_lookahead_tail_and_natural_reap() {
    use citrus_vst3_effect_fixture::{INSTRUMENT_CATEGORY, INSTRUMENT_CLASS_ID, INSTRUMENT_NAME};
    use vst3_host::{MidiChannel, MidiEvent};
    let fixture =
        PathBuf::from(std::env::var_os("CITRUS_EFFECT_FIXTURE").expect("explicit fixture"))
            .canonicalize()
            .unwrap();
    let expected = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/bundles/CitrusStereoDelayFixture.vst3")
        .canonicalize()
        .unwrap();
    assert_eq!(
        fixture, expected,
        "only the repository source-built bundle is admitted"
    );
    let helper =
        PathBuf::from(std::env::var_os("CITRUS_EFFECT_HELPER").expect("explicit matching helper"))
            .canonicalize()
            .unwrap();
    assert!(helper.is_file());
    let saved = {
        let mut host = Vst3Host::builder()
            .sample_rate(48_000.0)
            .block_size(2048)
            .tempo(120.0)
            .time_signature(4, 4)
            .with_process_isolation(false)
            .build()
            .unwrap();
        let mut source = host
            .load_main_thread_plugin_class(&fixture, INSTRUMENT_CLASS_ID)
            .unwrap();
        assert_eq!(source.info().uid, INSTRUMENT_CLASS_ID);
        assert_eq!(source.info().name, INSTRUMENT_NAME);
        assert_eq!(source.info().category, INSTRUMENT_CATEGORY);
        source.set_process_mode(ProcessMode::Offline).unwrap();
        assert_eq!(source.get_parameter(0).unwrap(), 0.5);
        assert_eq!(source.get_parameter(1000).unwrap(), 0.0);
        source.save_state().unwrap()
    };
    assert_eq!(saved.len(), 40);
    with_child(&helper,|client,deadline| {
        let loaded = client.load_plugin(LoadSpec { path:fixture.to_str().unwrap(),sample_rate:48_000.0,
            tempo:120.0,time_sig_numerator:4,time_sig_denominator:4,class_id:INSTRUMENT_CLASS_ID },deadline)
            .map_err(|e|format!("Instrument Load: {e:?}"))?;
        if loaded.uid != INSTRUMENT_CLASS_ID || loaded.name != INSTRUMENT_NAME
            || loaded.category != INSTRUMENT_CATEGORY || !loaded.has_midi_input
            || loaded.has_midi_output || loaded.has_gui || loaded.audio_inputs != 0
        { return Err(format!("wrong actual Instrument class: {loaded:?}")); }
        client.set_offline(deadline).map_err(|e|format!("Offline: {e:?}"))?;
        client.load_state(&saved,deadline).map_err(|e|format!("opaque restore: {e:?}"))?;
        if client.parameter(0,deadline).map_err(|e|format!("gain: {e:?}"))? != 0.5
            || client.latency_samples(deadline).map_err(|e|format!("latency: {e:?}"))? != 32
            || client.tail_samples(deadline).map_err(|e|format!("tail: {e:?}"))? != 32
        { return Err("actual gain/latency/tail differs".into()); }
        let audio = client.checked_audio_layout(deadline).map_err(|e|format!("audio: {e:?}"))?;
        let events = client.event_layout(deadline).map_err(|e|format!("events: {e:?}"))?;
        if !audio.inputs.is_empty() || audio.outputs.len() != 1
            || events.inputs.len() != 1 || !events.outputs.is_empty()
            || events.inputs[0].channel_count != 16 || !events.inputs[0].is_main
        { return Err(format!("actual layouts differ: {audio:?}/{events:?}")); }
        client.select_main_stereo_output(&audio,deadline).map_err(|e|format!("select: {e:?}"))?;
        client.start_processing(deadline).map_err(|e|format!("Start: {e:?}"))?;
        for block in 0..3 {
            let changes: &[(i32,bool)] = match block {
                0 => &[(0,true),(1,false),(127,true)],
                1 => &[(1,false)], _ => &[],
            };
            for &(offset,on) in changes {
                let event = if on { MidiEvent::NoteOn {channel:MidiChannel::Ch1,note:63,velocity:127} }
                    else { MidiEvent::NoteOff {channel:MidiChannel::Ch1,note:63,velocity:0} };
                client.send_note(event,offset,deadline).map_err(|e|format!("MIDI: {e:?}"))?;
            }
            let start = block * 128;
            let transport = ProcessTransport { sample_position:start as i64,
                quarter_note_position:start as f64 / 24_000.0,tempo:120.0,playing:false,
                time_sig_numerator:4,time_sig_denominator:4 };
            let audio = client.process_silent_generator(0,2,transport,deadline)
                .map_err(|e|format!("actual MIDI DSP: {e:?}"))?;
            if !audio.transport_applied || audio.output_event_count != 0 || audio.output_events_lost
                || audio.outputs.len() != 2 || audio.outputs.iter().any(|c|c.len()!=128)
            { return Err(format!("actual block refused: {audio:?}")); }
            for (channel, samples) in audio.outputs.iter().enumerate() {
                for (offset,&actual) in samples.iter().enumerate() {
                    let raw = start + offset;
                    let expected = if matches!(raw,32|159|160) {
                        if channel==0 {0.25} else {-0.125}
                    } else {0.0};
                    if actual != expected {return Err(format!("channel{channel} rawframe{raw}: {actual} != {expected}"));}
                }
            }
        }
        if client.parameter(1000,deadline).map_err(|e|format!("count: {e:?}"))? != 3.0 / 65_536.0
            || client.parameter(1001,deadline).map_err(|e|format!("position: {e:?}"))? != 256.0/16_777_216.0
            || client.parameter(1005,deadline).map_err(|e|format!("mode: {e:?}"))? != 1.0
            || client.parameter(1006,deadline).map_err(|e|format!("clear: {e:?}"))? != 1.0
        {return Err("actual DSP probes differ".into());}
        client.stop_processing(deadline).map_err(|e|format!("Stop: {e:?}"))?;
        eprintln!("REAL_SDK_LOOKAHEAD_INSTRUMENT_PASS class={INSTRUMENT_CLASS_ID}; state_bytes=40; latency=32; tail=32; actual_nonzero_frames=32,159,160; exact_stereo_samples=768; calls=3; natural_cleanup_required=true");
        Ok(())
    }).unwrap();
}
