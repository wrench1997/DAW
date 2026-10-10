//! Session-only producer-owned PCM. This map is independent of ordinary cache
//! pruning and never reads the external file again. No entry is evicted until a
//! fenced project-session transition. 192 MiB PCM and 4096 × 4 KiB paths are
//! separate bounds; PCM limits alone do not bound metadata overhead.
use super::*;
use crate::occurrence_media::VerifiedOccurrencePcm;
use std::collections::hash_map::Entry;

pub(super) const MAX_RETAINED_CLIP_PCM_BYTES: usize = 192 * 1024 * 1024;
pub(super) const MAX_RETAINED_CLIP_ENTRIES: usize = 4096;
const MAX_RETAINED_PATH_BYTES: usize = 4096;

#[cfg(test)]
thread_local! {
    static TEST_PCM_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_RETAINED_CLIP_PCM_BYTES) };
}
#[cfg(all(test, target_os = "linux", feature = "experimental-fresh-helper"))]
pub(super) fn genuine_hooks_inactive() -> bool {
    TEST_PCM_LIMIT.with(|limit| limit.get() == MAX_RETAINED_CLIP_PCM_BYTES)
}
#[cfg(test)]
pub(in crate::app) struct TestPcmLimit(usize);
#[cfg(test)]
impl Drop for TestPcmLimit {
    fn drop(&mut self) {
        TEST_PCM_LIMIT.with(|limit| limit.set(self.0));
    }
}
#[cfg(test)]
pub(in crate::app) fn lower_test_pcm_limit(bytes: usize) -> TestPcmLimit {
    assert!(bytes <= MAX_RETAINED_CLIP_PCM_BYTES);
    TestPcmLimit(TEST_PCM_LIMIT.with(|limit| limit.replace(bytes)))
}
fn pcm_limit() -> usize {
    #[cfg(test)]
    {
        TEST_PCM_LIMIT.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        MAX_RETAINED_CLIP_PCM_BYTES
    }
}

pub(super) struct RetainedVerifiedClipMedia {
    operation_id: u64,
    project_session: u64,
    asset_id: u64,
    pcm: VerifiedOccurrencePcm,
    charged_bytes: usize,
}
pub(super) struct PreparedRetainedClipMedia {
    entry: RetainedVerifiedClipMedia,
    expected_entries: usize,
    expected_bytes: usize,
    installed_bytes: usize,
}
pub(super) struct RetainedClipPreparationFailure {
    pub(super) message: String,
    pub(super) pcm: VerifiedOccurrencePcm,
}

fn retained_budget(current: usize, proposed: usize, entries: usize) -> Result<usize, String> {
    if entries >= MAX_RETAINED_CLIP_ENTRIES {
        return Err("Session converted-media metadata limit reached (4096 entries)".into());
    }
    current.checked_add(proposed)
        .filter(|total| *total <= pcm_limit())
        .ok_or_else(|| "Session converted-media PCM limit reached (192 MiB); the WAV is retained without insertion".into())
}
fn pcm_bytes(pcm: &VerifiedOccurrencePcm) -> Option<usize> {
    pcm.samples().len().checked_mul(std::mem::size_of::<f32>())
}
fn copied_path(path: &Path) -> Result<PathBuf, String> {
    let value = path.to_str().ok_or("Converted-media path is not UTF-8")?;
    if value.len() > MAX_RETAINED_PATH_BYTES || !path.is_absolute() {
        return Err("Converted-media path exceeds its bounded absolute path contract".into());
    }
    let mut copy = String::new();
    copy.try_reserve_exact(value.len())
        .map_err(|_| "Unable to reserve converted-media path")?;
    copy.push_str(value);
    Ok(PathBuf::from(copy))
}
impl RetainedVerifiedClipMedia {
    fn cached(&self) -> Result<CachedAudioAssetRegistration, String> {
        Ok(CachedAudioAssetRegistration {
            project_session: self.project_session,
            project_asset_id: self.asset_id,
            path: copied_path(self.pcm.path())?,
            samples: Arc::clone(self.pcm.samples()),
            sample_rate: self.pcm.sample_rate(),
            channels: self.pcm.channels(),
        })
    }
    fn matches_asset(&self, asset: &AudioAsset) -> bool {
        asset.id == self.asset_id
            && asset.path == self.pcm.path()
            && asset.sample_rate == self.pcm.sample_rate()
            && asset.channels == self.pcm.channels()
            && asset.bits_per_sample == 32
            && usize::try_from(asset.frames).ok() == Some(self.pcm.frames())
            && self.pcm.frames().checked_mul(usize::from(asset.channels))
                == Some(self.pcm.samples().len())
    }
    fn matches_cache(&self, cache: &CachedAudioAssetRegistration) -> bool {
        cache.project_session == self.project_session
            && cache.project_asset_id == self.asset_id
            && cache.path == self.pcm.path()
            && cache.sample_rate == self.pcm.sample_rate()
            && cache.channels == self.pcm.channels()
            && Arc::ptr_eq(&cache.samples, self.pcm.samples())
    }
}
impl PreparedRetainedClipMedia {
    pub(super) fn cached(&self) -> Result<CachedAudioAssetRegistration, String> {
        self.entry.cached()
    }
    pub(super) fn stamp(&self) -> crate::occurrence_request::OccurrenceStamp {
        self.entry.pcm.binding().stamp
    }
}

