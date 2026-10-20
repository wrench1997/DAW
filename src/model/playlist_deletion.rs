//! Playlist deletion owns only the sources orphaned by that deletion.
use std::collections::HashSet;

use super::{ClipKind, Project, normalize_playlist_clip_groups};

impl Project {
    /// Delete the already-expanded selection and its placement-owned data.
    ///
    /// Lanes without Automation placements are valid project-global automation.
    /// Only a lane referenced by an Automation clip removed in this operation
    /// can be orphaned by it. Removing its final placement must remove that lane
    /// too, or it would unexpectedly become global automation. Muted placements
    /// still own their source, just as they do in playback and export.
    pub(crate) fn delete_playlist_clips(&mut self, clip_ids: &HashSet<u32>) -> bool {
        let removed_clip_ids = self
            .clips
            .iter()
            .filter(|clip| clip_ids.contains(&clip.id))
            .map(|clip| clip.id)
            .collect::<HashSet<_>>();
        if removed_clip_ids.is_empty() {
            return false;
        }
        let removed_automation_ids = self
            .clips
            .iter()
            .filter(|clip| clip.kind == ClipKind::Automation && removed_clip_ids.contains(&clip.id))
            .filter_map(|clip| clip.automation_id)
            .collect::<HashSet<_>>();

        self.clips
            .retain(|clip| !removed_clip_ids.contains(&clip.id));
        self.audio_clip_mixer_destinations
            .retain(|destination| !removed_clip_ids.contains(&destination.clip_id));
        if !removed_automation_ids.is_empty() {
            let remaining_automation_ids = self
                .clips
                .iter()
                .filter(|clip| clip.kind == ClipKind::Automation)
                .filter_map(|clip| clip.automation_id)
                .collect::<HashSet<_>>();
            self.automation_lanes.retain(|automation| {
                !removed_automation_ids.contains(&automation.id)
                    || remaining_automation_ids.contains(&automation.id)
            });
        }
        normalize_playlist_clip_groups(&mut self.clips);
        true
    }
}

#[cfg(test)]
#[path = "playlist_deletion_tests.rs"]
mod tests;
