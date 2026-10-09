//! DAW reset transactions through the real VST3 backend and a command-recording helper.
use super::*;
use std::os::unix::fs::PermissionsExt;

struct ResetHelper {
    root: PathBuf,
    script: PathBuf,
    log: PathBuf,
    mode: PathBuf,
    identity: PathBuf,
    maximum: PathBuf,
}

impl ResetHelper {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "citrus-reset-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("helper.py");
        let log = root.join("commands.jsonl");
        let mode = root.join("mode");
        std::fs::write(&mode, "ok").unwrap();
        let identity = root.join("identity.json");
        std::fs::write(&identity, r#"["00000000000000000000000000000001","1"]"#).unwrap();
        let maximum = root.join("maximum");
        std::fs::write(&maximum, "2048").unwrap();
        let source = format!(
            r#"#!/usr/bin/python3
import json,sys,pathlib
log=pathlib.Path({log:?})
mode=pathlib.Path({mode:?})
identity=pathlib.Path({identity:?})
maximum=pathlib.Path({maximum:?})
for line in sys.stdin:
    command=json.loads(line)
    with log.open('a') as f: f.write(json.dumps(command)+'\n')
    kind=command if isinstance(command,str) else next(iter(command))
    selected=mode.read_text()
    if kind=='LoadPlugin':
        uid,version=json.loads(identity.read_text())
        response={{'PluginInfo':{{'vendor':'Citrus tests','name':'Reset probe','version':version,'category':'Instrument','uid':uid,'has_gui':False,'audio_inputs':0,'audio_outputs':1,'output_channels':2,'has_midi_input':True,'has_midi_output':False}}}}
    elif kind=='ResetOriginSupport':
        response=({{'Error':{{'message':'old helper unsupported'}}}} if selected=='old' else {{'ResetOriginSupport':{{'support':{{'contract_version':1,'max_block_frames':int(maximum.read_text()),'processing':True}}}}}})
    elif selected==kind:
        response={{'Error':{{'message':'deliberate '+kind+' refusal'}}}}
    elif kind=='ProcessResetOrigin':
        response={{'ResetOriginReport':{{'report':{{'frames':command[kind]['frames'],'discarded_prior_midi_events':0,'discarded_midi_events':2,'discarded_prior_parameter_points':0,'discarded_parameter_points':3,'output_events_lost':selected=='loss','parameter_output_fault':False,'process_error':'SDK refusal' if selected=='sdk' else None}}}}}}
    elif kind=='NativeDirtyRevision': response={{'NativeDirtyRevision':{{'revision':0}}}}
    else: response={{'Success':{{'message':'ok'}}}}
    print(json.dumps(response),flush=True)
"#,
            log = log.to_string_lossy(),
            mode = mode.to_string_lossy(),
            identity = identity.to_string_lossy(),
            maximum = maximum.to_string_lossy()
        );
        std::fs::write(&script, source).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            root,
            script,
            log,
            mode,
            identity,
            maximum,
        }
    }
    fn set_identity(&self, uid: &str, version: &str) {
        std::fs::write(&self.identity, serde_json::to_vec(&(uid, version)).unwrap()).unwrap();
    }
    fn set_maximum(&self, frames: usize) {
        std::fs::write(&self.maximum, frames.to_string()).unwrap();
    }
    fn spec(&self) -> PluginLoadSpec {
        let path = self.root.join("fixture.vst3");
        std::fs::write(&path, b"fake helper owns this test instance").unwrap();
        let mut spec = PluginLoadSpec::from_descriptor(PluginDescriptor {
            id: "reset-test".into(),
            name: "Reset probe".into(),
            vendor: "Citrus tests".into(),
            path,
            format: PluginFormat::Vst3,
            category: "Instrument".into(),
            is_instrument: true,
            verified: false,
            vst3_metadata: None,
            scan_error: None,
        });
        spec.vst3_helper_path = Some(self.script.clone());
        spec.initial_state = vec![1, 2, 3];
        spec
    }
    fn select(&self, value: &str) {
        std::fs::write(&self.mode, value).unwrap();
    }
    fn clear(&self) {
        std::fs::write(&self.log, "").unwrap();
    }
    fn commands(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn backend(&self) -> Vst3Backend {
        let config = reset_config();
        let mut backend = Vst3Backend::load(self.spec(), config).unwrap();
        backend.prepare(config).unwrap();
        backend
    }
}
impl Drop for ResetHelper {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn reset_config() -> PluginPrepareConfig {
    PluginPrepareConfig {
        sample_rate: 48_000.0,
        max_block_frames: 2048,
    }
}
fn kind(value: &serde_json::Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| value.as_object().unwrap().keys().next().unwrap())
}

