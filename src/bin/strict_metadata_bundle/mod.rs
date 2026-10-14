//! One closed owner operation for the reviewed selected Kontakt metadata path.
//! Every accepted call invokes the four original getters afresh, in order.
use super::Plugin;
use vst3_host::{
    Error, Result,
    plugin::{CheckedAudioBusLayout, EventBusLayout, PluginInfo},
    process_isolation::{
        HostResponse, MetadataGetter, SelectedMetadataAdmission, SelectedMetadataPolicy,
        StrictMetadataSnapshot,
    },
};

trait MetadataOwner {
    fn actual_info(&self) -> &PluginInfo;
    fn checked_audio(&self) -> Result<CheckedAudioBusLayout>;
    fn events(&self) -> Result<EventBusLayout>;
    fn latency(&self) -> Result<u32>;
    fn tail(&self) -> Result<u32>;
}
impl MetadataOwner for Plugin {
    fn actual_info(&self) -> &PluginInfo {
        self.info()
    }
    fn checked_audio(&self) -> Result<CheckedAudioBusLayout> {
        self.checked_audio_bus_layout()
    }
    fn events(&self) -> Result<EventBusLayout> {
        self.event_bus_layout()
    }
    fn latency(&self) -> Result<u32> {
        self.try_latency_samples()
    }
    fn tail(&self) -> Result<u32> {
        self.try_tail_samples()
    }
}

