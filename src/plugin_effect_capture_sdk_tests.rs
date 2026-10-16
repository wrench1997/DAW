//! Explicit real-fixture acceptance for the source backend -> strict Effect clone boundary.
#![cfg(all(
    feature = "experimental-fresh-helper",
    any(windows, target_os = "linux")
))]

use super::*;
use crate::instrument_bounce::{BOUNCE_PREPARED_MAXIMUM, BounceAudioBus};
use std::{cell::RefCell, mem::ManuallyDrop, process::Command};
use vst3_host::{ProcessTransport, strict_offline as strict};

const CLASS: &str = "43545253454658440000000000000001";
const NAME: &str = "Citrus Stereo Delay Fixture";
const VENDOR: &str = "Citrus Studio Test Fixtures";
const VERSION: &str = "1.0.0";
const CATEGORY: &str = "Fx|Delay";
const INSTANCE: u64 = 915;
const GAIN: f64 = 0.123_456_789_123_456_78;
const SOURCE_BUDGET: Duration = Duration::from_secs(30);

thread_local! {
    static SOURCE_QUARANTINE: RefCell<Vec<ManuallyDrop<PluginChain>>> = const { RefCell::new(Vec::new()) };
    static STRICT_QUARANTINE: RefCell<Vec<ManuallyDrop<strict::Client>>> = const { RefCell::new(Vec::new()) };
}

/// The original source guard remains owned until its retained poll actually joins.
struct SourceOwner {
    chain: Option<PluginChain>,
    cleanup: Option<RetainedShutdownPoll>,
}

impl SourceOwner {
    fn chain(&self) -> &PluginChain {
        self.chain.as_ref().unwrap()
    }

    fn finish(&mut self) -> RetainedShutdownPoll {
        if let Some(outcome) = self.cleanup {
            return outcome;
        }
        let deadline = Instant::now() + SOURCE_BUDGET;
        let outcome = loop {
            let outcome = self.chain.as_mut().unwrap().guard.poll_shutdown_retained();
            if outcome != RetainedShutdownPoll::Pending || Instant::now() >= deadline {
                break outcome;
            }
            thread::sleep(Duration::from_millis(1));
        };
        self.cleanup = Some(outcome);
        if outcome != RetainedShutdownPoll::Joined {
            SOURCE_QUARANTINE.with(|slot| {
                slot.borrow_mut()
                    .push(ManuallyDrop::new(self.chain.take().unwrap()))
            });
        }
        outcome
    }
}

impl Drop for SourceOwner {
    fn drop(&mut self) {
        let outcome = self.finish();
        if outcome != RetainedShutdownPoll::Joined && !thread::panicking() {
            panic!("real Effect source worker cleanup failed: {outcome:?}; exact owner retained");
        }
    }
}

/// Disposal runs on normal errors and unwind; pending direct-child ownership is retained.
struct StrictOwner {
    client: Option<strict::Client>,
    cleanup: Option<Result<(), String>>,
}

impl StrictOwner {
    fn finish(&mut self) -> Result<(), String> {
        if let Some(result) = &self.cleanup {
            return result.clone();
        }
        let observation = self.client.as_mut().unwrap().dispose(
            Instant::now() + Duration::from_secs(2),
            Instant::now() + Duration::from_secs(6),
        );
        let reaped = matches!(observation.state, strict::CleanupState::Reaped(_));
        let natural_success = matches!(observation.state, strict::CleanupState::Reaped(status) if status.success())
            && observation.shutdown_failure.is_none()
            && !observation.kill_requested
            && observation.kill_error.is_none()
            && observation.wait_error.is_none()
            && observation.pipe_error.is_none();
        let result = if natural_success {
            Ok(())
        } else {
            Err(format!(
                "strict Effect child did not naturally exit and reap: {observation:?}"
            ))
        };
        if !reaped {
            STRICT_QUARANTINE.with(|slot| {
                slot.borrow_mut()
                    .push(ManuallyDrop::new(self.client.take().unwrap()))
            });
        }
        self.cleanup = Some(result.clone());
        result
    }
}

