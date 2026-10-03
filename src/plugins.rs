use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use object::Object;
use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

#[path = "plugin_runtime.rs"]
#[allow(dead_code)] // public control-plane surface is consumed incrementally by Mixer/UI code
pub mod plugin_runtime;

#[cfg(all(feature = "vst3", target_os = "windows"))]
const VST3_HELPER_FILE_NAME: &str = "vst3-host-helper.exe";
#[cfg(all(feature = "vst3", not(target_os = "windows")))]
const VST3_HELPER_FILE_NAME: &str = "vst3-host-helper";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFormat {
    Vst2,
    Vst3,
}

impl PluginFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vst2 => "VST2",
            Self::Vst3 => "VST3",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginDescriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub path: PathBuf,
    pub format: PluginFormat,
    pub category: String,
    pub is_instrument: bool,
    pub verified: bool,
}

pub fn default_scan_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for env_name in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(env_name) {
            let root = PathBuf::from(root);
            paths.push(root.join("Common Files").join("VST3"));
            paths.push(root.join("VstPlugins"));
            paths.push(root.join("Steinberg").join("VstPlugins"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local)
                .join("Programs")
                .join("Common")
                .join("VST3"),
        );
    }
    paths.retain(|path| path.exists());
    paths.sort();
    paths.dedup();
    paths
}

pub fn scan(paths: &[PathBuf]) -> Vec<PluginDescriptor> {
    let mut plugins = Vec::new();
    let mut seen = HashSet::new();

    for root in paths {
        if !root.exists() {
            continue;
        }
        let walker = WalkDir::new(root)
            .follow_links(false)
            .max_depth(8)
            .into_iter()
            .filter_entry(|entry| !hidden_or_arch(entry));

        for entry in walker.filter_map(Result::ok) {
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default();

            if entry.file_type().is_dir() && extension.eq_ignore_ascii_case("vst3") {
                let canonical = path.to_path_buf();
                if seen.insert(canonical.clone()) {
                    plugins.push(descriptor_from_path(&canonical, PluginFormat::Vst3, true));
                }
                continue;
            }

            if !entry.file_type().is_file() {
                continue;
            }
            if extension.eq_ignore_ascii_case("vst3") {
                let canonical = path.to_path_buf();
                if seen.insert(canonical.clone()) {
                    plugins.push(descriptor_from_path(
                        &canonical,
                        PluginFormat::Vst3,
                        inspect_export(path, b"GetPluginFactory"),
                    ));
                }
            } else if extension.eq_ignore_ascii_case("dll") {
                let is_vst3 = inspect_export(path, b"GetPluginFactory");
                let is_vst2 =
                    inspect_export(path, b"VSTPluginMain") || inspect_export(path, b"main");
                let format = if is_vst3 {
                    Some(PluginFormat::Vst3)
                } else if is_vst2 {
                    Some(PluginFormat::Vst2)
                } else {
                    None
                };
                if let Some(format) = format {
                    let canonical = path.to_path_buf();
                    if seen.insert(canonical.clone()) {
                        plugins.push(descriptor_from_path(&canonical, format, true));
                    }
                }
            }
        }
    }
    plugins.sort_by_key(|plugin| plugin.name.to_lowercase());
    plugins
}

fn hidden_or_arch(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    entry.depth() > 0 && (name.starts_with('.') || name.eq_ignore_ascii_case("resources"))
}

fn descriptor_from_path(path: &Path, format: PluginFormat, verified: bool) -> PluginDescriptor {
    let name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown plugin")
        .replace('_', " ");
    let lower = name.to_lowercase();
    let is_effect = ["eq", "comp", "delay", "verb", "limiter", "filter", "fx"]
        .iter()
        .any(|hint| lower.contains(hint));
    PluginDescriptor {
        id: format!("{}:{}", format.label(), path.to_string_lossy()),
        name,
        vendor: "Unknown vendor".into(),
        path: path.to_path_buf(),
        format,
        category: if is_effect { "Effect" } else { "Instrument" }.into(),
        is_instrument: !is_effect,
        verified,
    }
}

fn inspect_export(path: &Path, symbol: &[u8]) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    let Ok(file) = object::File::parse(data.as_slice()) else {
        return false;
    };
    file.exports()
        .map(|exports| exports.iter().any(|export| export.name() == symbol))
        .unwrap_or(false)
}

#[cfg(feature = "vst3")]
fn installed_vst3_helper_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Unable to locate the Citrus Studio executable: {error}"))?;
    vst3_helper_path_beside(&executable)
}