#[test]
fn daw_reset_load_and_prepare_reject_low_maximum_before_helper_or_state_mutation() {
    let helper = ResetHelper::new();
    for maximum in [1, 17, 47, 64, 127] {
        let mut spec = helper.spec();
        spec.vst3_helper_path = Some(helper.root.join("missing-helper"));
        let error = Vst3Backend::load(
            spec,
            PluginPrepareConfig {
                max_block_frames: maximum,
                ..reset_config()
            },
        )
        .err()
        .unwrap();
        assert!(error.contains("at least 128"));
    }
    assert!(helper.commands().is_empty());
    let mut backend = helper.backend();
    helper.clear();
    backend.pending_state = vec![7, 8, 9];
    assert!(
        backend
            .prepare(PluginPrepareConfig {
                max_block_frames: 47,
                ..reset_config()
            })
            .is_err()
    );
    assert!(helper.commands().is_empty());
    assert_eq!(backend.pending_state, [7, 8, 9]);
    assert!(backend.prepared);
    assert_eq!(backend.max_block_frames, 2048);
}

#[test]
fn daw_reset_old_helper_is_rejected_before_pending_state_and_lifecycle() {
    let helper = ResetHelper::new();
    helper.select("old");
    let mut backend = Vst3Backend::load(helper.spec(), reset_config()).unwrap();
    helper.clear();
    assert!(
        backend
            .prepare(reset_config())
            .unwrap_err()
            .contains("ResetOriginSupport")
    );
    assert_eq!(
        helper.commands().iter().map(kind).collect::<Vec<_>>(),
        ["ResetOriginSupport"]
    );
    assert_eq!(backend.pending_state, [1, 2, 3]);
    assert!(!backend.prepared);
}

#[test]
fn daw_reset_transaction_orders_preflight_panic_origin_and_lifecycle_on_every_slot_mode() {
    for (uid, version, reset_frames) in [
        ("00000000000000000000000000000001", "1", 128),
        ("ABCDEF019182FAEB566D624153675854", "1.3.4", 256),
    ] {
        for (enabled, bypassed) in [(true, false), (true, true), (false, false)] {
            for mode in [
                "ok",
                "old",
                "MidiPanic",
                "sdk",
                "loss",
                "StopProcessing",
                "StartProcessing",
            ] {
                let helper = ResetHelper::new();
                helper.set_identity(uid, version);
                let mut backend = helper.backend();
                backend
                    .set_transport(PluginTransport {
                        sample_position: 65_432,
                        quarter_note_position: 23.5,
                        playing: true,
                        ..PluginTransport::default()
                    })
                    .unwrap();
                backend
                    .send_midi(MidiMessage::new([0x90, 60, 100], 64))
                    .unwrap();
                helper.clear();
                helper.select(mode);
                let mut slots = vec![WorkerSlot {
                    backend: Some(Box::new(backend)),
                    config: SlotConfig {
                        enabled,
                        bypassed,
                        wet: 1.0,
                    },
                    fault: None,
                    parameter_catalog_cache: None,
                    native_base_ids: Vec::new(),
                }];
                let metrics = BridgeMetrics::default();
                let (mut events, _) = RingBuffer::new(8);
                reset_worker_epoch(91, &mut slots, &mut events, &metrics);
                let commands = helper.commands();
                let kinds = commands.iter().map(kind).collect::<Vec<_>>();
                assert_eq!(kinds[0], "ResetOriginSupport");
                assert!(!kinds.iter().any(|kind| {
                    [
                        "Process",
                        "SaveState",
                        "TakeParameterChanges",
                        "TakeParameterEdits",
                    ]
                    .contains(kind)
                }));
                if mode == "old" {
                    assert_eq!(kinds, ["ResetOriginSupport"]);
                } else {
                    assert_eq!(
                        kinds.iter().filter(|kind| **kind == "SendMidiAt").count(),
                        48
                    );
                    let panic = kinds.iter().position(|kind| *kind == "MidiPanic").unwrap();
                    assert!(panic > 48);
                    if mode == "MidiPanic" {
                        assert!(!kinds.contains(&"ProcessResetOrigin"));
                    } else {
                        let process = kinds
                            .iter()
                            .position(|kind| *kind == "ProcessResetOrigin")
                            .unwrap();
                        assert!(process > panic);
                        let request = &commands[process]["ProcessResetOrigin"];
                        assert_eq!(request["frames"], reset_frames);
                        assert_eq!(
                            kinds
                                .iter()
                                .filter(|kind| **kind == "ProcessResetOrigin")
                                .count(),
                            1
                        );
                        assert_eq!(request["transport"]["playing"], false);
                        assert_eq!(request["transport"]["sample_position"], 65_432);
                        assert_eq!(request["transport"]["quarter_note_position"], 23.5);
                        if ["sdk", "loss"].contains(&mode) {
                            assert!(!kinds.contains(&"StopProcessing"));
                        } else {
                            assert!(
                                kinds
                                    .iter()
                                    .position(|kind| *kind == "StopProcessing")
                                    .unwrap()
                                    > process
                            );
                            assert_eq!(
                                kinds.contains(&"StartProcessing"),
                                mode != "StopProcessing"
                            );
                        }
                    }
                }
                assert_eq!(slots[0].fault.is_some(), mode != "ok");
                assert_eq!(
                    metrics.reset_faults.load(Ordering::Acquire),
                    u64::from(mode != "ok")
                );
                if mode != "ok" {
                    helper.clear();
                    helper.select("ok");
                    reset_worker_epoch(92, &mut slots, &mut events, &metrics);
                    assert!(helper.commands().is_empty());
                }
            }
        }
    }
}

