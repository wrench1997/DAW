//! Actual optional VST3 adapter path through a command-recording protocol stand-in.
//! No vendor binary or production helper is executed by these tests.
use super::*;
use runtime::{PluginBackend, PluginChain, RetainedShutdownPoll, Vst3Backend};
use std::{
    cell::RefCell,
    mem::ManuallyDrop,
    ops::Deref,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const WORKER_JOIN_BUDGET: Duration = Duration::from_secs(5);
static PROTOCOL_FIXTURE_BLOCKED: AtomicBool = AtomicBool::new(false);

// One unresolved owner poisons later fixture admission. The test thread's teardown must not
// implicitly drop its retained guard or helper files; this is failure evidence, never success.
thread_local! {
    static RETAINED_PROTOCOL_FAILURE: RefCell<Option<ManuallyDrop<RetainedProtocolFailure>>> = const { RefCell::new(None) };
}

struct RetainedProtocolFailure {
    _chain: PluginChain,
    _helper: Helper,
}

#[derive(Clone)]
struct Helper(Arc<HelperFiles>);

struct HelperFiles {
    root: PathBuf,
    script: PathBuf,
    log: PathBuf,
    plugin: PathBuf,
    preserve_evidence: AtomicBool,
}

impl Deref for Helper {
    type Target = HelperFiles;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Helper {
    fn new(mode: &str) -> Self {
        assert!(
            !PROTOCOL_FIXTURE_BLOCKED.load(Ordering::Acquire),
            "A protocol fixture still owns an unjoined worker; later admission refused"
        );
        RETAINED_PROTOCOL_FAILURE.with(|retained| {
            assert!(
                retained.borrow().is_none(),
                "protocol fixture quarantine is occupied"
            );
        });
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "citrus-source-restoration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let script = root.join("helper.py");
        let log = root.join("commands.jsonl");
        let plugin = root.join("fixture.vst3");
        std::fs::write(&plugin, b"Protocol stand-in only, not a VST binary").unwrap();
        let templates = serde_json::json!({
            "LoadPlugin": {"PluginInfo": {"vendor":"Unattested vendor","name":"Restoration fixture","version":"1","category":"Instrument|Synth","uid":UID,"has_gui":false,"audio_inputs":0,"audio_outputs":1,"output_channels":2,"has_midi_input":true,"has_midi_output":false,"compatibility":[]}},
            "AudioBusLayout": {"AudioBusLayout":{"layout":{"inputs":[],"outputs":[{"channel_count":2,"active":true}]}}},
            "CheckedAudioBusLayout": {"CheckedAudioBusLayout":{"layout":{"inputs":[],"outputs":[{"channel_count":2,"is_main":true,"active":true}]}}},
            "EventBusLayout": {"EventBusLayout":{"layout":{"inputs":[{"channel_count":16,"is_main":true,"active":true}],"outputs":[]}}},
            "ResetOriginSupport": {"ResetOriginSupport":{"support":{"contract_version":1,"max_block_frames":2048,"processing":false}}},
            "GetAllParameters": {"Parameters":{"params":[
                {"id":17,"name":"Last base","value":0.0,"min":0.0,"max":1.0,"default":0.5,"unit":"","step_count":0,"can_automate":true,"is_read_only":false,"is_bypass":false,"flags":0},
                {"id":7,"name":"First base","value":0.0,"min":0.0,"max":1.0,"default":0.5,"unit":"","step_count":0,"can_automate":true,"is_read_only":false,"is_bypass":false,"flags":0}]}},
            "Editor": {"EditorState":{"state":{"supported":false,"has_editor":false,"open":false,"width":0,"height":0,"generation":2}}},
            "NativeDirtyRevision":{"NativeDirtyRevision":{"revision":0}},
            "TakeParameterEdits":{"ParameterEdits":{"edits":[]}},
            "TakeHostNotifications":{"HostNotifications":{"notifications":[]}},
            "ServiceHostRequests":{"RestartFlags":{"bits":0}},
            "LatencySamples":{"LatencySamples":{"samples":0}},
            "TailSamples":{"TailSamples":{"samples":0}},
            "Success":{"Success":{"message":"ok"}},
            "Unexpected":{"Error":{"message":"Unexpected restoration fixture command"}}
        });
        // Verify every template against the actual decoder before any helper may be launched.
        let encoded = serde_json::to_string(&templates).unwrap();
        let _: BTreeMap<String, vst3_host::process_isolation::HostResponse> =
            serde_json::from_str(&encoded).unwrap();
        let program = r#"#!/usr/bin/env python3
import copy,json,signal,sys
signal.alarm(15)
templates=json.loads(TEMPLATES)
mode=MODE
log=LOG
values={7:0.0,17:0.0}
started=False
latency_reads=0
for line in sys.stdin:
    request=json.loads(line)
    with open(log,'a') as f: f.write(json.dumps(request)+'\n')
    name=request if isinstance(request,str) else next(iter(request))
    data={} if isinstance(request,str) else request[name]
    response=copy.deepcopy(templates.get(name,templates['Unexpected']))
    if name=='LoadPlugin':
        info=response['PluginInfo']
        info['uid']=data.get('class_id') or '00000000000000000000000000000002'
        if mode=='wrong_uid': info['uid']='00000000000000000000000000000002'
        if mode=='wrong_version': info['version']='2'
        if mode=='wrong_name': info['name']='Different actual name'
        if mode=='wrong_category': info['category']='Instrument|Other'
    elif name=='GetAllParameters':
        if mode=='missing': response['Parameters']['params']=response['Parameters']['params'][1:]
        if mode=='readonly': response['Parameters']['params'][0]['is_read_only']=True
    elif name=='SetParameter':
        values[data['id']]=data['value']
        if mode=='coupled_setter' and data['id']==17: values[7]=0.5
        response=templates['Success']
    elif name=='GetParameter':
        response={'ParameterValue':{'value':values[data['id']]}}
    elif name=='StartProcessing':
        started=True
        if mode=='base_on_start': values[17]=0.5
        response=templates['Success']
    elif name=='CheckedAudioBusLayout':
        if mode=='metadata_on_setter' and values[17]!=0.0: response['CheckedAudioBusLayout']['layout']['outputs'][0]['channel_count']=1
        if mode=='metadata_on_start' and started: response['CheckedAudioBusLayout']['layout']['outputs'][0]['channel_count']=1
    elif name=='LatencySamples':
        latency_reads+=1
        if (mode=='ready_timing' and latency_reads>=2) or (mode=='snapshot_timing' and latency_reads>=3) or (mode=='later_timing' and latency_reads>=4) or (mode=='fresh_later_timing' and latency_reads>=7):
            response['LatencySamples']['samples']=37
    elif name=='TailSamples':
        if mode=='fresh_tail_settles' and started: response['TailSamples']['samples']=1
    elif name in ('LoadState','StopProcessing','Shutdown','UnloadPlugin','SetProcessMode'):
        response=templates['Success']
    print(json.dumps(response),flush=True)
    if name=='Shutdown': break
"#
            .replace("TEMPLATES", &serde_json::to_string(&encoded).unwrap())
            .replace("MODE", &serde_json::to_string(mode).unwrap())
            .replace("LOG", &serde_json::to_string(&log).unwrap());
        std::fs::write(&script, program).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self(Arc::new(HelperFiles {
            root,
            script,
            log,
            plugin,
            preserve_evidence: AtomicBool::new(false),
        }))
    }

    fn spec(&self) -> PluginLoadSpec {
        let mut spec = super::spec();
        spec.descriptor.path = self.plugin.clone();
        spec.vst3_helper_path = Some(self.script.clone());
        spec
    }
    fn expected(&self) -> SourceRestorationExpectation {
        SourceRestorationExpectation::new(
            2,
            41,
            "descriptor-path-id".into(),
            self.plugin.clone(),
            48_000,
            metadata(),
            vec![(7, 0.25), (17, 0.75)],
        )
        .unwrap()
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
            .map(|request| {
                request
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| request.as_object().unwrap().keys().next().unwrap().clone())
            })
            .collect()
    }
}

impl Drop for HelperFiles {
    fn drop(&mut self) {
        if !std::thread::panicking() && !self.preserve_evidence.load(Ordering::Acquire) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

/// The script alarm bounds fixture activity, not thread join. Poll the exact retained owner
/// for a bounded observation period, then quarantine it with the same helper files if pending.
struct Worker {
    chain: Option<PluginChain>,
    helper: Helper,
}

impl Worker {
    fn new(chain: PluginChain, helper: &Helper) -> Self {
        Self {
            chain: Some(chain),
            helper: helper.clone(),
        }
    }

    fn chain(&mut self) -> &mut PluginChain {
        self.chain
            .as_mut()
            .expect("fixture owns its exact worker chain")
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let was_panicking = std::thread::panicking();
        let deadline = Instant::now() + WORKER_JOIN_BUDGET;
        let outcome = loop {
            let outcome = self.chain().guard.poll_shutdown_retained();
            if outcome != RetainedShutdownPoll::Pending {
                break outcome;
            }
            if Instant::now() >= deadline {
                self.helper.preserve_evidence.store(true, Ordering::Release);
                PROTOCOL_FIXTURE_BLOCKED.store(true, Ordering::Release);
                let retained = RetainedProtocolFailure {
                    _chain: self
                        .chain
                        .take()
                        .expect("pending fixture owns its exact chain"),
                    _helper: self.helper.clone(),
                };
                RETAINED_PROTOCOL_FAILURE.with(|slot| {
                    // Admission above excludes a second fixture on this poisoned thread.
                    *slot.borrow_mut() = Some(ManuallyDrop::new(retained));
                });
                if !was_panicking {
                    panic!(
                        "Restoration fixture join is still pending; exact owners and evidence quarantined"
                    );
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        if outcome == RetainedShutdownPoll::WorkerPanicked {
            self.helper.preserve_evidence.store(true, Ordering::Release);
            if !was_panicking {
                panic!("Restoration fixture worker did not join successfully; evidence retained");
            }
        }
    }
}

#[test]
fn actual_restoration_rejects_wrong_loaded_identity_before_state_or_prepare() {
    for mode in ["wrong_uid", "wrong_version", "wrong_name", "wrong_category"] {
        let helper = Helper::new(mode);
        let result =
            Vst3Backend::load_with_restoration(helper.spec(), config(), Some(helper.expected()));
        assert!(result.is_err(), "{mode}");
        let names = helper.names();
        assert!(names.iter().any(|name| name == "LoadPlugin"));
        assert!(!names.iter().any(|name| matches!(
            name.as_str(),
            "LoadState" | "ResetOriginSupport" | "StartProcessing" | "SetParameter"
        )));
    }
}

#[test]
fn actual_restoration_preflights_all_bases_before_set_and_detects_setter_start_drift() {
    for mode in [
        "missing",
        "readonly",
        "coupled_setter",
        "base_on_start",
        "metadata_on_setter",
        "metadata_on_start",
    ] {
        let helper = Helper::new(mode);
        let mut backend =
            Vst3Backend::load_with_restoration(helper.spec(), config(), Some(helper.expected()))
                .unwrap();
        assert!(backend.prepare(config()).is_err(), "{mode}");
        if matches!(mode, "missing" | "readonly") {
            assert!(
                !helper
                    .names()
                    .iter()
                    .any(|name| matches!(name.as_str(), "SetParameter" | "StartProcessing"))
            );
        }
        let before_retry = helper.requests().len();
        assert!(backend.prepare(config()).is_err());
        assert_eq!(
            helper.requests().len(),
            before_retry,
            "one-shot preparation must not replay"
        );
    }
}

#[test]
fn actual_restoration_selects_exact_class_and_sets_sorted_bases_once_before_start() {
    let helper = Helper::new("ok");
    let mut backend =
        Vst3Backend::load_with_restoration(helper.spec(), config(), Some(helper.expected()))
            .unwrap();
    backend.prepare(config()).unwrap();
    let requests = helper.requests();
    let load = requests
        .iter()
        .find_map(|request| request.get("LoadPlugin"))
        .unwrap();
    assert_eq!(load["class_id"], UID);
    let setters: Vec<_> = requests
        .iter()
        .filter_map(|request| request.get("SetParameter"))
        .map(|setter| {
            (
                setter["id"].as_u64().unwrap(),
                setter["value"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(setters, vec![(7, 0.25), (17, 0.75)]);
    let names = helper.names();
    let index = |name| {
        names
            .iter()
            .position(|candidate| candidate == name)
            .unwrap()
    };
    assert!(index("LoadState") < index("GetAllParameters"));
    assert!(index("GetAllParameters") < index("SetParameter"));
    assert!(
        names
            .iter()
            .rposition(|name| name == "SetParameter")
            .unwrap()
            < index("StartProcessing")
    );
    assert!(
        index("StartProcessing")
            < names
                .iter()
                .rposition(|name| name == "GetParameter")
                .unwrap()
    );
    assert!(index("StartProcessing") < index("CheckedAudioBusLayout"));
    assert!(!names.iter().any(|name| matches!(
        name.as_str(),
        "Process" | "ProcessResetOrigin" | "SaveState"
    )));
}

#[test]
fn ordinary_none_loader_keeps_default_class_and_does_not_restore_generic_bases() {
    let helper = Helper::new("ok");
    let mut spec = helper.spec();
    spec.class_uid = None;
    let mut backend = Vst3Backend::load(spec, config()).unwrap();
    backend.prepare(config()).unwrap();
    assert_eq!(
        backend.plugin.info().uid,
        "00000000000000000000000000000002"
    );
    assert!(!helper.names().iter().any(|name| matches!(
        name.as_str(),
        "GetAllParameters" | "SetParameter" | "CheckedAudioBusLayout"
    )));
}

#[test]
fn actual_worker_checks_both_initial_timing_reads_then_drops_only_temporary_pin() {
    for (mode, expected_ready, expected_fault, expected_latency) in [
        ("ready_timing", false, true, 0),
        ("snapshot_timing", true, true, 0),
        ("ok", true, false, 0),
        ("later_timing", true, false, 37),
    ] {
        let helper = Helper::new(mode);
        let mut worker = Worker::new(
            PluginChain::spawn_recreated_source(
                2,
                41,
                helper.spec(),
                config(),
                helper.expected(),
                false,
            )
            .unwrap(),
            &helper,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let stats = worker.chain().control.stats();
            let snapshot = worker.chain().control.metrics.plugin_latency_snapshot();
            if (expected_fault
                && stats.faults != 0
                && snapshot.is_some_and(|snapshot| snapshot.active_mask == 0))
                || (!expected_fault
                    && snapshot.is_some_and(|snapshot| {
                        snapshot.active_mask == 1
                            && snapshot.total_plugin_latency_samples == expected_latency
                    }))
            {
                break;
            }
            assert!(Instant::now() < deadline, "worker did not reach {mode}");
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut ready = false;
        while let Some(event) = worker.chain().control.try_next_event() {
            if matches!(event, runtime::RuntimeEvent::SlotReady { .. }) {
                ready = true;
            }
        }
        assert_eq!(ready, expected_ready, "{mode}");
        assert_eq!(
            worker.chain().control.stats().faults != 0,
            expected_fault,
            "{mode}"
        );
        if expected_fault {
            assert_eq!(
                worker
                    .chain()
                    .control
                    .metrics
                    .plugin_latency_snapshot()
                    .unwrap()
                    .active_mask,
                0
            );
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match worker.chain().guard.poll_shutdown_retained() {
                RetainedShutdownPoll::Joined => break,
                RetainedShutdownPoll::WorkerPanicked => panic!("fixture worker panicked"),
                RetainedShutdownPoll::Pending => {
                    assert!(
                        Instant::now() < deadline,
                        "fixture shutdown exceeded budget"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        assert_eq!(worker.chain().control.stats().detached_workers, 0);
    }
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn recorded_fresh_source_keeps_its_independent_lifetime_pin_after_initial_snapshot() {
    let helper = Helper::new("fresh_later_timing");
    let mut worker = Worker::new(
        PluginChain::spawn_recreated_source(
            2,
            41,
            helper.spec(),
            config(),
            helper.expected(),
            true,
        )
        .unwrap(),
        &helper,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    let mut pinned_failure = false;
    loop {
        while let Some(event) = worker.chain().control.try_next_event() {
            match event {
                runtime::RuntimeEvent::SlotReady { .. } => ready = true,
                runtime::RuntimeEvent::SlotFault { message, .. } => {
                    pinned_failure |=
                        message.contains("Fresh bounce source latency_samples changed");
                }
                _ => {}
            }
        }
        if pinned_failure
            && worker
                .chain()
                .control
                .metrics
                .plugin_latency_snapshot()
                .is_some_and(|snapshot| snapshot.active_mask == 0)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fresh lifetime pin was not retained"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        ready,
        "both temporary initialization reads preceded the later fresh-pin refusal"
    );
}

#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
#[test]
fn fresh_initial_tail_settlement_must_still_equal_restoration_expectation() {
    for expected_tail in [0, 1] {
        let helper = Helper::new("fresh_tail_settles");
        let mut expected = helper.expected();
        expected.metadata.tail_samples = expected_tail;
        let mut backend =
            Vst3Backend::load_with_restoration(helper.spec(), config(), Some(expected)).unwrap();
        backend.fresh_bounce_setup = Some(runtime::FreshBounceSourceSetup::Pending);
        assert_eq!(backend.prepare(config()).is_ok(), expected_tail == 1);
        assert!(matches!(
            backend.fresh_bounce_setup,
            Some(runtime::FreshBounceSourceSetup::Pinned {
                tail_samples: 1,
                ..
            })
        ));
    }
}
