//! One authoritative, read-only geometry/clock contract for Pattern placements.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternLengthPolicy {
    Manual,
    Follow,
}

impl Pattern {
    /// The source Pattern's existing UI/PAT period, not the legacy SONG minimum.
    /// Full bars and the existing four-bar extension blocks remain unchanged.
    #[must_use]
    pub fn content_period_beats(&self) -> f64 {
        self.piano_period_beats((self.length_steps.max(1) as f64 / 4.0).max(4.0))
    }
}

impl Clip {
    #[must_use]
    pub fn follows_pattern(&self) -> bool {
        self.kind == ClipKind::Pattern
            && self.pattern_length_policy == Some(PatternLengthPolicy::Follow)
    }

    /// Pattern is supplied by the owning Project or its already-indexed view.
    /// A missing Follow source is invalid geometry, never a stored-length fallback.
    #[must_use]
    pub fn resolved_length_beats(&self, pattern: Option<&Pattern>) -> f32 {
        if self.follows_pattern() {
            pattern.map_or(f32::NAN, |pattern| pattern.content_period_beats() as f32)
        } else {
            self.length
        }
    }

    #[must_use]
    pub fn piano_period_beats(&self, pattern: &Pattern, legacy_minimum: f64) -> f64 {
        if self.kind == ClipKind::Pattern && self.pattern_length_policy.is_some() {
            pattern.content_period_beats()
        } else {
            pattern.piano_period_beats(legacy_minimum)
        }
    }

    /// A real user trim fixes the visible extent without changing its source
    /// clock. Existing Manual callers retain the legacy clock and absent field.
    pub fn set_manual_pattern_length(&mut self, length: f32) {
        self.length = length;
        if self.kind == ClipKind::Pattern && self.pattern_length_policy.is_some() {
            self.pattern_length_policy = Some(PatternLengthPolicy::Manual);
        }
    }
}

impl Project {
    #[must_use]
    pub fn clip_length_beats(&self, clip: &Clip) -> f32 {
        clip.resolved_length_beats(
            self.patterns
                .iter()
                .find(|pattern| pattern.id == clip.pattern_id),
        )
    }

    /// A transient resolved image for edit geometry, conversion admission and
    /// control-thread compilation. Persist/history keep the original source.
    #[must_use]
    pub fn resolved_clip_geometry(&self, clip: &Clip) -> Clip {
        let mut resolved = clip.clone();
        resolved.length = self.clip_length_beats(clip);
        resolved
    }

    pub(super) fn validate_pattern_length_policies(&self) -> Result<()> {
        for clip in &self.clips {
            if clip.pattern_length_policy.is_none() {
                continue;
            }
            anyhow::ensure!(
                self.format_version >= 14,
                "Pattern length policy requires project format 14; normalize the project before saving"
            );
            anyhow::ensure!(
                clip.kind == ClipKind::Pattern,
                "Clip {} has Pattern length policy but is not a Pattern Clip",
                clip.id
            );
            anyhow::ensure!(
                self.patterns
                    .iter()
                    .filter(|pattern| pattern.id == clip.pattern_id)
                    .count()
                    == 1,
                "Clip {} requires one unambiguous source Pattern {}",
                clip.id,
                clip.pattern_id
            );
            let length = self.clip_length_beats(clip);
            anyhow::ensure!(
                length.is_finite() && length > 0.0,
                "Clip {} has invalid Pattern extent",
                clip.id
            );
        }
        Ok(())
    }
}