fn actual_policy_matches(info: &PluginInfo, policy: SelectedMetadataPolicy) -> bool {
    match policy {
        SelectedMetadataPolicy::Kontakt841SelectedStereo => {
            info.uid == "5653544E694B386B6F6E74616B742038"
                && info.version == "8.4.1"
                && info
                    .category
                    .split('|')
                    .any(|token| token.trim() == "Instrument")
        }
    }
}
enum Failure {
    Getter {
        getter: MetadataGetter,
        error: Error,
    },
    Admission(SelectedMetadataAdmission),
}
fn queried<T>(value: Result<T>, getter: MetadataGetter) -> std::result::Result<T, Failure> {
    value.map_err(|error| Failure::Getter { getter, error })
}
fn read_owner(owner: &impl MetadataOwner, policy: SelectedMetadataPolicy) -> HostResponse {
    if !actual_policy_matches(owner.actual_info(), policy) {
        return HostResponse::StrictMetadataAdmissionRejected {
            stage: SelectedMetadataAdmission::FactoryIdentity,
        };
    }
    let complete = (|| {
        let audio = queried(owner.checked_audio(), MetadataGetter::CheckedAudioBusLayout)?;
        let events = queried(owner.events(), MetadataGetter::EventBusLayout)?;
        // The same predicate is used by the backend. Preserve the original
        // early event-input guard before invoking latency or tail.
        if !events.admits_selected_bounce_input() {
            return Err(Failure::Admission(SelectedMetadataAdmission::EventInput));
        }
        let latency_samples = queried(owner.latency(), MetadataGetter::LatencySamples)?;
        let tail_samples = queried(owner.tail(), MetadataGetter::TailSamples)?;
        Ok(StrictMetadataSnapshot {
            audio,
            events,
            latency_samples,
            tail_samples,
        })
    })();
    match complete {
        Ok(snapshot) => HostResponse::StrictMetadataSnapshot { snapshot },
        Err(Failure::Admission(stage)) => HostResponse::StrictMetadataAdmissionRejected { stage },
        Err(Failure::Getter { getter, error }) => match error {
            Error::MetadataQueryRejected(reason) => {
                HostResponse::StrictMetadataRejected { getter, reason }
            }
            error => HostResponse::StrictMetadataFailed {
                getter,
                message: format!("{}: {error}", getter.label()),
            },
        },
    }
}
pub(super) fn read(plugin: &Plugin, policy: SelectedMetadataPolicy) -> HostResponse {
    read_owner(plugin, policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use vst3_host::{
        MetadataQueryRejection,
        plugin::{CheckedAudioBusConfig, EventBusConfig},
    };
    struct Owner {
        info: PluginInfo,
        calls: RefCell<Vec<MetadataGetter>>,
        fail: Option<MetadataGetter>,
        rejected: bool,
        event_active: Option<bool>,
        latency: Cell<u32>,
    }
    impl Owner {
        fn new() -> Self {
            Self {
                info: PluginInfo {
                    path: "mock-only.vst3".into(),
                    name: "Kontakt".into(),
                    vendor: "Native Instruments".into(),
                    version: "8.4.1".into(),
                    category: "Instrument|Sampler".into(),
                    uid: "5653544E694B386B6F6E74616B742038".into(),
                    audio_inputs: 0,
                    audio_outputs: 32,
                    has_midi_input: true,
                    has_midi_output: true,
                    has_gui: true,
                },
                calls: RefCell::new(Vec::new()),
                fail: None,
                rejected: false,
                event_active: Some(true),
                latency: Cell::new(0),
            }
        }
        fn call(&self, getter: MetadataGetter) -> Result<()> {
            self.calls.borrow_mut().push(getter);
            if self.fail == Some(getter) {
                if self.rejected {
                    Err(Error::MetadataQueryRejected(
                        MetadataQueryRejection::PendingTopology,
                    ))
                } else {
                    Err(Error::Other("actual getter failed".into()))
                }
            } else {
                Ok(())
            }
        }
    }
    impl MetadataOwner for Owner {
        fn actual_info(&self) -> &PluginInfo {
            &self.info
        }
        fn checked_audio(&self) -> Result<CheckedAudioBusLayout> {
            self.call(MetadataGetter::CheckedAudioBusLayout)?;
            Ok(CheckedAudioBusLayout {
                inputs: Vec::new(),
                outputs: (0..32)
                    .map(|index| CheckedAudioBusConfig {
                        channel_count: 2,
                        is_main: index == 0,
                        active: Some(index == 0),
                    })
                    .collect(),
            })
        }
        fn events(&self) -> Result<EventBusLayout> {
            self.call(MetadataGetter::EventBusLayout)?;
            Ok(EventBusLayout {
                inputs: vec![EventBusConfig {
                    channel_count: 16,
                    is_main: true,
                    active: self.event_active,
                }],
                outputs: vec![EventBusConfig {
                    channel_count: 16,
                    is_main: true,
                    active: Some(false),
                }],
            })
        }
        fn latency(&self) -> Result<u32> {
            self.call(MetadataGetter::LatencySamples)?;
            Ok(self.latency.get())
        }
        fn tail(&self) -> Result<u32> {
            self.call(MetadataGetter::TailSamples)?;
            Ok(u32::MAX)
        }
    }
    const POLICY: SelectedMetadataPolicy = SelectedMetadataPolicy::Kontakt841SelectedStereo;
    const ORDER: [MetadataGetter; 4] = [
        MetadataGetter::CheckedAudioBusLayout,
        MetadataGetter::EventBusLayout,
        MetadataGetter::LatencySamples,
        MetadataGetter::TailSamples,
    ];

    #[test]
    fn selected_bundle_request_policy_and_payload_are_closed() {
        let command: vst3_host::process_isolation::HostCommand = serde_json::from_str(
            r#"{"StrictMetadataSnapshot":{"policy":"Kontakt841SelectedStereo"}}"#,
        )
        .unwrap();
        assert!(matches!(
            command,
            vst3_host::process_isolation::HostCommand::StrictMetadataSnapshot(_)
        ));
        for invalid in [
            r#"{"StrictMetadataSnapshot":{"policy":"Generic"}}"#,
            r#"{"StrictMetadataSnapshot":{"policy":"Kontakt841SelectedStereo","disable_validation":true}}"#,
            r#"{"StrictMetadataSnapshot":{"policy":"Kontakt841SelectedStereo","policy":"Kontakt841SelectedStereo"}}"#,
            r#"{"StrictMetadataSnapshot":{}}"#,
        ] {
            assert!(
                serde_json::from_str::<vst3_host::process_isolation::HostCommand>(invalid).is_err()
            );
        }
    }

    #[test]
    fn selected_bundle_owner_fresh_order_and_complete_inactive_bus_fields() {
        let owner = Owner::new();
        for latency in [0, 17] {
            owner.latency.set(latency);
            let HostResponse::StrictMetadataSnapshot { snapshot } = read_owner(&owner, POLICY)
            else {
                panic!("expected complete actual getter result")
            };
            assert_eq!(snapshot.audio.outputs.len(), 32);
            assert_eq!(snapshot.audio.outputs[31].active, Some(false));
            assert_eq!(snapshot.events.outputs[0].active, Some(false));
            assert_eq!(snapshot.latency_samples, latency);
            assert_eq!(snapshot.tail_samples, u32::MAX);
        }
        assert_eq!(*owner.calls.borrow(), ORDER.repeat(2));
    }
    #[test]
    fn selected_bundle_owner_actual_factory_gate_runs_no_getter() {
        for field in [0, 1, 2] {
            let mut owner = Owner::new();
            match field {
                0 => owner.info.uid = "project-path-id".into(),
                1 => owner.info.version = "8.4.2".into(),
                _ => owner.info.category = "Fx".into(),
            }
            assert!(matches!(
                read_owner(&owner, POLICY),
                HostResponse::StrictMetadataAdmissionRejected {
                    stage: SelectedMetadataAdmission::FactoryIdentity
                }
            ));
            assert!(owner.calls.borrow().is_empty());
        }
    }
    #[test]
    fn selected_bundle_owner_event_admission_precedes_failing_later_getter() {
        for active in [None, Some(false)] {
            let mut owner = Owner::new();
            owner.event_active = active;
            owner.fail = Some(MetadataGetter::LatencySamples);
            assert!(matches!(
                read_owner(&owner, POLICY),
                HostResponse::StrictMetadataAdmissionRejected {
                    stage: SelectedMetadataAdmission::EventInput
                }
            ));
            assert_eq!(*owner.calls.borrow(), ORDER[..2]);
        }
    }
    #[test]
    fn selected_bundle_owner_each_actual_failure_preserves_first_getter() {
        for (index, getter) in ORDER.into_iter().enumerate() {
            let mut owner = Owner::new();
            owner.fail = Some(getter);
            assert!(
                matches!(read_owner(&owner, POLICY), HostResponse::StrictMetadataFailed { getter: observed, message } if observed == getter && message == format!("{}: actual getter failed", getter.label()))
            );
            assert_eq!(*owner.calls.borrow(), ORDER[..=index]);
        }
    }
    #[test]
    fn selected_bundle_owner_typed_sdk_refusal_preserved() {
        let mut owner = Owner::new();
        owner.fail = Some(MetadataGetter::EventBusLayout);
        owner.rejected = true;
        assert!(matches!(
            read_owner(&owner, POLICY),
            HostResponse::StrictMetadataRejected {
                getter: MetadataGetter::EventBusLayout,
                reason: MetadataQueryRejection::PendingTopology
            }
        ));
        assert_eq!(*owner.calls.borrow(), ORDER[..2]);
    }
    #[test]
    fn selected_bundle_event_input_predicate_exact_boundary_matrix() {
        for count in [0, 1, 2] {
            for channels in [0, 1, 16, 17] {
                for main in [false, true] {
                    for active in [None, Some(false), Some(true)] {
                        let layout = EventBusLayout {
                            inputs: vec![
                                EventBusConfig {
                                    channel_count: channels,
                                    is_main: main,
                                    active
                                };
                                count
                            ],
                            outputs: Vec::new(),
                        };
                        assert_eq!(
                            layout.admits_selected_bounce_input(),
                            count == 1
                                && (1..=16).contains(&channels)
                                && main
                                && active == Some(true)
                        );
                    }
                }
            }
        }
    }
}