impl Drop for StrictOwner {
    fn drop(&mut self) {
        if self.cleanup.is_some() {
            return;
        }
        let result = self.finish();
        if let Err(error) = result
            && !thread::panicking()
        {
            panic!("{error}");
        }
    }
}

fn wait_for_source(source: &SourceOwner, deadline: Instant) {
    loop {
        let chain = source.chain();
        let metrics = &chain.control.metrics;
        assert_eq!(
            chain.control.stats().faults,
            0,
            "actual source load/prepare/admin failed"
        );
        if chain.control.plugin_endpoint_snapshot().is_some()
            && metrics.worker_settlement.valid.load(Ordering::Acquire)
            && metrics
                .worker_settlement
                .reset_completed_epoch
                .load(Ordering::Acquire)
                == INITIAL_TRANSPORT_EPOCH
            && metrics.worker_settlement.busy.load(Ordering::Acquire) == 0
            && !chain.control.has_outstanding_admin_work()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "actual source worker did not settle"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn checked<T>(stage: &str, result: Result<T, strict::Failure>) -> Result<T, String> {
    result.map_err(|error| format!("{stage}: {error:?}"))
}

fn exercise_clone(helper: &std::path::Path, state: &EffectPluginState) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut command = Command::new(helper);
    let client = match strict::Client::spawn(&mut command, deadline) {
        Ok(client) => client,
        Err(failure) => {
            let primary = format!("strict spawn: {:?}", failure.failure);
            if let Some(client) = failure.cleanup {
                let mut owner = StrictOwner {
                    client: Some(*client),
                    cleanup: None,
                };
                let cleanup = owner.finish();
                return Err(format!("{primary}; spawn cleanup: {cleanup:?}"));
            }
            return Err(primary);
        }
    };
    let mut owner = StrictOwner {
        client: Some(client),
        cleanup: None,
    };
    let result = clone_body(owner.client.as_mut().unwrap(), deadline, state);
    let cleanup = owner.finish();
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(format!("{primary}; {cleanup}")),
    }
}

