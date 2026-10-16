//! Plugin-free DSP/control tests. Real SDK PCM proof has its own opt-in exact.
use super::super::generator_controls::GeneratorControls;
use super::*;
use crate::{
    automation::{
        AutomationCurve, AutomationLane, AutomationLoop, AutomationPoint, AutomationTarget,
    },
    instrument_bounce::GeneratorAutomationCatalog,
    plugins::plugin_runtime::PluginParameterDescriptor,
};

fn descriptor(id: u32) -> PluginParameterDescriptor {
    PluginParameterDescriptor {
        id,
        name: format!("P{id}"),
        unit: String::new(),
        current_normalized: 0.9,
        default_normalized: Some(0.5),
        step_count: Some(0),
        automatable: true,
        read_only: false,
        bypass: false,
    }
}
fn lane(p: &mut Project, id: u64, curve: AutomationCurve, points: &[(u64, f64)]) {
    let mut value = AutomationLane::new(AutomationTarget::PluginParameter {
        instance: 41,
        parameter: 17,
    });
    value.set_curve(curve);
    value.replace_points(
        points
            .iter()
            .map(|&(f, v)| AutomationPoint::new(f as f64 / 4000.0, v)),
    );
    p.automation_lanes.push(ProjectAutomation {
        id,
        name: format!("Q128 {id}"),
        lane: value,
    });
}
fn placement(p: &mut Project, lane: u64, start: u64, end: u64, slip: u64) {
    let mut clip = p.clips[0].clone();
    clip.id = 500 + lane as u32;
    clip.kind = ClipKind::Automation;
    clip.automation_id = Some(lane);
    clip.start = start as f32 / 4000.0;
    clip.length = (end - start) as f32 / 4000.0;
    clip.source_offset = slip as f32 / 4000.0;
    p.clips.push(clip);
}
fn automated_capture(p: &Project, n: u64) -> CapturedInstrumentBounce {
    let mut captured = capture(n, 256, 32, 2);
    captured.plan =
        InstrumentBouncePlan::prepare_master_song(p, p.channels[0].id, 8000, 256).unwrap();
    assert_eq!(captured.plan.timeline().duration_frames(), n);
    captured.identity.project_fingerprint = captured.plan.project_fingerprint();
    captured.plan.generator_automation_catalog = Some(GeneratorAutomationCatalog {
        revision: 9,
        parameters: vec![descriptor(17)],
    });
    captured.state.parameter_bases.insert(17, 0.9); // Deliberately different from Project .625.
    captured
}
fn project(n: u64) -> Project {
    let mut p = fixture_project(n);
    p.piano_notes.clear();
    p.plugin_instances[0].parameters.insert(17, 0.625);
    p
}

struct RenderParameterBackend {
    metadata: BouncePluginMetadata,
    gain: f32,
    mode: u32,
    changed: bool,
    control: ExportControl,
    log: Arc<Mutex<Vec<&'static str>>>,
}
impl RenderBackend for RenderParameterBackend {
    fn prepare(
        &mut self,
        _: &CapturedInstrumentBounce,
        _: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        self.log.lock().unwrap().push("prepare");
        Ok(self.metadata.clone())
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        let mut metadata = self.metadata.clone();
        if self.changed {
            metadata.latency_samples += 1;
        }
        Ok(metadata)
    }
    fn set_automated_parameters(&mut self, values: &[(u32, f32)]) -> Result<(), BackendFailure> {
        self.log.lock().unwrap().push("parameters");
        if self.mode == 3 || self.mode == 4 {
            self.control.cancel();
        }
        if self.mode == 1 || self.mode == 4 {
            return Err(BackendFailure::from("actual setter failure"));
        }
        self.changed = self.mode == 2;
        self.gain = values[0].1;
        Ok(())
    }
    fn send_midi(&mut self, _: [u8; 3], _: u16) -> Result<(), BackendFailure> {
        self.log.lock().unwrap().push("MIDI");
        Ok(())
    }
    fn process(&mut self, _: PluginTransport, frames: usize) -> Result<BlockReply, BackendFailure> {
        self.log.lock().unwrap().push("process");
        Ok(BlockReply {
            channels: vec![vec![self.gain; frames]; 2],
            transport_applied: true,
            output_events: 0,
            output_events_lost: false,
        })
    }
    fn dispose(&mut self, _: Instant) -> DisposalObservation {
        self.log.lock().unwrap().push("dispose");
        DisposalObservation {
            terminated: true,
            error: None,
            diagnostics: vec![],
        }
    }
}
impl Drop for RenderParameterBackend {
    fn drop(&mut self) {
        self.log.lock().unwrap().push("drop");
    }
}

