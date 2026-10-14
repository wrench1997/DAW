//! Fresh, unpublished bounce-source participation. This is deliberately separate
//! from ordinary realtime Kontakt routing and keeps every declared audio/event bus.

use super::{BouncePluginMetadata, Vst3Backend};
use vst3_host::audio::{AudioBusLayout, BusDirection, MediaType};

trait Access {
    fn processing(&self) -> bool;
    fn service_stopped(&mut self) -> Result<bool, String>;
    fn actual(&self) -> Result<BouncePluginMetadata, String>;
    fn storage(&self) -> Result<AudioBusLayout, String>;
    fn acknowledge(
        &mut self,
        direction: BusDirection,
        index: usize,
        active: bool,
    ) -> Result<(), String>;
}

impl Access for Vst3Backend {
    fn processing(&self) -> bool {
        self.plugin.is_processing()
    }
    fn service_stopped(&mut self) -> Result<bool, String> {
        self.plugin
            .service_host_requests()
            .map(|flags| flags.reload_component())
            .map_err(|error| error.to_string())
    }
    fn actual(&self) -> Result<BouncePluginMetadata, String> {
        self.bounce_metadata(None)
            .map_err(|error| error.to_string())
    }
    fn storage(&self) -> Result<AudioBusLayout, String> {
        self.plugin
            .audio_bus_layout()
            .map_err(|error| error.to_string())
    }
    fn acknowledge(
        &mut self,
        direction: BusDirection,
        index: usize,
        active: bool,
    ) -> Result<(), String> {
        self.plugin
            .set_bus_active(MediaType::Audio, direction, index as i32, active)
            .map_err(|error| error.to_string())
    }
}

pub(super) fn configure_stopped(
    backend: &mut Vst3Backend,
    sample_rate: u32,
) -> Result<BouncePluginMetadata, String> {
    configure(backend, sample_rate)
}

fn configure<A: Access>(access: &mut A, sample_rate: u32) -> Result<BouncePluginMetadata, String> {
    if access.processing() {
        return Err("Fresh bounce stereo selection requires stopped processing".into());
    }
    if access.service_stopped()? || access.processing() {
        return Err(
            "Fresh bounce stereo selection encountered a component reload or resumed processing"
                .into(),
        );
    }
    let mut actual = access.actual()?;
    if !actual.uses_selected_stereo_bounce() {
        return Err("Fresh bounce selected-output factory identity is not qualified".into());
    }
    let mut storage = access.storage()?;
    for (declared, prepared) in [
        (&actual.input_buses, &storage.inputs),
        (&actual.output_buses, &storage.outputs),
    ] {
        if declared.len() != prepared.len()
            || declared.iter().zip(prepared).any(|(bus, prepared)| {
                bus.channel_count != prepared.channel_count
                    || bus.active.is_some_and(|active| active != prepared.active)
            })
        {
            return Err(
                "Fresh bounce complete SDK declaration differs from prepared storage".into(),
            );
        }
    }
    // This anticipated participation is only a preflight proposal. It becomes
    // authoritative exclusively after each owner setter and full actual reread.
    let mut proposal = actual.clone();
    for bus in &mut proposal.input_buses {
        bus.active = Some(true);
    }
    for (index, bus) in proposal.output_buses.iter_mut().enumerate() {
        bus.active = Some(index == 0);
    }
    proposal.validate(sample_rate)?;
    for direction in [BusDirection::Input, BusDirection::Output] {
        let count = match direction {
            BusDirection::Input => actual.input_buses.len(),
            BusDirection::Output => actual.output_buses.len(),
        };
        for index in 0..count {
            let active = direction == BusDirection::Input || index == 0;
            access.acknowledge(direction, index, active)?;
            match direction {
                BusDirection::Input => {
                    actual.input_buses[index].active = Some(active);
                    storage.inputs[index].active = active;
                }
                BusDirection::Output => {
                    actual.output_buses[index].active = Some(active);
                    storage.outputs[index].active = active;
                }
            }
            if access.processing() || access.actual()? != actual || access.storage()? != storage {
                return Err("Fresh bounce complete topology/storage changed during selected-output SDK acknowledgment".into());
            }
        }
    }
    let checked = access.actual()?;
    checked.validate(sample_rate)?;
    if checked != proposal || access.storage()? != storage || access.processing() {
        return Err("Fresh bounce complete selection differs before Start".into());
    }
    Ok(checked)
}

#[cfg(test)]
#[path = "bounce_selected_outputs_tests.rs"]
mod tests;
