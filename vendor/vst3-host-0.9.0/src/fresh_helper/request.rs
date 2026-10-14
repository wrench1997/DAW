//! Synchronous, stack-only view of the deliberately closed request subset.
//!
//! Caller-owned state, audio and text remain borrowed through validation, size
//! counting and emission. Nothing in this module queues a borrow or stores one
//! in an error. Conversion examines only the command tag and scalar fields;
//! the codec checks the original exchange budget before any payload traversal.

use super::ErrorKind;
use crate::audio::{AudioBusBuffer, AudioBusConfig, BusDirection, MediaType};
use crate::midi::MidiEvent;
use crate::plugin::{ProcessMode, ProcessTransport, StateContext};
use crate::process_isolation::{HostCommand, SelectedMetadataPolicy};

#[derive(Debug, Clone, Copy)]
pub(crate) enum StateContextView<'a> {
    Project,
    Preset { path: Option<&'a str> },
}

impl<'a> From<&'a StateContext> for StateContextView<'a> {
    fn from(context: &'a StateContext) -> Self {
        match context {
            StateContext::Project => Self::Project,
            StateContext::Preset { path } => Self::Preset {
                path: path.as_deref(),
            },
        }
    }
}

// This fixed, immutable channel is the only sample source for the closed
// generator operation. Repetition is a view, not an allocated channel vector.
static SILENT_Q128_CHANNEL: [f32; 128] = [0.0; 128];

#[derive(Debug, Clone, Copy)]
pub(crate) enum FlatInputView<'a> {
    Borrowed(&'a [Vec<f32>]),
    SilentQ128(usize),
}

impl<'a> FlatInputView<'a> {
    pub(crate) fn channel_count(self) -> usize {
        match self {
            Self::Borrowed(channels) => channels.len(),
            Self::SilentQ128(channels) => channels,
        }
    }

    pub(crate) fn iter(self) -> impl ExactSizeIterator<Item = &'a [f32]> {
        (0..self.channel_count()).map(move |index| match self {
            Self::Borrowed(channels) => channels[index].as_slice(),
            Self::SilentQ128(_) => SILENT_Q128_CHANNEL.as_slice(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RequestView<'a> {
    ProbeFactory {
        path: &'a str,
    },
    LoadPlugin {
        path: &'a str,
        sample_rate: f64,
        block_size: u32,
        tempo: f64,
        time_sig_numerator: i32,
        time_sig_denominator: i32,
        class_id: Option<&'a str>,
    },
    LoadState {
        data: &'a [u8],
        context: StateContextView<'a>,
    },
    SaveState,
    Process {
        inputs: FlatInputView<'a>,
        frames: u32,
        transport: Option<ProcessTransport>,
    },
    ProcessBuses {
        inputs: &'a [AudioBusBuffer],
        outputs: &'a [AudioBusConfig],
        frames: u32,
        transport: Option<ProcessTransport>,
    },
    GetParameter {
        id: u32,
    },
    LatencySamples,
    TailSamples,
    SetProcessMode {
        mode: ProcessMode,
    },
    ResetOriginSupport,
    StartProcessing,
    StopProcessing,
    NativeDirtyRevision,
    GetAllParameters,
    SetParameter {
        id: u32,
        value: f64,
    },
    SendMidiAt {
        event: MidiEvent,
        sample_offset: i32,
    },
    CheckedAudioBusLayout,
    StrictMetadataSnapshot {
        policy: SelectedMetadataPolicy,
    },
    EventBusLayout,
    SetBusActive {
        media_type: MediaType,
        direction: BusDirection,
        bus_index: i32,
        active: bool,
    },
}

impl RequestView<'_> {
    // Count validation stays inside the original exchange budget. This creates
    // only a synchronous view; no caller samples or variable payload are owned.
    pub(crate) fn silent_generator(input_channels: usize, transport: ProcessTransport) -> Self {
        Self::Process {
            inputs: FlatInputView::SilentQ128(input_channels),
            frames: 128,
            transport: Some(transport),
        }
    }
}

impl<'a> TryFrom<&'a HostCommand> for RequestView<'a> {
    type Error = ErrorKind;

    fn try_from(command: &'a HostCommand) -> Result<Self, Self::Error> {
        Ok(match command {
            HostCommand::ProbeFactory { path } => Self::ProbeFactory { path },
            HostCommand::LoadPlugin {
                path,
                sample_rate,
                block_size,
                tempo,
                time_sig_numerator,
                time_sig_denominator,
                class_id,
            } => Self::LoadPlugin {
                path,
                sample_rate: *sample_rate,
                block_size: *block_size,
                tempo: *tempo,
                time_sig_numerator: *time_sig_numerator,
                time_sig_denominator: *time_sig_denominator,
                class_id: class_id.as_deref(),
            },
            HostCommand::LoadState { data, context } => Self::LoadState {
                data,
                context: context.into(),
            },
            HostCommand::SaveState => Self::SaveState,
            HostCommand::Process {
                inputs,
                frames,
                transport,
            } => Self::Process {
                inputs: FlatInputView::Borrowed(inputs),
                frames: *frames,
                transport: *transport,
            },
            HostCommand::ProcessBuses {
                inputs,
                outputs,
                frames,
                transport,
            } => Self::ProcessBuses {
                inputs,
                outputs,
                frames: *frames,
                transport: *transport,
            },
            HostCommand::GetParameter { id } => Self::GetParameter { id: *id },
            HostCommand::LatencySamples => Self::LatencySamples,
            HostCommand::TailSamples => Self::TailSamples,
            HostCommand::SetProcessMode { mode } => Self::SetProcessMode { mode: *mode },
            HostCommand::ResetOriginSupport => Self::ResetOriginSupport,
            HostCommand::StartProcessing => Self::StartProcessing,
            HostCommand::StopProcessing => Self::StopProcessing,
            HostCommand::NativeDirtyRevision => Self::NativeDirtyRevision,
            HostCommand::GetAllParameters => Self::GetAllParameters,
            HostCommand::SetParameter { id, value } => Self::SetParameter {
                id: *id,
                value: *value,
            },
            HostCommand::SendMidiAt {
                event,
                sample_offset,
            } => Self::SendMidiAt {
                event: *event,
                sample_offset: *sample_offset,
            },
            HostCommand::CheckedAudioBusLayout => Self::CheckedAudioBusLayout,
            HostCommand::StrictMetadataSnapshot(request) => Self::StrictMetadataSnapshot {
                policy: request.policy,
            },
            HostCommand::EventBusLayout => Self::EventBusLayout,
            HostCommand::SetBusActive {
                media_type,
                direction,
                bus_index,
                active,
            } => Self::SetBusActive {
                media_type: *media_type,
                direction: *direction,
                bus_index: *bus_index,
                active: *active,
            },
            _ => return Err(ErrorKind::UnsupportedRequest),
        })
    }
}
