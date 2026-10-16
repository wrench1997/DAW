//! Additive, explicitly pinned complete Saved13 input. This does not create a
//! source worker, render owner, streaming observer or native qualification run.
//! The existing short Inputs::load -> scratch -> whole_scratch is unchanged.
use super::*;
use crate::{
    automation::{AutomationCurve, AutomationTarget},
    mixer_graph::MASTER_MIXER_TRACK_ID,
    model::{CURRENT_PROJECT_FORMAT_VERSION, PluginInstance, ProjectMigrationDiagnostic},
};

const FULL_INPUT_OPT_IN: &str = "actual-noire-complete-saved13-input-v1";
const SAVED13_FORMAT_VERSION: u32 = 12;
const SAVED13_BYTES: usize = 1_298_729;
const SAVED13_STATE_BYTES: usize = 591_772;
const SAVED13_STATE_SHA256: &str =
    "5d902e91f5eb267f4cf71bd3c39f12f8551076017708530a243015adc075502b";
const SAVED13_BUNDLE: &str = r"C:\Program Files\Common Files\VST3\Kontakt 8.vst3";
const PROJECT_BYTE_LIMIT: u64 = 2 * 1024 * 1024;

/// File/state pins establish input identity, not the identity of a live SDK
/// instance or the recovery of every internal NI parameter.
pub(super) struct FullInputPins {
    pub project_path: PathBuf,
    pub project_bytes: usize,
    pub project_sha256: String,
    pub saved_state_bytes: usize,
    pub saved_state_sha256: String,
    pub actual_bundle_path: PathBuf,
}

/// Stable persisted identities; no vector index or synthetic source identity.
pub(super) struct ValidatedSource {
    pub plugin_instance_id: u64,
    pub channel_id: u32,
    pub descriptor: PluginDescriptor,
    pub state_bytes: usize,
    pub state_sha256: String,
}

pub(super) struct FullInputs {
    pub project: Project,
    pub source: ValidatedSource,
    pub pins: FullInputPins,
}

/// Covers every persisted model field and each field intentionally skipped by
/// Project serialization. Keep comparison private: snapshots contain opaque
/// bytes encoded by the production serializer and must never be logged.
#[derive(PartialEq)]
struct ModelSnapshot {
    persisted: Vec<u8>,
    legacy_piano_notes: Vec<u8>,
    pattern_selection: Vec<Vec<bool>>,
    legacy_selection: Vec<bool>,
    runtime_statuses: Vec<(u64, PluginRuntimeStatus)>,
    waveform_bits: Vec<(u64, Vec<u32>)>,
    migration_diagnostics: Vec<ProjectMigrationDiagnostic>,
}

fn model_snapshot(project: &Project) -> Check<ModelSnapshot> {
    Ok(ModelSnapshot {
        persisted: serde_json::to_vec(project).map_err(error)?,
        legacy_piano_notes: serde_json::to_vec(&project.piano_notes).map_err(error)?,
        pattern_selection: project
            .patterns
            .iter()
            .map(|pattern| pattern.notes.iter().map(|note| note.selected).collect())
            .collect(),
        legacy_selection: project
            .piano_notes
            .iter()
            .map(|note| note.selected)
            .collect(),
        runtime_statuses: project
            .plugin_instances
            .iter()
            .map(|instance| (instance.id, instance.runtime_status))
            .collect(),
        waveform_bits: project
            .audio_assets
            .iter()
            .map(|asset| {
                (
                    asset.id,
                    asset
                        .waveform_peaks
                        .iter()
                        .map(|peak| peak.to_bits())
                        .collect(),
                )
            })
            .collect(),
        migration_diagnostics: project.migration_diagnostics.clone(),
    })
}

fn require_unchanged_saved13_load(decoded: &Project, loaded: &Project) -> Check {
    require(
        decoded.format_version == SAVED13_FORMAT_VERSION,
        "pinned full input has an unexpected original project format",
    )?;
    // Project::load advances this pinned v12 file to the current schema. Allow
    // that one field only: normalizing the expected model would hide repairs
    // to notes, state, bases, routing or legacy defaults (AUTO false/no policy).
    let mut expected = decoded.clone();
    expected.format_version = CURRENT_PROJECT_FORMAT_VERSION;
    require(
        model_snapshot(loaded)? == model_snapshot(&expected)?,
        "production Project::load repaired or changed the pinned full model",
    )
}

