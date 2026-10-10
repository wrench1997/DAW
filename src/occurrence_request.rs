//! Closed, dormant selected-occurrence request shared by the pure planner and
//! capture/render workers. This module has no App, UI, or filesystem dependency.
use crate::mixer_graph::MixerTrackId;
use crate::timeline::{NoteSourceDescriptor, TimelineEvent, TimelineEventKind};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OccurrenceStamp {
    pub project_session: u64,
    pub project_revision: u64,
    pub timeline_revision: u64,
    pub epoch: u64,
    pub engine_session: u64,
    pub endpoint_id: u64,
    pub channel_id: u32,
    pub instance_id: u64,
    pub mixer_track_id: MixerTrackId,
    pub native_generation: u64,
    /// Raw backend revision is intentionally absent: only actual dedicated
    /// capture can establish that private authority. This is App-relative.
    pub app_native_dirty_revision: u64,
    pub native_capture_serial: u64,
    pub parameter_capture_serial: u64,
    pub native_control_request: u64,
    pub parameter_control_request: u64,
}
impl OccurrenceStamp {
    pub(crate) fn validate(self) -> Result<(), String> {
        if self.project_session == 0
            || self.project_revision == 0
            || self.timeline_revision == 0
            || self.epoch == 0
            || self.engine_session == 0
            || self.endpoint_id == 0
            || self.channel_id == 0
            || self.instance_id == 0
            || self.mixer_track_id == 0
        {
            return Err("A complete actual occurrence source/runtime stamp is required".into());
        }
        Ok(())
    }
}

/// An explicit reviewed cutoff/tail choice bound to one exact source snapshot.
/// This type records consent provenance; calling its constructor is not itself
/// a user-interface acknowledgment. The future App adapter owns that action.
#[derive(PartialEq, Eq)]
pub(crate) struct FixedTailAcknowledgment {
    source_snapshot: Arc<[u8]>,
    stamp: OccurrenceStamp,
    clip_id: u32,
    sample_rate: u32,
    tail_frames: u32,
}
impl FixedTailAcknowledgment {
    pub(crate) fn review(
        source_snapshot: Arc<[u8]>,
        stamp: OccurrenceStamp,
        clip_id: u32,
        sample_rate: u32,
        tail_frames: u32,
    ) -> Result<Self, String> {
        stamp.validate()?;
        if source_snapshot.is_empty()
            || source_snapshot.len() > 64 * 1024 * 1024
            || clip_id == 0
            || !(8_000..=192_000).contains(&sample_rate)
            || tail_frames > sample_rate.saturating_mul(30)
        {
            return Err("The reviewed source snapshot or fixed-tail choice is invalid".into());
        }
        Ok(Self {
            source_snapshot,
            stamp,
            clip_id,
            sample_rate,
            tail_frames,
        })
    }
    pub(crate) fn snapshot(&self) -> &Arc<[u8]> {
        &self.source_snapshot
    }
    pub(crate) fn stamp(&self) -> OccurrenceStamp {
        self.stamp
    }
    pub(crate) fn clip_id(&self) -> u32 {
        self.clip_id
    }
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub(crate) fn tail_frames(&self) -> u32 {
        self.tail_frames
    }
}

