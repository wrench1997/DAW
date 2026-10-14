//! Explicit stereo routing for the locally qualified Kontakt factory class.
//!
//! This is a realtime host choice, not a reduced component declaration. Bounce and
//! recreated-source admission continue to inspect all declared audio/event buses.

use vst3_host::plugin::{CheckedAudioBusLayout, EventBusLayout};

const KONTAKT_UID: &str = "5653544E694B386B6F6E74616B742038";
const OUTPUT_BUSES: usize = 32;

pub(super) fn eligible(
    uid: &str,
    version: &str,
    category: &str,
    fresh_bounce: bool,
    restoration: bool,
) -> bool {
    !fresh_bounce
        && !restoration
        && uid == KONTAKT_UID
        && version == "8.4.1"
        && category.split('|').any(|part| part.trim() == "Instrument")
}

/// The complete event declaration/activation must survive our audio-only choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    events: EventBusLayout,
}

/// Shared with the production backend so tests exercise the SDK call ordering and
/// failure boundaries, rather than a presentation model of the selected output.
pub(super) trait Access {
    fn is_processing(&self) -> bool;
    /// Service pending I/O/latency at the stopped owner boundary; return reload request.
    fn service_stopped_requests(&mut self) -> Result<bool, String>;
    fn checked_audio(&self) -> Result<CheckedAudioBusLayout, String>;
    fn events(&self) -> Result<EventBusLayout, String>;
    fn set_output_active(&mut self, index: i32, active: bool) -> Result<(), String>;
    fn start(&mut self) -> Result<(), String>;
    fn stop(&mut self) -> Result<(), String>;
    fn load_state(&mut self, state: &[u8]) -> Result<(), String>;
}

fn declared_audio(layout: &CheckedAudioBusLayout) -> Result<(), String> {
    if !layout.inputs.is_empty()
        || layout.outputs.len() != OUTPUT_BUSES
        || layout
            .outputs
            .iter()
            .any(|bus| bus.channel_count != 2 || !bus.is_main)
    {
        return Err(
            "Kontakt realtime output selection requires the complete declared topology: zero audio inputs and 32 main stereo outputs"
                .into(),
        );
    }
    Ok(())
}

fn checked_events(events: &EventBusLayout) -> Result<(), String> {
    if events.inputs.len() > 32
        || events.outputs.len() > 32
        || events
            .inputs
            .iter()
            .chain(&events.outputs)
            .any(|bus| !(1..=16).contains(&bus.channel_count) || bus.active.is_none())
    {
        return Err(
            "Kontakt realtime output selection requires known complete event-bus activation".into(),
        );
    }
    Ok(())
}

pub(super) fn verify<A: Access>(access: &A, selection: &Selection) -> Result<(), String> {
    let layout = access.checked_audio().map_err(|error| {
        format!("Kontakt realtime checked audio topology verification failed: {error}")
    })?;
    declared_audio(&layout)?;
    for (index, bus) in layout.outputs.iter().enumerate() {
        if bus.active != Some(index == 0) {
            return Err(format!(
                "Kontakt realtime output {index} activation mismatch: expected {}, observed {:?}",
                index == 0,
                bus.active
            ));
        }
    }
    let events = access
        .events()
        .map_err(|error| format!("Kontakt realtime event topology verification failed: {error}"))?;
    checked_events(&events)?;
    if events != selection.events {
        return Err("Kontakt realtime complete event-bus declaration or activation changed".into());
    }
    Ok(())
}

pub(super) fn configure_stopped<A: Access>(
    access: &mut A,
    previous: Option<&Selection>,
) -> Result<Selection, String> {
    if access.is_processing() {
        return Err("Kontakt realtime output selection requires stopped processing".into());
    }
    let reload = access.service_stopped_requests().map_err(|error| {
        format!("Kontakt realtime stopped host-request service failed: {error}")
    })?;
    if reload {
        return Err("Kontakt requested component reload during stopped output selection; recreate the source before processing".into());
    }
    if access.is_processing() {
        return Err("Kontakt stopped host-request service unexpectedly resumed processing".into());
    }
    // PendingTopology and SDK query failures are errors. No activation is inferred
    // from defaults and no setter runs until the entire declaration is checked.
    let layout = access.checked_audio().map_err(|error| {
        format!("Kontakt realtime checked audio topology preflight failed: {error}")
    })?;
    declared_audio(&layout)?;
    let events = access
        .events()
        .map_err(|error| format!("Kontakt realtime event topology preflight failed: {error}"))?;
    checked_events(&events)?;
    if previous.is_some_and(|selection| selection.events != events) {
        return Err(
            "Kontakt realtime complete event topology changed before output selection".into(),
        );
    }
    let selection = Selection { events };
    for index in 0..OUTPUT_BUSES {
        access
            .set_output_active(index as i32, index == 0)
            .map_err(|error| {
                format!("Kontakt realtime output {index} SDK activation failed: {error}")
            })?;
    }
    verify(access, &selection)?;
    Ok(selection)
}

