//! Dormant selected-generator engine. There is deliberately no VST3 backend implementation.
//! Only tests implement the backend contract until strict isolated lifetime/I/O is qualified.
use super::{
    BOUNCE_QUANTUM, BouncePluginMetadata, BounceSourceIdentity, CapturedInstrumentBounce,
    MAX_BOUNCE_FRAMES,
};
use crate::{
    export::{StereoFrame, write_stereo_float32_checked},
    export_job::ExportControl,
    plugins::plugin_runtime::PluginTransport,
    timeline::{
        NoteSourceDescriptor, PluginRouteDestination,
        TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM, TimelineEvent, TimelineEventKind,
    },
    timeline_executor::MAX_ACTIVE_NOTES,
};
use std::{
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    Cancelled,
    Deadline,
    Invalid,
    Backend,
    Panic,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderFailure {
    pub kind: FailureKind,
    pub primary: String,
    pub cleanup: Vec<String>,
}
impl RenderFailure {
    fn new(kind: FailureKind, primary: impl Into<String>) -> Self {
        Self {
            kind,
            primary: primary.into(),
            cleanup: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct RenderControl {
    pub export: ExportControl,
    pub deadline: Instant,
}
impl RenderControl {
    pub fn checkpoint(&self) -> Result<(), RenderFailure> {
        self.export
            .check_cancelled()
            .map_err(|e| RenderFailure::new(FailureKind::Cancelled, e.to_string()))?;
        if Instant::now() >= self.deadline {
            return Err(RenderFailure::new(
                FailureKind::Deadline,
                "Instrument render deadline expired",
            ));
        }
        Ok(())
    }
    fn call<T>(&self, call: impl FnOnce() -> Result<T, String>) -> Result<T, RenderFailure> {
        self.call_checked(call, |_| Ok(()))
    }
    fn call_checked<T>(
        &self,
        call: impl FnOnce() -> Result<T, String>,
        validate: impl FnOnce(&T) -> Result<(), RenderFailure>,
    ) -> Result<T, RenderFailure> {
        self.checkpoint()?;
        // A dispatched failure/panic or malformed reply wins over cancellation
        // that arrives in flight. Validate before observing the post-call fence.
        let value = catch_unwind(AssertUnwindSafe(call))
            .map_err(|_| RenderFailure::new(FailureKind::Panic, "Instrument backend panicked"))?
            .map_err(|e| RenderFailure::new(FailureKind::Backend, e))?;
        validate(&value)?;
        self.checkpoint()?;
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScheduledMidi {
    pub frame: u64,
    pub data: [u8; 3],
}
#[derive(Clone, Copy)]
struct ActiveNote {
    id: u64,
    on_frame: u64,
    channel: u32,
    pitch: u8,
    track: u16,
    source: NoteSourceDescriptor,
}

/// Single event cursor, exact compiler order. All selected identities are accounted for.
pub(crate) fn map_selected_notes(
    events: &[TimelineEvent],
    channel: u32,
    song_frames: u64,
    control: &RenderControl,
) -> Result<Vec<ScheduledMidi>, RenderFailure> {
    control.checkpoint()?;
    let invalid = |s: &str| RenderFailure::new(FailureKind::Invalid, s);
    if events.len() > crate::timeline::MAX_TIMELINE_EVENTS {
        return Err(invalid("Timeline event bound exceeded"));
    }
    let mut active: [Option<ActiveNote>; MAX_ACTIVE_NOTES] = [None; MAX_ACTIVE_NOTES];
    let mut seen = HashSet::new();
    seen.try_reserve(events.len())
        .map_err(|e| invalid(&format!("Note identity allocation failed: {e}")))?;
    let mut mapped = Vec::new();
    mapped
        .try_reserve(events.len())
        .map_err(|e| invalid(&format!("MIDI allocation failed: {e}")))?;
    let mut previous = 0;
    let mut quantum = None;
    let mut count = 0;
    for (index, event) in events.iter().enumerate() {
        control
            .export
            .work_progress(index, events.len(), 0, 1000)
            .map_err(|e| RenderFailure::new(FailureKind::Cancelled, e.to_string()))?;
        control.checkpoint()?;
        if event.frame < previous {
            return Err(invalid("Timeline event order regressed"));
        }
        previous = event.frame;
        let message = match event.kind {
            TimelineEventKind::NoteOn {
                note_id,
                channel_id,
                note,
                velocity,
                gain,
                mixer_track,
                source,
            } if channel_id == channel => {
                if event.frame >= song_frames
                    || note_id == 0
                    || note > 127
                    || !velocity.is_finite()
                    || !gain.is_finite()
                    || !(velocity * gain).is_finite()
                    || !seen.insert(note_id)
                {
                    return Err(invalid(
                        "Selected NoteOn has an invalid frame, value or duplicate identity",
                    ));
                }
                let slot = active
                    .iter_mut()
                    .find(|entry| entry.is_none())
                    .ok_or_else(|| {
                        invalid("Selected instrument exceeds 512 active note identities")
                    })?;
                *slot = Some(ActiveNote {
                    id: note_id,
                    on_frame: event.frame,
                    channel: channel_id,
                    pitch: note,
                    track: mixer_track,
                    source,
                });
                let velocity = (((velocity * gain).clamp(0.0, 1.0) * 127.0).round() as u8).max(1);
                Some([0x90, note, velocity])
            }
            TimelineEventKind::NoteOff {
                note_id,
                channel_id,
                note,
                mixer_track,
                source,
            } if channel_id == channel => {
                let index = active
                    .iter()
                    .position(|entry| entry.is_some_and(|n| n.id == note_id))
                    .ok_or_else(|| invalid("Selected NoteOff has no active identity"))?;
                let on =
                    active[index].ok_or_else(|| invalid("Selected note binding disappeared"))?;
                if event.frame <= on.on_frame
                    || event.frame > song_frames
                    || on.channel != channel_id
                    || on.pitch != note
                    || on.track != mixer_track
                    || on.source != source
                {
                    return Err(invalid(
                        "Selected NoteOff does not match its full NoteOn identity/range",
                    ));
                }
                active[index] = None;
                (!active
                    .iter()
                    .flatten()
                    .any(|n| n.channel == channel_id && n.pitch == note))
                .then_some([0x80, note, 0])
            }
            _ => None,
        };
        if let Some(data) = message {
            let block = event.frame / BOUNCE_QUANTUM as u64;
            if quantum != Some(block) {
                quantum = Some(block);
                count = 0;
            }
            count += 1;
            if count > TIMELINE_ENDPOINT_TIMELINE_MAX_EVENTS_PER_QUANTUM {
                return Err(invalid(
                    "Selected instrument exceeds 96 Timeline MIDI events in one Q128 quantum",
                ));
            }
            mapped.push(ScheduledMidi {
                frame: event.frame,
                data,
            });
        }
    }
    if active.iter().any(Option::is_some) {
        return Err(invalid(
            "Selected instrument contains unclosed note identities",
        ));
    }
    control.checkpoint()?;
    Ok(mapped)
}

pub(crate) struct PreparedRender {
    pub identity: BounceSourceIdentity,
    pub metadata: BouncePluginMetadata,
    pub sample_rate: u32,
    pub song_frames: u64,
    pub tail_frames: u32,
    pub retained_frames: usize,
    pub generated_frames: u64,
    pub tempo: f64,
    pub midi: Vec<ScheduledMidi>,
}
impl PreparedRender {
    pub fn from_capture(
        capture: &CapturedInstrumentBounce,
        acknowledge_fixed_tail: bool,
        control: &RenderControl,
    ) -> Result<Self, RenderFailure> {
        control.checkpoint()?;
        let plan = capture.plan();
        let timeline = plan.timeline();
        let invalid = |s: &str| RenderFailure::new(FailureKind::Invalid, s);
        if !acknowledge_fixed_tail {
            return Err(invalid(
                "Explicit fixed-tail truncation acknowledgment is required",
            ));
        }
        capture
            .metadata()
            .validate(plan.sample_rate())
            .map_err(|e| invalid(&e))?;
        if !timeline.plugin_routes().iter().any(|route| {
            route.instance_id == plan.instance_id()
                && route.destination
                    == (PluginRouteDestination::Generator {
                        channel_id: plan.channel_id(),
                        slot: 0,
                    })
        }) {
            return Err(invalid(
                "Compiled generator route differs from captured selection",
            ));
        }
        let song_frames = timeline.duration_frames();
        let (retained_frames, generated_frames) = render_frame_bounds(
            song_frames,
            plan.tail_frames(),
            capture.metadata().latency_samples,
        )?;
        let tempo = timeline.plugin_transport_tempo();
        if !tempo.is_finite() || tempo <= 0.0 {
            return Err(invalid("Invalid constant tempo"));
        }
        let midi = map_selected_notes(timeline.events(), plan.channel_id(), song_frames, control)?;
        Ok(Self {
            identity: capture.identity(),
            metadata: capture.metadata().clone(),
            sample_rate: plan.sample_rate(),
            song_frames,
            tail_frames: plan.tail_frames(),
            retained_frames,
            generated_frames,
            tempo,
            midi,
        })
    }
}

fn render_frame_bounds(
    song_frames: u64,
    tail: u32,
    latency: u32,
) -> Result<(usize, u64), RenderFailure> {
    let invalid = |s: &str| RenderFailure::new(FailureKind::Invalid, s);
    if song_frames == 0 {
        return Err(invalid("Zero-length instrument render"));
    }
    let retained = song_frames
        .checked_add(u64::from(tail))
        .ok_or_else(|| invalid("Retained frame overflow"))?;
    let end = retained
        .checked_add(u64::from(latency))
        .ok_or_else(|| invalid("Latency frame overflow"))?;
    // Include a matched off exactly at N even when N itself is a Q128 boundary.
    let generated = end
        .max(
            song_frames
                .checked_add(1)
                .ok_or_else(|| invalid("Closing frame overflow"))?,
        )
        .checked_add(BOUNCE_QUANTUM as u64 - 1)
        .ok_or_else(|| invalid("Padding overflow"))?
        / BOUNCE_QUANTUM as u64
        * BOUNCE_QUANTUM as u64;
    if generated > MAX_BOUNCE_FRAMES || retained > MAX_BOUNCE_FRAMES {
        return Err(invalid("Rounded render exceeds 24-million-frame bound"));
    }
    let retained = usize::try_from(retained)
        .map_err(|_| invalid("Retained frames exceed platform capacity"))?;
    crate::export_options::WavSampleFormat::Float32
        .data_size_for_frames(retained)
        .map_err(|e| invalid(&e.to_string()))?;
    Ok((retained, generated))
}

fn record_cleanup_error(errors: &mut Vec<String>, message: String) {
    let bounded: String = message.chars().take(2048).collect();
    if !errors.contains(&bounded) {
        if errors.len() < 8 {
            errors.push(bounded);
        } else if errors.len() == 8 {
            errors.push("Additional cleanup diagnostics omitted after the first eight".into());
        }
    }
}

pub(crate) struct BlockReply {
    pub channels: Vec<Vec<f32>>,
    pub transport_applied: bool,
    pub output_events: usize,
    pub output_events_lost: bool,
}
/// A failed cleanup is not necessarily termination. Ownership remains with this backend.
pub(crate) struct DisposalObservation {
    pub terminated: bool,
    pub error: Option<String>,
}

/// Only plugin-free fixtures implement this in this checkpoint. A real adapter requires the
/// separately reviewed strict-isolation contract; ordinary Vst3Backend does not satisfy it.
pub(crate) trait RenderBackend: 'static {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<BouncePluginMetadata, String>;
    fn metadata(&mut self) -> Result<BouncePluginMetadata, String>;
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), String>;
    fn process(&mut self, transport: PluginTransport, frames: usize) -> Result<BlockReply, String>;
    /// Must be idempotently polled; normal cancellation cannot skip cleanup. Pending/failed
    /// disposal retains the owner. Acknowledgment includes all plugin/process/I/O resources.
    fn dispose(&mut self) -> DisposalObservation;
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderReport {
    pub identity: BounceSourceIdentity,
    pub frames: usize,
    pub native_latency: u32,
    pub reported_tail: u32,
    pub fixed_tail: u32,
    pub generated_frames: u64,
    pub sample_peak: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RenderOutcome {
    Complete(RenderReport),
    Failed(RenderFailure),
}

#[derive(Clone, Default)]
pub(crate) struct RenderAdmission(Arc<AtomicBool>);
pub(crate) struct RenderReceipt {
    receiver: Receiver<RenderOutcome>,
    control: RenderControl,
    terminated: Arc<AtomicBool>,
}
impl RenderReceipt {
    pub fn cancel(&self) -> bool {
        self.control.export.cancel()
    }
    pub fn termination_acknowledged(&self) -> bool {
        self.terminated.load(Ordering::Acquire)
    }
    pub fn try_recv(&self) -> Result<RenderOutcome, TryRecvError> {
        if !self.termination_acknowledged() {
            return Err(TryRecvError::Empty);
        }
        self.receiver.try_recv()
    }
}
impl Drop for RenderReceipt {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl RenderAdmission {
    pub fn is_busy(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    /// The factory here creates an empty fixture owner, not a loaded plugin. Strict production
    /// spawn/load ownership is deliberately absent until its foundation is qualified.
    pub fn start<B: RenderBackend>(
        &self,
        capture: CapturedInstrumentBounce,
        path: PathBuf,
        acknowledge_fixed_tail: bool,
        control: RenderControl,
        fixture_owner: impl FnOnce() -> B + Send + 'static,
    ) -> Result<RenderReceipt, String> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "An instrument renderer still owns this job")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let terminated = Arc::new(AtomicBool::new(false));
        let ack = Arc::clone(&terminated);
        let busy = Arc::clone(&self.0);
        let worker_control = control.clone();
        let spawn = std::thread::Builder::new()
            .name("dormant-instrument-render".into())
            .spawn(move || {
                let outcome = run_owned(
                    capture,
                    path,
                    acknowledge_fixed_tail,
                    &worker_control,
                    fixture_owner,
                );
                let _ = sender.try_send(outcome);
                drop(sender);
                // Every backend/file owner has terminated before publication of this final ack.
                busy.store(false, Ordering::Release);
                ack.store(true, Ordering::Release);
            });
        if let Err(error) = spawn {
            self.0.store(false, Ordering::Release);
            return Err(error.to_string());
        }
        Ok(RenderReceipt {
            receiver,
            control,
            terminated,
        })
    }
}

fn run_owned<B: RenderBackend>(
    capture: CapturedInstrumentBounce,
    path: PathBuf,
    acknowledge_fixed_tail: bool,
    control: &RenderControl,
    fixture_owner: impl FnOnce() -> B,
) -> RenderOutcome {
    let prepared = match catch_unwind(AssertUnwindSafe(|| {
        PreparedRender::from_capture(&capture, acknowledge_fixed_tail, control)
    })) {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(error)) => return RenderOutcome::Failed(error),
        Err(_) => {
            return RenderOutcome::Failed(RenderFailure::new(
                FailureKind::Panic,
                "Render preflight panicked",
            ));
        }
    };
    let mut backend = match catch_unwind(AssertUnwindSafe(fixture_owner)) {
        Ok(backend) => backend,
        Err(_) => {
            return RenderOutcome::Failed(RenderFailure::new(
                FailureKind::Panic,
                "Empty fixture owner construction panicked",
            ));
        }
    };
    // Backend stays outside the unwind scope so every rendering panic still enters disposal.
    let rendered = catch_unwind(AssertUnwindSafe(|| {
        render_audio(&capture, &prepared, &mut backend, control)
    }))
    .unwrap_or_else(|_| {
        Err(RenderFailure::new(
            FailureKind::Panic,
            "Instrument render panicked",
        ))
    });
    let mut cleanup_errors = Vec::new();
    loop {
        let observation = catch_unwind(AssertUnwindSafe(|| backend.dispose()));
        match observation {
            Ok(observation) => {
                if let Some(error) = observation.error {
                    record_cleanup_error(&mut cleanup_errors, error);
                }
                if observation.terminated {
                    break;
                }
            }
            Err(_) => {
                let error = "Instrument disposal panicked; termination is unproven".to_owned();
                record_cleanup_error(&mut cleanup_errors, error);
            }
        }
        // No result/lease release while resources remain. This is an owner-retained wait,
        // not another musical/setup request or an automatic fresh helper retry.
        std::thread::sleep(Duration::from_millis(1));
    }
    if catch_unwind(AssertUnwindSafe(|| drop(backend))).is_err() {
        cleanup_errors.push("Disposed backend destructor panicked".into());
    }
    let audio = match rendered {
        Err(mut error) => {
            error.cleanup = cleanup_errors;
            return RenderOutcome::Failed(error);
        }
        Ok(_) if !cleanup_errors.is_empty() => {
            return RenderOutcome::Failed(RenderFailure {
                kind: FailureKind::Backend,
                primary: "Instrument cleanup failed".into(),
                cleanup: cleanup_errors,
            });
        }
        Ok(audio) => audio,
    };
    if let Err(error) = control.checkpoint() {
        return RenderOutcome::Failed(error);
    }
    let written = catch_unwind(AssertUnwindSafe(|| {
        write_stereo_float32_checked(
            &path,
            prepared.sample_rate,
            &audio,
            &control.export,
            control.deadline,
        )
    }));
    match written {
        Ok(Ok(())) => RenderOutcome::Complete(RenderReport {
            identity: prepared.identity,
            frames: prepared.retained_frames,
            native_latency: prepared.metadata.latency_samples,
            reported_tail: prepared.metadata.tail_samples,
            fixed_tail: prepared.tail_frames,
            generated_frames: prepared.generated_frames,
            sample_peak: audio
                .iter()
                .fold(0.0_f32, |p, f| p.max(f.left.abs()).max(f.right.abs())),
        }),
        Ok(Err(error)) => RenderOutcome::Failed(RenderFailure::new(
            FailureKind::Output,
            format!("{error:#}"),
        )),
        Err(_) => RenderOutcome::Failed(RenderFailure::new(
            FailureKind::Panic,
            "Checked WAV writer panicked",
        )),
    }
}

fn render_audio<B: RenderBackend>(
    capture: &CapturedInstrumentBounce,
    prepared: &PreparedRender,
    backend: &mut B,
    control: &RenderControl,
) -> Result<Vec<StereoFrame>, RenderFailure> {
    let invalid = |s: &str| RenderFailure::new(FailureKind::Invalid, s);
    let mut audio = Vec::new();
    audio
        .try_reserve_exact(prepared.retained_frames)
        .map_err(|e| invalid(&format!("Audio allocation failed: {e}")))?;
    control.call_checked(
        || backend.prepare(capture, control),
        |actual| {
            if actual != &prepared.metadata {
                return Err(invalid("Fresh metadata differs from captured metadata"));
            }
            Ok(())
        },
    )?;
    let mut next = 0;
    let keep_start = u64::from(prepared.metadata.latency_samples);
    let keep_end = keep_start + prepared.retained_frames as u64;
    for start in (0..prepared.generated_frames).step_by(BOUNCE_QUANTUM) {
        control.call_checked(
            || backend.metadata(),
            |before| {
                if before != &prepared.metadata {
                    return Err(invalid("Instrument metadata changed before Process"));
                }
                Ok(())
            },
        )?;
        while let Some(event) = prepared
            .midi
            .get(next)
            .filter(|event| event.frame < start + BOUNCE_QUANTUM as u64)
        {
            let offset = event
                .frame
                .checked_sub(start)
                .and_then(|n| u16::try_from(n).ok())
                .filter(|&n| n < BOUNCE_QUANTUM as u16)
                .ok_or_else(|| invalid("Invalid scheduled MIDI offset"))?;
            control.call(|| backend.send_midi(event.data, offset))?;
            next += 1;
        }
        let transport = PluginTransport {
            sample_position: start as i64,
            quarter_note_position: start as f64 * prepared.tempo
                / (f64::from(prepared.sample_rate) * 60.0),
            tempo: prepared.tempo,
            playing: true,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
        };
        let expected = prepared.metadata.output_buses[0].0;
        let block = control.call_checked(
            || backend.process(transport, BOUNCE_QUANTUM),
            |block| {
                if !block.transport_applied
                    || block.output_events != 0
                    || block.output_events_lost
                    || block.channels.len() != expected
                    || block.channels.iter().any(|c| {
                        c.len() != BOUNCE_QUANTUM || c.iter().any(|value| !value.is_finite())
                    })
                {
                    return Err(invalid(
                        "Invalid exact Process output shape, sample, transport or MIDI/loss",
                    ));
                }
                Ok(())
            },
        )?;
        control.call_checked(
            || backend.metadata(),
            |after| {
                if after != &prepared.metadata {
                    return Err(invalid("Instrument metadata changed after Process"));
                }
                Ok(())
            },
        )?;
        // The entire reply above is checked, including discarded latency and closing padding.
        for index in 0..BOUNCE_QUANTUM {
            let frame = start + index as u64;
            if (keep_start..keep_end).contains(&frame) {
                audio.push(StereoFrame {
                    left: block.channels[0][index],
                    right: block.channels[expected - 1][index],
                });
            }
        }
    }
    if next != prepared.midi.len() || audio.len() != prepared.retained_frames {
        return Err(invalid(
            "Render did not consume its complete schedule/output interval",
        ));
    }
    control.checkpoint()?;
    Ok(audio)
}

#[cfg(test)]
#[path = "instrument_bounce_render_tests.rs"]
mod tests;