/// General source-model checks are intentionally separate from Saved13 pins.
/// Passing synthetic data here never qualifies it as the real full input.
fn validate_source_model(project: &Project) -> Check<ValidatedSource> {
    require(
        project.tempo.is_finite()
            && (20.0..=400.0).contains(&project.tempo)
            && project.swing.is_finite()
            && (0.0..=1.0).contains(&project.swing)
            && project.song_length_beats.is_finite()
            && (4.0..=4096.0).contains(&project.song_length_beats),
        "full input has invalid persisted transport bases",
    )?;
    let plugin_ids: BTreeSet<_> = project
        .plugin_instances
        .iter()
        .map(|item| item.id)
        .collect();
    let channel_ids: BTreeSet<_> = project.channels.iter().map(|item| item.id).collect();
    require(
        !plugin_ids.contains(&0)
            && plugin_ids.len() == project.plugin_instances.len()
            && !channel_ids.contains(&0)
            && channel_ids.len() == project.channels.len(),
        "full input has invalid or duplicate plugin/channel identities",
    )?;
    let mut sources = project
        .plugin_instances
        .iter()
        .filter(|item| item.uid == KONTAKT_UID);
    let source = sources.next().ok_or("full input has no Kontakt class")?;
    require(
        sources.next().is_none(),
        "full input Kontakt class is ambiguous",
    )?;
    require(
        source.format == ModelPluginFormat::Vst3
            && source.role == PluginRole::Instrument
            && source.enabled
            && !source.bypass
            && source.wet.is_finite()
            && (0.0..=1.0).contains(&source.wet),
        "full input source is not an enabled non-bypassed VST3 instrument",
    )?;
    require(
        source.path.is_absolute()
            && source.parameters.len() <= 4096
            && source
                .parameters
                .values()
                .all(|base| base.is_finite() && (0.0..=1.0).contains(base)),
        "full input has an invalid source path or parameter bases",
    )?;
    state_envelope(&source.opaque_state)?;
    let mut channels = project
        .channels
        .iter()
        .filter(|item| item.instrument_plugin_instance_id == Some(source.id));
    let channel = channels
        .next()
        .ok_or("full input source has no channel assignment")?;
    require(
        channels.next().is_none(),
        "full input source assignment is ambiguous",
    )?;
    require(
        channel.volume.is_finite()
            && (0.0..=1.0).contains(&channel.volume)
            && channel.pan.is_finite()
            && (-1.0..=1.0).contains(&channel.pan)
            && project.mixer_tracks.iter().all(|track| {
                track.volume.is_finite()
                    && (0.0..=1.5).contains(&track.volume)
                    && track.pan.is_finite()
                    && (-1.0..=1.0).contains(&track.pan)
                    && track.peak.is_finite()
                    && (0.0..=1.0).contains(&track.peak)
            }),
        "full input has invalid channel or mixer bases",
    )?;
    project.validate_mixer_graph().map_err(error)?;
    require(
        project
            .mixer_tracks
            .iter()
            .filter(|track| track.id == channel.mixer_track)
            .count()
            == 1,
        "full input source channel has no unique mixer destination",
    )?;
    let descriptor = plugins::plugin_load_identity_from_instance(source).descriptor;
    // Metadata remains None until an actual scanner/helper establishes it.
    // A saved UID/path is not authoritative factory/version evidence.
    require(
        descriptor.id == source.uid
            && descriptor.path == source.path
            && descriptor.format == plugins::PluginFormat::Vst3
            && descriptor.is_instrument
            && descriptor.vst3_metadata.is_none(),
        "full input descriptor differs from the persisted source identity",
    )?;
    Ok(ValidatedSource {
        plugin_instance_id: source.id,
        channel_id: channel.id,
        descriptor,
        state_bytes: source.opaque_state.len(),
        state_sha256: oracles::sha256_hex(&source.opaque_state),
    })
}

fn retain_validated_project(project: Project) -> Check<(Project, ValidatedSource)> {
    let source = validate_source_model(&project)?;
    // Move the complete model back to the caller; never construct a blank,
    // change gains/routes, clear lanes, or project it into the short scratch.
    Ok((project, source))
}

fn require_saved13_file(bytes: &[u8]) -> Check<String> {
    require(
        bytes.len() == SAVED13_BYTES,
        "full input is not the complete pinned Saved13 file",
    )?;
    let hash = oracles::sha256_hex(bytes);
    require(
        hash == SOURCE_SHA256,
        "full input differs from pinned Saved13",
    )?;
    Ok(hash)
}