#[cfg(feature = "vst3")]
fn vst3_helper_path_beside(executable: &Path) -> Result<PathBuf, String> {
    let executable_directory = executable
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            format!(
                "Unable to locate the installation directory containing '{}'",
                executable.display()
            )
        })?;
    let helper = executable_directory.join(VST3_HELPER_FILE_NAME);
    if !helper.is_file() {
        return Err(format!(
            "VST3 process isolation is unavailable because the required helper is missing at '{}'. Reinstall Citrus Studio or place the matching '{}' beside the application executable.",
            helper.display(),
            VST3_HELPER_FILE_NAME
        ));
    }
    Ok(helper)
}

#[derive(Default)]
pub struct NativePluginHost {
    _private: (),
}

impl NativePluginHost {
    /// Spawn one ordered Mixer insert chain. Starting the worker is synchronous; individual
    /// plug-in load success arrives asynchronously as `RuntimeEvent::SlotReady`/`SlotFault`.
    ///
    /// `max_block_frames` is the largest chunk the host can ever submit (Citrus uses
    /// `MAX_PLUGIN_BLOCK_FRAMES`), not the device's nominal/minimum callback size. A larger
    /// callback than this value is rejected by the realtime endpoint and deliberately falls back
    /// to dry audio.
    #[allow(dead_code)] // retained for third-party/control-plane callers without project ids
    pub fn spawn_chain(
        &self,
        mut specs: Vec<plugin_runtime::PluginLoadSpec>,
        sample_rate: f32,
        max_block_frames: u32,
    ) -> Result<plugin_runtime::PluginChain, String> {
        prepare_runtime_specs(&mut specs)?;
        plugin_runtime::PluginChain::spawn(
            specs,
            plugin_runtime::PluginPrepareConfig {
                sample_rate: f64::from(sample_rate),
                max_block_frames: max_block_frames as usize,
            },
        )
    }

    /// Spawn one ordered chain whose physical slots carry the project's stable,
    /// nonzero plug-in instance identities. This is the production entry point
    /// for chains that may receive compiled Timeline parameter automation.
    pub fn spawn_identified_chain(
        &self,
        mut specs: Vec<(u64, plugin_runtime::PluginLoadSpec)>,
        sample_rate: f32,
        max_block_frames: u32,
    ) -> Result<plugin_runtime::PluginChain, String> {
        for (_, spec) in &mut specs {
            prepare_runtime_spec(spec)?;
        }
        plugin_runtime::PluginChain::spawn_identified(
            specs,
            plugin_runtime::PluginPrepareConfig {
                sample_rate: f64::from(sample_rate),
                max_block_frames: max_block_frames as usize,
            },
        )
    }
}

#[allow(dead_code)] // used by the compatibility host entry point above
fn prepare_runtime_specs(specs: &mut [plugin_runtime::PluginLoadSpec]) -> Result<(), String> {
    for spec in specs {
        prepare_runtime_spec(spec)?;
    }
    Ok(())
}

fn prepare_runtime_spec(spec: &mut plugin_runtime::PluginLoadSpec) -> Result<(), String> {
    if !spec.descriptor.path.exists() {
        return Err(format!(
            "Plug-in does not exist: '{}'",
            spec.descriptor.path.display()
        ));
    }
    #[cfg(not(feature = "vst2"))]
    if spec.descriptor.format == PluginFormat::Vst2 {
        return Err("This build does not include VST2 hosting".into());
    }
    #[cfg(not(feature = "vst3"))]
    if spec.descriptor.format == PluginFormat::Vst3 {
        return Err("This build does not include VST3 hosting".into());
    }
    #[cfg(feature = "vst3")]
    if spec.descriptor.format == PluginFormat::Vst3 && spec.vst3_helper_path.is_none() {
        spec.vst3_helper_path = Some(installed_vst3_helper_path()?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_paths_have_no_duplicates() {
        let paths = default_scan_paths();
        let unique: HashSet<_> = paths.iter().collect();
        assert_eq!(paths.len(), unique.len());
    }

    #[cfg(feature = "vst3")]
    #[test]
    fn vst3_helper_is_resolved_only_beside_the_application() {
        let directory = std::env::temp_dir().join(format!(
            "citrus-vst3-helper-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("Citrus Studio.exe");
        let helper = directory.join(VST3_HELPER_FILE_NAME);
        std::fs::write(&helper, b"test helper").unwrap();

        assert_eq!(vst3_helper_path_beside(&executable).unwrap(), helper);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(feature = "vst3")]
    #[test]
    fn missing_vst3_helper_has_an_actionable_error() {
        let executable = std::env::temp_dir()
            .join(format!("citrus-missing-helper-{}", std::process::id()))
            .join("Citrus Studio.exe");
        let expected_helper = executable.parent().unwrap().join(VST3_HELPER_FILE_NAME);

        let error = vst3_helper_path_beside(&executable).unwrap_err();

        assert!(error.contains("VST3 process isolation is unavailable"));
        assert!(error.contains(&expected_helper.display().to_string()));
        assert!(error.contains("Reinstall Citrus Studio"));
    }
}