fn clone_body(
    client: &mut strict::Client,
    deadline: Instant,
    state: &EffectPluginState,
) -> Result<(), String> {
    let source = &state.metadata;
    let identity = strict::EffectIdentity {
        class_id: source.identity.class_uid.clone(),
        vendor: source.identity.vendor.clone(),
        name: source.identity.name.clone(),
        version: source.identity.version.clone(),
        category: source.identity.category.clone(),
    };
    let path = source.path.to_str().ok_or("fixture path is not UTF-8")?;
    let loaded = checked(
        "load captured actual Effect identity",
        client.load_stereo_effect(
            strict::LoadSpec {
                path,
                sample_rate: source.sample_rate,
                tempo: source.tempo,
                time_sig_numerator: source.time_sig_numerator,
                time_sig_denominator: source.time_sig_denominator,
                class_id: &source.identity.class_uid,
            },
            &identity,
            deadline,
        ),
    )?;
    if loaded.has_gui || loaded.has_midi_input || loaded.has_midi_output {
        return Err(format!("unexpected real fixture capabilities: {loaded:?}"));
    }
    checked("set Offline", client.set_offline(deadline))?;
    checked(
        "load actual captured opaque state",
        client.load_state(&state.opaque_state, deadline),
    )?;
    if checked("opaque gain readback", client.parameter(0, deadline))?.to_bits() != GAIN.to_bits() {
        return Err("actual source opaque state did not restore Gain".into());
    }
    // Move only the disposable clone away from the opaque value, so a no-op or
    // rounded base setter cannot pass by inheriting the source state's exact gain.
    checked(
        "perturb clone before base restoration",
        client.set_parameter(0, 0.5, deadline),
    )?;
    if checked("perturbed clone readback", client.parameter(0, deadline))?.to_bits()
        != 0.5_f64.to_bits()
    {
        return Err("disposable clone did not leave its opaque-state gain".into());
    }
    for (&id, &base) in &state.parameter_bases {
        checked(
            "restore exact f64 captured base",
            client.set_parameter_f64(id, base, deadline),
        )?;
        let actual = checked("captured base readback", client.parameter(id, deadline))?;
        if actual.to_bits() != base.to_bits() {
            return Err(format!("captured base {id}: actual {actual} != {base}"));
        }
    }
    let layout = checked(
        "complete clone audio layout",
        client.checked_audio_layout(deadline),
    )?;
    checked(
        "select captured main stereo buses",
        client.select_main_stereo_output(&layout, deadline),
    )?;
    let clone = checked("admit stopped clone", client.admit_stereo_effect(deadline))?;
    let inputs: Vec<_> = clone
        .audio
        .inputs
        .iter()
        .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
        .collect();
    let outputs: Vec<_> = clone
        .audio
        .outputs
        .iter()
        .map(|bus| BounceAudioBus::new(bus.channel_count, bus.is_main, bus.active))
        .collect();
    let event_inputs: Vec<_> = clone
        .events
        .inputs
        .iter()
        .map(|bus| (bus.channel_count, bus.is_main, bus.active))
        .collect();
    let event_outputs: Vec<_> = clone
        .events
        .outputs
        .iter()
        .map(|bus| (bus.channel_count, bus.is_main, bus.active))
        .collect();
    if clone.identity != identity
        || clone.path != path
        || clone.sample_rate != source.sample_rate
        || clone.max_block_frames as usize != source.max_block_frames
        || clone.tempo != source.tempo
        || clone.time_sig_numerator != source.time_sig_numerator
        || clone.time_sig_denominator != source.time_sig_denominator
        || clone.state_bytes != state.opaque_state.len()
        || inputs != source.input_buses
        || outputs != source.output_buses
        || event_inputs != source.event_input_buses
        || event_outputs != source.event_output_buses
        || clone.latency_samples != source.latency_samples
        || clone.tail_samples != source.tail_samples
    {
        return Err(format!(
            "actual clone metadata differs from private source: clone={clone:?}; source={source:?}"
        ));
    }
    if checked("pre-DSP clone count", client.parameter(1000, deadline))? != 0.0 {
        return Err("clone processed positive frames before explicit DSP".into());
    }
    checked("start admitted clone", client.start_processing(deadline))?;
    for block in 0..3 {
        let mut pcm = [vec![0.0f32; 128], vec![0.0f32; 128]];
        if block == 0 {
            pcm[0][0] = 2.0;
            pcm[1][0] = -1.0;
        }
        let start = block * 128;
        let transport = ProcessTransport {
            sample_position: start as i64,
            quarter_note_position: start as f64 / source.sample_rate * source.tempo / 60.0,
            tempo: source.tempo,
            playing: false,
            time_sig_numerator: source.time_sig_numerator,
            time_sig_denominator: source.time_sig_denominator,
        };
        let audio = checked(
            "actual stereo PCM/drain",
            client.process_stereo_effect(&pcm, transport, deadline),
        )?;
        if !audio.transport_applied
            || audio.output_events_lost
            || audio.output_event_count != 0
            || audio.outputs.len() != 2
            || audio.outputs.iter().any(|channel| channel.len() != 128)
        {
            return Err(format!(
                "invalid real PCM reply for block {block}: {audio:?}"
            ));
        }
        for (channel, samples) in audio.outputs.iter().enumerate() {
            let input = if channel == 0 { 2.0f32 } else { -1.0f32 };
            for (frame, &actual) in samples.iter().enumerate() {
                let taps = if frame == 32 && block < 2 {
                    if block == 0 { input } else { 0.5 * input }
                } else {
                    0.0
                };
                // The fixture retains state/base at f64 precision, then casts gain once
                // at its f32 DSP boundary before multiplying the independent two-tap sum.
                let expected = GAIN as f32 * taps;
                if actual != expected {
                    return Err(format!(
                        "real PCM channel {channel} block {block} frame {frame}: {actual} != {expected}"
                    ));
                }
            }
        }
    }
    if checked(
        "actual clone process-count probe",
        client.parameter(1000, deadline),
    )? != 3.0 / 65_536.0
    {
        return Err("real clone did not process exactly three positive Q128 blocks".into());
    }
    if checked(
        "actual clone active stereo Offline probe",
        client.parameter(1005, deadline),
    )? != 1.0
    {
        return Err("real strict clone was not active stereo Offline".into());
    }
    checked("stop clone", client.stop_processing(deadline))?;
    Ok(())
}

