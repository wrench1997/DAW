//! Session-only identity of an actually prepared VST3. Never a catalog declaration.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedVst3Identity {
    path: PathBuf,
    requested_class_uid: Option<String>,
    class_uid: String,
    version: String,
    category: String,
    has_midi_input: bool,
    has_midi_output: bool,
}

impl LoadedVst3Identity {
    #[cfg(feature = "vst3")]
    pub(super) fn from_prepared_plugin(
        path: PathBuf,
        requested_class_uid: Option<String>,
        info: &vst3_host::plugin::PluginInfo,
        events: &vst3_host::plugin::EventBusLayout,
    ) -> Result<Self, String> {
        if !valid_class_uid(&info.uid) || info.version.is_empty() {
            return Err(
                "Prepared VST3 export identity requires an actual class UID and version".into(),
            );
        }
        let active = |buses: &[vst3_host::plugin::EventBusConfig]| {
            buses
                .iter()
                .any(|bus| bus.channel_count > 0 && bus.active == Some(true))
        };
        Ok(Self {
            path,
            requested_class_uid,
            class_uid: info.uid.clone(),
            version: info.version.clone(),
            category: info.category.clone(),
            has_midi_input: info.has_midi_input && active(&events.inputs),
            has_midi_output: info.has_midi_output && active(&events.outputs),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn requested_class_uid(&self) -> Option<&str> {
        self.requested_class_uid.as_deref()
    }
    pub(crate) fn class_uid(&self) -> &str {
        &self.class_uid
    }
    pub(crate) fn version(&self) -> &str {
        &self.version
    }
    pub(crate) fn category(&self) -> &str {
        &self.category
    }
    pub(crate) fn is_instrument_with_midi_input(&self) -> bool {
        self.has_midi_input
            && self
                .category
                .split('|')
                .any(|token| token.trim() == "Instrument")
            && valid_class_uid(&self.class_uid)
            && !self.version.is_empty()
    }

    /// Explicit mock identity; available only in the test profile, never from a catalog.
    #[cfg(test)]
    pub(crate) fn test_fixture(
        path: PathBuf,
        uid: &str,
        version: &str,
        category: &str,
        midi_in: bool,
        midi_out: bool,
    ) -> Self {
        Self {
            path,
            requested_class_uid: None,
            class_uid: uid.into(),
            version: version.into(),
            category: category.into(),
            has_midi_input: midi_in,
            has_midi_output: midi_out,
        }
    }
}

fn valid_class_uid(uid: &str) -> bool {
    uid.len() == 32
        && uid.bytes().all(|byte| byte.is_ascii_hexdigit())
        && uid.bytes().any(|byte| byte != b'0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_eligibility_uses_exact_instrument_token_and_real_midi_not_a_name_substring() {
        let identity = |category, midi| {
            LoadedVst3Identity::test_fixture(
                "module.vst3".into(),
                "00112233445566778899AABBCCDDEEFF",
                "1",
                category,
                midi,
                false,
            )
        };
        for category in ["Instrument", "Instrument|Synth", "Fx| Instrument "] {
            assert!(identity(category, true).is_instrument_with_midi_input());
            assert!(!identity(category, false).is_instrument_with_midi_input());
        }
        for category in ["", "Fx", "instrument", "NotInstrument", "InstrumentFX"] {
            assert!(!identity(category, true).is_instrument_with_midi_input());
        }
    }
}
