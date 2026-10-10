//! Test-only line-protocol stand-in. No VST binary, production helper or native UI is loaded.
use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
const UID: &str = "ABCDEF019182FAEB566D624153675854";

struct FakeHelper {
    dir: PathBuf,
    script: PathBuf,
    log: PathBuf,
    plugin: PathBuf,
}

impl FakeHelper {
    fn new(mode: &str) -> Self {
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "citrus-bounce-protocol-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-helper");
        let log = dir.join("requests.jsonl");
        let plugin = dir.join("no-vendor-code.vst3");
        std::fs::write(&plugin, b"protocol fixture only").unwrap();
        let program = r#"#!/usr/bin/env python3
import json, sys
mode = MODE
log = LOG
saved = False
loaded = False
processing = False
parameter_applied = False
uid = 'ABCDEF019182FAEB566D624153675854'
for line in sys.stdin:
    request = json.loads(line)
    with open(log, 'a') as stream:
        stream.write(json.dumps(request) + '\n')
    name = request if isinstance(request, str) else next(iter(request))
    data = {} if isinstance(request, str) else request[name]
    if name == 'LoadPlugin':
        result = {'PluginInfo': {'vendor':'Fixture','name':'Fixture','version':'1.3.4','category':'Instrument|Synth','uid':uid,'has_gui':True,'audio_inputs':0,'audio_outputs':1,'output_channels':2,'has_midi_input':True,'has_midi_output':False}}
    elif name == 'AudioBusLayout':
        result = {'AudioBusLayout': {'layout': {'inputs': [], 'outputs': [{'channel_count':2,'active':True}]}}}
    elif name == 'CheckedAudioBusLayout':
        if mode == 'old_audio_helper':
            result = {'Error': {'message':'unsupported command'}}
        elif mode == 'audio_wrong_reply':
            result = {'Success': {'message':'wrong'}}
        elif mode == 'audio_sdk_error':
            result = {'Error': {'message':'getBusInfo failed'}}
        elif mode == 'audio_unsupported':
            result = {'MetadataQueryRejected': {'reason':'Unsupported'}}
        elif mode == 'audio_pending':
            result = {'MetadataQueryRejected': {'reason':'PendingTopology'}}
        elif (mode == 'audio_changed_on_start' and processing) or (mode == 'audio_changed_on_parameter' and parameter_applied):
            result = {'MetadataQueryRejected': {'reason':'InconsistentTopology'}}
        else:
            result = {'CheckedAudioBusLayout': {'layout': {'inputs': [], 'outputs': [{'channel_count':2,'is_main':True,'active':True}]}}}
    elif name == 'EventBusLayout':
        if mode == 'old_helper':
            result = {'Error': {'message':'unsupported command'}}
        else:
            bus = {'channel_count':16,'is_main':True,'active':True}
            outputs = [bus] if (mode == 'outputs_after_state' and loaded) or (mode == 'metadata_change' and saved) else []
            inputs = [bus, bus] if mode == 'extra_input' else [dict(bus, active=None)] if mode == 'unknown_activation' else [bus]
            result = {'EventBusLayout': {'layout': {'inputs':inputs,'outputs':outputs}}}
    elif name == 'ResetOriginSupport':
        result = {'ResetOriginSupport': {'support': {'contract_version':1,'max_block_frames':2048,'processing':processing}}}
    elif name == 'Editor':
        if data['command'] != 'Query':
            result = {'Error': {'message':'editor mutation forbidden in fixture'}}
        else:
            generation = 3 if mode == 'generation_change' and saved else 2
            result = {'EditorState': {'state': {'supported':True,'has_editor':True,'open':mode == 'open','width':0,'height':0,'generation':generation}}}
    elif name == 'NativeDirtyRevision':
        result = {'NativeDirtyRevision': {'revision':10 if mode == 'revision_change' and saved else 9}}
    elif name == 'LatencySamples' or name == 'TailSamples':
        result = {name: {'samples':0}}
    elif name == 'GetAllParameters':
        parameters = [] if mode == 'unknown_base' else [{'id':17,'name':'Gain','value':0.75,'min':0.0,'max':1.0,'default':0.5,'unit':'','step_count':0,'can_automate':True,'is_read_only':False,'is_bypass':False}]
        result = {'Parameters': {'params':parameters}}
    elif name == 'GetParameter':
        result = {'ParameterValue': {'value':0.75}}
    elif name == 'SaveState':
        saved = True
        result = {'State': {'data':[] if mode == 'empty_state' else [1,2,3]}}
    elif name in ('SetProcessMode','LoadState','StartProcessing','StopProcessing','SetParameter','Shutdown','UnloadPlugin'):
        if name == 'SetParameter': parameter_applied = True
        if name == 'LoadState': loaded = True
        if name == 'StartProcessing': processing = True
        if name == 'StopProcessing': processing = False
        result = {'Success': {'message':'ok'}}
    else:
        result = {'Error': {'message':'forbidden/unimplemented command: ' + name}}
    print(json.dumps(result), flush=True)
    if name == 'Shutdown': break
"#;
        let program = program
            .replace("MODE", &serde_json::to_string(mode).unwrap())
            .replace("LOG", &serde_json::to_string(&log).unwrap());
        std::fs::write(&script, program).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            dir,
            script,
            log,
            plugin,
        }
    }

    fn spec(&self, state: Vec<u8>) -> super::super::PluginLoadSpec {
        super::super::PluginLoadSpec {
            descriptor: super::super::PluginDescriptor {
                id: "path-id".into(),
                name: "Fixture".into(),
                vendor: "Fixture".into(),
                path: self.plugin.clone(),
                format: super::super::PluginFormat::Vst3,
                category: "Instrument".into(),
                is_instrument: true,
                verified: true,
                vst3_metadata: None,
                scan_error: None,
            },
            class_uid: Some(UID.into()),
            vst3_helper_path: Some(self.script.clone()),
            initial_state: state,
            enabled: true,
            bypassed: false,
            wet: 1.0,
        }
    }

    fn source(&self) -> Vst3Backend {
        let config = PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 2048,
        };
        let mut source = Vst3Backend::load(self.spec(vec![]), config).unwrap();
        source.prepare(config).unwrap();
        std::fs::write(&self.log, b"").unwrap();
        source
    }

    fn requests(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn names(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.as_object().unwrap().keys().next().unwrap().clone())
            })
            .collect()
    }
}

