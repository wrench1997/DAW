//! Shared actual VST3 adapter protocol fixture; no plug-in binary or device is used.
use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

pub(super) struct ResetHelper {
    pub(super) root: PathBuf,
    script: PathBuf,
    log: PathBuf,
    mode: PathBuf,
    identity: PathBuf,
    identity_traits: PathBuf,
    maximum: PathBuf,
    pub(super) layout: PathBuf,
}

impl ResetHelper {
    pub(super) fn new() -> Self {
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
        let identity_traits = root.join("identity-traits.json");
        std::fs::write(
            &identity_traits,
            r#"{"category":"Instrument","midi_input":true,"midi_output":false}"#,
        )
        .unwrap();
        let maximum = root.join("maximum");
        std::fs::write(&maximum, "2048").unwrap();
        let layout = root.join("layout.json");
        std::fs::write(
            &layout,
            r#"{"inputs":[],"outputs":[{"channel_count":2,"active":true}]}"#,
        )
        .unwrap();
        let source = format!(
            r#"#!/usr/bin/python3
import json,sys,pathlib,os,time
log=pathlib.Path({log:?})
mode=pathlib.Path({mode:?})
identity=pathlib.Path({identity:?})
identity_traits=pathlib.Path({identity_traits:?})
maximum=pathlib.Path({maximum:?})
layout=pathlib.Path({layout:?})
child_record=log.with_name('child-pending.json')
child_record.write_text(json.dumps({{'pid':os.getpid()}}))
child_record.replace(log.with_name('child.json'))
if log.with_name('hold-child-start').exists():
    while not log.with_name('release-child-start').exists(): time.sleep(0.001)
shutdown=False
started=False
for line in sys.stdin:
    command=json.loads(line)
    with log.open('a') as f: f.write(json.dumps(command)+'\n')
    kind=command if isinstance(command,str) else next(iter(command))
    if kind=='Shutdown':
        shutdown=True
        break
    selected=mode.read_text()
    if kind=='LoadPlugin' and selected=='LoadPlugin':
        response={{'Error':{{'message':'deliberate LoadPlugin refusal'}}}}
    elif kind=='LoadPlugin':
        uid,version=json.loads(identity.read_text())
        traits=json.loads(identity_traits.read_text())
        response={{'PluginInfo':{{'vendor':'Citrus tests','name':'Reset probe','version':version,'category':traits['category'],'uid':uid,'has_gui':False,'audio_inputs':0,'audio_outputs':1,'output_channels':2,'has_midi_input':traits['midi_input'],'has_midi_output':traits['midi_output']}}}}
    elif kind=='ResetOriginSupport':
        response=({{'Error':{{'message':'old helper unsupported'}}}} if selected=='old' else {{'ResetOriginSupport':{{'support':{{'contract_version':1,'max_block_frames':int(maximum.read_text()),'processing':True}}}}}})
    elif selected==kind:
        response={{'Error':{{'message':'deliberate '+kind+' refusal'}}}}
    elif kind=='ProcessResetOrigin':
        response={{'ResetOriginReport':{{'report':{{'frames':command[kind]['frames'],'discarded_prior_midi_events':0,'discarded_midi_events':2,'discarded_prior_parameter_points':0,'discarded_parameter_points':3,'output_events_lost':selected=='loss','parameter_output_fault':False,'process_error':'SDK refusal' if selected=='sdk' else None}}}}}}
    elif kind=='AudioBusLayout':
        if selected=='layout-exit': sys.exit(9)
        response=({{'Success':{{'message':'wrong layout response'}}}} if selected=='layout-wrong' else {{'AudioBusLayout':{{'layout':json.loads(layout.read_text())}}}})
    elif kind=='CheckedAudioBusLayout':
        current=json.loads(layout.read_text())
        checked={{direction:[{{'channel_count':bus['channel_count'],'is_main':index==0,'active':bus['active']}} for index,bus in enumerate(current[direction])] for direction in ['inputs','outputs']}}
        response={{'CheckedAudioBusLayout':{{'layout':checked}}}}
    elif kind=='EventBusLayout':
        traits=json.loads(identity_traits.read_text())
        response={{'EventBusLayout':{{'layout':{{'inputs':[{{'channel_count':16,'is_main':True,'active':selected!='inactive-event'}}] if traits['midi_input'] else [],'outputs':[{{'channel_count':16,'is_main':True,'active':True}}] if traits['midi_output'] else []}}}}}}
    elif kind=='LatencySamples': response={{'LatencySamples':{{'samples':37}}}}
    elif kind=='TailSamples': response=({{'Error':{{'message':'deliberate post-Start tail refusal'}}}} if selected=='post-start-tail' and started else {{'TailSamples':{{'samples':511}}}})
    elif kind=='StartProcessing':
        started=True
        response={{'Success':{{'message':'ok'}}}}
    elif kind=='NativeDirtyRevision': response={{'NativeDirtyRevision':{{'revision':0}}}}
    elif kind=='Editor': response={{'EditorState':{{'state':{{'supported':False,'has_editor':False,'open':False,'width':0,'height':0,'generation':0}}}}}}
    elif kind=='TakeParameterEdits': response={{'ParameterEdits':{{'edits':[]}}}}
    elif kind=='TakeHostNotifications': response={{'HostNotifications':{{'notifications':[]}}}}
    elif kind=='ServiceHostRequests': response={{'RestartFlags':{{'bits':0}}}}
    else: response={{'Success':{{'message':'ok'}}}}
    print(json.dumps(response),flush=True)
log.with_name('child-exit.json').write_text(json.dumps({{'pid':os.getpid(),'shutdown':shutdown,'exit_code':0}}))
"#,
            log = log.to_string_lossy(),
            mode = mode.to_string_lossy(),
            identity = identity.to_string_lossy(),
            identity_traits = identity_traits.to_string_lossy(),
            maximum = maximum.to_string_lossy(),
            layout = layout.to_string_lossy()
        );
        std::fs::write(&script, source).unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        #[cfg(windows)]
        let script = windows_launcher(&root);
        Self {
            root,
            script,
            log,
            mode,
            identity,
            identity_traits,
            maximum,
            layout,
        }
    }
    pub(super) fn set_identity(&self, uid: &str, version: &str) {
        std::fs::write(&self.identity, serde_json::to_vec(&(uid, version)).unwrap()).unwrap();
    }
    pub(super) fn set_identity_traits(&self, category: &str, midi_input: bool, midi_output: bool) {
        std::fs::write(
            &self.identity_traits,
            serde_json::to_vec(&serde_json::json!({
                "category":category,"midi_input":midi_input,"midi_output":midi_output,
            }))
            .unwrap(),
        )
        .unwrap();
    }
    /// Hold the helper before its first request so Windows can retain the exact
    /// Python handle even when LOAD qualification immediately disposes it.
    pub(super) fn hold_child_start(&self) -> ProtocolChildStartGate {
        std::fs::write(
            self.root.join("hold-child-start"),
            b"hold before first request",
        )
        .unwrap();
        ProtocolChildStartGate {
            root: self.root.clone(),
        }
    }
    pub(super) fn set_maximum(&self, frames: usize) {
        std::fs::write(&self.maximum, frames.to_string()).unwrap();
    }
    pub(super) fn spec(&self) -> PluginLoadSpec {
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
            vst3_catalog: None,
            scan_error: None,
        });
        spec.vst3_helper_path = Some(self.script.clone());
        spec.initial_state = vec![1, 2, 3];
        spec
    }
    pub(super) fn select(&self, value: &str) {
        std::fs::write(&self.mode, value).unwrap();
    }
    pub(super) fn clear(&self) {
        std::fs::write(&self.log, "").unwrap();
    }
    pub(super) fn commands(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    pub(super) fn backend(&self) -> Vst3Backend {
        let config = reset_config();
        let mut backend = Vst3Backend::load(self.spec(), config).unwrap();
        backend.prepare(config).unwrap();
        backend
    }

    /// Open the exact Python process before requesting shutdown. On Windows the
    /// normal helper owner holds CMD, so Joined alone cannot prove this child exited.
    pub(super) fn observe_child(&self) -> Result<ProtocolChild, String> {
        let record: serde_json::Value = serde_json::from_slice(
            &std::fs::read(self.root.join("child.json")).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let pid = record["pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|&pid| pid != 0)
            .ok_or("helper child PID missing")?;
        #[cfg(windows)]
        let process = {
            use std::os::windows::io::{FromRawHandle, OwnedHandle};
            use winapi::um::{
                processthreadsapi::OpenProcess, winnt::PROCESS_QUERY_LIMITED_INFORMATION,
            };
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if handle.is_null() {
                return Err(format!(
                    "open exact protocol Python child: {}",
                    std::io::Error::last_os_error()
                ));
            }
            unsafe { OwnedHandle::from_raw_handle(handle.cast()) }
        };
        Ok(ProtocolChild {
            pid,
            root: self.root.clone(),
            #[cfg(windows)]
            process,
        })
    }
}

pub(super) struct ProtocolChildStartGate {
    root: PathBuf,
}

impl ProtocolChildStartGate {
    pub(super) fn observe_and_release(self, helper: &ResetHelper) -> Result<ProtocolChild, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.root.join("child.json").is_file() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        let child = helper.observe_child();
        // Drop always releases, including failed observation and test unwind.
        drop(self);
        child
    }
}

impl Drop for ProtocolChildStartGate {
    fn drop(&mut self) {
        let _ = std::fs::write(
            self.root.join("release-child-start"),
            b"release original child",
        );
    }
}
impl Drop for ResetHelper {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("retained protocol helper evidence: {}", self.root.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
pub(super) fn reset_config() -> PluginPrepareConfig {
    PluginPrepareConfig {
        sample_rate: 48_000.0,
        max_block_frames: 2048,
    }
}
pub(super) fn kind(value: &serde_json::Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| value.as_object().unwrap().keys().next().unwrap())
}

pub(super) struct ProtocolChild {
    pid: u32,
    root: PathBuf,
    #[cfg(windows)]
    process: std::os::windows::io::OwnedHandle,
}

impl ProtocolChild {
    pub(super) fn assert_clean_shutdown(&self) {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use winapi::um::processthreadsapi::GetExitCodeProcess;
            let mut code = 259;
            assert_ne!(
                unsafe { GetExitCodeProcess(self.process.as_raw_handle().cast(), &mut code) },
                0,
                "query exact protocol Python process: {}",
                std::io::Error::last_os_error()
            );
            assert_eq!(
                code, 0,
                "the observed Python process must actually exit 0, not remain STILL_ACTIVE"
            );
        }
        let record: serde_json::Value = serde_json::from_slice(
            &std::fs::read(self.root.join("child-exit.json"))
                .expect("helper normal script exit record"),
        )
        .unwrap();
        assert_eq!(
            record,
            serde_json::json!({"pid":self.pid,"shutdown":true,"exit_code":0})
        );
    }
}

/// ASCII-only, synchronous test launcher. Rust's Windows Command supports .cmd;
/// production still launches its supplied helper path through PluginHostProcess.
/// No CALL/START, command arguments from data, or background descendants are used.
#[cfg(windows)]
fn windows_launcher(root: &std::path::Path) -> PathBuf {
    if let Some(executable) = std::env::var_os("CITRUS_STRICT_TEST_PYTHON") {
        let executable = executable
            .to_str()
            .expect("test Python executable must be Unicode");
        assert!(
            !executable.is_empty()
                && !executable
                    .chars()
                    .any(|ch| matches!(ch, '"' | '\r' | '\n' | '\0')),
            "CITRUS_STRICT_TEST_PYTHON must be a single executable path, not a command line"
        );
    }
    let launcher = root.join("helper.cmd");
    std::fs::write(&launcher, concat!(
        "@echo off\r\nsetlocal DisableDelayedExpansion\r\n",
        "if defined CITRUS_STRICT_TEST_PYTHON goto configured\r\n",
        "python -u \"%~dp0helper.py\"\r\nexit /b %errorlevel%\r\n",
        ":configured\r\n\"%CITRUS_STRICT_TEST_PYTHON%\" -u \"%~dp0helper.py\"\r\nexit /b %errorlevel%\r\n",
    )).unwrap();
    launcher
}
