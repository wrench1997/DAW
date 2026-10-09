//! Pure presentation models and friendly formatting for the System Settings UI.
//!
//! This module deliberately does not render egui widgets. Keeping the device
//! state reduction here makes the compact Settings surface deterministic and
//! testable without constructing an application or opening an audio device.

use crate::audio_device::{
    AudioBufferSizeRequest, AudioChannelRequest, AudioDeviceProfile, AudioDeviceSelection,
    AudioSampleFormat, AudioSampleFormatRequest, AudioSampleRateRequest, AudioStreamFaultKind,
    AudioStreamTelemetrySnapshot,
};

/// Describe the observed stream's host rather than assuming the build platform's default.
/// No device enumeration or stream startup is performed to render this label.
pub fn format_audio_backend(running_profile: Option<&AudioDeviceProfile>) -> String {
    match running_profile.map(|profile| &profile.selection) {
        None => "CPAL / offline (no active stream)".into(),
        Some(AudioDeviceSelection::Stable(identity)) => format!("CPAL / {}", identity.host_id),
        Some(AudioDeviceSelection::SystemDefault) => "CPAL / host not reported".into(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    #[default]
    Audio,
    Midi,
    Files,
    Project,
    Debug,
    About,
}

impl SettingsPage {
    pub const ALL: [Self; 6] = [
        Self::Audio,
        Self::Midi,
        Self::Files,
        Self::Project,
        Self::Debug,
        Self::About,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Midi => "MIDI",
            Self::Files => "Files",
            Self::Project => "Project",
            Self::Debug => "Debug",
            Self::About => "About",
        }
    }

    /// Stable semantic key for either a painted icon or an accessibility label.
    pub const fn icon_key(self) -> &'static str {
        match self {
            Self::Audio => "audio-wave",
            Self::Midi => "midi-plug",
            Self::Files => "folder",
            Self::Project => "project-file",
            Self::Debug => "diagnostics",
            Self::About => "information",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsSeverity {
    #[default]
    Neutral,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AudioSettingsState {
    Offline,
    Switching,
    #[default]
    Opening,
    Running,
    Xrun,
    Failed,
}

impl AudioSettingsState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Offline => "Offline",
            Self::Switching => "Switching",
            Self::Opening => "Opening",
            Self::Running => "Running",
            Self::Xrun => "Running / XRUN",
            Self::Failed => "Failed",
        }
    }
}

/// Minimal app-owned input required to render the Audio settings summary.
///
/// `apply_permitted` is supplied by the app because project/save/recording
/// barriers are intentionally outside this UI module. The presentation also
/// disables Apply while a device transaction is already in progress.
#[derive(Clone, Copy, Debug)]
pub struct AudioSettingsInput<'a> {
    pub device_name: Option<&'a str>,
    pub running_profile: Option<&'a AudioDeviceProfile>,
    pub telemetry: Option<AudioStreamTelemetrySnapshot>,
    pub transition_in_progress: bool,
    pub draft_changed: bool,
    pub apply_permitted: bool,
}

