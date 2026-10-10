//! Synchronous, stack-only view of the deliberately closed request subset.
//!
//! Caller-owned state, audio and text remain borrowed through validation, size
//! counting and emission. Nothing in this module queues a borrow or stores one
//! in an error. Conversion examines only the command tag and scalar fields;
//! the codec checks the original exchange budget before any payload traversal.

use super::ErrorKind;
use crate::audio::{AudioBusBuffer, AudioBusConfig};
use crate::midi::MidiEvent;
use crate::plugin::{ProcessMode, ProcessTransport, StateContext};
use crate::process_isolation::HostCommand;

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

#[derive(Debug, Clone, Copy)]
pub(crate) enum RequestView<'a> {
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
        inputs: &'a [Vec<f32>],
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
    EventBusLayout,
}

impl<'a> TryFrom<&'a HostCommand> for RequestView<'a> {
    type Error = ErrorKind;

    fn try_from(command: &'a HostCommand) -> Result<Self, Self::Error> {
        Ok(match command {
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
                inputs,
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
            HostCommand::EventBusLayout => Self::EventBusLayout,
            _ => return Err(ErrorKind::UnsupportedRequest),
        })
    }
}