impl Drop for FakeHelper {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn capture_state() -> BouncePluginState {
    BouncePluginState {
        metadata: BouncePluginMetadata {
            class_uid: UID.into(),
            version: "1.3.4".into(),
            name: "Fixture".into(),
            category: "Instrument|Synth".into(),
            event_input_buses: vec![(16, true, Some(true))],
            event_output_buses: vec![],
            input_buses: vec![],
            output_buses: vec![(2, true)],
            latency_samples: 0,
            tail_samples: 0,
        },
        opaque_state: vec![1, 2, 3],
        parameter_bases: BTreeMap::from([(17, 0.75)]),
        native_generation: 2,
        native_dirty_revision: 9,
    }
}

#[test]
fn actual_vst3_capture_queries_closed_state_current_bases_and_all_fences_only() {
    let fake = FakeHelper::new("normal");
    let mut source = fake.source();
    let state = source.capture_bounce_state(&[17]).unwrap();
    assert_eq!(state, capture_state());
    assert_eq!(
        fake.names(),
        [
            "Editor",
            "NativeDirtyRevision",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples",
            "SaveState",
            "GetAllParameters",
            "GetParameter",
            "Editor",
            "NativeDirtyRevision",
            "CheckedAudioBusLayout",
            "EventBusLayout",
            "LatencySamples",
            "TailSamples"
        ]
    );
    assert!(source.prepared);
    assert!(!source.last_transport.playing);
    assert!(source.plugin.is_processing());
}

#[test]
fn actual_vst3_capture_rejects_open_editor_unknown_keys_empty_state_and_changed_fences() {
    for mode in [
        "open",
        "unknown_base",
        "empty_state",
        "generation_change",
        "revision_change",
        "metadata_change",
        "old_helper",
        "extra_input",
        "unknown_activation",
    ] {
        let fake = FakeHelper::new(mode);
        let mut source = fake.source();
        assert!(source.capture_bounce_state(&[17]).is_err(), "{mode}");
        let names = fake.names();
        assert!(
            !names.iter().any(|name| matches!(
                name.as_str(),
                "Process"
                    | "ProcessBuses"
                    | "ProcessResetOrigin"
                    | "LoadState"
                    | "StopProcessing"
                    | "StartProcessing"
            )),
            "{mode}: {names:?}"
        );
        assert!(fake.requests().iter().all(|request| {
            request
                .get("Editor")
                .is_none_or(|data| data["command"] == "Query")
        }));
        if mode == "open" {
            assert_eq!(names, ["Editor"]);
        }
        if mode == "unknown_base" {
            assert!(!names.iter().any(|name| name == "GetParameter"));
        }
    }
}

#[test]
fn actual_fresh_factory_selects_offline_before_restore_and_start_without_warmup_or_resave() {
    let fake = FakeHelper::new("normal");
    let state = capture_state();
    let owner =
        OfflineInstrument::load_snapshot(fake.spec(state.opaque_state.clone()), 48_000, &state)
            .unwrap();
    assert_eq!(owner.metadata(), &state.metadata);
    let names = fake.names();
    let index = |wanted: &str| names.iter().position(|name| name == wanted).unwrap();
    assert!(index("SetProcessMode") < index("LoadState"));
    assert!(index("LoadState") < index("StartProcessing"));
    assert!(index("StartProcessing") < index("SetParameter"));
    assert!(index("SetParameter") < index("EventBusLayout"));
    assert!(!names.iter().any(|name| matches!(
        name.as_str(),
        "Process" | "ProcessBuses" | "ProcessResetOrigin" | "SaveState" | "Editor"
    )));
    let requests = fake.requests();
    assert!(requests.iter().any(|request| {
        request
            .get("SetProcessMode")
            .is_some_and(|data| data["mode"] == "Offline")
    }));
    assert!(requests.iter().any(|request| {
        request
            .get("SetParameter")
            .is_some_and(|data| data["id"] == 17 && data["value"] == 0.75)
    }));
}

#[test]
fn actual_fresh_factory_rejects_post_restore_event_output_and_old_helper() {
    for mode in [
        "outputs_after_state",
        "old_helper",
        "extra_input",
        "unknown_activation",
    ] {
        let fake = FakeHelper::new(mode);
        let state = capture_state();
        assert!(
            OfflineInstrument::load_snapshot(fake.spec(state.opaque_state.clone()), 48_000, &state)
                .is_err(),
            "{mode}"
        );
        let names = fake.names();
        assert!(names.iter().any(|name| name == "LoadState"));
        assert!(names.iter().any(|name| name == "EventBusLayout"));
        assert!(!names.iter().any(|name| matches!(
            name.as_str(),
            "Process" | "ProcessBuses" | "ProcessResetOrigin" | "SaveState"
        )));
    }
}

#[test]
fn capture_metadata_typed_refusal_is_harmless_but_unknown_remote_errors_fault() {
    for (mode, harmless) in [
        ("audio_unsupported", true),
        ("audio_pending", true),
        ("audio_changed_on_start", true),
        ("old_audio_helper", false),
        ("old_helper", false),
        ("audio_wrong_reply", false),
        ("audio_sdk_error", false),
    ] {
        let fake = FakeHelper::new(mode);
        let mut source = fake.source();
        let result = source.capture_bounce_state(&[17]);
        assert!(
            if harmless {
                matches!(result, Err(BounceCaptureError::Rejected(_)))
            } else {
                matches!(result, Err(BounceCaptureError::BackendFailed(_)))
            },
            "{mode}"
        );
        assert!(!fake.names().iter().any(|name| name == "SaveState"));
        assert!(source.prepared);
    }
}

#[test]
fn fresh_factory_rejects_audio_changes_at_start_or_parameter_application_and_sdk_errors() {
    for mode in [
        "audio_changed_on_start",
        "audio_changed_on_parameter",
        "audio_sdk_error",
        "audio_wrong_reply",
        "old_audio_helper",
        "audio_pending",
    ] {
        let fake = FakeHelper::new(mode);
        let state = capture_state();
        assert!(
            OfflineInstrument::load_snapshot(fake.spec(state.opaque_state.clone()), 48_000, &state)
                .is_err(),
            "{mode}"
        );
        let names = fake.names();
        assert!(names.iter().any(|name| name == "CheckedAudioBusLayout"));
        // Existing backend Drop closes the failed fresh candidate; no editor was opened.
        assert!(fake.requests().iter().all(|request| {
            request
                .get("Editor")
                .is_none_or(|data| data["command"] == "Close")
        }));
        assert!(!names.iter().any(|name| matches!(
            name.as_str(),
            "Process" | "ProcessBuses" | "ProcessResetOrigin" | "SaveState"
        )));
    }
}
