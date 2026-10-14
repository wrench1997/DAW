use super::*;
use std::cell::{Cell, RefCell};
use vst3_host::plugin::{CheckedAudioBusConfig, EventBusConfig};

fn restart<A: Access>(
    access: &mut A,
    previous: Option<&Selection>,
    state: Option<&[u8]>,
    stop_first: bool,
) -> Result<Selection, String> {
    let mut cleanup_attempted = false;
    super::restart(access, previous, state, stop_first, &mut cleanup_attempted)
}

#[derive(Clone, Copy, Default)]
enum StartChange {
    #[default]
    None,
    Reactivate,
    StereoWidth,
    EventActivation,
}

struct FakeAccess {
    audio: CheckedAudioBusLayout,
    event_layout: EventBusLayout,
    processing: bool,
    pending_io: bool,
    service_clears_io: bool,
    reload: bool,
    fail_action: Option<&'static str>,
    fail_audio_query: Option<usize>,
    fail_event_query: Option<usize>,
    ignore_activation: Option<usize>,
    start_change: StartChange,
    fail_cleanup: bool,
    audio_queries: Cell<usize>,
    event_queries: Cell<usize>,
    calls: RefCell<Vec<String>>,
}

impl Default for FakeAccess {
    fn default() -> Self {
        let events = vec![EventBusConfig {
            channel_count: 16,
            is_main: true,
            active: Some(true),
        }];
        Self {
            audio: CheckedAudioBusLayout {
                inputs: Vec::new(),
                outputs: vec![
                    CheckedAudioBusConfig {
                        channel_count: 2,
                        is_main: true,
                        active: Some(true),
                    };
                    32
                ],
            },
            event_layout: EventBusLayout {
                inputs: events.clone(),
                outputs: events,
            },
            processing: false,
            pending_io: false,
            service_clears_io: true,
            reload: false,
            fail_action: None,
            fail_audio_query: None,
            fail_event_query: None,
            ignore_activation: None,
            start_change: StartChange::None,
            fail_cleanup: false,
            audio_queries: Cell::new(0),
            event_queries: Cell::new(0),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl FakeAccess {
    fn log(&self, action: impl Into<String>) {
        self.calls.borrow_mut().push(action.into());
    }

    fn reject(&self, action: &str) -> Result<(), String> {
        if self.fail_action == Some(action) {
            Err(format!("SDK rejected {action}"))
        } else {
            Ok(())
        }
    }

    fn trace(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn no_start(&self) {
        assert!(!self.processing);
        assert!(!self.trace().iter().any(|call| call == "start"));
    }
}

impl Access for FakeAccess {
    fn is_processing(&self) -> bool {
        self.processing
    }

    fn service_stopped_requests(&mut self) -> Result<bool, String> {
        self.log("service");
        assert!(!self.processing);
        self.reject("service")?;
        if self.service_clears_io {
            self.pending_io = false;
        }
        Ok(self.reload)
    }

    fn checked_audio(&self) -> Result<CheckedAudioBusLayout, String> {
        self.log("checked_audio");
        let query = self.audio_queries.get() + 1;
        self.audio_queries.set(query);
        if self.pending_io {
            return Err("PendingTopology".into());
        }
        if self.fail_audio_query == Some(query) {
            return Err(format!("SDK audio query {query} failed"));
        }
        Ok(self.audio.clone())
    }

    fn events(&self) -> Result<EventBusLayout, String> {
        self.log("events");
        let query = self.event_queries.get() + 1;
        self.event_queries.set(query);
        if self.fail_event_query == Some(query) {
            return Err(format!("SDK event query {query} failed"));
        }
        Ok(self.event_layout.clone())
    }

    fn set_output_active(&mut self, index: i32, active: bool) -> Result<(), String> {
        assert!(!self.processing);
        let action = format!("set:{index}:{active}");
        self.log(action.clone());
        self.reject(&action)?;
        if self.ignore_activation != Some(index as usize) {
            self.audio.outputs[index as usize].active = Some(active);
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        self.log("start");
        self.reject("start")?;
        self.processing = true;
        match self.start_change {
            StartChange::None => {}
            StartChange::Reactivate => self.audio.outputs[1].active = Some(true),
            StartChange::StereoWidth => self.audio.outputs[0].channel_count = 4,
            StartChange::EventActivation => self.event_layout.outputs[0].active = None,
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.log("stop");
        if self.fail_cleanup && self.processing {
            return Err("SDK stop cleanup failed".into());
        }
        self.reject("stop")?;
        self.processing = false;
        Ok(())
    }

    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        self.log(format!("load:{}", state.len()));
        assert!(!self.processing);
        self.reject("load")?;
        self.pending_io = true;
        for bus in &mut self.audio.outputs {
            bus.active = Some(true);
        }
        Ok(())
    }
}

#[test]
fn exact_factory_identity_and_separate_bounce_restoration_fences() {
    assert!(eligible(
        KONTAKT_UID,
        "8.4.1",
        "Instrument|Sampler",
        false,
        false
    ));
    for (uid, version, category, fresh, restoration) in [
        ("other", "8.4.1", "Instrument", false, false),
        (KONTAKT_UID, "8.4.0", "Instrument", false, false),
        (KONTAKT_UID, "8.4.1", "Fx", false, false),
        (KONTAKT_UID, "8.4.1", "InstrumentFake", false, false),
        (KONTAKT_UID, "8.4.1", "Instrument", true, false),
        (KONTAKT_UID, "8.4.1", "Instrument", false, true),
    ] {
        assert!(!eligible(uid, version, category, fresh, restoration));
        // This is the production eligibility gate: other classes and every
        // Pending/Consumed/Pinned/restoration source issue no selector commands.
        let mut access = FakeAccess::default();
        if eligible(uid, version, category, fresh, restoration) {
            restart(&mut access, None, None, false).unwrap();
        }
        assert!(access.trace().is_empty());
    }
}

#[test]
fn pending_state_is_serviced_then_all_outputs_are_sdk_selected_before_start() {
    let mut access = FakeAccess::default();
    let original_events = access.event_layout.clone();
    let selection = restart(&mut access, None, Some(&[1, 2, 3]), false).unwrap();
    let trace = access.trace();
    assert_eq!(
        &trace[..4],
        ["load:3", "service", "checked_audio", "events"]
    );
    assert_eq!(trace[4], "set:0:true");
    for index in 1..32 {
        assert_eq!(trace[index + 4], format!("set:{index}:false"));
    }
    assert_eq!(
        &trace[36..],
        [
            "checked_audio",
            "events",
            "start",
            "checked_audio",
            "events"
        ]
    );
    assert_eq!(access.audio.outputs.len(), 32);
    assert!(
        access
            .audio
            .outputs
            .iter()
            .all(|bus| bus.is_main && bus.channel_count == 2)
    );
    assert_eq!(access.event_layout, original_events);
    assert_eq!(selection.events, original_events);
    assert!(access.processing);
}

#[test]
fn prepared_load_and_stopped_retry_share_selection_without_extra_process() {
    let mut access = FakeAccess::default();
    let first = restart(&mut access, None, None, false).unwrap();
    access.calls.borrow_mut().clear();
    let restored = restart(&mut access, Some(&first), Some(&[8, 9]), true).unwrap();
    assert_eq!(
        &access.trace()[..5],
        ["stop", "load:2", "service", "checked_audio", "events"]
    );
    assert_eq!(first, restored);
    // Production reset retains panic and ResetOrigin before this stopped boundary.
    access.stop().unwrap();
    access.calls.borrow_mut().clear();
    restart(&mut access, Some(&restored), None, false).unwrap();
    assert_eq!(&access.trace()[..3], ["service", "checked_audio", "events"]);
    assert!(
        !access
            .trace()
            .iter()
            .any(|call| call.starts_with("load") || call == "stop")
    );
    assert_eq!(
        access
            .trace()
            .iter()
            .filter(|call| *call == "start")
            .count(),
        1
    );
}

#[test]
fn pending_topology_and_unsupported_reload_never_reach_activation_or_start() {
    for (service_clears_io, reload, fragment) in [
        (false, false, "PendingTopology"),
        (true, true, "component reload"),
    ] {
        let mut access = FakeAccess {
            pending_io: true,
            service_clears_io,
            reload,
            ..FakeAccess::default()
        };
        let error = restart(&mut access, None, None, false).unwrap_err();
        assert!(error.contains(fragment), "{error}");
        assert!(!access.trace().iter().any(|call| call.starts_with("set:")));
        access.no_start();
    }
}

#[test]
fn full_topology_and_known_events_are_checked_before_any_setter() {
    for scenario in 0..5 {
        let mut access = FakeAccess::default();
        match scenario {
            0 => access.audio.inputs.push(access.audio.outputs[0].clone()),
            1 => {
                access.audio.outputs.pop();
            }
            2 => access.audio.outputs[31].channel_count = 1,
            3 => access.audio.outputs[31].is_main = false,
            _ => access.event_layout.outputs[0].active = None,
        }
        assert!(restart(&mut access, None, None, false).is_err());
        assert!(!access.trace().iter().any(|call| call.starts_with("set:")));
        access.no_start();
    }
}

#[test]
fn sdk_pre_and_post_query_failures_and_rejected_setter_do_not_start() {
    for (audio_query, event_query, action, expected) in [
        (Some(1), None, None, "SDK audio query 1 failed"),
        (None, Some(1), None, "SDK event query 1 failed"),
        (Some(2), None, None, "SDK audio query 2 failed"),
        (None, Some(2), None, "SDK event query 2 failed"),
        (
            None,
            None,
            Some("set:12:false"),
            "SDK rejected set:12:false",
        ),
        (None, None, Some("service"), "SDK rejected service"),
    ] {
        let mut access = FakeAccess {
            fail_audio_query: audio_query,
            fail_event_query: event_query,
            fail_action: action,
            ..FakeAccess::default()
        };
        let error = restart(&mut access, None, None, false).unwrap_err();
        assert!(error.contains(expected), "{error}");
        access.no_start();
    }
}

#[test]
fn false_ack_or_unknown_activation_is_never_inferred_from_sample_shape() {
    for original_active in [Some(true), None] {
        let mut access = FakeAccess {
            ignore_activation: Some(13),
            ..FakeAccess::default()
        };
        access.audio.outputs[13].active = original_active;
        let error = restart(&mut access, None, None, false).unwrap_err();
        assert!(error.contains("output 13 activation mismatch"), "{error}");
        access.no_start();
    }
}

#[test]
fn post_start_drift_or_sdk_query_failure_stops_actual_processing() {
    for start_change in [
        StartChange::Reactivate,
        StartChange::StereoWidth,
        StartChange::EventActivation,
    ] {
        let mut access = FakeAccess {
            start_change,
            ..FakeAccess::default()
        };
        assert!(restart(&mut access, None, None, false).is_err());
        assert!(!access.processing);
        assert_eq!(access.trace().last().unwrap(), "stop");
    }
    for (audio, event) in [(Some(3), None), (None, Some(3))] {
        let mut access = FakeAccess {
            fail_audio_query: audio,
            fail_event_query: event,
            ..FakeAccess::default()
        };
        assert!(restart(&mut access, None, None, false).is_err());
        assert!(!access.processing);
        assert_eq!(access.trace().last().unwrap(), "stop");
    }
}

#[test]
fn cleanup_failure_is_returned_with_original_topology_error() {
    let mut access = FakeAccess {
        start_change: StartChange::Reactivate,
        fail_cleanup: true,
        ..FakeAccess::default()
    };
    let mut cleanup_attempted = false;
    let error = super::restart(&mut access, None, None, false, &mut cleanup_attempted).unwrap_err();
    assert!(cleanup_attempted);
    // The production outer failure handler uses this same status. Even a Stop
    // failure cannot trigger another cleanup attempt or any default resume.
    let error = stop_after_failure(&mut access, error, &mut cleanup_attempted);
    assert_eq!(
        access.trace().iter().filter(|call| *call == "stop").count(),
        1
    );
    assert!(error.contains("output 1 activation mismatch"), "{error}");
    assert!(
        error.contains("processing-stop cleanup failed: SDK stop cleanup failed"),
        "{error}"
    );
}

#[test]
fn failure_before_start_still_gets_one_necessary_outer_cleanup() {
    let mut access = FakeAccess {
        processing: true,
        ..FakeAccess::default()
    };
    let mut cleanup_attempted = false;
    let error = super::restart(&mut access, None, None, false, &mut cleanup_attempted).unwrap_err();
    assert!(error.contains("requires stopped processing"));
    assert!(!cleanup_attempted);
    assert!(access.trace().is_empty());
    let error = stop_after_failure(&mut access, error, &mut cleanup_attempted);
    assert!(error.contains("requires stopped processing"));
    assert!(cleanup_attempted);
    assert_eq!(access.trace(), ["stop"]);
    assert!(!access.processing);
}

#[test]
fn rejected_load_state_never_consumes_selection_or_restarts() {
    let mut access = FakeAccess::default();
    let selection = restart(&mut access, None, None, false).unwrap();
    access.calls.borrow_mut().clear();
    access.fail_action = Some("load");
    let error = restart(&mut access, Some(&selection), Some(&[4]), true).unwrap_err();
    assert!(error.contains("SDK rejected load"));
    assert_eq!(access.trace(), ["stop", "load:1"]);
    access.no_start();
}

#[test]
fn feedback_verification_is_read_only_and_rejects_unknown_or_reactivated_output() {
    let mut access = FakeAccess::default();
    let selection = restart(&mut access, None, None, false).unwrap();
    for active in [Some(true), None] {
        access.audio.outputs[31].active = active;
        access.calls.borrow_mut().clear();
        assert!(verify(&access, &selection).is_err());
        assert_eq!(access.trace(), ["checked_audio"]);
        assert!(access.processing);
    }
}

#[test]
fn event_descriptor_drift_before_reselection_stops_without_changing_buses() {
    let mut access = FakeAccess::default();
    let selection = restart(&mut access, None, None, false).unwrap();
    access.event_layout.outputs[0].is_main = false;
    access.calls.borrow_mut().clear();
    let error = restart(&mut access, Some(&selection), None, true).unwrap_err();
    assert!(error.contains("complete event topology changed"), "{error}");
    assert_eq!(
        access.trace(),
        ["stop", "service", "checked_audio", "events"]
    );
    access.no_start();
}
