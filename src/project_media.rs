//! Explicit, read-only-on-disk recovery of moved project audio.
//!
//! Relinking changes only the selected asset's reference. WAV decoding and filesystem checks
//! run on a single control-side worker, never on the audio callback. A reviewed result belongs
//! to one window epoch, project session and exact original asset identity.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::SystemTime,
};

use anyhow::{Context, Result, ensure};

use crate::{
    model::{AudioAsset, Project},
    wav::{self, WavAsset, WavMetadata},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaIdentity {
    pub id: u64,
    pub path: PathBuf,
    sample_rate: u32,
    channels: u16,
    frames: u64,
    bits_per_sample: u16,
}

impl From<&AudioAsset> for MediaIdentity {
    fn from(asset: &AudioAsset) -> Self {
        Self {
            id: asset.id,
            path: asset.path.clone(),
            sample_rate: asset.sample_rate,
            channels: asset.channels,
            frames: asset.frames,
            bits_per_sample: asset.bits_per_sample,
        }
    }
}

pub fn media_references_changed(before: &[AudioAsset], after: &[AudioAsset]) -> bool {
    before.iter().map(MediaIdentity::from).collect::<Vec<_>>()
        != after.iter().map(MediaIdentity::from).collect::<Vec<_>>()
}

#[derive(Clone, Debug)]
pub struct MediaReport {
    pub identity: MediaIdentity,
    /// Availability is intentionally not a claim that an unplayed file was decoded.
    pub error: Option<String>,
}

fn inspect_media(identities: Vec<MediaIdentity>) -> Vec<MediaReport> {
    identities
        .into_iter()
        .map(|identity| {
            let result = fs::File::open(&identity.path).and_then(|file| {
                if file.metadata()?.is_file() {
                    Ok(())
                } else {
                    Err(std::io::Error::other("The path is not a regular file"))
                }
            });
            MediaReport {
                identity,
                error: result.err().map(|error| error.to_string()),
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MediaFileStamp {
    bytes: u64,
    modified: SystemTime,
}

impl MediaFileStamp {
    fn read(path: &Path) -> Result<Self> {
        let metadata =
            fs::metadata(path).with_context(|| format!("Unable to inspect {}", path.display()))?;
        ensure!(
            metadata.is_file(),
            "The selected path is not a regular file"
        );
        Ok(Self {
            bytes: metadata.len(),
            modified: metadata
                .modified()
                .context("Unable to verify the file modification time")?,
        })
    }
}

fn validate_replacement(expected: &MediaIdentity, actual: &WavMetadata) -> Result<()> {
    ensure!(
        expected.sample_rate == actual.sample_rate
            && expected.channels == actual.channels
            && expected.frames == actual.frames,
        "Audio properties do not match. Project expects {} Hz, {} channel(s), {} frames; selected file has {} Hz, {} channel(s), {} frames. Choose the original audio to preserve Clip timing and source offsets, or import different audio as a new Clip.",
        expected.sample_rate,
        expected.channels,
        expected.frames,
        actual.sample_rate,
        actual.channels,
        actual.frames,
    );
    Ok(())
}

#[derive(Debug)]
pub struct PreparedMediaRelink {
    session: u64,
    pub expected: MediaIdentity,
    pub replacement: PathBuf,
    pub decoded: WavAsset,
    stamp: MediaFileStamp,
}

impl PreparedMediaRelink {
    fn prepare(session: u64, expected: MediaIdentity, path: &Path) -> Result<Self> {
        // The picker normally supplies absolute paths. Canonicalize explicitly so Save As and
        // future launches do not accidentally reinterpret a reference relative to process CWD.
        let replacement = path
            .canonicalize()
            .with_context(|| format!("Unable to locate {}", path.display()))?;
        let stamp = MediaFileStamp::read(&replacement)?;
        let decoded = wav::read_wav(&replacement)
            .with_context(|| format!("Unable to decode {}", replacement.display()))?;
        validate_replacement(&expected, &decoded.metadata)?;
        ensure!(
            stamp == MediaFileStamp::read(&replacement)?,
            "The selected file changed during decoding. Locate it again."
        );
        Ok(Self {
            session,
            expected,
            replacement,
            decoded,
            stamp,
        })
    }

    fn verify_file(self) -> Result<Self> {
        ensure!(
            self.stamp == MediaFileStamp::read(&self.replacement)?,
            "The selected file changed after review. Locate it again."
        );
        Ok(self)
    }

    pub fn matches(&self, session: u64, assets: &[AudioAsset]) -> bool {
        self.session == session
            && assets
                .iter()
                .filter(|asset| asset.id == self.expected.id)
                .count()
                == 1
            && assets
                .iter()
                .any(|asset| MediaIdentity::from(asset) == self.expected)
    }

    /// Pure project edit: never modifies either source file or the saved project.
    pub fn apply_to(&self, session: u64, project: &mut Project) -> Result<()> {
        ensure!(
            self.matches(session, &project.audio_assets),
            "The project or original media reference changed. Locate the audio again."
        );
        validate_replacement(&self.expected, &self.decoded.metadata)?;
        let asset = project
            .audio_assets
            .iter_mut()
            .find(|asset| asset.id == self.expected.id)
            .expect("matches checked one exact asset");
        asset.path = self.replacement.clone();
        asset.bits_per_sample = self.decoded.metadata.bits_per_sample;
        asset.waveform_peaks.clear();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaJobKind {
    Inspect,
    Prepare,
    Apply,
}

enum MediaWorkerPayload {
    Reports(Vec<MediaReport>),
    Prepared(Result<PreparedMediaRelink, String>),
    Apply(Result<PreparedMediaRelink, String>),
}

struct MediaWorker {
    epoch: u64,
    session: u64,
    kind: MediaJobKind,
    receiver: Receiver<MediaWorkerPayload>,
}

#[derive(Default)]
pub struct ProjectMediaManager {
    pub open: bool,
    pub reports: Vec<MediaReport>,
    pub candidate: Option<PreparedMediaRelink>,
    pub notice: Option<String>,
    epoch: u64,
    worker: Option<MediaWorker>,
    refresh_when_idle: bool,
}

impl ProjectMediaManager {
    pub fn open(&mut self, session: u64, assets: &[AudioAsset]) {
        self.invalidate();
        self.open = true;
        self.reports.clear();
        self.notice = None;
        self.refresh_when_idle = true;
        self.refresh_if_idle(session, assets);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.refresh_when_idle = false;
        self.invalidate();
    }

    pub fn cancel(&mut self) {
        self.refresh_when_idle = false;
        self.invalidate();
        self.notice =
            Some("Pending media check canceled. No additional references were changed.".into());
    }

    fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.candidate = None;
        // Retain the receiver until its worker ends: repeated Close/Open/Cancel cannot create
        // unbounded simultaneous large WAV decodes. No blocking join on the UI thread.
    }

    pub fn busy(&self) -> bool {
        self.worker.is_some()
    }

    pub fn busy_label(&self) -> &'static str {
        match self.worker.as_ref() {
            Some(worker) if worker.epoch != self.epoch => "Finishing canceled media check…",
            Some(worker) => match worker.kind {
                MediaJobKind::Inspect => "Checking media paths…",
                MediaJobKind::Prepare => "Validating selected WAV…",
                MediaJobKind::Apply => "Rechecking selected file before applying…",
            },
            None => "",
        }
    }

    fn start(
        &mut self,
        session: u64,
        kind: MediaJobKind,
        job: impl FnOnce() -> MediaWorkerPayload + Send + 'static,
    ) {
        debug_assert!(self.worker.is_none());
        let (sender, receiver) = mpsc::channel();
        self.worker = Some(MediaWorker {
            epoch: self.epoch,
            session,
            kind,
            receiver,
        });
        std::thread::spawn(move || {
            let _ = sender.send(job());
        });
    }

    pub fn refresh(&mut self, session: u64, assets: &[AudioAsset]) {
        if self.busy() || self.candidate.is_some() {
            return;
        }
        self.refresh_when_idle = true;
        self.refresh_if_idle(session, assets);
    }

    fn refresh_if_idle(&mut self, session: u64, assets: &[AudioAsset]) {
        if !self.open || !self.refresh_when_idle || self.busy() {
            return;
        }
        self.refresh_when_idle = false;
        let identities = assets.iter().map(MediaIdentity::from).collect();
        self.start(session, MediaJobKind::Inspect, move || {
            MediaWorkerPayload::Reports(inspect_media(identities))
        });
    }

    pub fn locate(&mut self, session: u64, asset: &AudioAsset, path: PathBuf) {
        if !self.open || self.busy() || self.candidate.is_some() {
            return;
        }
        self.notice = None;
        let expected = MediaIdentity::from(asset);
        self.start(session, MediaJobKind::Prepare, move || {
            MediaWorkerPayload::Prepared(
                PreparedMediaRelink::prepare(session, expected, &path)
                    .map_err(|error| format!("{error:#}")),
            )
        });
    }

    pub fn apply(&mut self, session: u64, assets: &[AudioAsset]) {
        if !self.open || self.busy() {
            return;
        }
        let Some(candidate) = self.candidate.take() else {
            return;
        };
        if !candidate.matches(session, assets) {
            self.notice = Some(
                "The project or original media reference changed. Locate the audio again.".into(),
            );
            return;
        }
        self.start(session, MediaJobKind::Apply, move || {
            MediaWorkerPayload::Apply(
                candidate
                    .verify_file()
                    .map_err(|error| format!("{error:#}")),
            )
        });
    }

    /// Returns a final candidate only after the user explicitly applied the reviewed file.
    pub fn poll(&mut self, session: u64, assets: &[AudioAsset]) -> Option<PreparedMediaRelink> {
        let received = self
            .worker
            .as_ref()
            .map(|worker| worker.receiver.try_recv());
        let mut apply = None;
        match received {
            Some(Ok(payload)) => {
                let worker = self.worker.take().expect("worker was polled");
                if self.open && worker.epoch == self.epoch && worker.session == session {
                    match payload {
                        MediaWorkerPayload::Reports(reports) => {
                            self.reports = reports
                                .into_iter()
                                .filter(|report| {
                                    assets
                                        .iter()
                                        .any(|asset| MediaIdentity::from(asset) == report.identity)
                                })
                                .collect();
                        }
                        MediaWorkerPayload::Prepared(result)
                        | MediaWorkerPayload::Apply(result) => match result {
                            Ok(candidate) if candidate.matches(session, assets) => {
                                if worker.kind == MediaJobKind::Apply {
                                    apply = Some(candidate);
                                } else {
                                    self.candidate = Some(candidate);
                                }
                            }
                            Ok(_) => {
                                self.notice = Some(
                                        "The project or original media reference changed. Locate the audio again.".into()
                                    );
                            }
                            Err(error) => self.notice = Some(error),
                        },
                    }
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                let worker = self.worker.take().expect("worker was polled");
                if self.open && worker.epoch == self.epoch && worker.session == session {
                    self.notice = Some(
                        "Media worker stopped. No references were changed; retry Locate.".into(),
                    );
                }
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => {}
        }
        self.refresh_if_idle(session, assets);
        apply
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ClipKind;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        directory: PathBuf,
        path: PathBuf,
        asset: AudioAsset,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "citrus-media-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join("relocated.wav");
            fs::write(&path, wav_bytes(48_000, 1, 16, 16)).unwrap();
            let asset = AudioAsset {
                id: 71,
                name: "Original recording".into(),
                path: directory.join("missing.wav"),
                sample_rate: 48_000,
                channels: 1,
                bits_per_sample: 16,
                frames: 16,
                waveform_peaks: vec![0.9],
            };
            Self {
                directory,
                path,
                asset,
            }
        }
        fn prepared(&self) -> PreparedMediaRelink {
            PreparedMediaRelink::prepare(9, MediaIdentity::from(&self.asset), &self.path).unwrap()
        }
        fn project(&self) -> Project {
            let mut project = Project {
                audio_assets: vec![self.asset.clone()],
                ..Project::default()
            };
            let clip = &mut project.clips[0];
            clip.kind = ClipKind::Audio;
            clip.audio_asset_id = Some(self.asset.id);
            clip.audio_source_offset_frame = Some(3);
            clip.fade_in = 0.25;
            clip.fade_out = 0.3;
            project
                .audio_clip_mixer_destinations
                .push(crate::model::AudioClipMixerDestination {
                    clip_id: clip.id,
                    mixer_track_id: project.mixer_tracks[1].id,
                });
            project
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn wav_bytes(rate: u32, channels: u16, bits: u16, frames: u32) -> Vec<u8> {
        let block_align = channels * (bits / 8);
        let data_bytes = frames * u32::from(block_align);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        bytes.resize(bytes.len() + data_bytes as usize, 0);
        bytes
    }

    fn deliver(
        manager: &mut ProjectMediaManager,
        session: u64,
        kind: MediaJobKind,
        payload: MediaWorkerPayload,
    ) {
        let (sender, receiver) = mpsc::channel();
        sender.send(payload).unwrap();
        manager.open = true;
        manager.worker = Some(MediaWorker {
            epoch: manager.epoch,
            session,
            kind,
            receiver,
        });
    }

    #[test]
    fn relink_changes_only_selected_reference_and_preserves_source_and_clip_geometry() {
        let fixture = Fixture::new();
        let source_bytes = fs::read(&fixture.path).unwrap();
        let mut project = fixture.project();
        let before = serde_json::to_value(&project).unwrap();
        fixture.prepared().apply_to(9, &mut project).unwrap();
        assert_eq!(
            project.audio_assets[0].path,
            fixture.path.canonicalize().unwrap()
        );
        assert!(project.audio_assets[0].waveform_peaks.is_empty());
        let mut expected = before;
        expected["audio_assets"][0]["path"] =
            serde_json::to_value(fixture.path.canonicalize().unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&project).unwrap(), expected);
        assert_eq!(fs::read(&fixture.path).unwrap(), source_bytes);
        assert!(!fixture.asset.path.exists());
    }

    #[test]
    fn relink_save_reopen_keeps_absolute_path_and_reports_missing_again() {
        let fixture = Fixture::new();
        let mut project = fixture.project();
        fixture.prepared().apply_to(9, &mut project).unwrap();
        let saved = fixture.directory.join("elsewhere").join("song.citrus");
        project.save(&saved).unwrap();
        let reopened = Project::load(&saved).unwrap();
        assert_eq!(
            reopened.audio_assets[0].path,
            fixture.path.canonicalize().unwrap()
        );
        let identity = MediaIdentity::from(&reopened.audio_assets[0]);
        assert!(inspect_media(vec![identity.clone()])[0].error.is_none());
        fs::remove_file(&fixture.path).unwrap();
        assert!(inspect_media(vec![identity])[0].error.is_some());
        assert_eq!(
            Project::load(&saved).unwrap().audio_assets[0].path,
            reopened.audio_assets[0].path
        );
    }

    #[test]
    fn relink_rejects_each_timing_mismatch_without_touching_project_or_file() {
        let fixture = Fixture::new();
        for (rate, channels, frames) in [(44_100, 1, 16), (48_000, 2, 16), (48_000, 1, 17)] {
            fs::write(&fixture.path, wav_bytes(rate, channels, 16, frames)).unwrap();
            let original = fs::read(&fixture.path).unwrap();
            let error =
                PreparedMediaRelink::prepare(9, MediaIdentity::from(&fixture.asset), &fixture.path)
                    .unwrap_err()
                    .to_string();
            assert!(error.contains("Project expects 48000 Hz, 1 channel(s), 16 frames"));
            assert!(error.contains("selected file has"));
            assert_eq!(fs::read(&fixture.path).unwrap(), original);
            assert!(!fixture.asset.path.exists());
        }
    }

    #[test]
    fn relink_allows_storage_bit_depth_change_without_retiming() {
        let fixture = Fixture::new();
        fs::write(&fixture.path, wav_bytes(48_000, 1, 24, 16)).unwrap();
        let mut project = fixture.project();
        fixture.prepared().apply_to(9, &mut project).unwrap();
        assert_eq!(project.audio_assets[0].bits_per_sample, 24);
        assert_eq!(project.clips[0].audio_source_offset_frame, Some(3));
    }

    #[test]
    fn malformed_missing_and_directory_replacements_are_rejected() {
        let fixture = Fixture::new();
        fs::write(&fixture.path, b"not a WAV").unwrap();
        for path in [&fixture.path, &fixture.asset.path, &fixture.directory] {
            assert!(
                PreparedMediaRelink::prepare(9, MediaIdentity::from(&fixture.asset), path).is_err()
            );
        }
    }

    #[test]
    fn relink_cannot_apply_to_a_new_session_changed_identity_or_ambiguous_asset() {
        let fixture = Fixture::new();
        let prepared = fixture.prepared();
        for case in 0..7 {
            let mut project = fixture.project();
            let session = if case == 0 { 10 } else { 9 };
            match case {
                1 => project.audio_assets[0].path = "another.wav".into(),
                2 => project.audio_assets[0].sample_rate = 96_000,
                3 => project.audio_assets[0].channels = 2,
                4 => project.audio_assets[0].frames = 100,
                5 => project.audio_assets.clear(),
                6 => project.audio_assets.push(fixture.asset.clone()),
                _ => {}
            }
            let before = serde_json::to_value(&project).unwrap();
            assert!(prepared.apply_to(session, &mut project).is_err());
            assert_eq!(serde_json::to_value(&project).unwrap(), before);
        }
    }

    #[test]
    fn apply_rechecks_reviewed_file_and_rejects_deletion_or_replacement() {
        let fixture = Fixture::new();
        let prepared = fixture.prepared();
        fs::write(&fixture.path, wav_bytes(48_000, 1, 16, 17)).unwrap();
        assert!(prepared.verify_file().is_err());
        fs::write(&fixture.path, wav_bytes(48_000, 1, 16, 16)).unwrap();
        let prepared = fixture.prepared();
        fs::remove_file(&fixture.path).unwrap();
        assert!(prepared.verify_file().is_err());
    }

    #[test]
    fn prepare_results_are_reviewed_and_never_implicitly_applied() {
        let fixture = Fixture::new();
        let mut manager = ProjectMediaManager::default();
        deliver(
            &mut manager,
            9,
            MediaJobKind::Prepare,
            MediaWorkerPayload::Prepared(Ok(fixture.prepared())),
        );
        assert!(
            manager
                .poll(9, std::slice::from_ref(&fixture.asset))
                .is_none()
        );
        assert!(manager.candidate.is_some());
        assert!(!manager.busy());
    }

    #[test]
    fn cancel_close_and_new_session_discard_even_completed_apply_results() {
        let fixture = Fixture::new();
        for action in 0..3 {
            let mut manager = ProjectMediaManager::default();
            deliver(
                &mut manager,
                9,
                MediaJobKind::Apply,
                MediaWorkerPayload::Apply(Ok(fixture.prepared())),
            );
            match action {
                0 => manager.cancel(),
                1 => manager.close(),
                _ => {}
            }
            assert!(
                manager
                    .poll(
                        if action == 2 { 10 } else { 9 },
                        std::slice::from_ref(&fixture.asset)
                    )
                    .is_none()
            );
            assert!(manager.candidate.is_none());
            assert!(!manager.busy());
        }
    }

    #[test]
    fn close_reopen_does_not_spawn_parallel_decoders_or_accept_the_old_candidate() {
        let fixture = Fixture::new();
        let mut manager = ProjectMediaManager::default();
        deliver(
            &mut manager,
            9,
            MediaJobKind::Prepare,
            MediaWorkerPayload::Prepared(Ok(fixture.prepared())),
        );
        let epoch = manager.epoch;
        manager.close();
        manager.open(9, std::slice::from_ref(&fixture.asset));
        assert_eq!(manager.worker.as_ref().unwrap().epoch, epoch);
        assert!(
            manager
                .poll(9, std::slice::from_ref(&fixture.asset))
                .is_none()
        );
        assert!(manager.candidate.is_none());
        assert_eq!(manager.worker.as_ref().unwrap().kind, MediaJobKind::Inspect);
    }

    #[test]
    fn changed_selection_rejects_prepared_result_without_overwriting_it() {
        let fixture = Fixture::new();
        let mut manager = ProjectMediaManager::default();
        deliver(
            &mut manager,
            9,
            MediaJobKind::Prepare,
            MediaWorkerPayload::Prepared(Ok(fixture.prepared())),
        );
        let mut changed = fixture.asset.clone();
        changed.path = "other.wav".into();
        assert!(manager.poll(9, &[changed]).is_none());
        assert!(manager.candidate.is_none());
        assert!(
            manager
                .notice
                .as_ref()
                .unwrap()
                .contains("reference changed")
        );
    }

    #[test]
    fn reference_comparison_detects_undo_redo_changes_but_ignores_waveform_cache() {
        let fixture = Fixture::new();
        let before = vec![fixture.asset.clone()];
        let mut after = before.clone();
        after[0].waveform_peaks.clear();
        assert!(!media_references_changed(&before, &after));
        after[0].path = fixture.path.clone();
        assert!(media_references_changed(&before, &after));
        assert!(media_references_changed(&after, &before));
    }

    #[test]
    fn inspection_retains_each_missing_file_error_without_mutation() {
        let fixture = Fixture::new();
        let mut directory = MediaIdentity::from(&fixture.asset);
        directory.path = fixture.directory.clone();
        let reports = inspect_media(vec![MediaIdentity::from(&fixture.asset), directory]);
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|report| report.error.is_some()));
        assert!(!fixture.asset.path.exists());
    }
    #[test]
    fn worker_flow_requires_review_then_explicit_apply_and_ignores_repeated_requests() {
        use std::time::{Duration, Instant};
        fn drain(
            manager: &mut ProjectMediaManager,
            assets: &[AudioAsset],
        ) -> Option<PreparedMediaRelink> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let applied = manager.poll(9, assets);
                if !manager.busy() {
                    return applied;
                }
                assert!(Instant::now() < deadline, "media worker did not finish");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let fixture = Fixture::new();
        let mut project = fixture.project();
        let original = serde_json::to_value(&project).unwrap();
        let mut manager = ProjectMediaManager::default();
        manager.open(9, &project.audio_assets);
        assert!(drain(&mut manager, &project.audio_assets).is_none());
        assert!(manager.reports[0].error.is_some());
        manager.locate(9, &fixture.asset, fixture.path.clone());
        // A second picker/request cannot change the selected target during decoding.
        manager.locate(9, &fixture.asset, fixture.asset.path.clone());
        assert!(drain(&mut manager, &project.audio_assets).is_none());
        assert!(manager.candidate.is_some());
        assert_eq!(serde_json::to_value(&project).unwrap(), original);
        manager.apply(9, &project.audio_assets);
        manager.apply(9, &project.audio_assets);
        let approved = drain(&mut manager, &project.audio_assets).unwrap();
        approved.apply_to(9, &mut project).unwrap();
        assert_eq!(
            project.audio_assets[0].path,
            fixture.path.canonicalize().unwrap()
        );
        assert!(manager.poll(9, &project.audio_assets).is_none());
    }

    #[test]
    fn disconnected_worker_reports_failure_without_publishing_a_candidate() {
        let fixture = Fixture::new();
        let mut manager = ProjectMediaManager::default();
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        manager.open = true;
        manager.worker = Some(MediaWorker {
            epoch: 0,
            session: 9,
            kind: MediaJobKind::Prepare,
            receiver,
        });
        assert!(
            manager
                .poll(9, std::slice::from_ref(&fixture.asset))
                .is_none()
        );
        assert!(manager.candidate.is_none());
        assert!(manager.notice.unwrap().contains("worker stopped"));
    }
}