pub(super) fn stop_after_failure<A: Access>(
    access: &mut A,
    error: String,
    cleanup_attempted: &mut bool,
) -> String {
    if !*cleanup_attempted && access.is_processing() {
        // Mark before the SDK call. A rejected Stop is still the one bounded
        // cleanup attempt; an outer failure handler must not silently retry it.
        *cleanup_attempted = true;
        if let Err(cleanup) = access.stop() {
            return format!("{error}; Kontakt processing-stop cleanup failed: {cleanup}");
        }
    }
    error
}

pub(super) fn start_verified<A: Access>(
    access: &mut A,
    selection: &Selection,
    cleanup_attempted: &mut bool,
) -> Result<(), String> {
    let result = access.start().and_then(|()| verify(access, selection));
    result.map_err(|error| stop_after_failure(access, error, cleanup_attempted))
}

/// Ordinary prepared state restore and the stopped Retry boundary use this same
/// transaction. No failed LoadState, query or SDK activation reaches Start.
pub(super) fn restart<A: Access>(
    access: &mut A,
    previous: Option<&Selection>,
    state: Option<&[u8]>,
    stop_first: bool,
    cleanup_attempted: &mut bool,
) -> Result<Selection, String> {
    if stop_first {
        access.stop()?;
    }
    if let Some(state) = state {
        access.load_state(state)?;
    }
    let selection = configure_stopped(access, previous)?;
    start_verified(access, &selection, cleanup_attempted)?;
    Ok(selection)
}

impl Access for super::Vst3Backend {
    fn is_processing(&self) -> bool {
        self.plugin.is_processing()
    }

    fn service_stopped_requests(&mut self) -> Result<bool, String> {
        self.plugin
            .service_host_requests()
            .map(|flags| flags.reload_component())
            .map_err(|error| error.to_string())
    }

    fn checked_audio(&self) -> Result<CheckedAudioBusLayout, String> {
        self.plugin
            .checked_audio_bus_layout()
            .map_err(|error| error.to_string())
    }

    fn events(&self) -> Result<EventBusLayout, String> {
        self.plugin
            .event_bus_layout()
            .map_err(|error| error.to_string())
    }

    fn set_output_active(&mut self, index: i32, active: bool) -> Result<(), String> {
        self.plugin
            .set_bus_active(
                vst3_host::MediaType::Audio,
                vst3_host::BusDirection::Output,
                index,
                active,
            )
            .map_err(|error| error.to_string())
    }

    fn start(&mut self) -> Result<(), String> {
        #[cfg(test)]
        self.observation.call(
            super::Operation::StartProcessing,
            super::Boundary::Attempt,
            None,
            Some(self.last_transport),
            None,
        );
        let result = self
            .plugin
            .start_processing()
            .map_err(|error| error.to_string());
        #[cfg(test)]
        self.observation.string_result(
            super::Operation::StartProcessing,
            None,
            Some(self.last_transport),
            &result,
        );
        result
    }

    fn stop(&mut self) -> Result<(), String> {
        #[cfg(test)]
        self.observation.call(
            super::Operation::StopProcessing,
            super::Boundary::Attempt,
            None,
            Some(self.last_transport),
            None,
        );
        let result = self
            .plugin
            .stop_processing()
            .map_err(|error| error.to_string());
        #[cfg(test)]
        self.observation.string_result(
            super::Operation::StopProcessing,
            None,
            Some(self.last_transport),
            &result,
        );
        result
    }

    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        let result = self
            .plugin
            .load_state(state)
            .map_err(|error| error.to_string());
        #[cfg(test)]
        self.observation.state_load(state, &result);
        result
    }
}

#[cfg(test)]
#[path = "kontakt_realtime_outputs_tests.rs"]
mod tests;
