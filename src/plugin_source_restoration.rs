//! Optional, unpublished source restoration. Ordinary loading does not enter this path.

use super::MAX_PLUGIN_PARAMETER_CATALOG_ITEMS;
#[cfg(any(feature = "vst3", test))]
use super::{PluginLoadSpec, PluginPrepareConfig};
use crate::instrument_bounce::BouncePluginMetadata;
#[cfg(any(feature = "vst3", test))]
use crate::instrument_bounce::MAX_BOUNCE_STATE_BYTES;
use std::path::PathBuf;

const MAX_BINDING_TEXT_BYTES: usize = 4_096;

/// Pre-reserved exact session/module/class/base authority for one recreated source.
/// The descriptor ID remains a descriptor ID; vendor is not attested by bounce metadata.
pub(crate) struct SourceRestorationExpectation {
    #[cfg(any(feature = "vst3", test))]
    project_session: u64,
    #[cfg(any(feature = "vst3", test))]
    instance_id: u64,
    #[cfg(any(feature = "vst3", test))]
    descriptor_id: String,
    #[cfg(any(feature = "vst3", test))]
    module_path: PathBuf,
    #[cfg(any(feature = "vst3", test))]
    sample_rate: u32,
    metadata: BouncePluginMetadata,
    #[cfg(any(feature = "vst3", test))]
    bases: Vec<(u32, f32)>,
}

impl SourceRestorationExpectation {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        project_session: u64,
        instance_id: u64,
        descriptor_id: String,
        module_path: PathBuf,
        sample_rate: u32,
        metadata: BouncePluginMetadata,
        bases: Vec<(u32, f32)>,
    ) -> Result<Self, String> {
        let bounded = |text: &str| !text.is_empty() && text.len() <= MAX_BINDING_TEXT_BYTES;
        if project_session == 0
            || instance_id == 0
            || !(8_000..=192_000).contains(&sample_rate)
            || !bounded(&descriptor_id)
            || !module_path.to_str().is_some_and(bounded)
            || [&metadata.version, &metadata.name, &metadata.category]
                .iter()
                .any(|text| !bounded(text))
            || bases.len() > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS
            || bases.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || bases
                .iter()
                .any(|(_, value)| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("Invalid or unbounded recreated-source restoration binding".into());
        }
        metadata.validate(sample_rate)?;
        Ok(Self {
            #[cfg(any(feature = "vst3", test))]
            project_session,
            #[cfg(any(feature = "vst3", test))]
            instance_id,
            #[cfg(any(feature = "vst3", test))]
            descriptor_id,
            #[cfg(any(feature = "vst3", test))]
            module_path,
            #[cfg(any(feature = "vst3", test))]
            sample_rate,
            metadata,
            #[cfg(any(feature = "vst3", test))]
            bases,
        })
    }

    pub(crate) fn expected_timing(&self) -> (u32, u32) {
        (self.metadata.latency_samples, self.metadata.tail_samples)
    }

    #[cfg(any(feature = "vst3", test))]
    pub(crate) fn validate_spec(
        &self,
        project_session: u64,
        instance_id: u64,
        spec: &PluginLoadSpec,
        config: PluginPrepareConfig,
    ) -> Result<(), String> {
        if project_session != self.project_session
            || instance_id != self.instance_id
            || spec.descriptor.id != self.descriptor_id
            || spec.descriptor.path != self.module_path
            || spec.descriptor.format != super::PluginFormat::Vst3
            || !spec.descriptor.is_instrument
            || !spec.enabled
            || spec.bypassed
            || spec.wet != 1.0
            || spec.class_uid.as_deref() != Some(self.metadata.class_uid.as_str())
            || config.sample_rate != f64::from(self.sample_rate)
            || spec.initial_state.is_empty()
            || spec.initial_state.len() > MAX_BOUNCE_STATE_BYTES
        {
            return Err("Recreated source does not match its exact session, instance, module, class, rate or authoritative state binding".into());
        }
        Ok(())
    }

    #[cfg(any(feature = "vst3", test))]
    pub(super) fn check_loaded_identity(
        &self,
        uid: &str,
        version: &str,
        name: &str,
        category: &str,
    ) -> Result<(), String> {
        if uid != self.metadata.class_uid
            || version != self.metadata.version
            || name != self.metadata.name
            || category != self.metadata.category
        {
            return Err("Actual recreated-source factory UID/version/name/category differs from the captured identity".into());
        }
        Ok(())
    }

    /// Complete catalog admission precedes the first setter. No name or clamped value is used.
    #[cfg(any(feature = "vst3", test))]
    pub(super) fn restore_bases(&self, backend: &mut impl RestorationAccess) -> Result<(), String> {
        let catalog = backend.restoration_catalog()?;
        if catalog.len() > MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
            return Err("Recreated-source catalog exceeds the admitted bound".into());
        }
        for (index, parameter) in catalog.iter().enumerate() {
            if catalog[..index]
                .iter()
                .any(|prior| prior.id == parameter.id)
            {
                return Err("Recreated-source catalog has duplicate parameter IDs".into());
            }
        }
        for &(id, _) in &self.bases {
            if !catalog
                .iter()
                .any(|parameter| parameter.id == id && !parameter.read_only)
            {
                return Err(format!(
                    "Authoritative recreated-source base {id} is missing or read-only"
                ));
            }
        }
        for &(id, value) in &self.bases {
            backend.restoration_set(id, f64::from(value))?;
            check_base(id, value, backend.restoration_get(id)?)?;
        }
        Ok(())
    }

    /// The final read sees every earlier setter and Start, including cross-parameter effects.
    #[cfg(any(feature = "vst3", test))]
    pub(super) fn verify_after_start(
        &self,
        backend: &impl RestorationAccess,
    ) -> Result<(), String> {
        for &(id, value) in &self.bases {
            check_base(id, value, backend.restoration_get(id)?)?;
        }
        let metadata = backend.restoration_metadata()?;
        metadata.validate(self.sample_rate)?;
        if metadata != self.metadata {
            return Err(
                "Recreated-source full metadata changed after authoritative bases and Start".into(),
            );
        }
        Ok(())
    }
}