#[test]
#[ignore = "requires explicit source-built CITRUS_EFFECT_FIXTURE and matching production CITRUS_EFFECT_HELPER; no installed fallback"]
fn actual_sdk_private_effect_source_capture_restores_strict_clone_pcm_and_naturally_reaps() {
    let _transport_lock = crate::plugins::helper_transport_test_lock();
    assert_ne!(
        f64::from(GAIN as f32).to_bits(),
        GAIN.to_bits(),
        "the source/base fixture must exercise precision that the legacy f32 setter loses"
    );
    let fixture = PathBuf::from(
        std::env::var_os("CITRUS_EFFECT_FIXTURE").expect("set CITRUS_EFFECT_FIXTURE explicitly"),
    )
    .canonicalize()
    .expect("source-built Effect fixture exists");
    let expected = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/vst3-effect/target/bundles/CitrusStereoDelayFixture.vst3")
        .canonicalize()
        .expect("repository fixture bundle exists");
    assert_eq!(
        fixture, expected,
        "only this repository's source-built fixture is admitted"
    );
    let helper = PathBuf::from(
        std::env::var_os("CITRUS_EFFECT_HELPER")
            .expect("set matching CITRUS_EFFECT_HELPER explicitly"),
    )
    .canonicalize()
    .expect("explicit production helper exists");
    assert!(helper.is_file());
    let mut spec = PluginLoadSpec::from_descriptor(PluginDescriptor {
        id: "actual-private-effect-fixture".into(),
        name: NAME.into(),
        vendor: VENDOR.into(),
        path: fixture.clone(),
        format: PluginFormat::Vst3,
        category: CATEGORY.into(),
        is_instrument: false,
        verified: true,
        vst3_metadata: None,
        vst3_catalog: None,
        scan_error: None,
    });
    spec.class_uid = Some(CLASS.into());
    spec.vst3_helper_path = Some(helper.clone());
    let config = PluginPrepareConfig {
        sample_rate: 48_000.0,
        max_block_frames: BOUNCE_PREPARED_MAXIMUM,
    };
    let chain = PluginChain::spawn_identified_with_backend_factory(
        &[INSTANCE],
        move || {
            let mut backend = Vst3Backend::load(spec, config)
                .expect("real source Vst3Backend loaded on its owning worker");
            // Configure the actual source through its existing f64 plugin interface before
            // handing it to the normal worker prepare/capture path. The f32 trait/UI setter
            // cannot supply this source value and must not be used as its authority.
            backend
                .plugin
                .set_parameter(0, GAIN)
                .expect("real source accepts the non-f32-representable f64 Gain");
            assert_eq!(
                backend.plugin.get_parameter(0).unwrap().to_bits(),
                GAIN.to_bits(),
                "actual source f64 setter/readback must preserve all gain bits"
            );
            vec![BackendSlot::new(Box::new(backend))]
        },
        config,
    )
    .expect("real identified Vst3Backend worker spawned");
    let mut source = SourceOwner {
        chain: Some(chain),
        cleanup: None,
    };
    let deadline = Instant::now() + SOURCE_BUDGET;
    // Keep the real initial epoch: an explicit discontinuity has its own positive-frame reset.
    // The worker itself publishes initial reset settlement after actual load/prepare completes.
    wait_for_source(&source, deadline);
    let endpoint = source.chain().control.plugin_endpoint_snapshot().unwrap();
    assert_eq!(
        (endpoint.slot_count(), endpoint.slot(0).unwrap().instance_id),
        (1, INSTANCE)
    );
    assert_eq!(
        (
            endpoint.slot(0).unwrap().latency_samples,
            endpoint.tail_samples()
        ),
        (32, 160)
    );
    let request = EffectCaptureSlotRequest {
        instance_id: INSTANCE,
        config: SlotConfig::default(),
        path: fixture.clone(),
        requested_class_uid: Some(CLASS.into()),
        parameter_ids: vec![0],
    };
    let receiver = source
        .chain()
        .control
        .request_effect_chain_state(
            INITIAL_TRANSPORT_EPOCH,
            endpoint,
            vec![request.clone()],
            deadline,
        )
        .expect("actual stopped source capture admitted");
    let receipt = loop {
        match receiver.try_recv() {
            Ok(result) => break result.expect("actual Vst3Backend private capture succeeds"),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("actual source private reply lost"),
        }
        assert!(
            Instant::now() < deadline,
            "actual source capture did not terminate"
        );
        thread::sleep(Duration::from_millis(1));
    };
    assert!(receiver.termination_acknowledged());
    while !source
        .chain()
        .control
        .effect_chain_receipt_current(&receipt)
    {
        assert!(
            Instant::now() < deadline,
            "actual source receipt did not become current"
        );
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(receipt.epoch, INITIAL_TRANSPORT_EPOCH);
    assert_eq!(receipt.slots.len(), 1);
    assert_eq!(receipt.slots[0].request, request);
    let state = &receipt.slots[0].state;
    state.validate(&[0]).unwrap();
    let metadata = &state.metadata;
    assert_eq!(
        (
            &*metadata.identity.class_uid,
            &*metadata.identity.name,
            &*metadata.identity.vendor,
            &*metadata.identity.version,
            &*metadata.identity.category
        ),
        (CLASS, NAME, VENDOR, VERSION, CATEGORY)
    );
    assert_eq!(
        (&metadata.path, metadata.requested_class_uid.as_deref()),
        (&fixture, Some(CLASS))
    );
    assert_eq!(
        (
            metadata.sample_rate,
            metadata.max_block_frames,
            metadata.tempo,
            metadata.time_sig_numerator,
            metadata.time_sig_denominator
        ),
        (48_000.0, 2048, 128.0, 4, 4)
    );
    assert_eq!(
        metadata.input_buses,
        vec![BounceAudioBus::new(2, true, Some(true))]
    );
    assert_eq!(
        metadata.output_buses,
        vec![BounceAudioBus::new(2, true, Some(true))]
    );
    assert!(metadata.event_input_buses.is_empty() && metadata.event_output_buses.is_empty());
    assert_eq!(
        (
            metadata.latency_samples,
            metadata.tail_samples,
            state.opaque_state.len()
        ),
        (32, 160, 40)
    );
    assert_eq!(
        state
            .catalog
            .iter()
            .map(|parameter| parameter.id)
            .collect::<Vec<_>>(),
        vec![0, 1000, 1001, 1002, 1003, 1004, 1005, 1006]
    );
    assert_eq!(state.parameter_bases.len(), 1);
    assert_eq!(state.parameter_bases[&0].to_bits(), GAIN.to_bits());
    assert!(
        state
            .catalog
            .iter()
            .filter(|parameter| parameter.id != 0)
            .all(|parameter| parameter.read_only)
    );
    assert_eq!(
        state
            .catalog
            .iter()
            .find(|parameter| parameter.id == 1000)
            .unwrap()
            .current_normalized,
        0.0,
        "actual SDK source count: private capture must not process positive frames"
    );
    let native = source.chain().control.native_editor_snapshot(0).unwrap();
    assert_eq!(
        (native.capture_serial, native.parameter_capture_serial),
        (0, 0)
    );
    assert!(native.captured_state.is_none() && native.captured_parameters.is_none());
    assert_eq!(
        source.finish(),
        RetainedShutdownPoll::Joined,
        "original source worker must join before the strict clone"
    );
    assert!(source.chain().guard.join.is_none());
    assert_eq!(source.chain().control.stats().detached_workers, 0);
    drop(receiver);
    drop(source);
    exercise_clone(&helper, state).unwrap();
    eprintln!(
        "REAL_PRIVATE_EFFECT_CAPTURE_PASS source_sdk_process_calls=0; source_original_worker=joined; opaque_state_bytes=40; restored_gain={GAIN:.17}; restored_gain_bits={:016x}; non_f32_source_base=true; clone_q128_blocks=3; latency=32; last_tap=160; strict_child=natural_exit_0_and_reaped; offline_probe1005=1",
        GAIN.to_bits()
    );
}
