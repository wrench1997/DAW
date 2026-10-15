//! Real bounded admin queues and worker receipts through ordinary App polling; no DLL/device.
use super::*;
use crate::plugins::plugin_runtime::{
    AudioThreadEndpoint, BackendSlot, PluginBackend, PluginChain, PluginPrepareConfig,
    ShutdownOutcome,
};
use std::{
    collections::BTreeMap,
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[derive(Default)]
struct Gate {
    entered: AtomicBool,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Gate {
    fn wait(&self) {
        self.entered.store(true, Ordering::SeqCst);
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

#[derive(Default)]
struct Probe {
    values: Mutex<BTreeMap<u32, f32>>,
    sets: AtomicUsize,
    saves: AtomicUsize,
    query_gate: Gate,
    first_set_gate: Gate,
    block_first_set: bool,
    reject: Option<u32>,
}

struct RestoreBackend(Arc<Probe>);
impl PluginBackend for RestoreBackend {
    fn name(&self) -> &str {
        "generic restore fixture"
    }
    fn prepare(&mut self, _: PluginPrepareConfig) -> Result<(), String> {
        Ok(())
    }
    fn process(&mut self, _: &mut [f32], _: &mut [f32], _: usize) -> Result<(), String> {
        Ok(())
    }
    fn send_midi(&mut self, _: plugins::plugin_runtime::MidiMessage) -> Result<(), String> {
        Ok(())
    }
    fn set_parameter(&mut self, id: u32, value: f32) -> Result<(), String> {
        let previous = self.0.sets.fetch_add(1, Ordering::SeqCst);
        if previous == 0 && self.0.block_first_set {
            self.0.first_set_gate.wait();
        }
        if self.0.reject == Some(id) {
            return Err(format!("fixture rejects base {id}"));
        }
        self.0.values.lock().unwrap().insert(id, value);
        Ok(())
    }
    fn get_parameter(&mut self, id: u32) -> Result<f32, String> {
        if id == u32::MAX {
            self.0.query_gate.wait();
        }
        Ok(*self.0.values.lock().unwrap().get(&id).unwrap_or(&0.0))
    }
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        self.0.saves.fetch_add(1, Ordering::SeqCst);
        Ok(vec![9, 8, 7])
    }
    fn load_state(&mut self, _: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn latency_samples(&self) -> u32 {
        0
    }
    fn tail_samples(&self) -> u32 {
        0
    }
}

struct Fixture {
    app: Box<CitrusApp>,
    audio: Vec<AudioThreadEndpoint>,
    probes: Vec<Arc<Probe>>,
    channel: u32,
    instance: u64,
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "restore fixture did not reach its actual receipt"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

impl Fixture {
    fn new(count: u32, block_first_set: bool, reject: Option<u32>) -> Self {
        let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        let mut app = CitrusApp::new_boxed_with_services(&cc, false);
        assert!(app.audio.is_none());
        app.project = Project::blank();
        let instance = commit_loaded_plugin(
            &mut app.project,
            PluginPickerTarget::ChannelDevice { channel: 0 },
            &PluginDescriptor {
                id: "restore-fixture".into(),
                name: "Restore fixture".into(),
                vendor: "TEST".into(),
                path: PathBuf::from("/not-a-plugin/restore.so"),
                format: ScannedPluginFormat::Vst2,
                category: "Instrument".into(),
                is_instrument: true,
                verified: false,
                vst3_metadata: None,
                vst3_catalog: None,
                scan_error: None,
            },
        )
        .unwrap();
        app.project.plugin_instances[0].parameters =
            (0..count).map(|id| (id, (id % 10) as f32 / 10.0)).collect();
        app.project.plugin_instances[0].opaque_state = vec![42];
        app.mark_saved();
        app.undo_stack.clear();
        app.redo_stack.clear();
        let channel = app.project.channels[0].id;
        let mut fixture = Self {
            app,
            audio: Vec::new(),
            probes: Vec::new(),
            channel,
            instance,
        };
        fixture.attach(901, block_first_set, reject);
        fixture
    }

    fn attach(&mut self, endpoint_id: u64, block_first_set: bool, reject: Option<u32>) {
        let probe = Arc::new(Probe {
            block_first_set,
            reject,
            ..Default::default()
        });
        let backend_probe = probe.clone();
        let PluginChain {
            audio,
            control,
            guard,
        } = PluginChain::spawn_identified_with_backend_factory(
            &[self.instance],
            move || vec![BackendSlot::new(Box::new(RestoreBackend(backend_probe)))],
            PluginPrepareConfig {
                sample_rate: 48_000.0,
                max_block_frames: 128,
            },
        )
        .unwrap();
        if let Some(old) = self.app.running_generator_chains.remove(&self.channel) {
            for old_probe in &self.probes {
                old_probe.query_gate.release();
                old_probe.first_set_gate.release();
            }
            assert_eq!(
                old.guard.shutdown_blocking(Duration::from_secs(3)),
                ShutdownOutcome::Joined
            );
        }
        self.app.running_generator_chains.insert(
            self.channel,
            RunningGeneratorChain {
                project_session: self.app.project_session,
                endpoint_id,
                instance_id: self.instance,
                mixer_track: 1,
                control,
                guard,
                readiness: GeneratorReadiness::default(),
            },
        );
        self.audio.push(audio);
        self.probes.push(probe);
    }

    fn control(&self) -> &PluginChainControl {
        &self.app.running_generator_chains[&self.channel].control
    }
    fn probe(&self) -> &Arc<Probe> {
        self.probes.last().unwrap()
    }
    fn start(&mut self) {
        wait_until(|| {
            self.app.poll_plugin_runtime_events();
            !self.app.plugin_initializations.is_empty()
                || self.app.project.plugin_instances[0].runtime_status
                    == PluginRuntimeStatus::Loaded
        });
    }
    fn finish(&mut self) {
        wait_until(|| {
            self.app.poll_plugin_runtime_events();
            self.app.plugin_initializations.is_empty()
                && self.app.project.plugin_instances[0].runtime_status
                    == PluginRuntimeStatus::Loaded
        });
    }
    fn saturate(&self) {
        assert!(self.control().query_parameter_tagged(0, u32::MAX, 700));
        wait_until(|| self.probe().query_gate.entered.load(Ordering::SeqCst));
        for _ in 0..64 {
            assert!(self.control().set_slot_config(0, SlotConfig::default()));
        }
        assert!(!self.control().set_slot_config(0, SlotConfig::default()));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for probe in &self.probes {
            probe.query_gate.release();
            probe.first_set_gate.release();
        }
        self.audio.clear();
        for (_, chain) in self.app.running_generator_chains.drain() {
            let joined = chain.guard.shutdown_blocking(Duration::from_secs(3));
            if !std::thread::panicking() {
                assert_eq!(joined, ShutdownOutcome::Joined);
            }
        }
        for (_, chain) in self.app.running_insert_chains.drain() {
            let joined = chain.guard.shutdown_blocking(Duration::from_secs(3));
            if !std::thread::panicking() {
                assert_eq!(joined, ShutdownOutcome::Joined);
            }
        }
    }
}

#[test]
fn generic_restore_full_admin_queue_retains_one_and_130_bases_until_exact_acks() {
    for count in [1, 130] {
        let mut f = Fixture::new(count, false, None);
        let before = project_fingerprint(&f.app.project);
        f.saturate();
        assert!(
            f.app.plugin_topology_is_settling(),
            "Installed before Ready must also wait for persisted bases"
        );
        assert!(!f.app.plugin_runtime_is_ready_for_transport());
        f.start();
        assert_eq!(f.probe().sets.load(Ordering::SeqCst), 0);
        assert_eq!(
            f.app.project.plugin_instances[0].runtime_status,
            PluginRuntimeStatus::Unloaded
        );
        assert!(f.app.plugin_topology_is_settling());
        assert!(!f.app.plugin_runtime_is_ready_for_transport());
        assert!(
            !f.app.plugin_initialization_failed(),
            "queue pressure is not a plug-in fault"
        );
        f.probe().query_gate.release();
        f.finish();
        assert_eq!(
            *f.probe().values.lock().unwrap(),
            f.app.project.plugin_instances[0].parameters
        );
        assert_eq!(f.probe().sets.load(Ordering::SeqCst), count as usize);
        assert_eq!(project_fingerprint(&f.app.project), before);
        assert!(!f.app.dirty);
        assert!(f.app.undo_stack.is_empty());
        assert!(f.app.redo_stack.is_empty());
        assert!(f.app.plugin_runtime_is_ready_for_transport());
    }
}

#[test]
fn generic_restore_backend_rejection_latches_failure_blocks_save_and_can_reload() {
    let mut f = Fixture::new(5, false, Some(2));
    wait_until(|| {
        f.app.poll_plugin_runtime_events();
        f.app.plugin_initialization_failed()
    });
    assert_eq!(f.probe().sets.load(Ordering::SeqCst), 3);
    assert_eq!(
        f.app.project.plugin_instances[0].runtime_status,
        PluginRuntimeStatus::Crashed
    );
    assert!(!f.app.plugin_runtime_is_ready_for_transport());
    assert!(
        !f.app.plugin_topology_is_settling(),
        "the explicit reload controls remain usable"
    );
    let path = std::env::temp_dir().join(format!(
        "citrus-rejected-restore-{}-{}.citrus",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    f.app.begin_project_save(ProjectSaveRequest::Manual {
        path: path.clone(),
        lifecycle: None,
    });
    f.app.poll_save_barrier();
    assert!(f.app.save_barrier.is_none());
    assert!(
        !path.exists(),
        "a partial restore must not be captured as a clean saved project"
    );
    for _ in 0..4 {
        f.app.poll_plugin_runtime_events();
    }
    assert_eq!(
        f.probe().sets.load(Ordering::SeqCst),
        3,
        "rejected commands are not automatically repeated"
    );
    let bases = f.app.project.plugin_instances[0].parameters.clone();
    f.attach(902, false, None);
    f.finish();
    assert_eq!(*f.probe().values.lock().unwrap(), bases);
}

#[test]
fn generic_restore_wrong_session_endpoint_slot_and_tag_cannot_ack_a_base() {
    let mut f = Fixture::new(1, true, None);
    f.start();
    wait_until(|| f.probe().first_set_gate.entered.load(Ordering::SeqCst));
    let request_id = f.app.next_plugin_parameter_request_id - 1;
    let endpoint = ParameterEndpoint {
        kind: ParameterEndpointKind::Generator,
        id: 901,
    };
    for case in 0..4 {
        let session = f.app.project_session + u64::from(case == 0);
        let endpoint = ParameterEndpoint {
            id: endpoint.id + u64::from(case == 1),
            ..endpoint
        };
        let event = RuntimeEvent::ParameterSetAck {
            slot: usize::from(case == 2),
            request_id: request_id + u64::from(case == 3),
            id: 0,
            value: 0.0,
        };
        f.app
            .observe_plugin_initialization(session, endpoint, &[f.instance], &event);
        f.app.drive_plugin_initializations(true);
        assert_eq!(
            f.app.project.plugin_instances[0].runtime_status,
            PluginRuntimeStatus::Unloaded,
            "case {case}"
        );
    }
    f.probe().first_set_gate.release();
    f.finish();
    assert_eq!(f.probe().sets.load(Ordering::SeqCst), 1);
}

#[test]
fn generic_restore_timeout_keeps_worker_owner_and_late_ack_cannot_publish_loaded() {
    let mut f = Fixture::new(1, true, None);
    f.start();
    wait_until(|| f.probe().first_set_gate.entered.load(Ordering::SeqCst));
    let worker_id = f.app.running_generator_chains[&f.channel]
        .guard
        .genuine_worker_id();
    f.app
        .plugin_initializations
        .values_mut()
        .next()
        .unwrap()
        .last_progress = Instant::now() - AUDIO_RESTART_RESTORE_TIMEOUT;
    f.app.poll_plugin_runtime_events();
    assert!(f.app.plugin_initialization_failed());
    assert_eq!(
        f.app.running_generator_chains[&f.channel]
            .guard
            .genuine_worker_id(),
        worker_id
    );
    f.probe().first_set_gate.release();
    wait_until(|| !f.control().has_outstanding_admin_work());
    f.app.poll_plugin_runtime_events();
    assert_eq!(
        f.app.project.plugin_instances[0].runtime_status,
        PluginRuntimeStatus::Crashed
    );
    assert!(!f.app.plugin_runtime_is_ready_for_transport());
    assert_eq!(f.probe().sets.load(Ordering::SeqCst), 1);
}

#[test]
fn generic_restore_empty_bases_keep_legacy_ready_and_initial_state_contract() {
    let mut f = Fixture::new(0, false, None);
    f.app.project.plugin_instances[0].opaque_state.clear();
    f.saturate();
    f.start();
    assert_eq!(
        f.app.project.plugin_instances[0].runtime_status,
        PluginRuntimeStatus::Loaded
    );
    assert!(f.app.plugin_initializations.is_empty());
    assert_eq!(f.probe().sets.load(Ordering::SeqCst), 0);
    assert_eq!(f.probe().saves.load(Ordering::SeqCst), 0);
    f.probe().query_gate.release();
}

#[test]
fn generic_restore_initial_state_queue_backpressure_does_not_lose_the_request() {
    let mut f = Fixture::new(1, false, None);
    f.app.project.plugin_instances[0].opaque_state.clear();
    f.start();
    // The first Set is ahead of this blocking query; its ACK is already queued
    // when App next polls, but all 64 admin queue positions remain occupied.
    f.saturate();
    f.app.poll_plugin_runtime_events();
    assert!(f.app.plugin_initializations_pending());
    assert_eq!(
        f.app.project.plugin_instances[0].runtime_status,
        PluginRuntimeStatus::Unloaded
    );
    assert_eq!(f.probe().saves.load(Ordering::SeqCst), 0);
    f.probe().query_gate.release();
    f.finish();
    wait_until(|| {
        f.app.poll_plugin_runtime_events();
        f.app.project.plugin_instances[0].opaque_state == vec![9, 8, 7]
    });
    assert_eq!(f.probe().saves.load(Ordering::SeqCst), 1);
    assert_eq!(f.probe().sets.load(Ordering::SeqCst), 1);
}

#[test]
fn generic_restore_project_replacement_cancels_old_replay_without_using_late_ack() {
    let mut f = Fixture::new(1, true, None);
    f.start();
    wait_until(|| f.probe().first_set_gate.entered.load(Ordering::SeqCst));
    let old_session = f.app.project_session;
    assert!(f.app.install_project(Project::blank(), None, false));
    assert_ne!(f.app.project_session, old_session);
    assert!(f.app.plugin_initializations.is_empty());
    let before = project_fingerprint(&f.app.project);
    f.probe().first_set_gate.release();
    wait_until(|| !f.control().has_outstanding_admin_work());
    f.app.poll_plugin_runtime_events();
    assert!(f.app.project.plugin_instances.is_empty());
    assert_eq!(project_fingerprint(&f.app.project), before);
    assert!(f.app.plugin_initializations.is_empty());
}

#[test]
fn generic_restore_insert_slots_wait_for_each_slots_own_ack() {
    let mut f = Fixture::new(2, false, None);
    let old = f.app.running_generator_chains.remove(&f.channel).unwrap();
    assert_eq!(
        old.guard.shutdown_blocking(Duration::from_secs(3)),
        ShutdownOutcome::Joined
    );
    f.app.project.channels[0].instrument_plugin_instance_id = None;
    f.app.project.plugin_instances[0].role = PluginRole::Effect;
    let mut second = f.app.project.plugin_instances[0].clone();
    second.id += 1;
    second.parameters.insert(0, 0.75);
    let ids = vec![f.instance, second.id];
    f.app.project.plugin_instances.push(second);
    let track = f.app.project.mixer_track_id_at_runtime_slot(1).unwrap();
    f.app.project.mixer_insert_slots = ids
        .iter()
        .enumerate()
        .map(|(slot, &plugin_instance_id)| MixerInsertSlotRef {
            track,
            slot,
            plugin_instance_id,
        })
        .collect();
    let probes = (0..2)
        .map(|_| {
            Arc::new(Probe {
                block_first_set: true,
                ..Default::default()
            })
        })
        .collect::<Vec<_>>();
    let backend_probes = probes.clone();
    let PluginChain {
        audio,
        control,
        guard,
    } = PluginChain::spawn_identified_with_backend_factory(
        &ids,
        move || {
            backend_probes
                .into_iter()
                .map(|probe| BackendSlot::new(Box::new(RestoreBackend(probe))))
                .collect()
        },
        PluginPrepareConfig {
            sample_rate: 48_000.0,
            max_block_frames: 128,
        },
    )
    .unwrap();
    f.audio.push(audio);
    f.probes.extend(probes.iter().cloned());
    f.app.running_insert_chains.insert(
        1,
        RunningInsertChain {
            project_session: f.app.project_session,
            endpoint_id: 903,
            instance_ids: ids,
            control,
            guard,
        },
    );
    wait_until(|| {
        f.app.poll_plugin_runtime_events();
        f.app.plugin_initializations.len() == 2
    });
    wait_until(|| {
        probes
            .iter()
            .any(|probe| probe.first_set_gate.entered.load(Ordering::SeqCst))
    });
    let first = usize::from(!probes[0].first_set_gate.entered.load(Ordering::SeqCst));
    let second = 1 - first;
    assert!(
        f.app
            .project
            .plugin_instances
            .iter()
            .all(|instance| instance.runtime_status == PluginRuntimeStatus::Unloaded)
    );
    probes[first].first_set_gate.release();
    wait_until(|| probes[second].first_set_gate.entered.load(Ordering::SeqCst));
    f.app.poll_plugin_runtime_events();
    assert_eq!(
        f.app.project.plugin_instances[second].runtime_status,
        PluginRuntimeStatus::Unloaded
    );
    probes[second].first_set_gate.release();
    wait_until(|| {
        f.app.poll_plugin_runtime_events();
        f.app.plugin_initializations.is_empty()
    });
    for (instance, probe) in f.app.project.plugin_instances.iter().zip(probes) {
        assert_eq!(instance.runtime_status, PluginRuntimeStatus::Loaded);
        assert_eq!(*probe.values.lock().unwrap(), instance.parameters);
        assert_eq!(probe.sets.load(Ordering::SeqCst), 2);
    }
}