impl Default for AudioSettingsInput<'_> {
    fn default() -> Self {
        Self {
            device_name: None,
            running_profile: None,
            telemetry: None,
            transition_in_progress: false,
            draft_changed: false,
            apply_permitted: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSettingsPresentation {
    pub state: AudioSettingsState,
    pub severity: SettingsSeverity,
    pub status_label: &'static str,
    pub headline: String,
    pub detail: String,
    pub sample_rate_label: String,
    pub sample_format_label: String,
    pub channel_label: String,
    pub buffer_label: String,
    pub underruns: u64,
    pub draft_changed: bool,
    pub can_apply: bool,
}

impl AudioSettingsPresentation {
    #[must_use]
    pub fn new(input: AudioSettingsInput<'_>) -> Self {
        let telemetry = input.telemetry.unwrap_or_default();
        let profile = input.running_profile;
        let sample_rate = profile.and_then(profile_sample_rate);
        let sample_rate_label = profile.map_or_else(
            || "Not available".to_owned(),
            |profile| format_sample_rate_request(profile.sample_rate),
        );
        let sample_format_label = profile.map_or_else(
            || "Not available".to_owned(),
            |profile| format_sample_format_request(profile.sample_format),
        );
        let channel_label = profile.map_or_else(
            || "Not available".to_owned(),
            |profile| format_channel_request(profile.channels),
        );
        let buffer_label = profile.map_or_else(
            || "Not available".to_owned(),
            |profile| {
                telemetry.last_frames.map_or_else(
                    || format_buffer_request(profile.buffer_size, sample_rate),
                    |frames| format!("{} observed", format_buffer_frames(frames, sample_rate)),
                )
            },
        );
        let device_name = input
            .device_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("Selected audio device");
        let fatal_kind = telemetry.last_invalidating_error_kind;
        let has_fatal_fault = fatal_kind.invalidates_stream();

        let (state, severity, headline, detail) = if has_fatal_fault {
            (
                AudioSettingsState::Failed,
                SettingsSeverity::Error,
                format!("{device_name} failed"),
                format!(
                    "{}; audio playback remains paused",
                    format_fault_kind(fatal_kind)
                ),
            )
        } else if input.transition_in_progress {
            (
                AudioSettingsState::Switching,
                SettingsSeverity::Warning,
                "Switching audio device".to_owned(),
                "The replacement is opening and waiting for its first callback".to_owned(),
            )
        } else if profile.is_none() {
            (
                AudioSettingsState::Offline,
                SettingsSeverity::Error,
                "Audio engine is offline".to_owned(),
                "Select an output device and apply the change".to_owned(),
            )
        } else if telemetry.callback_count == 0 {
            (
                AudioSettingsState::Opening,
                SettingsSeverity::Neutral,
                format!("Opening {device_name}"),
                "Waiting for the first audio callback".to_owned(),
            )
        } else if telemetry.xrun_count != 0
            || telemetry.last_error_kind == AudioStreamFaultKind::Xrun
        {
            (
                AudioSettingsState::Xrun,
                SettingsSeverity::Warning,
                format!("{device_name} is running"),
                format!(
                    "{} underrun{} detected; audio remains active",
                    telemetry.xrun_count,
                    if telemetry.xrun_count == 1 { "" } else { "s" }
                ),
            )
        } else if telemetry.last_error_kind.is_nonfatal_warning() {
            (
                AudioSettingsState::Running,
                SettingsSeverity::Warning,
                format!("{device_name} is running"),
                format!(
                    "{}; audio continues without realtime-priority guarantees",
                    format_fault_kind(telemetry.last_error_kind)
                ),
            )
        } else {
            (
                AudioSettingsState::Running,
                SettingsSeverity::Success,
                format!("{device_name} is running"),
                format!("Open at {sample_rate_label}, {sample_format_label}, {channel_label}"),
            )
        };

        Self {
            state,
            severity,
            status_label: state.label(),
            headline,
            detail,
            sample_rate_label,
            sample_format_label,
            channel_label,
            buffer_label,
            underruns: telemetry.xrun_count,
            draft_changed: input.draft_changed,
            can_apply: input.apply_permitted && !input.transition_in_progress,
        }
    }
}

#[must_use]
pub fn format_sample_rate_hz(sample_rate: u32) -> String {
    if sample_rate == 0 {
        "Unknown sample rate".to_owned()
    } else {
        format!("{} Hz", format_grouped_u64(u64::from(sample_rate)))
    }
}

#[must_use]
pub fn format_sample_rate_request(request: AudioSampleRateRequest) -> String {
    match request {
        AudioSampleRateRequest::DeviceDefault => "Device default".to_owned(),
        AudioSampleRateRequest::Exact(sample_rate) => format_sample_rate_hz(sample_rate),
        AudioSampleRateRequest::Nearest(sample_rate) => {
            format!("Closest to {}", format_sample_rate_hz(sample_rate))
        }
    }
}

#[must_use]
pub fn format_sample_format(format: AudioSampleFormat) -> &'static str {
    match format {
        AudioSampleFormat::I8 => "Signed 8-bit PCM",
        AudioSampleFormat::I16 => "Signed 16-bit PCM",
        AudioSampleFormat::I24 => "Signed 24-bit PCM",
        AudioSampleFormat::I32 => "Signed 32-bit PCM",
        AudioSampleFormat::I64 => "Signed 64-bit PCM",
        AudioSampleFormat::U8 => "Unsigned 8-bit PCM",
        AudioSampleFormat::U16 => "Unsigned 16-bit PCM",
        AudioSampleFormat::U24 => "Unsigned 24-bit PCM",
        AudioSampleFormat::U32 => "Unsigned 32-bit PCM",
        AudioSampleFormat::U64 => "Unsigned 64-bit PCM",
        AudioSampleFormat::F32 => "32-bit float",
        AudioSampleFormat::F64 => "64-bit float",
    }
}

#[must_use]
pub fn format_sample_format_request(request: AudioSampleFormatRequest) -> String {
    match request {
        AudioSampleFormatRequest::DeviceDefault => "Device default".to_owned(),
        AudioSampleFormatRequest::Automatic => "Automatic (32-bit float preferred)".to_owned(),
        AudioSampleFormatRequest::Exact(format) => format_sample_format(format).to_owned(),
    }
}

#[must_use]
pub fn format_channel_request(request: AudioChannelRequest) -> String {
    match request {
        AudioChannelRequest::DeviceDefault => "Device default".to_owned(),
        AudioChannelRequest::Exact(1) => "Mono (1 channel)".to_owned(),
        AudioChannelRequest::Exact(2) => "Stereo (2 channels)".to_owned(),
        AudioChannelRequest::Exact(channels) => format!("{channels} channels"),
        AudioChannelRequest::Nearest(1) => "Closest to mono".to_owned(),
        AudioChannelRequest::Nearest(2) => "Closest to stereo".to_owned(),
        AudioChannelRequest::Nearest(channels) => format!("Closest to {channels} channels"),
    }
}

#[must_use]
pub fn buffer_latency_milliseconds(frames: u32, sample_rate: u32) -> Option<u32> {
    if frames == 0 || sample_rate == 0 {
        return None;
    }
    let numerator = u64::from(frames)
        .saturating_mul(1_000)
        .saturating_add(u64::from(sample_rate) / 2);
    Some(
        u32::try_from(numerator / u64::from(sample_rate))
            .unwrap_or(u32::MAX)
            .max(1),
    )
}

#[must_use]
pub fn format_buffer_frames(frames: u32, sample_rate: Option<u32>) -> String {
    let samples = if frames == 1 {
        "1 sample".to_owned()
    } else {
        format!("{} samples", format_grouped_u64(u64::from(frames)))
    };
    sample_rate
        .and_then(|sample_rate| buffer_latency_milliseconds(frames, sample_rate))
        .map_or(samples.clone(), |milliseconds| {
            format!("{samples} ({milliseconds} ms)")
        })
}

#[must_use]
pub fn format_buffer_request(request: AudioBufferSizeRequest, sample_rate: Option<u32>) -> String {
    match request {
        AudioBufferSizeRequest::BackendDefault => "Backend default".to_owned(),
        AudioBufferSizeRequest::Fixed(frames) => format_buffer_frames(frames, sample_rate),
        AudioBufferSizeRequest::Nearest(frames) => {
            format!("Closest to {}", format_buffer_frames(frames, sample_rate))
        }
    }
}

#[must_use]
pub const fn format_fault_kind(kind: AudioStreamFaultKind) -> &'static str {
    match kind {
        AudioStreamFaultKind::None => "No stream fault",
        AudioStreamFaultKind::Xrun => "Audio buffer underrun",
        AudioStreamFaultKind::DeviceChanged => "Audio device changed",
        AudioStreamFaultKind::DeviceNotAvailable => "Audio device is unavailable",
        AudioStreamFaultKind::PermissionDenied => "Audio device permission was denied",
        AudioStreamFaultKind::StreamInvalidated => "Audio stream was invalidated",
        AudioStreamFaultKind::DeviceBusy => "Audio device is busy",
        AudioStreamFaultKind::HostUnavailable => "Audio host is unavailable",
        AudioStreamFaultKind::RealtimeDenied => "Realtime priority was unavailable",
        AudioStreamFaultKind::ResourceExhausted => "Audio resources were exhausted",
        AudioStreamFaultKind::UnsupportedConfig => "Audio configuration is unsupported",
        AudioStreamFaultKind::BackendError => "Audio backend error",
        AudioStreamFaultKind::Other => "Unknown audio stream error",
    }
}