impl CitrusApp {
    /// Every reservation and recoverable validation is performed before the
    /// App commit CAS/removal. Refusal returns the producer-owned PCM intact.
    // Err returns the exact producer-owned PCM without allocating on refusal.
    #[allow(clippy::result_large_err)]
    pub(super) fn prepare_retained_clip_media(
        &mut self,
        operation_id: u64,
        asset_id: u64,
        pcm: VerifiedOccurrencePcm,
    ) -> Result<PreparedRetainedClipMedia, RetainedClipPreparationFailure> {
        let validation = (|| {
            let binding = pcm.binding();
            binding.stamp.validate()?;
            if operation_id == 0
                || asset_id == 0
                || self.project_session != binding.stamp.project_session
                || binding.source.project_session != self.project_session
                || binding.source.channel_id != binding.stamp.channel_id
                || binding.source.instance_id != binding.stamp.instance_id
                || binding.source.endpoint_id != binding.stamp.endpoint_id
                || binding.source.timeline_revision != binding.stamp.timeline_revision
                || binding.source.epoch != binding.stamp.epoch
                || binding.end_frame.checked_sub(binding.start_frame) != Some(binding.song_frames)
                || binding.clip_id == 0
                || binding.pattern_id == 0
                || pcm.frames() == 0
                || pcm.frames() > crate::export::MAX_OCCURRENCE_MEDIA_FRAMES
                || binding.sample_rate != pcm.sample_rate()
                || binding
                    .song_frames
                    .checked_add(u64::from(binding.tail_frames))
                    != u64::try_from(pcm.frames()).ok()
                || pcm.frames().checked_mul(2) != Some(pcm.samples().len())
            {
                return Err(
                    "Verified PCM does not match its actual operation/source/session extent".into(),
                );
            }
            // Reject even an apparently identical entry. Never overwrite another
            // operation and never charge a second handle of one entry twice.
            if self.retained_clip_media.contains_key(&asset_id) {
                return Err("Converted-media asset ID is already reserved in this session".into());
            }
            copied_path(pcm.path())?; // Check recoverable metadata allocation now.
            let bytes = pcm_bytes(&pcm).ok_or("Converted-media PCM byte extent overflow")?;
            let actual = self
                .retained_clip_media
                .values()
                .try_fold(0usize, |sum, entry| sum.checked_add(entry.charged_bytes));
            if actual != Some(self.retained_clip_pcm_bytes) {
                return Err("Converted-media session byte ledger is inconsistent".into());
            }
            let installed = retained_budget(
                self.retained_clip_pcm_bytes,
                bytes,
                self.retained_clip_media.len(),
            )?;
            self.retained_clip_media
                .try_reserve(1)
                .map_err(|_| "Unable to reserve retained converted-media owner")?;
            Ok((bytes, installed))
        })();
        match validation {
            Ok((charged_bytes, installed_bytes)) => Ok(PreparedRetainedClipMedia {
                entry: RetainedVerifiedClipMedia {
                    operation_id,
                    project_session: self.project_session,
                    asset_id,
                    pcm,
                    charged_bytes,
                },
                expected_entries: self.retained_clip_media.len(),
                expected_bytes: self.retained_clip_pcm_bytes,
                installed_bytes,
            }),
            Err(message) => Err(RetainedClipPreparationFailure { message, pcm }),
        }
    }
    fn retained_clip_preparation_matches(&self, prepared: &PreparedRetainedClipMedia) -> bool {
        prepared.entry.operation_id != 0
            && self.project_session == prepared.entry.project_session
            && self.retained_clip_media.len() == prepared.expected_entries
            && self.retained_clip_pcm_bytes == prepared.expected_bytes
            && !self
                .retained_clip_media
                .contains_key(&prepared.entry.asset_id)
            && self.retained_clip_media.capacity() > self.retained_clip_media.len()
            && prepared.expected_entries < MAX_RETAINED_CLIP_ENTRIES
            && prepared
                .expected_bytes
                .checked_add(prepared.entry.charged_bytes)
                == Some(prepared.installed_bytes)
            && prepared.installed_bytes <= MAX_RETAINED_CLIP_PCM_BYTES
    }
    pub(super) fn revalidate_retained_clip_media(
        &self,
        prepared: &PreparedRetainedClipMedia,
    ) -> Result<(), String> {
        if self.retained_clip_preparation_matches(prepared) {
            Ok(())
        } else {
            Err("The prepared converted-media session reservation changed".into())
        }
    }
    /// Move-only insertion alongside the successful history swap. No allocation,
    /// file read, PCM copy, or replacement is permitted at this boundary.
    // A failed move-only insertion must return its exact owner without allocation.
    #[allow(clippy::result_large_err)]
    pub(super) fn install_retained_clip_media(
        &mut self,
        prepared: PreparedRetainedClipMedia,
    ) -> Result<(), PreparedRetainedClipMedia> {
        if !self.retained_clip_preparation_matches(&prepared) {
            return Err(prepared);
        }
        match self.retained_clip_media.entry(prepared.entry.asset_id) {
            Entry::Occupied(_) => Err(prepared),
            Entry::Vacant(slot) => {
                slot.insert(prepared.entry);
                self.retained_clip_pcm_bytes = prepared.installed_bytes;
                Ok(())
            }
        }
    }
    /// Reuse is authorized by the exact current Project record and session,
    /// never by the now-mutable file. Ordinary callback/Timeline ack gates remain.
    pub(super) fn try_restore_retained_clip_asset(
        &mut self,
        asset_id: u64,
    ) -> Result<bool, String> {
        let Some(entry) = self.retained_clip_media.get(&asset_id) else {
            return Ok(false);
        };
        let mut matches = self
            .project
            .audio_assets
            .iter()
            .filter(|asset| asset.id == asset_id);
        let Some(asset) = matches.next() else {
            return Err("Retained converted-media asset has no current Project record".into());
        };
        if matches.next().is_some()
            || entry.project_session != self.project_session
            || !entry.matches_asset(asset)
        {
            return Err(
                "Retained converted-media ID/path/metadata differs from the current Project".into(),
            );
        }
        if let Some(cache) = self.cached_audio_assets.get(&asset_id) {
            if !entry.matches_cache(cache) {
                return Err("Ordinary cache conflicts with sealed converted-media PCM".into());
            }
            return Ok(true);
        }
        let cache = entry.cached()?;
        self.cached_audio_assets
            .try_reserve(1)
            .map_err(|_| "Unable to reserve restored converted-media cache")?;
        match self.cached_audio_assets.entry(asset_id) {
            Entry::Occupied(_) => {
                Err("Ordinary media cache changed while restoring converted PCM".into())
            }
            Entry::Vacant(slot) => {
                slot.insert(cache);
                Ok(true)
            }
        }
    }
}

#[cfg(test)]
#[path = "clip_render_cache_tests.rs"]
mod tests;