pub(crate) struct OccurrenceRenderRequest {
    consent: Arc<FixedTailAcknowledgment>,
    pattern_id: u32,
    channel_id: u32,
    instance_id: u64,
    start_frame: u64,
    end_frame: u64,
    tempo: f64,
    local_events: Arc<[TimelineEvent]>,
}
impl OccurrenceRenderRequest {
    /// Called only after the planner has compiled the ORIGINAL Project and
    /// admitted exact source membership, sole use, routing and user consent.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        consent: Arc<FixedTailAcknowledgment>,
        pattern_id: u32,
        channel_id: u32,
        instance_id: u64,
        start_frame: u64,
        end_frame: u64,
        total_song_frames: u64,
        tempo: f64,
        local_events: Arc<[TimelineEvent]>,
    ) -> Result<Arc<Self>, String> {
        consent.stamp.validate()?;
        let n = end_frame
            .checked_sub(start_frame)
            .filter(|n| *n > 0)
            .ok_or("Occurrence requires a positive interval")?;
        let rate = consent.sample_rate;
        let reserved = n
            .checked_add(u64::from(consent.tail_frames))
            .and_then(|n| n.checked_add(u64::from(rate)))
            .and_then(|n| n.checked_add(127))
            .map(|n| n / 128 * 128);
        if pattern_id == 0
            || channel_id != consent.stamp.channel_id
            || instance_id != consent.stamp.instance_id
            || n > u64::from(rate) * 600
            || reserved.is_none_or(|n| n > 24_000_000)
            || end_frame
                .checked_add(u64::from(consent.tail_frames))
                .is_none_or(|end| end > total_song_frames)
            || !tempo.is_finite()
            || tempo <= 0.0
            || local_events.is_empty()
            || local_events.len() > crate::timeline::MAX_TIMELINE_EVENTS
        {
            return Err(
                "Occurrence identity, geometry, tempo, or bounded schedule is invalid".into(),
            );
        }
        let mut previous = 0;
        for event in local_events.iter() {
            let (event_channel, source, note_on) = match event.kind {
                TimelineEventKind::NoteOn {
                    channel_id, source, ..
                } => (channel_id, source, true),
                TimelineEventKind::NoteOff {
                    channel_id, source, ..
                } => (channel_id, source, false),
                _ => return Err("Occurrence schedule contains a non-MIDI event".into()),
            };
            let (source_clip, source_pattern) = match source {
                NoteSourceDescriptor::ChannelStep {
                    clip_id,
                    pattern_id,
                    ..
                }
                | NoteSourceDescriptor::LegacyPiano {
                    clip_id,
                    pattern_id,
                    ..
                } => (clip_id, pattern_id),
            };
            if event_channel != channel_id
                || source_clip != consent.clip_id
                || source_pattern != pattern_id
                || event.frame < previous
                || event.frame > n
                || (note_on && event.frame == n)
            {
                return Err(
                    "Occurrence schedule differs from its exact local source interval".into(),
                );
            }
            previous = event.frame;
        }
        Ok(Arc::new(Self {
            consent,
            pattern_id,
            channel_id,
            instance_id,
            start_frame,
            end_frame,
            tempo,
            local_events,
        }))
    }
    pub(crate) fn snapshot(&self) -> &Arc<[u8]> {
        self.consent.snapshot()
    }
    pub(crate) fn stamp(&self) -> OccurrenceStamp {
        self.consent.stamp()
    }
    pub(crate) fn consent(&self) -> &Arc<FixedTailAcknowledgment> {
        &self.consent
    }
    pub(crate) fn clip_id(&self) -> u32 {
        self.consent.clip_id()
    }
    pub(crate) fn pattern_id(&self) -> u32 {
        self.pattern_id
    }
    pub(crate) fn channel_id(&self) -> u32 {
        self.channel_id
    }
    pub(crate) fn instance_id(&self) -> u64 {
        self.instance_id
    }
    pub(crate) fn start_frame(&self) -> u64 {
        self.start_frame
    }
    pub(crate) fn end_frame(&self) -> u64 {
        self.end_frame
    }
    pub(crate) fn song_frames(&self) -> u64 {
        self.end_frame - self.start_frame
    }
    pub(crate) fn sample_rate(&self) -> u32 {
        self.consent.sample_rate()
    }
    pub(crate) fn tail_frames(&self) -> u32 {
        self.consent.tail_frames()
    }
    pub(crate) fn tempo(&self) -> f64 {
        self.tempo
    }
    pub(crate) fn local_events(&self) -> &[TimelineEvent] {
        &self.local_events
    }
}

impl std::fmt::Debug for FixedTailAcknowledgment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixedTailAcknowledgment")
            .field("stamp", &self.stamp)
            .field("clip_id", &self.clip_id)
            .field("sample_rate", &self.sample_rate)
            .field("tail_frames", &self.tail_frames)
            .field("snapshot_bytes", &self.source_snapshot.len())
            .finish()
    }
}
impl std::fmt::Debug for OccurrenceRenderRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OccurrenceRenderRequest")
            .field("consent", &self.consent)
            .field("pattern_id", &self.pattern_id)
            .field("start_frame", &self.start_frame)
            .field("end_frame", &self.end_frame)
            .field("event_count", &self.local_events.len())
            .finish()
    }
}
