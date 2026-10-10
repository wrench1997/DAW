//! Owner-recorded component acknowledgments, distinct from prepared participation.
//!
//! These records are never a query of plugin-private state. They describe a
//! successful host activateBus call, bound to the descriptor observed for that
//! call and revalidated when prepared storage is replaced. Unrequested defaults
//! and failed/invalidated calls remain unknown. No identity is inferred across
//! indistinguishable, unsignaled plugin-internal topology changes.

#![allow(clippy::unnecessary_cast)] // SDK enum representations vary by platform.

use super::{kAudio, kEvent, kInput, kOutput, BusInfo, BusTypes_};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BusBinding {
    media: i32,
    direction: i32,
    index: i32,
    channels: i32,
    kind: i32,
    name: [u16; 128],
    flags: u32,
}

impl BusBinding {
    #[allow(clippy::unnecessary_cast)]
    pub(super) fn from_info(
        media: i32,
        direction: i32,
        index: i32,
        info: &BusInfo,
    ) -> Option<Self> {
        let max_channels = if media == kAudio as i32 {
            256
        } else if media == kEvent as i32 {
            16
        } else {
            return None;
        };
        if index < 0
            || info.mediaType != media
            || info.direction != direction
            || !(1..=max_channels).contains(&info.channelCount)
            || !matches!(direction, d if d == kInput as i32 || d == kOutput as i32)
            || (info.busType != BusTypes_::kMain as i32 && info.busType != BusTypes_::kAux as i32)
        {
            return None;
        }
        Some(Self {
            media,
            direction,
            index,
            channels: info.channelCount,
            kind: info.busType,
            name: info.name.map(|c| c as u16),
            flags: info.flags,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ActivationRecord {
    active: bool,
    binding: BusBinding,
}

impl ActivationRecord {
    pub(super) fn acknowledged(active: bool, binding: Option<BusBinding>) -> Option<Self> {
        binding.map(|binding| Self { active, binding })
    }

    pub(super) fn observed(
        &self,
        participation: bool,
        binding: Option<&BusBinding>,
    ) -> Option<bool> {
        (self.active == participation && binding == Some(&self.binding)).then_some(self.active)
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub(super) struct BusActivationState {
    pub(super) audio_inputs: Vec<bool>,
    pub(super) audio_outputs: Vec<bool>,
    pub(super) event_inputs: Vec<bool>,
    pub(super) event_outputs: Vec<bool>,
    pub(super) audio_input_acks: Vec<Option<ActivationRecord>>,
    pub(super) audio_output_acks: Vec<Option<ActivationRecord>>,
    pub(super) event_input_acks: Vec<Option<ActivationRecord>>,
    pub(super) event_output_acks: Vec<Option<ActivationRecord>>,
}

impl BusActivationState {
    pub(super) fn invalidate_acknowledgments(&mut self) {
        self.audio_input_acks.fill(None);
        self.audio_output_acks.fill(None);
        self.event_input_acks.fill(None);
        self.event_output_acks.fill(None);
    }

    /// A new side length invalidates every old index. For unchanged lengths,
    /// retain only exact descriptor and participation bindings, never an index
    /// alone. This is owner setup, not processing-thread work.
    pub(super) fn retain_prepared_bindings(
        acks: &mut Vec<Option<ActivationRecord>>,
        participation: &[bool],
        bindings: &[Option<BusBinding>],
    ) {
        if acks.len() != bindings.len() || participation.len() != bindings.len() {
            acks.clear();
            acks.resize_with(bindings.len(), || None);
            return;
        }
        for ((ack, active), binding) in acks.iter_mut().zip(participation).zip(bindings) {
            if ack
                .as_ref()
                .and_then(|record| record.observed(*active, binding.as_ref()))
                .is_none()
            {
                *ack = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> BusInfo {
        // SDK BusInfo is a scalar/UTF-16 POD, with every field initialized here.
        let mut info: BusInfo = unsafe { std::mem::zeroed() };
        info.mediaType = kAudio as i32;
        info.direction = kOutput as i32;
        info.channelCount = 2;
        info.busType = BusTypes_::kAux as i32;
        info.name[0] = 65;
        info
    }

    #[test]
    fn complete_descriptor_binding_never_confuses_false_with_unknown() {
        let info = descriptor();
        let binding = BusBinding::from_info(kAudio as i32, kOutput as i32, 1, &info).unwrap();
        let ack = ActivationRecord::acknowledged(false, Some(binding.clone())).unwrap();
        assert_eq!(ack.observed(false, Some(&binding)), Some(false));
        assert_eq!(ack.observed(true, Some(&binding)), None);
        assert_eq!(ack.observed(false, None), None);
        for field in 0..7 {
            let mut changed = binding.clone();
            match field {
                0 => changed.media = kEvent as i32,
                1 => changed.direction = kInput as i32,
                2 => changed.index += 1,
                3 => changed.channels = 1,
                4 => changed.kind = BusTypes_::kMain as i32,
                5 => changed.name[127] = 1,
                _ => changed.flags = 1,
            }
            assert_eq!(ack.observed(false, Some(&changed)), None);
        }
        assert!(ActivationRecord::acknowledged(false, None).is_none());
    }

    #[test]
    fn invalid_descriptor_or_absent_bus_cannot_install_evidence() {
        for field in 0..6 {
            let mut info = descriptor();
            match field {
                0 => info.mediaType = kEvent as i32,
                1 => info.direction = kInput as i32,
                2 => info.channelCount = 0,
                3 => info.channelCount = 257,
                4 => info.busType = -1,
                _ => info.channelCount = -1,
            }
            assert!(BusBinding::from_info(kAudio as i32, kOutput as i32, 1, &info).is_none());
        }
        assert!(BusBinding::from_info(kAudio as i32, kOutput as i32, -1, &descriptor()).is_none());
    }

    #[test]
    fn changed_side_lengths_participation_and_missing_bindings_discard_without_resurrection() {
        let binding =
            BusBinding::from_info(kAudio as i32, kOutput as i32, 1, &descriptor()).unwrap();
        for change in 0..3 {
            let mut acks = vec![ActivationRecord::acknowledged(false, Some(binding.clone()))];
            match change {
                0 => BusActivationState::retain_prepared_bindings(&mut acks, &[], &[]),
                1 => BusActivationState::retain_prepared_bindings(
                    &mut acks,
                    &[true],
                    &[Some(binding.clone())],
                ),
                _ => BusActivationState::retain_prepared_bindings(&mut acks, &[false], &[None]),
            }
            assert!(acks.iter().all(Option::is_none));
            BusActivationState::retain_prepared_bindings(
                &mut acks,
                &[false],
                &[Some(binding.clone())],
            );
            assert_eq!(acks, [None]);
        }
    }
}