fn profile_sample_rate(profile: &AudioDeviceProfile) -> Option<u32> {
    match profile.sample_rate {
        AudioSampleRateRequest::DeviceDefault => None,
        AudioSampleRateRequest::Exact(sample_rate)
        | AudioSampleRateRequest::Nearest(sample_rate) => Some(sample_rate),
    }
}

fn format_grouped_u64(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_device::{AudioDeviceDirection, AudioDeviceSelection};

    fn running_profile() -> AudioDeviceProfile {
        AudioDeviceProfile {
            direction: AudioDeviceDirection::Output,
            selection: AudioDeviceSelection::SystemDefault,
            sample_rate: AudioSampleRateRequest::Exact(44_100),
            channels: AudioChannelRequest::Exact(2),
            sample_format: AudioSampleFormatRequest::Exact(AudioSampleFormat::F32),
            buffer_size: AudioBufferSizeRequest::Fixed(512),
        }
    }

    fn presentation(
        profile: &AudioDeviceProfile,
        telemetry: AudioStreamTelemetrySnapshot,
    ) -> AudioSettingsPresentation {
        AudioSettingsPresentation::new(AudioSettingsInput {
            device_name: Some("Studio Output"),
            running_profile: Some(profile),
            telemetry: Some(telemetry),
            transition_in_progress: false,
            draft_changed: false,
            apply_permitted: true,
        })
    }

    #[test]
    fn friendly_formatters_never_leak_debug_enum_syntax() {
        let labels = [
            format_sample_rate_request(AudioSampleRateRequest::DeviceDefault),
            format_sample_rate_request(AudioSampleRateRequest::Exact(48_000)),
            format_sample_format_request(AudioSampleFormatRequest::DeviceDefault),
            format_sample_format_request(AudioSampleFormatRequest::Exact(AudioSampleFormat::F32)),
            format_channel_request(AudioChannelRequest::DeviceDefault),
            format_buffer_request(AudioBufferSizeRequest::BackendDefault, Some(48_000)),
            format_buffer_request(AudioBufferSizeRequest::Fixed(256), Some(48_000)),
        ];
        for label in labels {
            assert!(!label.contains("Exact("), "debug syntax leaked: {label}");
            assert!(
                !label.contains("DeviceDefault"),
                "debug syntax leaked: {label}"
            );
        }
    }

    #[test]
    fn five_hundred_twelve_samples_at_44100_hz_is_twelve_ms() {
        assert_eq!(buffer_latency_milliseconds(512, 44_100), Some(12));
        assert_eq!(
            format_buffer_frames(512, Some(44_100)),
            "512 samples (12 ms)"
        );
    }

    #[test]
    fn callback_zero_is_opening_not_running() {
        let profile = running_profile();
        let view = presentation(&profile, AudioStreamTelemetrySnapshot::default());
        assert_eq!(view.state, AudioSettingsState::Opening);
        assert_eq!(view.status_label, "Opening");
        assert_eq!(view.severity, SettingsSeverity::Neutral);
        assert!(view.detail.contains("first audio callback"));
    }

    #[test]
    fn healthy_callback_is_running() {
        let profile = running_profile();
        let view = presentation(
            &profile,
            AudioStreamTelemetrySnapshot {
                callback_count: 1,
                last_frames: Some(512),
                minimum_frames: Some(512),
                maximum_frames: Some(512),
                ..Default::default()
            },
        );
        assert_eq!(view.state, AudioSettingsState::Running);
        assert_eq!(view.severity, SettingsSeverity::Success);
        assert_eq!(view.buffer_label, "512 samples (12 ms) observed");
    }

    #[test]
    fn xrun_is_a_warning_while_audio_remains_running() {
        let profile = running_profile();
        let view = presentation(
            &profile,
            AudioStreamTelemetrySnapshot {
                callback_count: 3,
                last_frames: Some(512),
                xrun_count: 2,
                last_error_kind: AudioStreamFaultKind::Xrun,
                last_error_revision: 1,
                ..Default::default()
            },
        );
        assert_eq!(view.state, AudioSettingsState::Xrun);
        assert_eq!(view.severity, SettingsSeverity::Warning);
        assert!(view.detail.contains("audio remains active"));
    }

    #[test]
    fn invalidating_fault_is_failed_even_after_callbacks() {
        let profile = running_profile();
        let view = presentation(
            &profile,
            AudioStreamTelemetrySnapshot {
                callback_count: 3,
                last_frames: Some(512),
                stream_error_count: 1,
                last_error_kind: AudioStreamFaultKind::DeviceNotAvailable,
                last_error_revision: 1,
                invalidating_error_count: 1,
                last_invalidating_error_kind: AudioStreamFaultKind::DeviceNotAvailable,
                last_invalidating_error_revision: 1,
                ..Default::default()
            },
        );
        assert_eq!(view.state, AudioSettingsState::Failed);
        assert_eq!(view.severity, SettingsSeverity::Error);
        assert!(view.detail.contains("unavailable"));
    }

    #[test]
    fn transition_disables_apply_but_preserves_draft_marker() {
        let profile = running_profile();
        let view = AudioSettingsPresentation::new(AudioSettingsInput {
            device_name: Some("Studio Output"),
            running_profile: Some(&profile),
            telemetry: Some(AudioStreamTelemetrySnapshot::default()),
            transition_in_progress: true,
            draft_changed: true,
            apply_permitted: true,
        });
        assert_eq!(view.state, AudioSettingsState::Switching);
        assert!(view.draft_changed);
        assert!(!view.can_apply);
    }
    #[test]
    fn backend_label_reports_only_observed_host_and_distinguishes_offline() {
        assert_eq!(
            format_audio_backend(None),
            "CPAL / offline (no active stream)"
        );
        let mut profile = AudioDeviceProfile::system_default_output();
        assert_eq!(
            format_audio_backend(Some(&profile)),
            "CPAL / host not reported"
        );
        for host in ["ALSA", "WASAPI", "CoreAudio"] {
            profile.selection =
                AudioDeviceSelection::Stable(crate::audio_device::AudioDeviceIdentity {
                    host_id: host.into(),
                    device_id: "presentation-only-fixture".into(),
                });
            assert_eq!(
                format_audio_backend(Some(&profile)),
                format!("CPAL / {host}")
            );
        }
    }
}
