//! Real bounded helper exchanges; mock PCM is not SDK/DSP evidence.
use super::*;
use crate::{
    automation::{AutomationLane, AutomationPoint, AutomationTarget},
    instrument_bounce::GeneratorAutomationCatalog,
    model::ProjectAutomation,
    plugins::plugin_runtime::PluginParameterDescriptor,
};

fn automated_capture(mock: &Mock) -> (Project, CapturedInstrumentBounce) {
    let path = mock.root.join("config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["generator_automation"] = json!(true);
    fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
    let (mut project, mut capture) = mock.capture();
    project.piano_notes.clear();
    for id in [17, 23] {
        let mut lane = AutomationLane::new(AutomationTarget::PluginParameter {
            instance: 41,
            parameter: id,
        });
        lane.replace_points([AutomationPoint::new(0.0, 0.5)]);
        project.automation_lanes.push(ProjectAutomation {
            id: u64::from(id),
            name: format!("Q128 {id}"),
            lane,
        });
    }
    let mut plan =
        InstrumentBouncePlan::prepare_master_song(&project, project.channels[0].id, 8000, 0)
            .unwrap();
    plan.spec.vst3_helper_path = Some(mock.root.join("helper.py"));
    plan.generator_automation_catalog = Some(GeneratorAutomationCatalog {
        revision: 7,
        parameters: [17, 23]
            .map(|id| PluginParameterDescriptor {
                id,
                name: format!("base {id}"),
                unit: String::new(),
                current_normalized: 0.5,
                default_normalized: Some(0.25),
                step_count: Some(0),
                automatable: true,
                read_only: false,
                bypass: false,
            })
            .into(),
    });
    capture.identity.project_fingerprint = plan.project_fingerprint();
    capture.plan = plan;
    (project, capture)
}

#[test]
fn strict_generator_q128_batch_writes_then_reads_every_parameter_before_any_dsp() {
    let _serial = serial();
    let mock = Mock::new("", "");
    let (project, capture) = automated_capture(&mock);
    let before = serde_json::to_vec(&project).unwrap();
    {
        let mut owner = DirectOwner::new();
        owner
            .backend
            .prepare(&capture, &control(Duration::from_secs(20)))
            .unwrap();
        owner
            .backend
            .set_automated_parameters(&[(17, 0.2), (23, 0.6)])
            .unwrap();
    }
    let commands = mock.commands();
    let at = |stage: &str| commands.iter().position(|s| s == stage).unwrap();
    assert!(at("SetParameter:3") < at("SetParameter:4"));
    assert!(at("SetParameter:4") < at("GetParameter:3"));
    assert!(at("GetParameter:3") < at("GetParameter:4"));
    assert!(!commands.iter().any(|s| s.starts_with("Process:")));
    assert_eq!(serde_json::to_vec(&project).unwrap(), before);
    mock.assert_cooperative(true);
    mock.assert_unchanged();
}

#[test]
fn strict_generator_q128_cross_parameter_side_effect_cannot_hide_behind_setter_ack() {
    let _serial = serial();
    let mock = Mock::new("automation_cross_parameter", "SetParameter:4");
    let (_, capture) = automated_capture(&mock);
    {
        let mut owner = DirectOwner::new();
        owner
            .backend
            .prepare(&capture, &control(Duration::from_secs(20)))
            .unwrap();
        let failure = owner
            .backend
            .set_automated_parameters(&[(17, 0.2), (23, 0.6)])
            .unwrap_err();
        assert_eq!(failure.kind, BackendFailureKind::Backend);
        assert!(
            failure
                .diagnostic()
                .contains("parameter 17 readback differs"),
            "{failure:?}"
        );
    }
    let commands = mock.commands();
    assert!(commands.iter().any(|s| s == "SetParameter:4"));
    assert!(commands.iter().any(|s| s == "GetParameter:3"));
    assert!(
        !commands
            .iter()
            .any(|s| s == "GetParameter:4" || s.starts_with("Process:"))
    );
    mock.assert_cooperative(true);
    mock.assert_unchanged();
}

#[test]
fn strict_generator_q128_catalog_drift_prevents_writes_or_rejects_completed_batch() {
    let _serial = serial();
    for (fault, stage, writes) in [
        ("catalog_readonly", "GetAllParameters:3", false),
        ("catalog_not_automatable", "GetAllParameters:3", false),
        ("catalog_missing", "GetAllParameters:4", true),
    ] {
        let mock = Mock::new(fault, stage);
        let (_, capture) = automated_capture(&mock);
        {
            let mut owner = DirectOwner::new();
            owner
                .backend
                .prepare(&capture, &control(Duration::from_secs(20)))
                .unwrap();
            let failure = owner
                .backend
                .set_automated_parameters(&[(17, 0.2), (23, 0.6)])
                .unwrap_err();
            assert_eq!(failure.kind, BackendFailureKind::Backend);
            assert!(
                failure.diagnostic().contains("catalog changed"),
                "{failure:?}"
            );
        }
        let commands = mock.commands();
        assert_eq!(commands.iter().any(|s| s == "SetParameter:3"), writes);
        assert!(!commands.iter().any(|s| s.starts_with("Process:")));
        mock.assert_cooperative(true);
        mock.assert_unchanged();
    }
}

#[test]
fn strict_generator_q128_setter_fault_seals_exchange_and_cancellation_adds_no_write() {
    let _serial = serial();
    for cancel in [false, true] {
        let mock = Mock::new(if cancel { "" } else { "error" }, "SetParameter:3");
        let (_, capture) = automated_capture(&mock);
        let ctl = control(Duration::from_secs(20));
        {
            let mut owner = DirectOwner::new();
            owner.backend.prepare(&capture, &ctl).unwrap();
            if cancel {
                ctl.export.cancel();
            }
            let failure = owner
                .backend
                .set_automated_parameters(&[(17, 0.2), (23, 0.6)])
                .unwrap_err();
            assert_eq!(
                failure.kind,
                if cancel {
                    BackendFailureKind::Cancelled
                } else {
                    BackendFailureKind::Backend
                }
            );
            if !cancel {
                assert_eq!(
                    failure.strict_stage,
                    Some("render.generator_automation.set_parameter")
                );
                assert_eq!(failure.strict_kind, Some(StrictKind::OperationRejected));
            }
        }
        let commands = mock.commands();
        assert_eq!(commands.iter().any(|s| s == "SetParameter:3"), !cancel);
        assert!(
            !commands.iter().any(|s| s == "SetParameter:4"
                || s == "GetParameter:3"
                || s.starts_with("Process:"))
        );
        mock.assert_reaped();
        mock.assert_unchanged();
    }
}