#[cfg(any(feature = "vst3", test))]
fn check_base(id: u32, expected: f32, actual: f64) -> Result<(), String> {
    if !actual.is_finite()
        || !(0.0..=1.0).contains(&actual)
        || (actual as f32).to_bits() != expected.to_bits()
    {
        return Err(format!(
            "Recreated-source base {id} did not read back its exact finite normalized f32 value"
        ));
    }
    Ok(())
}

#[cfg(any(feature = "vst3", test))]
#[derive(Clone)]
pub(super) struct RestorationParameter {
    pub(super) id: u32,
    pub(super) read_only: bool,
}

/// The real VST3 adapter uses raw f64 getters/setters, never generic f32/clamping adapters.
#[cfg(any(feature = "vst3", test))]
pub(super) trait RestorationAccess {
    fn restoration_catalog(&self) -> Result<Vec<RestorationParameter>, String>;
    fn restoration_set(&mut self, id: u32, value: f64) -> Result<(), String>;
    fn restoration_get(&self, id: u32) -> Result<f64, String>;
    fn restoration_metadata(&self) -> Result<BouncePluginMetadata, String>;
}

pub(super) fn check_initial_timing(
    actual: (u32, u32),
    expected: Option<(u32, u32)>,
) -> Result<(u32, u32), String> {
    if let Some(expected) = expected
        && actual != expected
    {
        return Err(format!(
            "Recreated-source initial timing changed: expected {expected:?}, observed {actual:?}"
        ));
    }
    Ok(actual)
}

#[cfg(test)]
#[path = "plugin_source_restoration_tests.rs"]
mod tests;
