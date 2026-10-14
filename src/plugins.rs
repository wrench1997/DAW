use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
};

use object::Object;
use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

#[path = "plugin_runtime.rs"]
#[allow(dead_code)] // public control-plane surface is consumed incrementally by Mixer/UI code
pub mod plugin_runtime;

#[path = "plugin_scan_lifecycle.rs"]
mod scan_lifecycle;
use scan_lifecycle::{ProbeFailure, ScanControl};

#[cfg(all(test, feature = "vst3", any(windows, target_os = "linux")))]
pub(crate) fn helper_transport_test_lock() -> std::sync::MutexGuard<'static, ()> {
    // Ordinary probes and strict clones share one process-global reservation.
    // Their actual-child fixtures must therefore share the same test lock.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub use scan_lifecycle::{ScanOutcome, ScanReport, ScanStopKind, ScanStopped};

#[cfg(all(
    any(target_os = "linux", windows),
    feature = "experimental-fresh-helper"
))]
#[path = "instrument_bounce_strict_backend.rs"]
#[allow(dead_code)] // Explicit dormant renderer entry only; no App/UI activation.
pub(crate) mod strict_backend;

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
    /// Metadata for the exact default audio class loaded by the isolated helper.
    /// Older caches have no authoritative VST3 metadata, regardless of `verified`.
    #[serde(default)]
    pub vst3_metadata: Option<Vst3ScanMetadata>,
    /// Actionable scan failure, distinct from a successfully loaded, unclassified plugin.
    #[serde(default)]
    pub scan_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vst3ScanMetadata {
    pub class_uid: String,
    pub category: String,
    pub has_midi_input: bool,
    pub has_midi_output: bool,
}

impl PluginDescriptor {
    #[allow(dead_code)] // available to MIDI routing consumers
    pub fn has_midi_input(&self) -> bool {
        self.vst3_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.has_midi_input)
    }

    #[allow(dead_code)] // available to MIDI routing consumers
    pub fn has_midi_output(&self) -> bool {
        self.vst3_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.has_midi_output)
    }
}

/// Old VST3 caches only checked a filename/export and must be rescanned. Preserve VST2
/// entries and descriptor IDs; this does not reinterpret or migrate saved project instances.
#[derive(Default)]
pub struct PluginCache {
    pub plugins: Vec<PluginDescriptor>,
    pub needs_rescan: bool,
}

pub fn decode_cache(content: &str) -> Result<PluginCache, serde_json::Error> {
    let mut plugins: Vec<PluginDescriptor> = serde_json::from_str(content)?;
    let previous_count = plugins.len();
    plugins.retain(|plugin| {
        plugin.format != PluginFormat::Vst3
            || plugin.vst3_metadata.is_some()
            || plugin.scan_error.is_some()
    });
    Ok(PluginCache {
        needs_rescan: previous_count != plugins.len(),
        plugins,
    })
}

/// Dropping a scan wakes the current exchange and prevents further probes. The
/// worker retains actual helper/pipe ownership through cleanup, never the UI.
pub struct PluginScan {
    receiver: Receiver<ScanOutcome>,
    control: Arc<ScanControl>,
}

impl PluginScan {
    pub fn try_recv(&self) -> Result<ScanOutcome, TryRecvError> {
        self.receiver.try_recv()
    }

    #[cfg(test)]
    pub(crate) fn with_test_outcome(outcome: Option<ScanOutcome>) -> Self {
        let (sender, receiver) = mpsc::channel();
        if let Some(outcome) = outcome {
            sender.send(outcome).unwrap();
        }
        Self {
            receiver,
            control: ScanControl::new(),
        }
    }
}

