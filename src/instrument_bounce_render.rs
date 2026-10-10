//! Dormant selected-generator engine with a closed, typed backend contract.
//! The opt-in strict adapter remains separate from the public export/UI caller.
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
        Arc, Mutex,
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

/// Backend errors retain cancellation and transport evidence without parsing diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendFailureKind {
    Cancelled,
    Deadline,
    Backend,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendDelivery {
    NotSent,
    MayHaveApplied,
}

pub(crate) const MAX_BACKEND_DIAGNOSTIC_BYTES: usize = 2048;
const MAX_CLEANUP_DIAGNOSTICS: usize = 8;
fn bounded_diagnostic(message: &str) -> String {
    let mut end = message.len().min(MAX_BACKEND_DIAGNOSTIC_BYTES);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BackendFailure {
    pub kind: BackendFailureKind,
    pub delivery: Option<BackendDelivery>,
    diagnostic: String,
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub strict_kind: Option<vst3_host::strict_offline::FailureKind>,
    #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
    pub strict_diagnostic_unavailable: Option<vst3_host::strict_offline::FailureKind>,
}
impl BackendFailure {
    pub fn new(kind: BackendFailureKind, diagnostic: impl AsRef<str>) -> Self {
        Self {
            kind,
            delivery: None,
            diagnostic: bounded_diagnostic(diagnostic.as_ref()),
            #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
            strict_kind: None,
            #[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
            strict_diagnostic_unavailable: None,
        }
    }
    pub fn with_delivery(mut self, delivery: BackendDelivery) -> Self {
        self.delivery = Some(delivery);
        self
    }
    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}
impl From<String> for BackendFailure {
    fn from(value: String) -> Self {
        Self::new(BackendFailureKind::Backend, value)
    }
}
impl From<&str> for BackendFailure {
    fn from(value: &str) -> Self {
        Self::new(BackendFailureKind::Backend, value)
    }
}
impl std::fmt::Display for BackendFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.diagnostic())
    }
}
#[cfg(all(target_os = "linux", feature = "experimental-fresh-helper"))]
impl From<vst3_host::strict_offline::Failure> for BackendFailure {
    fn from(value: vst3_host::strict_offline::Failure) -> Self {
        use vst3_host::strict_offline::{Delivery, FailureKind as StrictKind};
        let kind = match value.kind {
            StrictKind::Cancelled => BackendFailureKind::Cancelled,
            StrictKind::Deadline => BackendFailureKind::Deadline,
            _ => BackendFailureKind::Backend,
        };
        let fallback;
        let diagnostic = match value.diagnostic.as_deref() {
            Some(diagnostic) => diagnostic,
            None => {
                fallback = format!("Strict offline exchange failed: {:?}", value.kind);
                &fallback
            }
        };
        let mut failure = Self::new(kind, diagnostic).with_delivery(match value.delivery {
            Delivery::NotSent => BackendDelivery::NotSent,
            Delivery::MayHaveApplied => BackendDelivery::MayHaveApplied,
        });
        failure.strict_kind = Some(value.kind);
        failure.strict_diagnostic_unavailable = value.diagnostic_unavailable;
        failure
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenderFailure {
    pub kind: FailureKind,
    pub primary: String,
    /// Exact backend evidence for the primary failure, if it came from the backend.
    /// Boxing keeps the by-value Result envelope small. These bounded evidence
    /// allocations occur only on the renderer owner's error-reporting path.
    pub backend: Option<Box<BackendFailure>>,
    /// First typed disposal error, even when an earlier render fault wins.
    pub cleanup_backend: Option<Box<BackendFailure>>,
    /// Independent reporting-grace evidence, never a transport deadline.
    pub cleanup_grace_elapsed: bool,
    pub cleanup: Vec<String>,
}
impl RenderFailure {
    fn new(kind: FailureKind, primary: impl Into<String>) -> Self {
        Self {
            kind,
            primary: primary.into(),
            backend: None,
            cleanup_backend: None,
            cleanup_grace_elapsed: false,
            cleanup: Vec::new(),
        }
    }
}

impl From<BackendFailure> for RenderFailure {
    fn from(failure: BackendFailure) -> Self {
        let kind = match failure.kind {
            BackendFailureKind::Cancelled => FailureKind::Cancelled,
            BackendFailureKind::Deadline => FailureKind::Deadline,
            BackendFailureKind::Backend => FailureKind::Backend,
        };
        Self {
            kind,
            primary: failure.diagnostic().to_owned(),
            backend: Some(Box::new(failure)),
            cleanup_backend: None,
            cleanup_grace_elapsed: false,
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
    fn call<T>(
        &self,
        call: impl FnOnce() -> Result<T, BackendFailure>,
    ) -> Result<T, RenderFailure> {
        self.call_checked(call, |_| Ok(()))
    }
    fn call_checked<T>(
        &self,
        call: impl FnOnce() -> Result<T, BackendFailure>,
        validate: impl FnOnce(&T) -> Result<(), RenderFailure>,
    ) -> Result<T, RenderFailure> {
        self.checkpoint()?;
        // A dispatched failure/panic or malformed reply wins over cancellation
        // that arrives in flight. Validate before observing the post-call fence.
        let value = catch_unwind(AssertUnwindSafe(call))
            .map_err(|_| RenderFailure::new(FailureKind::Panic, "Instrument backend panicked"))?
            .map_err(RenderFailure::from)?;
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
    pub origin_frame: u64,
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
        if plan.occurrence_request().is_some() {
            return Err(RenderFailure::new(
                FailureKind::Invalid,
                "An occurrence capture requires the occurrence renderer entry point",
            ));
        }
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
            origin_frame: 0,
            song_frames,
            tail_frames: plan.tail_frames(),
            retained_frames,
            generated_frames,
            tempo,
            midi,
        })
    }

    pub(crate) fn from_occurrence(
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<Self, RenderFailure> {
        control.checkpoint()?;
        let provenance = capture
            .occurrence_provenance()
            .map_err(|e| RenderFailure::new(FailureKind::Invalid, e))?;
        let request = provenance.request();
        let plan = capture.plan();
        if !plan.timeline().plugin_routes().iter().any(|route| {
            route.instance_id == request.instance_id()
                && route.destination
                    == (PluginRouteDestination::Generator {
                        channel_id: request.channel_id(),
                        slot: 0,
                    })
        }) {
            return Err(RenderFailure::new(
                FailureKind::Invalid,
                "Occurrence route differs from captured source",
            ));
        }
        let (retained_frames, generated_frames) = render_frame_bounds(
            request.song_frames(),
            request.tail_frames(),
            capture.metadata().latency_samples,
        )?;
        request
            .start_frame()
            .checked_add(generated_frames)
            .filter(|end| *end <= i64::MAX as u64)
            .ok_or_else(|| {
                RenderFailure::new(
                    FailureKind::Invalid,
                    "Absolute occurrence transport exceeds its signed frame range",
                )
            })?;
        let midi = map_selected_notes(
            request.local_events(),
            request.channel_id(),
            request.song_frames(),
            control,
        )?;
        control.checkpoint()?;
        Ok(Self {
            identity: capture.identity(),
            metadata: capture.metadata().clone(),
            sample_rate: request.sample_rate(),
            origin_frame: request.start_frame(),
            song_frames: request.song_frames(),
            tail_frames: request.tail_frames(),
            retained_frames,
            generated_frames,
            tempo: request.tempo(),
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
    let bounded = bounded_diagnostic(&message);
    if !errors.contains(&bounded) {
        if errors.len() < MAX_CLEANUP_DIAGNOSTICS {
            errors.push(bounded);
        } else if errors.len() == MAX_CLEANUP_DIAGNOSTICS {
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
    /// First observed cleanup failure, preserved across Pending polls. Concrete
    /// termination is still mandatory, including after Cancelled or Deadline.
    pub error: Option<BackendFailure>,
    /// Secondary evidence only. The renderer bounds count and UTF-8 bytes on intake.
    pub diagnostics: Vec<String>,
}

/// Closed worker-owned backend seam. Ordinary Vst3Backend does not satisfy the
/// strict isolation, typed exchange failure and checked disposal contract.
pub(crate) trait RenderBackend: 'static {
    fn prepare(
        &mut self,
        capture: &CapturedInstrumentBounce,
        control: &RenderControl,
    ) -> Result<BouncePluginMetadata, BackendFailure>;
    fn metadata(&mut self) -> Result<BouncePluginMetadata, BackendFailure>;
    fn send_midi(&mut self, midi: [u8; 3], offset: u16) -> Result<(), BackendFailure>;
    fn process(
        &mut self,
        transport: PluginTransport,
        frames: usize,
    ) -> Result<BlockReply, BackendFailure>;
    /// Must be idempotently polled; normal cancellation cannot skip cleanup. Pending/failed
    /// disposal retains the owner. Acknowledgment includes all plugin/process/I/O resources.
    /// Each poll must return promptly, honoring this absolute grace deadline;
    /// after expiry it may only make nonblocking cleanup progress. The engine
    /// cannot turn an arbitrary blocking OS call into bounded termination.
    fn dispose(&mut self, deadline: Instant) -> DisposalObservation;
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RenderReport {
    pub identity: BounceSourceIdentity,
    pub origin_frame: u64,
    pub musical_frames: u64,
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

/// Nonterminal cleanup visibility never substitutes for termination acknowledgment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RenderStatus {
    Running,
    Disposing,
    CleanupPending { diagnostics: Vec<String> },
    Terminated,
}
struct JobProgress {
    status: Mutex<RenderStatus>,
    #[cfg(test)]
    expire_cleanup_grace: AtomicBool,
}
impl JobProgress {
    fn new() -> Self {
        Self {
            status: Mutex::new(RenderStatus::Running),
            #[cfg(test)]
            expire_cleanup_grace: AtomicBool::new(false),
        }
    }
    fn status(&self) -> RenderStatus {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    fn set(&self, value: RenderStatus) {
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = value;
    }
    fn grace_elapsed(&self, deadline: Instant) -> bool {
        let elapsed = Instant::now() >= deadline;
        #[cfg(test)]
        let elapsed = elapsed || self.expire_cleanup_grace.load(Ordering::Acquire);
        elapsed
    }
}
#[derive(Default)]
struct RenderSlot {
    busy: AtomicBool,
    // Retain visibility even when the caller drops its result receiver.
    job: Mutex<Option<Arc<JobProgress>>>,
}
#[derive(Clone, Default)]
pub(crate) struct RenderAdmission(Arc<RenderSlot>);
pub(crate) struct RenderReceipt {
    receiver: Receiver<RenderOutcome>,
    control: RenderControl,
    terminated: Arc<AtomicBool>,
    progress: Arc<JobProgress>,
}
impl RenderReceipt {
    pub fn status(&self) -> RenderStatus {
        self.progress.status()
    }
    #[cfg(test)]
    fn expire_cleanup_grace(&self) {
        self.progress
            .expire_cleanup_grace
            .store(true, Ordering::Release);
    }
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
        self.0.busy.load(Ordering::Acquire)
    }
    pub fn status(&self) -> Option<RenderStatus> {
        self.0
            .job
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|job| job.status())
    }
    /// The factory creates an empty backend on the worker. A loaded plugin/process
    /// owner must never cross threads; the backend owns spawn/load during prepare.
    pub fn start<B: RenderBackend>(
        &self,
        capture: CapturedInstrumentBounce,
        path: PathBuf,
        acknowledge_fixed_tail: bool,
        control: RenderControl,
        fixture_owner: impl FnOnce() -> B + Send + 'static,
    ) -> Result<RenderReceipt, String> {
        self.0
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "An instrument renderer still owns this job")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let terminated = Arc::new(AtomicBool::new(false));
        let ack = Arc::clone(&terminated);
        let slot = Arc::clone(&self.0);
        let progress = Arc::new(JobProgress::new());
        *self.0.job.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::clone(&progress));
        let worker_progress = Arc::clone(&progress);
        let worker_control = control.clone();
        let spawn = std::thread::Builder::new()
            .name("dormant-instrument-render".into())
            .spawn(move || {
                let outcome = run_owned(
                    capture,
                    path,
                    acknowledge_fixed_tail,
                    &worker_control,
                    &worker_progress,
                    fixture_owner,
                );
                let _ = sender.try_send(outcome);
                drop(sender);
                // Every backend/file owner has terminated before publication of this final ack.
                worker_progress.set(RenderStatus::Terminated);
                slot.busy.store(false, Ordering::Release);
                ack.store(true, Ordering::Release);
            });
        if let Err(error) = spawn {
            progress.set(RenderStatus::Terminated);
            self.0.busy.store(false, Ordering::Release);
            return Err(error.to_string());
        }
        Ok(RenderReceipt {
            receiver,
            control,
            terminated,
            progress,
        })
    }
}

fn run_owned<B: RenderBackend>(
    capture: CapturedInstrumentBounce,
    path: PathBuf,
    acknowledge_fixed_tail: bool,
    control: &RenderControl,
    progress: &JobProgress,
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
    let audio = match render_and_dispose(&capture, &prepared, control, progress, fixture_owner) {
        Ok(audio) => audio,
        Err(error) => return RenderOutcome::Failed(error),
    };
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
            origin_frame: prepared.origin_frame,
            musical_frames: prepared.song_frames,
            frames: prepared.retained_frames,
            native_latency: prepared.metadata.latency_samples,
            reported_tail: prepared.metadata.tail_samples,
            fixed_tail: prepared.tail_frames,
            generated_frames: prepared.generated_frames,
            sample_peak: audio
                .iter()
                .fold(0.0_f32, |p, f| p.max(f.left.abs()).max(f.right.abs())),
        }),
        Ok(Err(error)) => RenderOutcome::Failed(writer_failure(error)),
        Err(_) => RenderOutcome::Failed(RenderFailure::new(
            FailureKind::Panic,
            "Checked WAV writer panicked",
        )),
    }
}

fn render_and_dispose<B: RenderBackend>(
    capture: &CapturedInstrumentBounce,
    prepared: &PreparedRender,
    control: &RenderControl,
    progress: &JobProgress,
    fixture_owner: impl FnOnce() -> B,
) -> Result<Vec<StereoFrame>, RenderFailure> {
    let mut backend = match catch_unwind(AssertUnwindSafe(fixture_owner)) {
        Ok(backend) => backend,
        Err(_) => {
            return Err(RenderFailure::new(
                FailureKind::Panic,
                "Empty fixture owner construction panicked",
            ));
        }
    };
    // Backend stays outside the unwind scope so every rendering panic still enters disposal.
    let rendered = catch_unwind(AssertUnwindSafe(|| {
        render_audio(capture, prepared, &mut backend, control)
    }))
    .unwrap_or_else(|_| {
        Err(RenderFailure::new(
            FailureKind::Panic,
            "Instrument render panicked",
        ))
    });
    let (mut audio, mut failure) = match rendered {
        Ok(audio) => (Some(audio), None),
        Err(error) => (None, Some(error)),
    };
    let mut cleanup_errors = Vec::new();
    let mut cleanup_primary: Option<RenderFailure> = None;
    let mut cleanup_backend = None;
    let mut grace_expired = false;
    // One independent reporting/reap grace; never renew the caller's protocol
    // deadline. Pending polls retain the same backend and admission indefinitely.
    let cleanup_deadline = Instant::now() + Duration::from_secs(5);
    progress.set(RenderStatus::Disposing);
    loop {
        let observation = catch_unwind(AssertUnwindSafe(|| backend.dispose(cleanup_deadline)));
        let terminated = match observation {
            Ok(observation) => {
                if let Some(error) = observation.error {
                    if cleanup_backend.is_none() {
                        cleanup_backend = Some(Box::new(error.clone()));
                    }
                    let error = RenderFailure::from(error);
                    if cleanup_primary.is_none() {
                        cleanup_primary = Some(error);
                    } else if cleanup_primary.as_ref() != Some(&error) {
                        record_cleanup_error(&mut cleanup_errors, error.primary);
                    }
                }
                for diagnostic in observation.diagnostics {
                    // A backend must supply the typed first error. Fail closed if
                    // it returns only diagnostic evidence of cleanup trouble.
                    if cleanup_primary.is_none() {
                        cleanup_primary = Some(RenderFailure::from(BackendFailure::from(
                            "Instrument cleanup reported a diagnostic without a primary failure",
                        )));
                    }
                    record_cleanup_error(&mut cleanup_errors, diagnostic);
                }
                observation.terminated
            }
            Err(_) => {
                let error = RenderFailure::new(
                    FailureKind::Panic,
                    "Instrument disposal panicked; termination is unproven",
                );
                if cleanup_primary.is_none() {
                    cleanup_primary = Some(error);
                } else {
                    record_cleanup_error(&mut cleanup_errors, error.primary);
                }
                false
            }
        };
        // Check after every returned poll, including its terminal reply. Elapsed
        // reporting grace forbids publication but is not a transport Deadline or
        // an established helper/OS fault that could mask a later typed failure.
        let grace_elapsed = progress.grace_elapsed(cleanup_deadline);
        if grace_elapsed && !grace_expired {
            grace_expired = true;
            record_cleanup_error(
                &mut cleanup_errors,
                "Cleanup grace elapsed before termination was acknowledged".into(),
            );
        }
        if cleanup_primary.is_some() || grace_expired {
            // Release completed audio while the backend remains owned until reap.
            drop(audio.take());
        }
        if grace_elapsed && !terminated {
            let mut diagnostics = Vec::new();
            if let Some(error) = &cleanup_primary {
                record_cleanup_error(&mut diagnostics, error.primary.clone());
            }
            for error in &cleanup_errors {
                record_cleanup_error(&mut diagnostics, error.clone());
            }
            progress.set(RenderStatus::CleanupPending { diagnostics });
        }
        if terminated {
            break;
        }
        // This is not a completed cancellation, detached cleanup, replacement
        // helper, or lease release. Post-grace adapter polls must not wait.
        std::thread::sleep(if grace_elapsed {
            Duration::from_millis(250)
        } else {
            Duration::from_millis(10)
        });
    }
    if catch_unwind(AssertUnwindSafe(|| drop(backend))).is_err() {
        let error = RenderFailure::new(FailureKind::Panic, "Disposed backend destructor panicked");
        if cleanup_primary.is_none() {
            cleanup_primary = Some(error);
        } else {
            record_cleanup_error(&mut cleanup_errors, error.primary);
        }
    }
    if let Some(cleanup_error) = cleanup_primary {
        if let Some(render_error) = &mut failure {
            record_cleanup_error(&mut render_error.cleanup, cleanup_error.primary);
        } else {
            failure = Some(cleanup_error);
        }
    }
    // The final fence follows proven termination and backend destruction. Earlier
    // render or actual cleanup faults win over later flags. A reporting-grace
    // expiry alone cannot relabel a real cancellation/deadline as Backend.
    if failure.is_none() {
        failure = control.checkpoint().err();
    }
    if failure.is_none() && grace_expired {
        failure = Some(RenderFailure::new(
            FailureKind::Backend,
            "Instrument cleanup reporting grace elapsed",
        ));
    }
    if let Some(mut error) = failure {
        error.cleanup_backend = cleanup_backend;
        error.cleanup_grace_elapsed = grace_expired;
        for diagnostic in cleanup_errors {
            record_cleanup_error(&mut error.cleanup, diagnostic);
        }
        return Err(error);
    }
    Ok(audio.expect("successful render and checked disposal retain completed audio"))
}

fn writer_failure(error: anyhow::Error) -> RenderFailure {
    let kind = if error.is::<crate::export_job::ExportCancelled>() {
        FailureKind::Cancelled
    } else if error.is::<crate::export_job::ExportDeadlineExceeded>() {
        FailureKind::Deadline
    } else {
        FailureKind::Output
    };
    // Checked abort context retains the original typed cause and any failed
    // removal/path. Do not reduce this to the outer context's to_string().
    RenderFailure::new(kind, format!("{error:#}"))
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
        let absolute = prepared
            .origin_frame
            .checked_add(start)
            .and_then(|frame| i64::try_from(frame).ok())
            .ok_or_else(|| {
                RenderFailure::new(
                    FailureKind::Invalid,
                    "Absolute occurrence transport overflow",
                )
            })?;
        let transport = PluginTransport {
            sample_position: absolute,
            quarter_note_position: absolute as f64 * prepared.tempo
                / (f64::from(prepared.sample_rate) * 60.0),
            tempo: prepared.tempo,
            playing: true,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
        };
        let expected = prepared.metadata.output_buses[0].channel_count;
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

/// The occurrence writer carries its linked-file evidence separately from a
/// render/backend failure. A successful file is never retroactively cancelled.
#[derive(Debug)]
pub(crate) enum OccurrenceRenderOutcome {
    Complete {
        report: RenderReport,
        published: crate::occurrence_media::PublishedOccurrenceRender,
    },
    RenderFailed(RenderFailure),
    PublicationFailed(crate::occurrence_media::OccurrencePublicationFailure),
}

pub(crate) struct OccurrenceRenderReceipt {
    receiver: Receiver<OccurrenceRenderOutcome>,
    control: RenderControl,
    terminated: Arc<AtomicBool>,
    progress: Arc<JobProgress>,
}
impl OccurrenceRenderReceipt {
    pub(crate) fn cancel(&self) -> bool {
        self.control.export.cancel()
    }
    pub(crate) fn status(&self) -> RenderStatus {
        self.progress.status()
    }
    pub(crate) fn termination_acknowledged(&self) -> bool {
        self.terminated.load(Ordering::Acquire)
    }
    pub(crate) fn try_recv(&self) -> Result<OccurrenceRenderOutcome, TryRecvError> {
        if !self.termination_acknowledged() {
            return Err(TryRecvError::Empty);
        }
        self.receiver.try_recv()
    }
}
impl Drop for OccurrenceRenderReceipt {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl RenderAdmission {
    /// Same one-render admission and backend lifetime kernel as whole-song
    /// rendering. The occurrence writer alone uses create-new publication.
    pub(crate) fn start_occurrence<B: RenderBackend>(
        &self,
        capture: CapturedInstrumentBounce,
        path: PathBuf,
        control: RenderControl,
        factory: impl FnOnce() -> B + Send + 'static,
    ) -> Result<OccurrenceRenderReceipt, String> {
        self.0
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "An instrument renderer still owns this job")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let terminated = Arc::new(AtomicBool::new(false));
        let ack = Arc::clone(&terminated);
        let slot = Arc::clone(&self.0);
        let progress = Arc::new(JobProgress::new());
        *self.0.job.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::clone(&progress));
        let worker_progress = Arc::clone(&progress);
        let worker_control = control.clone();
        let spawn = std::thread::Builder::new()
            .name("dormant-occurrence-render".into())
            .spawn(move || {
                let outcome =
                    run_occurrence_owned(capture, path, &worker_control, &worker_progress, factory);
                let _ = sender.try_send(outcome);
                drop(sender);
                // The generated PCM and captured state have both been dropped before
                // this fence. A verifier may now own raw bytes plus decoded PCM.
                worker_progress.set(RenderStatus::Terminated);
                slot.busy.store(false, Ordering::Release);
                ack.store(true, Ordering::Release);
            });
        if let Err(error) = spawn {
            progress.set(RenderStatus::Terminated);
            self.0.busy.store(false, Ordering::Release);
            return Err(error.to_string());
        }
        Ok(OccurrenceRenderReceipt {
            receiver,
            control,
            terminated,
            progress,
        })
    }
}

fn run_occurrence_owned<B: RenderBackend>(
    capture: CapturedInstrumentBounce,
    path: PathBuf,
    control: &RenderControl,
    progress: &JobProgress,
    factory: impl FnOnce() -> B,
) -> OccurrenceRenderOutcome {
    let preflight = catch_unwind(AssertUnwindSafe(|| {
        let prepared = PreparedRender::from_occurrence(&capture, control)?;
        let provenance = capture
            .occurrence_provenance()
            .map_err(|e| RenderFailure::new(FailureKind::Invalid, e))?;
        control.checkpoint()?;
        Ok::<_, RenderFailure>((prepared, provenance))
    }));
    let (prepared, provenance) = match preflight {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => return OccurrenceRenderOutcome::RenderFailed(error),
        Err(_) => {
            return OccurrenceRenderOutcome::RenderFailed(RenderFailure::new(
                FailureKind::Panic,
                "Occurrence render preflight panicked",
            ));
        }
    };
    let audio = match render_and_dispose(&capture, &prepared, control, progress, factory) {
        Ok(audio) => audio,
        Err(error) => return OccurrenceRenderOutcome::RenderFailed(error),
    };
    let report = RenderReport {
        identity: prepared.identity,
        origin_frame: prepared.origin_frame,
        musical_frames: prepared.song_frames,
        frames: prepared.retained_frames,
        native_latency: prepared.metadata.latency_samples,
        reported_tail: prepared.metadata.tail_samples,
        fixed_tail: prepared.tail_frames,
        generated_frames: prepared.generated_frames,
        sample_peak: audio.iter().fold(0.0_f32, |peak, frame| {
            peak.max(frame.left.abs()).max(frame.right.abs())
        }),
    };
    // This producer owns its unwind boundary and typed post-link evidence. Do not
    // flatten a linked-path failure into the legacy string-only writer envelope.
    let published = crate::occurrence_media::publish_occurrence_render(
        provenance,
        &path,
        prepared.sample_rate,
        &audio,
        &control.export,
        control.deadline,
    );
    drop(audio);
    drop(capture);
    match published {
        Ok(published) => OccurrenceRenderOutcome::Complete { report, published },
        Err(error) => OccurrenceRenderOutcome::PublicationFailed(error),
    }
}