fn require_saved13_model(project: &Project, source: &ValidatedSource) -> Check {
    // These descriptive checks run only after the immutable whole-file pin.
    // They never replace the digest with a shape/count-based admission.
    require(
        source.plugin_instance_id == 1
            && source.channel_id == 1
            && source.state_bytes == SAVED13_STATE_BYTES
            && source.state_sha256 == SAVED13_STATE_SHA256
            && project.plugin_instances.len() == 1
            && project.channels.len() == 1
            && project.tempo == 84.0
            && project.swing == 0.0
            && project.song_length_beats == 160.0
            && project
                .patterns
                .iter()
                .map(|pattern| pattern.notes.len())
                .collect::<Vec<_>>()
                == [43, 106, 106, 107, 43]
            && project.clips.len() == 5
            && project.mixer_routes.len() == 31
            && project.mixer_tracks.len() == 32,
        "pinned full input no longer has the complete Saved13 model",
    )?;
    let channel = project
        .channels
        .iter()
        .find(|item| item.id == source.channel_id)
        .ok_or("pinned source channel is absent")?;
    let master = project
        .mixer_tracks
        .iter()
        .find(|item| item.id == MASTER_MIXER_TRACK_ID)
        .ok_or("pinned MASTER is absent")?;
    require(
        channel.volume == 0.55
            && channel.pan == 0.0
            && channel.mixer_track == MASTER_MIXER_TRACK_ID
            && !channel.muted
            && !channel.solo
            && master.volume == 0.7
            && master.pan == 0.0
            && !master.muted
            && !master.solo,
        "pinned original channel or MASTER bases/routing changed",
    )?;
    let placements = [
        (0.0, 16.0),
        (16.0, 32.0),
        (48.0, 32.0),
        (80.0, 32.0),
        (112.0, 16.0),
    ];
    require(
        project
            .clips
            .iter()
            .zip(placements)
            .all(|(clip, (start, length))| {
                clip.kind == ClipKind::Pattern
                    && clip.start == start
                    && clip.length == length
                    && clip.source_offset == 0.0
                    && clip.gain == 1.0
                    && !clip.muted
            }),
        "pinned placements or the intentional 128..160 empty coda changed",
    )?;
    require(
        project.automation_lanes.len() == 1,
        "pinned Master automation is absent",
    )?;
    let lane = &project.automation_lanes[0].lane;
    require(
        lane.target() == &AutomationTarget::MasterVolume
            && lane.is_enabled()
            && lane.curve() == AutomationCurve::Linear
            && lane.loop_region().is_none()
            && lane.points().len() == 3
            && lane
                .points()
                .iter()
                .zip([(0.0, 0.7), (120.0, 0.7), (128.0, 0.56)])
                .all(|(point, (position, value))| {
                    point.position == position && point.value == value && point.tension == 0.0
                }),
        "pinned global Master automation changed; it replaces the base and is not a second gain",
    )
}

impl FullInputs {
    pub(super) fn load_from_env() -> Check<Self> {
        require(
            std::env::var("CITRUS_NOIRE_FULL_INPUT_QA").ok().as_deref() == Some(FULL_INPUT_OPT_IN),
            "set the explicit complete Saved13 input opt-in",
        )?;
        Self::load_saved13(&absolute_env("CITRUS_NOIRE_FULL_QA_PROJECT")?)
    }

    fn load_saved13(path: &Path) -> Check<Self> {
        require(path.is_absolute(), "full input path must be absolute")?;
        let project_path = path.canonicalize().map_err(error)?;
        // Windows share-read permits Project::load to open this very path while
        // prohibiting a concurrent writer/delete/rename during the pin/load.
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let _read_lock = options.open(&project_path).map_err(error)?;
        let before = read_bounded(&project_path, PROJECT_BYTE_LIMIT)?;
        let project_sha256 = require_saved13_file(&before)?;
        let decoded: Project = serde_json::from_slice(&before).map_err(error)?;
        let project = Project::load(&project_path).map_err(error)?;
        require_unchanged_saved13_load(&decoded, &project)?;
        let after = read_bounded(&project_path, PROJECT_BYTE_LIMIT)?;
        require(
            before == after,
            "pinned full input changed during Project::load",
        )?;
        let (project, source) = retain_validated_project(project)?;
        require_saved13_model(&project, &source)?;
        require(
            source.descriptor.path == Path::new(SAVED13_BUNDLE),
            "pinned full input has an unexpected saved Kontakt path",
        )?;
        let actual_bundle_path = source.descriptor.path.canonicalize().map_err(error)?;
        require(
            actual_bundle_path == Path::new(SAVED13_BUNDLE).canonicalize().map_err(error)?
                && actual_bundle_path.exists(),
            "the actual saved Kontakt bundle path is absent or changed",
        )?;
        Ok(Self {
            pins: FullInputPins {
                project_path,
                project_bytes: before.len(),
                project_sha256,
                saved_state_bytes: source.state_bytes,
                saved_state_sha256: source.state_sha256.clone(),
                actual_bundle_path,
            },
            project,
            source,
        })
    }
}

#[cfg(test)]
include!("windows_noire_full_inputs_tests.rs");