impl Drop for PluginScan {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

pub fn start_scan(paths: Vec<PathBuf>) -> PluginScan {
    let (sender, receiver) = mpsc::channel();
    let control = ScanControl::new();
    let worker_control = control.clone();
    let failed_start = sender.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("plugin-scan".into())
        .spawn(move || {
            let mut probe = installed_vst3_probe(worker_control.clone());
            let outcome = scan_with_probe(&paths, worker_control.flag(), &mut probe);
            let _ = sender.send(outcome);
        })
    {
        let _ = failed_start.send(ScanOutcome::Stopped(ScanStopped::new(
            ScanStopKind::WorkerExited,
            format!("worker could not start: {error}"),
        )));
    }
    PluginScan { receiver, control }
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

#[allow(dead_code)] // synchronous discovery API for external/offline callers
pub fn scan(paths: &[PathBuf]) -> Vec<PluginDescriptor> {
    scan_checked(paths)
        .map(|report| report.plugins)
        .unwrap_or_default()
}

/// Checked synchronous discovery keeps stopped scans distinct from empty folders.
pub fn scan_checked(paths: &[PathBuf]) -> Result<ScanReport, ScanStopped> {
    let control = ScanControl::new();
    match scan_with_probe(
        paths,
        control.flag(),
        &mut installed_vst3_probe(control.clone()),
    ) {
        ScanOutcome::Completed(report) => Ok(report),
        ScanOutcome::Stopped(reason) => Err(reason),
    }
}

fn scan_with_probe(
    paths: &[PathBuf],
    cancelled: &AtomicBool,
    probe: &mut impl FnMut(&Path) -> Result<Vst3ProbeInfo, ProbeFailure>,
) -> ScanOutcome {
    let mut plugins = Vec::new();
    let mut probe_lifecycle = Vec::new();
    let mut seen = HashSet::new();

    fn add_probe(
        path: &Path,
        result: Result<Vst3ProbeInfo, ProbeFailure>,
        plugins: &mut Vec<PluginDescriptor>,
        receipts: &mut Vec<scan_lifecycle::ProbeLifecycle>,
    ) -> Result<(), ScanStopped> {
        match result {
            Ok(info) => {
                if let Some(receipt) = &info.lifecycle {
                    receipts.push(receipt.clone());
                }
                plugins.push(vst3_descriptor(path, Ok(info)));
            }
            Err(mut failure) => {
                if let Some(receipt) = failure.lifecycle.take() {
                    receipts.push(*receipt);
                }
                if let Some(mut stopped) = failure.stopped {
                    let mut retained = std::mem::take(receipts);
                    retained.append(&mut stopped.probe_lifecycle);
                    stopped.probe_lifecycle = retained;
                    return Err(stopped);
                }
                plugins.push(vst3_descriptor(path, Err(failure.message)));
            }
        }
        Ok(())
    }

    for root in paths {
        if !root.exists() {
            continue;
        }
        let mut walker = WalkDir::new(root)
            .follow_links(false)
            .max_depth(8)
            .into_iter()
            .filter_entry(|entry| !hidden_or_arch(entry));

        while let Some(entry) = walker.next() {
            if cancelled.load(Ordering::Acquire) {
                let mut stopped = ScanStopped::cancelled();
                stopped.probe_lifecycle = probe_lifecycle;
                return ScanOutcome::Stopped(stopped);
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default();

            if entry.file_type().is_dir() && extension.eq_ignore_ascii_case("vst3") {
                // A bundle is one module. Never index its nested binaries as extra plugins.
                walker.skip_current_dir();
                let canonical = path.to_path_buf();
                if seen.insert(canonical.clone())
                    && let Err(stopped) = add_probe(
                        &canonical,
                        probe(&canonical),
                        &mut plugins,
                        &mut probe_lifecycle,
                    )
                {
                    return ScanOutcome::Stopped(stopped);
                }
                continue;
            }

            if !entry.file_type().is_file() {
                continue;
            }
            if extension.eq_ignore_ascii_case("vst3") {
                let canonical = path.to_path_buf();
                if seen.insert(canonical.clone())
                    && let Err(stopped) = add_probe(
                        &canonical,
                        probe(&canonical),
                        &mut plugins,
                        &mut probe_lifecycle,
                    )
                {
                    return ScanOutcome::Stopped(stopped);
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
                        if format == PluginFormat::Vst3 {
                            if let Err(stopped) = add_probe(
                                &canonical,
                                probe(&canonical),
                                &mut plugins,
                                &mut probe_lifecycle,
                            ) {
                                return ScanOutcome::Stopped(stopped);
                            }
                        } else {
                            plugins.push(descriptor_from_path(&canonical, format, true));
                        }
                    }
                }
            }
        }
    }
    if cancelled.load(Ordering::Acquire) {
        let mut stopped = ScanStopped::cancelled();
        stopped.probe_lifecycle = probe_lifecycle;
        return ScanOutcome::Stopped(stopped);
    }
    plugins.sort_by_key(|plugin| plugin.name.to_lowercase());
    ScanOutcome::Completed(ScanReport {
        plugins,
        probe_lifecycle,
    })
}

struct Vst3ProbeInfo {
    name: String,
    vendor: String,
    metadata: Vst3ScanMetadata,
    lifecycle: Option<scan_lifecycle::ProbeLifecycle>,
}

fn installed_vst3_probe(
    control: Arc<ScanControl>,
) -> impl FnMut(&Path) -> Result<Vst3ProbeInfo, ProbeFailure> {
    #[cfg(feature = "vst3")]
    let helper = installed_vst3_helper_path();
    move |path| {
        #[cfg(feature = "vst3")]
        {
            let helper = helper.as_ref().map_err(|error| {
                ProbeFailure::stop(ScanStopKind::HelperUnavailable, error.clone())
            })?;
            probe_vst3_with_control(path, helper, std::time::Duration::from_secs(5), &control)
        }
        #[cfg(not(feature = "vst3"))]
        {
            let _ = (path, &control);
            Err(ProbeFailure::metadata(
                "This build does not include VST3 hosting",
            ))
        }
    }
}

#[cfg(all(test, feature = "vst3", unix))]
fn probe_vst3(path: &Path, helper: &Path) -> Result<Vst3ProbeInfo, String> {
    probe_vst3_with_timeout(path, helper, std::time::Duration::from_secs(5))
}

#[cfg(all(test, feature = "vst3", unix))]
fn probe_vst3_with_timeout(
    path: &Path,
    helper: &Path,
    timeout: std::time::Duration,
) -> Result<Vst3ProbeInfo, String> {
    probe_vst3_with_control(path, helper, timeout, &ScanControl::new())
        .map_err(|failure| failure.message)
}

#[cfg(all(feature = "vst3", any(windows, target_os = "linux")))]
fn probe_vst3_with_control(
    path: &Path,
    helper: &Path,
    timeout: std::time::Duration,
    control: &ScanControl,
) -> Result<Vst3ProbeInfo, ProbeFailure> {
    scan_lifecycle::probe(path, helper, timeout, control)
}

#[cfg(all(feature = "vst3", not(any(windows, target_os = "linux"))))]
fn probe_vst3_with_control(
    path: &Path,
    helper: &Path,
    timeout: std::time::Duration,
    _control: &ScanControl,
) -> Result<Vst3ProbeInfo, ProbeFailure> {
    probe_vst3_legacy(path, helper, timeout).map_err(ProbeFailure::metadata)
}

#[cfg(all(feature = "vst3", not(any(windows, target_os = "linux"))))]
fn probe_vst3_legacy(
    path: &Path,
    helper: &Path,
    timeout: std::time::Duration,
) -> Result<Vst3ProbeInfo, String> {
    use vst3_host::process_isolation::{HostCommand, HostResponse, PluginHostProcess};

    // Reuse the packaged production helper, never an environment/PATH-discovered binary.
    // Do not create an editor, start processing or load the module inside the application.
    let mut process = PluginHostProcess::new(Some(helper.to_path_buf()), timeout)?;
    process.set_slow_command_timeout(timeout);
    match process.send_command(HostCommand::LoadPlugin {
        path: path.to_string_lossy().into_owned(),
        sample_rate: 48_000.0,
        block_size: 128,
        tempo: 120.0,
        time_sig_numerator: 4,
        time_sig_denominator: 4,
        // Match the runtime's default-class policy, including multi-class modules. One
        // successful load attests only this class; it cannot classify the other classes.
        class_id: None,
    })? {
        HostResponse::PluginInfo {
            name,
            vendor,
            category,
            uid,
            has_midi_input,
            has_midi_output,
            ..
        } => Ok(Vst3ProbeInfo {
            name,
            vendor,
            lifecycle: None,
            metadata: Vst3ScanMetadata {
                class_uid: uid,
                category,
                has_midi_input,
                has_midi_output,
            },
        }),
        HostResponse::Error { message } => Err(message),
        _ => Err("Unexpected VST3 metadata response".into()),
    }
}

fn vst3_descriptor(path: &Path, result: Result<Vst3ProbeInfo, String>) -> PluginDescriptor {
    let mut descriptor = descriptor_from_path(path, PluginFormat::Vst3, false);
    // No filename fallback: a failed probe or missing category is unknown, not an instrument.
    descriptor.is_instrument = false;
    descriptor.category = "Unknown".into();
    let info = match result {
        Ok(info) => info,
        Err(error) => {
            descriptor.scan_error = Some(error);
            return descriptor;
        }
    };
    let has_token = |expected: &str| {
        info.metadata
            .category
            .split('|')
            .any(|token| token.trim().eq_ignore_ascii_case(expected))
    };
    descriptor.is_instrument = has_token("Instrument");
    descriptor.category = if descriptor.is_instrument {
        "Instrument"
    } else if has_token("Fx") {
        "Effect"
    } else if info.metadata.has_midi_output {
        "MIDI"
    } else {
        "Unknown"
    }
    .into();
    if !info.name.trim().is_empty() {
        descriptor.name = info.name;
    }
    if !info.vendor.trim().is_empty() {
        descriptor.vendor = info.vendor;
    }
    descriptor.verified = true;
    descriptor.vst3_metadata = Some(info.metadata);
    descriptor
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
        vst3_metadata: None,
        scan_error: None,
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

/// Shared persistable load description. The saved UID remains the historical descriptor ID;
/// it is not silently reinterpreted as a factory class UID by the bounce path.
pub(crate) fn plugin_load_spec_from_instance(
    instance: &crate::model::PluginInstance,
) -> plugin_runtime::PluginLoadSpec {
    let mut spec = plugin_load_identity_from_instance(instance);
    spec.initial_state.clone_from(&instance.opaque_state);
    spec
}

/// A captured occurrence restores only its authoritative capture, so preparation
/// must not retain another copy of the potentially stale Project state.
pub(crate) fn plugin_load_identity_from_instance(
    instance: &crate::model::PluginInstance,
) -> plugin_runtime::PluginLoadSpec {
    use crate::model::{PluginFormat as StoredFormat, PluginRole};
    plugin_runtime::PluginLoadSpec {
        descriptor: PluginDescriptor {
            id: instance.uid.clone(),
            name: instance.name.clone(),
            vendor: instance.vendor.clone(),
            path: instance.path.clone(),
            format: match instance.format {
                StoredFormat::Vst2 => PluginFormat::Vst2,
                StoredFormat::Vst3 => PluginFormat::Vst3,
            },
            category: match instance.role {
                PluginRole::Instrument => "Instrument",
                PluginRole::Effect => "Effect",
                PluginRole::Unknown => "Unknown role",
            }
            .to_owned(),
            is_instrument: instance.role == PluginRole::Instrument,
            verified: instance.path.exists(),
            vst3_metadata: None,
            scan_error: None,
        },
        class_uid: None,
        vst3_helper_path: None,
        initial_state: Vec::new(),
        enabled: instance.enabled,
        bypassed: instance.bypass,
        wet: instance.wet,
    }
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

    /// Explicit new-load opt-in; ordinary identified chains are unchanged.
    pub(crate) fn spawn_fresh_bounce_source(
        &self,
        instance_id: u64,
        mut spec: plugin_runtime::PluginLoadSpec,
        sample_rate: f32,
        max_block_frames: u32,
    ) -> Result<plugin_runtime::PluginChain, String> {
        prepare_runtime_spec(&mut spec)?;
        #[cfg(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        ))]
        {
            plugin_runtime::PluginChain::spawn_fresh_bounce_source(
                instance_id,
                spec,
                plugin_runtime::PluginPrepareConfig {
                    sample_rate: f64::from(sample_rate),
                    max_block_frames: max_block_frames as usize,
                },
            )
        }
        #[cfg(not(all(
            any(target_os = "linux", windows),
            feature = "experimental-fresh-helper"
        )))]
        {
            let _ = (instance_id, spec, sample_rate, max_block_frames);
            Err(
                "Experimental fresh-source loading requires Linux or Windows and experimental-fresh-helper"
                    .into(),
            )
        }
    }

    /// Session-bound recreation from authoritative conversion history.
    #[allow(clippy::too_many_arguments)] // explicit session/class/source authority
    pub(crate) fn spawn_recreated_source(
        &self,
        project_session: u64,
        instance_id: u64,
        mut spec: plugin_runtime::PluginLoadSpec,
        sample_rate: f32,
        max_block_frames: u32,
        expectation: plugin_runtime::SourceRestorationExpectation,
        fresh_load: bool,
    ) -> Result<plugin_runtime::PluginChain, String> {
        prepare_runtime_spec(&mut spec)?;
        #[cfg(feature = "vst3")]
        {
            plugin_runtime::PluginChain::spawn_recreated_source(
                project_session,
                instance_id,
                spec,
                plugin_runtime::PluginPrepareConfig {
                    sample_rate: f64::from(sample_rate),
                    max_block_frames: max_block_frames as usize,
                },
                expectation,
                fresh_load,
            )
        }
        #[cfg(not(feature = "vst3"))]
        {
            let _ = (
                project_session,
                instance_id,
                spec,
                sample_rate,
                max_block_frames,
                expectation,
                fresh_load,
            );
            Err("This build does not include VST3 source restoration".into())
        }
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

    fn metadata(category: &str, midi_in: bool, midi_out: bool) -> Vst3ProbeInfo {
        Vst3ProbeInfo {
            name: "Actual plugin name".into(),
            vendor: "Actual vendor".into(),
            lifecycle: None,
            metadata: Vst3ScanMetadata {
                class_uid: "ABCDEF019182FAEB566D624153465854".into(),
                category: category.into(),
                has_midi_input: midi_in,
                has_midi_output: midi_out,
            },
        }
    }

    // Synthetic traversal evidence only. Real child/reap acceptance belongs to
    // scan_lifecycle's protocol fixtures; these rows never claim SDK execution.
    fn synthetic_receipt(path: &Path, helper_pid: u32) -> scan_lifecycle::ProbeLifecycle {
        scan_lifecycle::ProbeLifecycle {
            path: path.to_path_buf(),
            helper_pid: Some(helper_pid),
            load_failure: None,
            reaped: true,
            exit_success: true,
            exit_code: Some(0),
            kill_requested: false,
            shutdown_failure: None,
            kill_error: None,
            wait_error: None,
            pipe_error: None,
        }
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "citrus-plugin-scan-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn vst3_factory_metadata_overrides_misleading_filenames() {
        let effect = vst3_descriptor(
            Path::new("Surge XT Effects.vst3"),
            Ok(metadata("Fx", false, false)),
        );
        assert_eq!(effect.category, "Effect");
        assert!(!effect.is_instrument);
        assert!(effect.verified);
        assert_eq!(effect.name, "Actual plugin name");
        assert_eq!(effect.vendor, "Actual vendor");
        assert_eq!(effect.id, "VST3:Surge XT Effects.vst3");

        let synth = vst3_descriptor(
            Path::new("Delay_Filter_FX.vst3"),
            Ok(metadata("Instrument|Synth", true, false)),
        );
        assert_eq!(synth.category, "Instrument");
        assert!(synth.is_instrument);
        assert!(synth.has_midi_input());
        assert!(!synth.has_midi_output());
    }

    #[test]
    fn vst3_category_tokens_and_midi_capabilities_are_independent() {
        for category in ["Fx", "Fx|Delay", "  fx | Filter  "] {
            let effect = vst3_descriptor(
                Path::new("Keyboard.vst3"),
                Ok(metadata(category, true, false)),
            );
            assert_eq!(effect.category, "Effect");
            assert!(!effect.is_instrument);
            assert!(effect.has_midi_input());
        }
        let midi_only = vst3_descriptor(Path::new("Generator.vst3"), Ok(metadata("", false, true)));
        assert_eq!(midi_only.category, "MIDI");
        assert!(!midi_only.is_instrument);
        assert!(midi_only.has_midi_output());
        assert!(!midi_only.has_midi_input());

        let instrument_without_events = vst3_descriptor(
            Path::new("Instrument.vst3"),
            Ok(metadata("Instrument", false, false)),
        );
        assert!(instrument_without_events.is_instrument);
        assert!(!instrument_without_events.has_midi_input());
        assert!(!instrument_without_events.has_midi_output());
    }

    #[test]
    fn vst3_unrecognized_or_absent_category_never_uses_filename_hints() {
        for category in ["", "Instrumental|FxLike", "Audio Module Class"] {
            for name in ["Synth.vst3", "Delay FX.vst3"] {
                let plugin = vst3_descriptor(Path::new(name), Ok(metadata(category, false, false)));
                assert_eq!(plugin.category, "Unknown");
                assert!(!plugin.is_instrument);
                assert!(plugin.verified); // successfully loaded; classification is still unknown
            }
        }
    }

    #[test]
    fn failed_vst3_probe_is_unknown_and_unverified() {
        let plugin = vst3_descriptor(Path::new("Piano.vst3"), Err("timed out".into()));
        assert_eq!(plugin.category, "Unknown");
        assert!(!plugin.is_instrument);
        assert!(!plugin.verified);
        assert!(plugin.vst3_metadata.is_none());
        assert!(!plugin.has_midi_input());
        assert!(!plugin.has_midi_output());
        assert_eq!(plugin.scan_error.as_deref(), Some("timed out"));
    }

    #[test]
    fn legacy_cache_verification_never_attests_vst3_metadata() {
        let old = r#"[
            {"id":"VST3:Surge XT Effects.vst3","name":"Surge XT Effects",
             "vendor":"Unknown vendor","path":"Surge XT Effects.vst3","format":"Vst3",
             "category":"Instrument","is_instrument":true,"verified":true},
            {"id":"VST2:Old.dll","name":"Old","vendor":"Old vendor","path":"Old.dll",
             "format":"Vst2","category":"Instrument","is_instrument":true,"verified":true}
        ]"#;
        // Structural decoding preserves legacy values; only the cache loader invalidates the
        // old VST3 index. Never change the serialized project role/path to repair a scan cache.
        let decoded: Vec<PluginDescriptor> = serde_json::from_str(old).unwrap();
        assert!(decoded[0].is_instrument);
        assert!(decoded[0].verified);
        assert!(!decoded[0].has_midi_input());
        assert!(!decoded[0].has_midi_output());
        let cache = decode_cache(old).unwrap();
        assert!(cache.needs_rescan);
        assert_eq!(cache.plugins.len(), 1);
        assert_eq!(cache.plugins[0].id, "VST2:Old.dll");
        assert!(cache.plugins[0].is_instrument);
    }

    #[test]
    fn authoritative_cache_round_trip_preserves_exact_class_and_path_identity() {
        let plugin = vst3_descriptor(
            Path::new("Multiple Classes.vst3"),
            Ok(metadata("Fx|Delay", false, true)),
        );
        let serialized = serde_json::to_string(&vec![plugin.clone()]).unwrap();
        let cache = decode_cache(&serialized).unwrap();
        assert!(!cache.needs_rescan);
        assert_eq!(cache.plugins.len(), 1);
        assert_eq!(cache.plugins[0].id, plugin.id);
        assert_eq!(cache.plugins[0].path, plugin.path);
        assert_eq!(cache.plugins[0].vst3_metadata, plugin.vst3_metadata);
        assert!(cache.plugins[0].has_midi_output());
        assert_eq!(cache.plugins[0].category, "Effect");
    }

    #[test]
    fn failed_probe_cache_preserves_actionable_error_without_claiming_verification() {
        let plugin = vst3_descriptor(
            Path::new("Missing helper.vst3"),
            Err("helper missing".into()),
        );
        let serialized = serde_json::to_string(&vec![plugin]).unwrap();
        let cache = decode_cache(&serialized).unwrap();
        assert!(!cache.needs_rescan); // A current, explicit failure is not a legacy heuristic.
        assert_eq!(cache.plugins.len(), 1);
        assert_eq!(
            cache.plugins[0].scan_error.as_deref(),
            Some("helper missing")
        );
        assert!(!cache.plugins[0].verified);
        assert_eq!(cache.plugins[0].category, "Unknown");
    }

    #[test]
    fn scan_indexes_only_the_probed_default_class_and_skips_bundle_internals() {
        let dir = TestDirectory::new();
        let bundle = dir.0.join("Multi Class Synth.vst3");
        let nested = bundle.join("Contents/x86_64-win/Incorrect Instrument.vst3");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, b"not an executable").unwrap();
        let mut calls = Vec::new();
        let mut probe = |path: &Path| {
            calls.push(path.to_path_buf());
            Ok(metadata("Fx|Delay", false, false))
        };
        let outcome = scan_with_probe(
            &[dir.0.clone(), bundle.clone()],
            &AtomicBool::new(false),
            &mut probe,
        );
        let ScanOutcome::Completed(report) = outcome else {
            panic!("synthetic default-class scan unexpectedly stopped")
        };
        let plugins = report.plugins;
        assert_eq!(calls, vec![bundle.clone()]);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].path, bundle);
        assert_eq!(plugins[0].category, "Effect");
        assert!(!plugins[0].is_instrument);
    }

    #[test]
    fn cancelling_scan_stops_before_next_probe_and_discards_partial_results() {
        let dir = TestDirectory::new();
        for name in ["One.vst3", "Two.vst3"] {
            std::fs::create_dir(dir.0.join(name)).unwrap();
        }
        let cancelled = AtomicBool::new(true);
        let mut calls = 0;
        let mut probe = |_: &Path| {
            calls += 1;
            cancelled.store(true, Ordering::Relaxed);
            Ok(metadata("Fx", false, false))
        };
        assert!(matches!(
            scan_with_probe(std::slice::from_ref(&dir.0), &cancelled, &mut probe),
            ScanOutcome::Stopped(ScanStopped {
                kind: ScanStopKind::Cancelled,
                ..
            })
        ));
        cancelled.store(false, Ordering::Relaxed);
        assert!(matches!(
            scan_with_probe(std::slice::from_ref(&dir.0), &cancelled, &mut probe),
            ScanOutcome::Stopped(ScanStopped {
                kind: ScanStopKind::Cancelled,
                ..
            })
        ));
        assert_eq!(calls, 1);
    }

    #[test]
    fn dropping_scan_job_sets_cancellation_without_joining_worker() {
        let (_, receiver) = mpsc::channel();
        let control = ScanControl::new();
        let scan = PluginScan {
            receiver,
            control: control.clone(),
        };
        drop(scan);
        assert!(control.cancelled());
    }

    #[test]
    fn cancelled_probe_keeps_retirement_evidence_without_publishing_partial_index() {
        let dir = TestDirectory::new();
        for name in ["First.vst3", "Second.vst3"] {
            std::fs::create_dir(dir.0.join(name)).unwrap();
        }
        let cancelled = AtomicBool::new(false);
        let mut calls = Vec::new();
        let mut probe = |path: &Path| {
            calls.push(path.to_path_buf());
            let mut info = metadata("Instrument", true, false);
            info.lifecycle = Some(synthetic_receipt(path, 101));
            cancelled.store(true, Ordering::Release);
            Ok(info)
        };
        let outcome = scan_with_probe(std::slice::from_ref(&dir.0), &cancelled, &mut probe);
        let ScanOutcome::Stopped(stopped) = outcome else {
            panic!("cancelled scan published its partial plugin index")
        };
        assert!(matches!(stopped.kind, ScanStopKind::Cancelled));
        assert_eq!(calls.len(), 1);
        assert_eq!(stopped.probe_lifecycle.len(), 1);
        assert_eq!(stopped.probe_lifecycle[0].path, calls[0]);
        assert_eq!(stopped.probe_lifecycle[0].helper_pid, Some(101));
        assert!(stopped.probe_lifecycle[0].reaped);
    }

    #[test]
    fn cancelling_last_probe_still_stops_instead_of_publishing_completed_report() {
        let dir = TestDirectory::new();
        let bundle = dir.0.join("Only.vst3");
        std::fs::create_dir(&bundle).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut calls = 0;
        let mut probe = |path: &Path| {
            calls += 1;
            let mut info = metadata("Fx", false, false);
            info.lifecycle = Some(synthetic_receipt(path, 102));
            cancelled.store(true, Ordering::Release);
            Ok(info)
        };
        let outcome = scan_with_probe(std::slice::from_ref(&bundle), &cancelled, &mut probe);
        let ScanOutcome::Stopped(stopped) = outcome else {
            panic!("cancellation at traversal EOF became Completed")
        };
        assert!(matches!(stopped.kind, ScanStopKind::Cancelled));
        assert_eq!(calls, 1);
        assert_eq!(stopped.probe_lifecycle.len(), 1);
        assert_eq!(stopped.probe_lifecycle[0].path, bundle);
    }

    #[test]
    fn stopped_probe_preserves_previous_current_and_nested_receipts_without_next_probe() {
        let dir = TestDirectory::new();
        for name in ["First.vst3", "Second.vst3", "Third.vst3"] {
            std::fs::create_dir(dir.0.join(name)).unwrap();
        }
        let mut calls = Vec::new();
        let mut expected_receipts = Vec::new();
        let mut probe = |path: &Path| {
            calls.push(path.to_path_buf());
            if calls.len() == 1 {
                let receipt = synthetic_receipt(path, 201);
                expected_receipts.push(receipt.clone());
                let mut info = metadata("Fx", false, false);
                info.lifecycle = Some(receipt);
                return Ok(info);
            }
            let mut current = synthetic_receipt(path, 202);
            current.reaped = false;
            current.exit_success = false;
            current.exit_code = None;
            current.wait_error = Some(5);
            let nested = synthetic_receipt(&dir.0.join("retained-callee-evidence"), 203);
            expected_receipts.extend([current.clone(), nested.clone()]);
            let mut stopped =
                ScanStopped::new(ScanStopKind::CleanupPending, "synthetic pending owner");
            stopped.probe_lifecycle.push(nested);
            Err(ProbeFailure {
                message: "synthetic pending owner".into(),
                stopped: Some(stopped),
                lifecycle: Some(Box::new(current)),
            })
        };
        let outcome = scan_with_probe(
            std::slice::from_ref(&dir.0),
            &AtomicBool::new(false),
            &mut probe,
        );
        let ScanOutcome::Stopped(stopped) = outcome else {
            panic!("pending owner published a partial scan report")
        };
        assert!(matches!(stopped.kind, ScanStopKind::CleanupPending));
        assert_eq!(stopped.detail, "synthetic pending owner");
        assert_eq!(calls.len(), 2);
        assert_eq!(
            serde_json::to_value(&stopped.probe_lifecycle).unwrap(),
            serde_json::to_value(&expected_receipts).unwrap()
        );
    }

    #[test]
    fn reaped_metadata_failure_is_unverified_and_keeps_receipt_in_completed_report() {
        let dir = TestDirectory::new();
        for name in ["First.vst3", "Second.vst3"] {
            std::fs::create_dir(dir.0.join(name)).unwrap();
        }
        let mut calls = Vec::new();
        let mut probe = |path: &Path| {
            calls.push(path.to_path_buf());
            let mut receipt = synthetic_receipt(path, 301 + calls.len() as u32);
            if calls.len() == 1 {
                let mut info = metadata("Fx", false, false);
                info.lifecycle = Some(receipt);
                return Ok(info);
            }
            receipt.load_failure = Some("synthetic refused metadata".into());
            let mut failed = ProbeFailure::metadata("synthetic refused metadata");
            failed.lifecycle = Some(Box::new(receipt));
            Err(failed)
        };
        let outcome = scan_with_probe(
            std::slice::from_ref(&dir.0),
            &AtomicBool::new(false),
            &mut probe,
        );
        let ScanOutcome::Completed(report) = outcome else {
            panic!("fully reaped per-plugin metadata failure stopped the entire scan")
        };
        assert_eq!(calls.len(), 2);
        assert_eq!(report.plugins.len(), 2);
        assert_eq!(report.probe_lifecycle.len(), 2);
        let succeeded = report
            .plugins
            .iter()
            .find(|plugin| plugin.path == calls[0])
            .unwrap();
        assert!(succeeded.verified);
        let failed = report
            .plugins
            .iter()
            .find(|plugin| plugin.path == calls[1])
            .unwrap();
        assert!(!failed.verified);
        assert_eq!(failed.category, "Unknown");
        assert!(failed.vst3_metadata.is_none());
        assert_eq!(
            failed.scan_error.as_deref(),
            Some("synthetic refused metadata")
        );
        assert_eq!(report.probe_lifecycle[1].path, calls[1]);
        assert_eq!(
            report.probe_lifecycle[1].load_failure.as_deref(),
            Some("synthetic refused metadata")
        );
        assert!(report.probe_lifecycle[1].reaped);
    }

    #[cfg(all(feature = "vst3", unix))]
    #[test]
    fn isolated_metadata_probe_bounds_a_hung_helper_load() {
        #[cfg(target_os = "linux")]
        let _guard = helper_transport_test_lock();
        use std::{
            os::unix::fs::PermissionsExt,
            time::{Duration, Instant},
        };
        let dir = TestDirectory::new();
        let helper = dir.0.join("trusted-hung-test-helper");
        // Repository-owned protocol fixture, not plugin code. exec avoids orphaning a child.
        std::fs::write(&helper, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let start = Instant::now();
        let result = probe_vst3_with_timeout(
            Path::new("not-loaded.vst3"),
            &helper,
            Duration::from_millis(100),
        );
        let Err(error) = result else {
            panic!("hung helper unexpectedly replied")
        };
        let expected = if cfg!(target_os = "linux") {
            "Deadline"
        } else {
            "Timed out"
        };
        assert!(error.contains(expected), "{error}");
        assert!(start.elapsed() < Duration::from_secs(4));
    }

    #[cfg(all(feature = "vst3", unix))]
    #[test]
    fn isolated_metadata_request_uses_only_runtime_default_class_without_gui_or_dsp() {
        #[cfg(target_os = "linux")]
        let _guard = helper_transport_test_lock();
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDirectory::new();
        let helper = dir.0.join("trusted-metadata-test-helper");
        let request = dir.0.join("request.json");
        let response =
            serde_json::to_string(&vst3_host::process_isolation::HostResponse::PluginInfo {
                vendor: "Fixture vendor".into(),
                name: "Default effect class".into(),
                version: "1".into(),
                category: "Fx".into(),
                uid: "ABCDEF019182FAEB566D624153465854".into(),
                has_gui: true,
                audio_inputs: 1,
                audio_outputs: 1,
                output_channels: 2,
                has_midi_input: false,
                has_midi_output: true,
                compatibility: Vec::new(),
            })
            .unwrap();
        std::fs::write(&helper, format!(
            "#!/bin/sh\nIFS= read -r line\nprintf '%s' \"$line\" > '{}'\nprintf '%s\\n' '{}'\nIFS= read -r shutdown\n",
            request.display(), response
        )).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let info =
            probe_vst3(Path::new("Multi Class.vst3"), &helper).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(info.name, "Default effect class");
        assert!(info.metadata.has_midi_output);
        let command: serde_json::Value =
            serde_json::from_slice(&std::fs::read(request).unwrap()).unwrap();
        assert!(command["LoadPlugin"]["class_id"].is_null());
        assert_eq!(command["LoadPlugin"]["path"], "Multi Class.vst3");
    }

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