#[test]
fn daw_reset_policy_uses_exact_loaded_factory_identity_and_version() {
    const INSTRUMENT: &str = "ABCDEF019182FAEB566D624153675854";
    const FX: &str = "ABCDEF019182FAEB566D624153465854";
    for (uid, version, expected) in [
        (INSTRUMENT, "1.3.4", 256),
        (INSTRUMENT, "1.3.3", 128),
        (INSTRUMENT, "1.3.5", 128),
        (INSTRUMENT, "1.3.4 ", 128),
        (FX, "1.3.4", 128),
        ("00000000000000000000000000000001", "1.3.4", 128),
    ] {
        let helper = ResetHelper::new();
        helper.set_identity(uid, version);
        let mut spec = helper.spec();
        // Requested metadata cannot choose the compatibility policy.
        spec.class_uid = Some(INSTRUMENT.into());
        spec.descriptor.name = "Surge XT".into();
        let backend = Vst3Backend::load(spec, reset_config()).unwrap();
        assert_eq!(backend.reset_origin_frames(), expected);
    }
}

#[test]
fn daw_surge_reset_capacity_is_checked_before_state_and_prepare_mutation() {
    let helper = ResetHelper::new();
    helper.set_identity("ABCDEF019182FAEB566D624153675854", "1.3.4");
    for maximum in [128, 255] {
        helper.clear();
        let error = Vst3Backend::load(
            helper.spec(),
            PluginPrepareConfig {
                max_block_frames: maximum,
                ..reset_config()
            },
        )
        .err()
        .unwrap();
        assert!(error.contains("at least 256"));
        let commands = helper.commands();
        assert!(commands.iter().any(|command| kind(command) == "LoadPlugin"));
        assert!(!commands.iter().any(|command| {
            [
                "LoadState",
                "Reconfigure",
                "StartProcessing",
                "MidiPanic",
                "SendMidiAt",
                "ProcessResetOrigin",
            ]
            .contains(&kind(command))
        }));
    }
    for maximum in [256, 2048] {
        let config = PluginPrepareConfig {
            max_block_frames: maximum,
            ..reset_config()
        };
        let mut backend = Vst3Backend::load(helper.spec(), config).unwrap();
        backend.prepare(config).unwrap();
        assert_eq!(backend.reset_origin_frames(), 256);
        helper.clear();
        backend.pending_state = vec![7, 8, 9];
        for insufficient in [128, 255] {
            assert!(
                backend
                    .prepare(PluginPrepareConfig {
                        max_block_frames: insufficient,
                        ..reset_config()
                    })
                    .unwrap_err()
                    .contains("at least 256")
            );
            assert!(helper.commands().is_empty());
            assert_eq!(backend.pending_state, [7, 8, 9]);
            assert!(backend.prepared);
            assert_eq!(backend.max_block_frames, maximum);
        }
    }
}

#[test]
fn daw_surge_reset_support_capacity_refuses_before_worker_safety_on_every_slot_mode() {
    for (enabled, bypassed) in [(true, false), (true, true), (false, false)] {
        let helper = ResetHelper::new();
        helper.set_identity("ABCDEF019182FAEB566D624153675854", "1.3.4");
        let backend = helper.backend();
        helper.set_maximum(128);
        helper.clear();
        let mut slots = vec![WorkerSlot {
            backend: Some(Box::new(backend)),
            config: SlotConfig {
                enabled,
                bypassed,
                wet: 1.0,
            },
            fault: None,
            parameter_catalog_cache: None,
            native_base_ids: Vec::new(),
        }];
        let metrics = BridgeMetrics::default();
        let (mut events, _) = RingBuffer::new(8);
        reset_worker_epoch(91, &mut slots, &mut events, &metrics);
        assert_eq!(
            helper.commands().iter().map(kind).collect::<Vec<_>>(),
            ["ResetOriginSupport"]
        );
        assert!(slots[0].fault.is_some());
        assert_eq!(metrics.reset_faults.load(Ordering::Acquire), 1);
    }
}

#[test]
fn daw_surge_prepare_rejects_authoritative_helper_capacity_before_state_or_lifecycle() {
    for already_prepared in [false, true] {
        let helper = ResetHelper::new();
        helper.set_identity("ABCDEF019182FAEB566D624153675854", "1.3.4");
        let mut backend = Vst3Backend::load(helper.spec(), reset_config()).unwrap();
        if already_prepared {
            backend.prepare(reset_config()).unwrap();
        }
        backend.pending_state = vec![7, 8, 9];
        for maximum in [128, 255] {
            helper.set_maximum(maximum);
            helper.clear();
            assert!(
                backend
                    .prepare(reset_config())
                    .unwrap_err()
                    .contains("at least 256")
            );
            assert_eq!(
                helper.commands().iter().map(kind).collect::<Vec<_>>(),
                ["ResetOriginSupport"]
            );
            assert_eq!(backend.pending_state, [7, 8, 9]);
            assert_eq!(backend.prepared, already_prepared);
            assert_eq!(backend.max_block_frames, 2048);
        }
    }
}