#[test]
fn generator_q128_renderer_orders_parameters_before_midi_process_and_disposes_every_failure() {
    for mode in 0..5 {
        let mut p = project(256);
        lane(&mut p, 1, AutomationCurve::Hold, &[(0, 0.25), (127, 0.75)]);
        let mut captured = automated_capture(&p, 256);
        // This fixture checks command order/cleanup, not a claimed lookahead DSP.
        captured.state.metadata.latency_samples = 0;
        let files = Files::new();
        let path = files.output();
        std::fs::write(&path, b"unchanged").unwrap();
        let before = serde_json::to_vec(&p).unwrap();
        let ctl = control();
        let backend_control = ctl.export.clone();
        let log = Arc::new(Mutex::new(Vec::new()));
        let worker_log = Arc::clone(&log);
        let metadata = captured.metadata().clone();
        let admission = RenderAdmission::default();
        let receipt = admission
            .start_master_song_mock(
                p.clone(),
                captured,
                WavExportOptions {
                    sample_rate: 8000,
                    sample_format: WavSampleFormat::Float32,
                    level_policy: crate::export_options::WavLevelPolicy::PreserveLevel,
                    tail_milliseconds: 32,
                },
                path.clone(),
                ctl,
                true,
                move || RenderParameterBackend {
                    metadata,
                    gain: 0.9,
                    mode,
                    changed: false,
                    control: backend_control,
                    log: worker_log,
                },
            )
            .unwrap();
        wait_until(|| receipt.termination_acknowledged() && !admission.is_busy());
        let outcome = receipt.try_recv().unwrap();
        let log = log.lock().unwrap();
        assert_eq!(log[log.len() - 2..], ["dispose", "drop"]);
        assert_eq!(serde_json::to_vec(&p).unwrap(), before);
        if mode == 0 {
            assert!(matches!(outcome, MasterSongRenderOutcome::Complete(_)));
            let parameter = log.iter().position(|s| *s == "parameters").unwrap();
            let midi = log.iter().position(|s| *s == "MIDI").unwrap();
            let process = log.iter().position(|s| *s == "process").unwrap();
            assert!(parameter < midi && midi < process);
            assert_eq!(log.iter().filter(|s| **s == "parameters").count(), 2);
        } else {
            let MasterSongRenderOutcome::Failed(failure) = outcome else {
                panic!("failed setter published WAV")
            };
            assert_eq!(
                failure.kind,
                match mode {
                    2 => FailureKind::Invalid,
                    3 => FailureKind::Cancelled,
                    _ => FailureKind::Backend,
                }
            );
            assert!(!log.iter().any(|s| *s == "MIDI" || *s == "process"));
            assert_eq!(std::fs::read(&path).unwrap(), b"unchanged");
        }
    }
}
#[derive(Default)]
struct ParameterBackend {
    writes: Vec<Vec<(u32, f32)>>,
    fail: bool,
}
impl RenderBackend for ParameterBackend {
    fn prepare(
        &mut self,
        _: &CapturedInstrumentBounce,
        _: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure> {
        panic!("cursor does not prepare")
    }
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure> {
        panic!("cursor does not query DSP")
    }
    fn send_midi(&mut self, _: [u8; 3], _: u16) -> Result<(), BackendFailure> {
        panic!("cursor does not emit MIDI")
    }
    fn process(&mut self, _: PluginTransport, _: usize) -> Result<BlockReply, BackendFailure> {
        panic!("cursor does not process")
    }
    fn dispose(&mut self, _: Instant) -> DisposalObservation {
        panic!("cursor does not own cleanup")
    }
    fn set_automated_parameters(&mut self, values: &[(u32, f32)]) -> Result<(), BackendFailure> {
        self.writes.push(values.to_vec());
        if self.fail {
            Err(BackendFailure::from("actual parameter fixture rejection"))
        } else {
            Ok(())
        }
    }
}
fn values(capture: &CapturedInstrumentBounce) -> Vec<f32> {
    let mut cursor = GeneratorControls::new(capture).unwrap().unwrap();
    let mut backend = ParameterBackend::default();
    for frame in (0..capture.plan.timeline().duration_frames() + 384).step_by(128) {
        cursor
            .apply(capture.plan.timeline(), frame, &mut backend, &control())
            .unwrap();
    }
    backend
        .writes
        .iter()
        .map(|v| {
            assert_eq!(v.len(), 1);
            assert_eq!(v[0].0, 17);
            v[0].1
        })
        .collect()
}

#[test]
fn generator_q128_controls_use_block_start_not_intraquantum_points_and_restore_project_base() {
    let mut p = project(768);
    lane(
        &mut p,
        1,
        AutomationCurve::Hold,
        &[(0, 0.25), (127, 0.75), (130, 0.5)],
    );
    placement(&mut p, 1, 0, 385, 0);
    assert_eq!(
        values(&automated_capture(&p, 768)),
        [0.25, 0.75, 0.5, 0.5, 0.625, 0.625]
    );
}

#[test]
fn generator_q128_controls_preserve_overlaps_slip_and_loop_layers() {
    let mut p = project(768);
    lane(&mut p, 1, AutomationCurve::Hold, &[(0, 0.2)]);
    placement(&mut p, 1, 0, 650, 0);
    lane(
        &mut p,
        2,
        AutomationCurve::Hold,
        &[(0, 0.8), (125, 0.4), (250, 0.8)],
    );
    p.automation_lanes[1]
        .lane
        .set_loop_region(Some(AutomationLoop::new(0.0, 250.0 / 4000.0).unwrap()))
        .unwrap();
    // Source offsets/loop lengths are exact binary beats (.03125/.0625).
    // Near-equal f32 placement and f64 authored boundaries otherwise legitimately
    // produce the compiler's AutomationRangeCollapsed refusal, not a valid loop.
    placement(&mut p, 2, 128, 503, 125);
    let actual = values(&automated_capture(&p, 768));
    assert_eq!(actual.len(), 6);
    for (a, b) in actual.iter().zip([0.2, 0.4, 0.8, 0.4, 0.2, 0.2]) {
        assert!((*a - b).abs() < 1e-6, "{actual:?}");
    }
}

#[test]
fn generator_q128_controls_use_compiled_tension_at_quantum_boundaries() {
    let mut p = project(4352);
    lane(
        &mut p,
        1,
        AutomationCurve::Tension,
        &[(0, 0.0), (4096, 1.0)],
    );
    p.automation_lanes[0].lane.replace_points([
        AutomationPoint::with_tension(0.0, 0.0, 0.5),
        AutomationPoint::new(4096.0 / 4000.0, 1.0),
    ]);
    // The production compiler approximates Tension with bounded linear spans
    // of at most2048 frames. A shorter curve's analytic midpoint is not the
    // compiled/RT value. Cross two spans to distinguish Tension from one ramp.
    let captured = automated_capture(&p, 4352);
    let actual = values(&captured);
    assert_eq!(actual.len(), 34);
    assert!((actual[16] - 0.0625).abs() < 1e-5);
    for (i, a) in actual.iter().enumerate() {
        let frame = (i * 128) as f32;
        let expected = if frame <= 2048.0 {
            0.0625 * frame / 2048.0
        } else if frame <= 4096.0 {
            0.0625 + 0.9375 * (frame - 2048.0) / 2048.0
        } else {
            1.0
        };
        assert!((*a - expected).abs() < 1e-5, "Qframe={frame}: {actual:?}");
    }
}

#[test]
fn generator_q128_tail_never_synthesizes_n_minus_one_or_layer_end_setters() {
    for (n, end) in [(255, 254), (256, 256), (257, 257)] {
        let mut p = project(n);
        lane(&mut p, 1, AutomationCurve::Linear, &[(0, 0.0), (512, 1.0)]);
        placement(&mut p, 1, 0, end, 0);
        let actual = values(&automated_capture(&p, n));
        assert_eq!(actual.len(), (n as usize).div_ceil(128));
        for (i, value) in actual.iter().enumerate() {
            assert!((*value - (i * 128) as f32 / 512.0).abs() < 1e-5);
        }
    }
}

#[test]
fn generator_q128_missing_captured_catalog_or_base_and_nonautomatable_metadata_refuse() {
    let mut p = project(256);
    lane(&mut p, 1, AutomationCurve::Hold, &[(0, 0.5)]);
    for case in 0..5 {
        let mut captured = automated_capture(&p, 256);
        match case {
            0 => captured.plan.generator_automation_catalog = None,
            1 => {
                captured.state.parameter_bases.clear();
            }
            2 => {
                captured
                    .plan
                    .generator_automation_catalog
                    .as_mut()
                    .unwrap()
                    .parameters[0]
                    .read_only = true
            }
            3 => {
                captured
                    .plan
                    .generator_automation_catalog
                    .as_mut()
                    .unwrap()
                    .parameters[0]
                    .automatable = false
            }
            _ => {
                captured
                    .plan
                    .generator_automation_catalog
                    .as_mut()
                    .unwrap()
                    .parameters[0]
                    .id = 99
            }
        }
        assert!(GeneratorControls::new(&captured).is_err(), "case{case}");
    }
}

#[test]
fn generator_q128_setter_failure_cancel_deadline_and_frame_order_stop_before_more_writes() {
    let mut p = project(256);
    lane(&mut p, 1, AutomationCurve::Hold, &[(0, 0.5)]);
    for case in 0..4 {
        let captured = automated_capture(&p, 256);
        let mut cursor = GeneratorControls::new(&captured).unwrap().unwrap();
        let mut backend = ParameterBackend {
            fail: case == 0,
            ..Default::default()
        };
        let mut control = control();
        if case == 1 {
            control.export.cancel();
        }
        if case == 2 {
            control.deadline = Instant::now();
        }
        let result = cursor.apply(
            captured.plan.timeline(),
            if case == 3 { 128 } else { 0 },
            &mut backend,
            &control,
        );
        assert!(result.is_err(), "case{case}");
        assert_eq!(backend.writes.len(), usize::from(case == 0));
    }
}
