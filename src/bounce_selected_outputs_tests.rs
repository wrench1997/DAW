use super::*;
use crate::instrument_bounce::BounceAudioBus;
use vst3_host::audio::AudioBusConfig;

#[derive(Default, Clone, Copy)]
enum Fault {
    #[default]
    None,
    AckError(usize),
    AckUnknown(usize),
    DescriptorDrift(usize),
    StorageDrift(usize),
    EventDrift(usize),
    Lifecycle(usize),
}

struct SourceOwner {
    metadata: BouncePluginMetadata,
    storage: AudioBusLayout,
    calls: Vec<(BusDirection, usize, bool)>,
    processing: bool,
    reload: bool,
    fault: Fault,
    opaque: Vec<u8>,
    bases: [(u32, f32); 2],
}

impl SourceOwner {
    fn restored() -> Self {
        Self {
            metadata: BouncePluginMetadata {
                class_uid: "5653544E694B386B6F6E74616B742038".into(),
                version: "8.4.1".into(),
                name: "Kontakt 8".into(),
                category: "Instrument|Sampler".into(),
                input_buses: vec![],
                output_buses: vec![BounceAudioBus::new(2, true, None); 32],
                event_input_buses: vec![(16, true, Some(true))],
                event_output_buses: vec![(16, true, Some(true))],
                latency_samples: 0,
                tail_samples: u32::MAX,
            },
            storage: AudioBusLayout {
                inputs: vec![],
                outputs: vec![
                    AudioBusConfig {
                        channel_count: 2,
                        active: true
                    };
                    32
                ],
            },
            calls: vec![],
            processing: false,
            reload: false,
            fault: Fault::None,
            opaque: vec![1, 2, 3],
            bases: [(17, 0.1), (23, 0.75)],
        }
    }
}

impl Access for SourceOwner {
    fn processing(&self) -> bool {
        self.processing
    }
    fn service_stopped(&mut self) -> Result<bool, String> {
        Ok(self.reload)
    }
    fn actual(&self) -> Result<BouncePluginMetadata, String> {
        Ok(self.metadata.clone())
    }
    fn storage(&self) -> Result<AudioBusLayout, String> {
        Ok(self.storage.clone())
    }
    fn acknowledge(
        &mut self,
        direction: BusDirection,
        index: usize,
        active: bool,
    ) -> Result<(), String> {
        self.calls.push((direction, index, active));
        if matches!(self.fault, Fault::AckError(at) if at == index) {
            return Err("actual SDK refused selection".into());
        }
        match direction {
            BusDirection::Input => {
                self.metadata.input_buses[index].active = Some(active);
                self.storage.inputs[index].active = active;
            }
            BusDirection::Output => {
                self.metadata.output_buses[index].active = Some(active);
                self.storage.outputs[index].active = active;
            }
        }
        match self.fault {
            Fault::AckUnknown(at) if at == index => self.metadata.output_buses[index].active = None,
            Fault::DescriptorDrift(at) if at == index => {
                self.metadata.output_buses[31].is_main = false
            }
            Fault::StorageDrift(at) if at == index => self.storage.outputs[31].channel_count = 1,
            Fault::EventDrift(at) if at == index => {
                self.metadata.event_output_buses[0].2 = Some(false)
            }
            Fault::Lifecycle(at) if at == index => self.processing = true,
            _ => {}
        }
        Ok(())
    }
}

#[test]
fn restored_source_acknowledges_all_32_declared_outputs_without_state_or_event_projection() {
    let mut source = SourceOwner::restored();
    let original = source.metadata.clone();
    let state = source.opaque.clone();
    let bases = source.bases;
    let selected = configure(&mut source, 48_000).unwrap();
    assert_eq!(
        source.calls,
        (0..32)
            .map(|index| (BusDirection::Output, index, index == 0))
            .collect::<Vec<_>>()
    );
    assert_eq!(selected.output_buses.len(), 32);
    assert!(
        selected
            .output_buses
            .iter()
            .enumerate()
            .all(|(index, bus)| bus.channel_count == 2
                && bus.is_main
                && bus.active == Some(index == 0))
    );
    assert_eq!(selected.event_input_buses, original.event_input_buses);
    assert_eq!(selected.event_output_buses, original.event_output_buses);
    assert_eq!(source.opaque, state);
    assert_eq!(source.bases, bases);
    assert!(!source.processing);
    // State load can invalidate every activation attestation. The same stopped
    // production transaction must obtain all 32 new ACKs before it can qualify.
    source
        .metadata
        .output_buses
        .fill(BounceAudioBus::new(2, true, None));
    source.storage.outputs.fill(AudioBusConfig {
        channel_count: 2,
        active: true,
    });
    source.calls.clear();
    assert_eq!(configure(&mut source, 48_000).unwrap(), selected);
    assert_eq!(source.calls.len(), 32);
}

#[test]
fn fresh_selected_source_fails_closed_on_any_owner_ack_binding_or_lifecycle_failure() {
    for fault in [
        Fault::AckError(8),
        Fault::AckUnknown(8),
        Fault::DescriptorDrift(8),
        Fault::StorageDrift(8),
        Fault::EventDrift(8),
        Fault::Lifecycle(8),
    ] {
        let mut source = SourceOwner::restored();
        source.fault = fault;
        assert!(configure(&mut source, 48_000).is_err());
        assert_eq!(
            source.calls.len(),
            9,
            "the failed candidate cannot continue SDK selection"
        );
        assert_eq!(source.opaque, [1, 2, 3]);
        assert_eq!(source.bases, [(17, 0.1), (23, 0.75)]);
    }
    for invalid in 0..5 {
        let mut source = SourceOwner::restored();
        match invalid {
            0 => source.reload = true,
            1 => source.processing = true,
            2 => source.metadata.version = "8.4.2".into(),
            3 => source
                .metadata
                .output_buses
                .push(BounceAudioBus::new(1, true, None)),
            _ => source.metadata.event_output_buses[0].2 = None,
        }
        assert!(configure(&mut source, 48_000).is_err());
        assert!(
            source.calls.is_empty(),
            "preflight must precede the first setter"
        );
    }
}
