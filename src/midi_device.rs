//! MIDI device ownership and bounded realtime mailbox boundaries.
//!
//! The platform connection objects live on the control thread (input) or a dedicated output
//! worker. The eventual audio integration receives only [`MidiInputReceiver`] and
//! [`MidiOutputSender`]; neither path shares the audio-command queue.
//!
//! This module stops at device ownership and mailbox delivery. Output packets are sent promptly
//! by the worker; sample-clock deadlines and audio-frame scheduling are intentionally not claimed.

use crate::midi_runtime::LiveMidiEvent;
#[cfg(any(windows, test))]
use crate::midi_runtime::{MidiEventPriority, classify_event_priority};
#[cfg(any(windows, test))]
use rtrb::RingBuffer;
use rtrb::{Consumer, Producer, PushError};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::thread::Thread;
#[cfg(any(windows, test))]
use std::thread::{self, JoinHandle};
#[cfg(any(windows, test))]
use std::time::{Duration, Instant};

#[cfg(any(windows, test))]
const OUTPUT_WORKER_IDLE: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MidiPortDirection {
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MidiPortKey {
    pub direction: MidiPortDirection,
    /// Backend-owned opaque stable identifier. Never parse this string or treat it as an index.
    pub stable_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiPortInfo {
    pub key: MidiPortKey,
    pub display_name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiPortRefresh {
    pub ports: Vec<MidiPortInfo>,
    pub added: Vec<MidiPortKey>,
    pub removed: Vec<MidiPortKey>,
}

/// Control-thread hot-plug catalog. A refresh is a complete snapshot replacement.
#[derive(Clone, Debug, Default)]
pub struct MidiPortCatalog {
    ports: Vec<MidiPortInfo>,
}

impl MidiPortCatalog {
    pub fn ports(&self) -> &[MidiPortInfo] {
        &self.ports
    }

    pub fn refresh(&mut self, mut discovered: Vec<MidiPortInfo>) -> MidiPortRefresh {
        discovered.sort_by(|left, right| left.key.cmp(&right.key));
        discovered.dedup_by(|left, right| left.key == right.key);
        let old = self
            .ports
            .iter()
            .map(|port| port.key.clone())
            .collect::<BTreeSet<_>>();
        let new = discovered
            .iter()
            .map(|port| port.key.clone())
            .collect::<BTreeSet<_>>();
        let added = new.difference(&old).cloned().collect();
        let removed = old.difference(&new).cloned().collect();
        self.ports = discovered;
        MidiPortRefresh {
            ports: self.ports.clone(),
            added,
            removed,
        }
    }
}

#[derive(Debug, Default)]
pub struct MidiInputOverload {
    panic_required: AtomicBool,
    dropped_noncritical: AtomicU64,
    rejected_messages: AtomicU64,
}

impl MidiInputOverload {
    pub fn panic_required(&self) -> bool {
        self.panic_required.load(Ordering::Acquire)
    }

    pub fn take_panic_required(&self) -> bool {
        self.panic_required.swap(false, Ordering::AcqRel)
    }

    pub fn dropped_noncritical(&self) -> u64 {
        self.dropped_noncritical.load(Ordering::Relaxed)
    }

    pub fn rejected_messages(&self) -> u64 {
        self.rejected_messages.load(Ordering::Relaxed)
    }
}

/// The sole consumer of one OS-input callback's dedicated SPSC.
pub struct MidiInputReceiver {
    connection_epoch: u64,
    consumer: Consumer<LiveMidiEvent>,
    overload: Arc<MidiInputOverload>,
}

impl MidiInputReceiver {
    pub const fn connection_epoch(&self) -> u64 {
        self.connection_epoch
    }

    pub fn try_pop(&mut self) -> Option<LiveMidiEvent> {
        self.consumer.pop().ok()
    }

    pub fn overload(&self) -> &Arc<MidiInputOverload> {
        &self.overload
    }
}

#[cfg(any(windows, test))]
struct MidiInputCallbackWriter {
    connection_epoch: u64,
    next_sequence: u64,
    origin: Instant,
    producer: Producer<LiveMidiEvent>,
    overload: Arc<MidiInputOverload>,
}

#[cfg(any(windows, test))]
impl MidiInputCallbackWriter {
    fn receive(&mut self, timestamp_us: u64, message: &[u8]) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let arrival_mono_ns = self.origin.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        let Ok(event) = LiveMidiEvent::from_message(
            self.connection_epoch,
            sequence,
            timestamp_us,
            arrival_mono_ns,
            message,
        ) else {
            self.overload
                .rejected_messages
                .fetch_add(1, Ordering::Relaxed);
            return;
        };
        if let Err(PushError::Full(event)) = self.producer.push(event) {
            if classify_event_priority(event) == MidiEventPriority::MustPreserve {
                self.overload.panic_required.store(true, Ordering::Release);
            } else {
                self.overload
                    .dropped_noncritical
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(any(windows, test))]
fn input_mailbox(
    capacity: usize,
    connection_epoch: u64,
) -> Result<(MidiInputCallbackWriter, MidiInputReceiver), MidiDeviceError> {
    if capacity == 0 {
        return Err(MidiDeviceError::ZeroCapacity);
    }
    let (producer, consumer) = RingBuffer::new(capacity);
    let overload = Arc::new(MidiInputOverload::default());
    Ok((
        MidiInputCallbackWriter {
            connection_epoch,
            next_sequence: 0,
            origin: Instant::now(),
            producer,
            overload: Arc::clone(&overload),
        },
        MidiInputReceiver {
            connection_epoch,
            consumer,
            overload,
        },
    ))
}

#[cfg(test)]
pub(crate) struct TestMidiInputSender(MidiInputCallbackWriter);

#[cfg(test)]
impl TestMidiInputSender {
    pub(crate) fn send(&mut self, timestamp_us: u64, message: &[u8]) {
        self.0.receive(timestamp_us, message);
    }
}

#[cfg(test)]
pub(crate) fn test_input_mailbox(
    capacity: usize,
    connection_epoch: u64,
) -> (TestMidiInputSender, MidiInputReceiver) {
    let (writer, receiver) = input_mailbox(capacity, connection_epoch).unwrap();
    (TestMidiInputSender(writer), receiver)
}

/// Fixed-size MIDI 1.0 output packet. System and variable-length messages do not enter this path.
///
/// This ownership/mailbox layer sends packets as soon as its worker receives them. A target device
/// frame/deadline and sample-clock scheduler belong to the later audio integration and are not
/// represented here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiOutputPacket {
    pub sequence: u64,
    pub len: u8,
    pub data: [u8; 3],
}

impl MidiOutputPacket {
    pub fn from_message(sequence: u64, message: &[u8]) -> Result<Self, MidiOutputMessageError> {
        let event = LiveMidiEvent::from_message(1, sequence, 0, 0, message)
            .map_err(|_| MidiOutputMessageError::UnsupportedMessage)?;
        Ok(Self {
            sequence,
            len: event.len,
            data: event.data,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data[..usize::from(self.len)]
    }

    /// Note-off and channel all-notes/all-sound-off messages bypass the ordinary output queue.
    pub const fn is_emergency(self) -> bool {
        self.data[0] & 0xf0 == 0x80
            || (self.data[0] & 0xf0 == 0xb0 && (self.data[1] == 120 || self.data[1] == 123))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiOutputMessageError {
    UnsupportedMessage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiOutputPushError {
    Full,
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MidiOutputWorkerState {
    Running = 0,
    Closing = 1,
    SendFailed = 2,
    Stopped = 3,
}

#[derive(Debug)]
pub struct MidiOutputStatus {
    panic_required: AtomicBool,
    dropped_ordinary: AtomicU64,
    producer_inflight: AtomicU64,
    state: AtomicU8,
}

impl Default for MidiOutputStatus {
    fn default() -> Self {
        Self {
            panic_required: AtomicBool::new(false),
            dropped_ordinary: AtomicU64::new(0),
            producer_inflight: AtomicU64::new(0),
            state: AtomicU8::new(MidiOutputWorkerState::Running as u8),
        }
    }
}

impl MidiOutputStatus {
    pub fn panic_required(&self) -> bool {
        self.panic_required.load(Ordering::Acquire)
    }

    pub fn take_panic_required(&self) -> bool {
        self.panic_required.swap(false, Ordering::AcqRel)
    }

    pub fn dropped_ordinary(&self) -> u64 {
        self.dropped_ordinary.load(Ordering::Relaxed)
    }

    pub fn state(&self) -> MidiOutputWorkerState {
        match self.state.load(Ordering::Acquire) {
            0 => MidiOutputWorkerState::Running,
            1 => MidiOutputWorkerState::Closing,
            2 => MidiOutputWorkerState::SendFailed,
            _ => MidiOutputWorkerState::Stopped,
        }
    }
}

struct MidiOutputAdmission<'a> {
    status: &'a MidiOutputStatus,
}

impl<'a> MidiOutputAdmission<'a> {
    fn begin(status: &'a MidiOutputStatus) -> Result<Self, MidiOutputPushError> {
        // Increment before reading the gate. Once the worker publishes Closing, a producer that
        // increments later can only reject; a producer that still observes Running is already
        // visible to the worker's zero-count barrier. This ordering avoids a late-increment race.
        status.producer_inflight.fetch_add(1, Ordering::SeqCst);
        if status.state.load(Ordering::SeqCst) != MidiOutputWorkerState::Running as u8 {
            status.producer_inflight.fetch_sub(1, Ordering::SeqCst);
            return Err(MidiOutputPushError::WorkerUnavailable);
        }
        Ok(Self { status })
    }
}

impl Drop for MidiOutputAdmission<'_> {
    fn drop(&mut self) {
        self.status.producer_inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Producer side intended for the audio callback. It owns no device connection and never blocks.
pub struct MidiOutputSender {
    ordinary: Producer<MidiOutputPacket>,
    emergency: Producer<MidiOutputPacket>,
    worker_thread: Thread,
    status: Arc<MidiOutputStatus>,
}

impl MidiOutputSender {
    pub fn try_send(&mut self, packet: MidiOutputPacket) -> Result<(), MidiOutputPushError> {
        let admission = match MidiOutputAdmission::begin(&self.status) {
            Ok(admission) => admission,
            Err(error) => {
                if packet.is_emergency() {
                    self.status.panic_required.store(true, Ordering::Release);
                }
                return Err(error);
            }
        };
        let result = if packet.is_emergency() {
            self.emergency.push(packet)
        } else {
            self.ordinary.push(packet)
        };
        match result {
            Ok(()) => {
                self.worker_thread.unpark();
                drop(admission);
                Ok(())
            }
            Err(PushError::Full(_)) if packet.is_emergency() => {
                self.status.panic_required.store(true, Ordering::Release);
                drop(admission);
                Err(MidiOutputPushError::Full)
            }
            Err(PushError::Full(_)) => {
                self.status.dropped_ordinary.fetch_add(1, Ordering::Relaxed);
                drop(admission);
                Err(MidiOutputPushError::Full)
            }
        }
    }

    pub fn status(&self) -> &Arc<MidiOutputStatus> {
        &self.status
    }
}

#[cfg(any(windows, test))]
trait MidiOutputSink: Send + 'static {
    fn send(&mut self, message: &[u8]) -> Result<(), ()>;
}

#[cfg(any(windows, test))]
struct MidiOutputWorker {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

#[cfg(any(windows, test))]
impl MidiOutputWorker {
    fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            join.thread().unpark();
            let _ = join.join();
        }
    }
}

#[cfg(any(windows, test))]
impl Drop for MidiOutputWorker {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

#[cfg(any(windows, test))]
fn start_output_worker<S: MidiOutputSink>(
    mut sink: S,
    ordinary_capacity: usize,
    emergency_capacity: usize,
) -> Result<(MidiOutputSender, MidiOutputWorker), MidiDeviceError> {
    if ordinary_capacity == 0 || emergency_capacity == 0 {
        return Err(MidiDeviceError::ZeroCapacity);
    }
    let (ordinary_tx, mut ordinary_rx) = RingBuffer::<MidiOutputPacket>::new(ordinary_capacity);
    let (emergency_tx, mut emergency_rx) = RingBuffer::<MidiOutputPacket>::new(emergency_capacity);
    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(MidiOutputStatus::default());
    let worker_stop = Arc::clone(&stop);
    let worker_status = Arc::clone(&status);
    let join = thread::Builder::new()
        .name("citrus-midi-output".into())
        .spawn(move || {
            loop {
                if worker_stop.load(Ordering::Acquire) {
                    // Close admission, wait for producers that observed Running, then drain every
                    // packet they admitted. This is the disconnect/save barrier for this mailbox.
                    worker_status
                        .state
                        .store(MidiOutputWorkerState::Closing as u8, Ordering::SeqCst);
                    while worker_status.producer_inflight.load(Ordering::SeqCst) != 0 {
                        thread::yield_now();
                    }
                    while let Ok(packet) = emergency_rx.pop() {
                        if sink.send(packet.bytes()).is_err() {
                            worker_status.panic_required.store(true, Ordering::Release);
                            worker_status
                                .state
                                .store(MidiOutputWorkerState::SendFailed as u8, Ordering::Release);
                            return;
                        }
                    }
                    while let Ok(packet) = ordinary_rx.pop() {
                        if sink.send(packet.bytes()).is_err() {
                            worker_status.panic_required.store(true, Ordering::Release);
                            worker_status
                                .state
                                .store(MidiOutputWorkerState::SendFailed as u8, Ordering::Release);
                            return;
                        }
                    }
                    worker_status
                        .state
                        .store(MidiOutputWorkerState::Stopped as u8, Ordering::Release);
                    return;
                }
                let mut did_work = false;
                while let Ok(packet) = emergency_rx.pop() {
                    did_work = true;
                    if sink.send(packet.bytes()).is_err() {
                        worker_status.panic_required.store(true, Ordering::Release);
                        worker_status
                            .state
                            .store(MidiOutputWorkerState::SendFailed as u8, Ordering::Release);
                        return;
                    }
                }
                if let Ok(packet) = ordinary_rx.pop() {
                    did_work = true;
                    if sink.send(packet.bytes()).is_err() {
                        worker_status.panic_required.store(true, Ordering::Release);
                        worker_status
                            .state
                            .store(MidiOutputWorkerState::SendFailed as u8, Ordering::Release);
                        return;
                    }
                }
                if !did_work {
                    thread::park_timeout(OUTPUT_WORKER_IDLE);
                }
            }
        })
        .map_err(|error| MidiDeviceError::Backend(error.to_string()))?;
    let worker_thread = join.thread().clone();
    Ok((
        MidiOutputSender {
            ordinary: ordinary_tx,
            emergency: emergency_tx,
            worker_thread,
            status,
        },
        MidiOutputWorker {
            stop,
            join: Some(join),
        },
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MidiInputConnectionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MidiOutputConnectionId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MidiDeviceError {
    #[error("MIDI devices are unsupported on this platform")]
    UnsupportedPlatform,
    #[error("MIDI mailbox capacity must be non-zero")]
    ZeroCapacity,
    #[error("MIDI port was not found: {0:?}")]
    PortNotFound(MidiPortKey),
    #[error("MIDI backend error: {0}")]
    Backend(String),
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug)]
struct NonZeroCounter(u64);

#[cfg(any(windows, test))]
impl Default for NonZeroCounter {
    fn default() -> Self {
        Self(1)
    }
}

#[cfg(any(windows, test))]
impl NonZeroCounter {
    fn take(&mut self) -> u64 {
        let value = self.0;
        self.0 = self.0.wrapping_add(1).max(1);
        value
    }
}

#[cfg(windows)]
mod windows_backend {
    use super::*;
    use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
    use std::collections::BTreeMap;

    struct MidirOutputSink(MidiOutputConnection);

    impl MidiOutputSink for MidirOutputSink {
        fn send(&mut self, message: &[u8]) -> Result<(), ()> {
            self.0.send(message).map_err(|_| ())
        }
    }

    struct ActiveInput {
        _port_key: MidiPortKey,
        _connection: MidiInputConnection<MidiInputCallbackWriter>,
    }

    struct ActiveOutput {
        _port_key: MidiPortKey,
        _worker: MidiOutputWorker,
    }

    /// Windows control-thread owner. midir 0.11 uses WinMM because the optional `winrt` feature
    /// is deliberately disabled in Cargo.toml.
    pub struct WindowsMidiDeviceManager {
        catalog: MidiPortCatalog,
        inputs: BTreeMap<MidiInputConnectionId, ActiveInput>,
        outputs: BTreeMap<MidiOutputConnectionId, ActiveOutput>,
        next_connection_id: NonZeroCounter,
        next_epoch: NonZeroCounter,
    }

    impl Default for WindowsMidiDeviceManager {
        fn default() -> Self {
            Self::new()
        }
    }

    impl WindowsMidiDeviceManager {
        pub fn new() -> Self {
            Self {
                catalog: MidiPortCatalog::default(),
                inputs: BTreeMap::new(),
                outputs: BTreeMap::new(),
                next_connection_id: NonZeroCounter::default(),
                next_epoch: NonZeroCounter::default(),
            }
        }

        pub fn ports(&self) -> &[MidiPortInfo] {
            self.catalog.ports()
        }

        /// Enumerates a complete fresh snapshot. Active connection ownership is deliberately
        /// unchanged: callers first detach any audio receiver and wait for its callback receipt,
        /// then explicitly disconnect the corresponding OS connection.
        pub fn refresh_ports(&mut self) -> Result<MidiPortRefresh, MidiDeviceError> {
            let discovered = enumerate_ports()?;
            Ok(self.catalog.refresh(discovered))
        }

        pub fn connect_input(
            &mut self,
            stable_id: &str,
            capacity: usize,
        ) -> Result<(MidiInputConnectionId, MidiInputReceiver), MidiDeviceError> {
            let mut midi_in = MidiInput::new("Citrus Studio MIDI input")
                .map_err(|error| MidiDeviceError::Backend(error.to_string()))?;
            midi_in.ignore(Ignore::All);
            let port = midi_in.find_port_by_id(stable_id).ok_or_else(|| {
                MidiDeviceError::PortNotFound(MidiPortKey {
                    direction: MidiPortDirection::Input,
                    stable_id: stable_id.to_owned(),
                })
            })?;
            let port_key = MidiPortKey {
                direction: MidiPortDirection::Input,
                stable_id: port.id(),
            };
            let epoch = self.next_epoch.take();
            let (callback, receiver) = input_mailbox(capacity, epoch)?;
            let connection = midi_in
                .connect(
                    &port,
                    "Citrus Studio MIDI input",
                    |timestamp_us, message, callback| callback.receive(timestamp_us, message),
                    callback,
                )
                .map_err(|error| MidiDeviceError::Backend(error.to_string()))?;
            let id = MidiInputConnectionId(self.next_connection_id.take());
            self.inputs.insert(
                id,
                ActiveInput {
                    _port_key: port_key,
                    _connection: connection,
                },
            );
            Ok((id, receiver))
        }

        pub fn disconnect_input(&mut self, id: MidiInputConnectionId) -> bool {
            self.inputs.remove(&id).is_some()
        }

        pub fn connect_output(
            &mut self,
            stable_id: &str,
            ordinary_capacity: usize,
            emergency_capacity: usize,
        ) -> Result<(MidiOutputConnectionId, MidiOutputSender), MidiDeviceError> {
            let midi_out = MidiOutput::new("Citrus Studio MIDI output")
                .map_err(|error| MidiDeviceError::Backend(error.to_string()))?;
            let port = midi_out.find_port_by_id(stable_id).ok_or_else(|| {
                MidiDeviceError::PortNotFound(MidiPortKey {
                    direction: MidiPortDirection::Output,
                    stable_id: stable_id.to_owned(),
                })
            })?;
            let port_key = MidiPortKey {
                direction: MidiPortDirection::Output,
                stable_id: port.id(),
            };
            let connection = midi_out
                .connect(&port, "Citrus Studio MIDI output")
                .map_err(|error| MidiDeviceError::Backend(error.to_string()))?;
            let (sender, worker) = start_output_worker(
                MidirOutputSink(connection),
                ordinary_capacity,
                emergency_capacity,
            )?;
            let id = MidiOutputConnectionId(self.next_connection_id.take());
            self.outputs.insert(
                id,
                ActiveOutput {
                    _port_key: port_key,
                    _worker: worker,
                },
            );
            Ok((id, sender))
        }

        pub fn disconnect_output(&mut self, id: MidiOutputConnectionId) -> bool {
            self.outputs.remove(&id).is_some()
        }
    }

    fn enumerate_ports() -> Result<Vec<MidiPortInfo>, MidiDeviceError> {
        let midi_in = MidiInput::new("Citrus Studio MIDI input enumeration")
            .map_err(|error| MidiDeviceError::Backend(error.to_string()))?;
        let midi_out = MidiOutput::new("Citrus Studio MIDI output enumeration")
            .map_err(|error| MidiDeviceError::Backend(error.to_string()))?;
        let mut ports = Vec::new();
        for port in midi_in.ports() {
            let display_name = midi_in
                .port_name(&port)
                .unwrap_or_else(|_| "Unavailable MIDI input".to_owned());
            ports.push(MidiPortInfo {
                key: MidiPortKey {
                    direction: MidiPortDirection::Input,
                    stable_id: port.id(),
                },
                display_name,
            });
        }
        for port in midi_out.ports() {
            let display_name = midi_out
                .port_name(&port)
                .unwrap_or_else(|_| "Unavailable MIDI output".to_owned());
            ports.push(MidiPortInfo {
                key: MidiPortKey {
                    direction: MidiPortDirection::Output,
                    stable_id: port.id(),
                },
                display_name,
            });
        }
        Ok(ports)
    }
}

#[cfg(windows)]
pub use windows_backend::WindowsMidiDeviceManager;

#[cfg(not(windows))]
pub struct WindowsMidiDeviceManager;

#[cfg(not(windows))]
impl WindowsMidiDeviceManager {
    pub fn new() -> Self {
        Self
    }

    pub fn ports(&self) -> &[MidiPortInfo] {
        &[]
    }

    pub fn refresh_ports(&mut self) -> Result<MidiPortRefresh, MidiDeviceError> {
        Err(MidiDeviceError::UnsupportedPlatform)
    }

    pub fn connect_input(
        &mut self,
        _stable_id: &str,
        _capacity: usize,
    ) -> Result<(MidiInputConnectionId, MidiInputReceiver), MidiDeviceError> {
        Err(MidiDeviceError::UnsupportedPlatform)
    }

    pub fn disconnect_input(&mut self, _id: MidiInputConnectionId) -> bool {
        false
    }

    pub fn connect_output(
        &mut self,
        _stable_id: &str,
        _ordinary_capacity: usize,
        _emergency_capacity: usize,
    ) -> Result<(MidiOutputConnectionId, MidiOutputSender), MidiDeviceError> {
        Err(MidiDeviceError::UnsupportedPlatform)
    }

    pub fn disconnect_output(&mut self, _id: MidiOutputConnectionId) -> bool {
        false
    }
}

#[cfg(not(windows))]
impl Default for WindowsMidiDeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn port(direction: MidiPortDirection, id: &str, name: &str) -> MidiPortInfo {
        MidiPortInfo {
            key: MidiPortKey {
                direction,
                stable_id: id.to_owned(),
            },
            display_name: name.to_owned(),
        }
    }

    #[test]
    fn catalog_hotplug_diff_uses_direction_and_opaque_stable_id() {
        let mut catalog = MidiPortCatalog::default();
        let first = catalog.refresh(vec![
            port(MidiPortDirection::Output, "same", "Out"),
            port(MidiPortDirection::Input, "same", "In"),
        ]);
        assert_eq!(first.added.len(), 2);
        let second = catalog.refresh(vec![
            port(MidiPortDirection::Input, "same", "Renamed"),
            port(MidiPortDirection::Input, "new", "New"),
        ]);
        assert_eq!(second.added.len(), 1);
        assert_eq!(second.removed.len(), 1);
        assert_eq!(second.ports[1].display_name, "Renamed");
    }

    #[test]
    fn input_callback_filters_system_and_stamps_epoch_and_sequence() {
        let (mut callback, mut receiver) = input_mailbox(4, 91).unwrap();
        callback.receive(10, &[0xf8]);
        callback.receive(11, &[0x90, 60, 100]);
        callback.receive(12, &[0x90, 60, 0]);
        let first = receiver.try_pop().unwrap();
        let second = receiver.try_pop().unwrap();
        assert_eq!((first.connection_epoch, first.sequence), (91, 1));
        assert_eq!((second.sequence, second.data), (2, [0x80, 60, 0]));
        assert_eq!(receiver.overload().rejected_messages(), 1);
    }

    #[test]
    fn input_overflow_drops_controls_but_requires_panic_for_note_edges() {
        let (mut callback, receiver) = input_mailbox(1, 1).unwrap();
        callback.receive(1, &[0x90, 60, 100]);
        callback.receive(2, &[0xb0, 1, 2]);
        assert_eq!(receiver.overload().dropped_noncritical(), 1);
        callback.receive(3, &[0x80, 60, 0]);
        assert!(receiver.overload().take_panic_required());
        assert!(!receiver.overload().panic_required());
    }

    struct MockSink(Arc<Mutex<Vec<[u8; 3]>>>);

    impl MidiOutputSink for MockSink {
        fn send(&mut self, message: &[u8]) -> Result<(), ()> {
            let mut bytes = [0; 3];
            bytes[..message.len()].copy_from_slice(message);
            self.0.lock().unwrap().push(bytes);
            Ok(())
        }
    }

    #[test]
    fn emergency_lane_accepts_while_the_ordinary_lane_is_full() {
        let (mut ordinary, _ordinary_rx) = RingBuffer::<MidiOutputPacket>::new(1);
        let (emergency, mut emergency_rx) = RingBuffer::<MidiOutputPacket>::new(1);
        ordinary
            .push(MidiOutputPacket::from_message(1, &[0x90, 60, 100]).unwrap())
            .unwrap();
        let status = Arc::new(MidiOutputStatus::default());
        let mut sender = MidiOutputSender {
            ordinary,
            emergency,
            worker_thread: thread::current(),
            status,
        };
        sender
            .try_send(MidiOutputPacket::from_message(2, &[0x80, 60, 0]).unwrap())
            .unwrap();
        assert_eq!(emergency_rx.pop().unwrap().data, [0x80, 60, 0]);
    }

    #[test]
    fn output_worker_owns_and_drains_its_sink() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let (mut sender, mut worker) =
            start_output_worker(MockSink(Arc::clone(&sent)), 1, 2).unwrap();
        sender
            .try_send(MidiOutputPacket::from_message(1, &[0x90, 60, 100]).unwrap())
            .unwrap();
        sender
            .try_send(MidiOutputPacket::from_message(2, &[0x80, 60, 0]).unwrap())
            .unwrap();
        for _ in 0..100 {
            if sent.lock().unwrap().len() == 2 {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        worker.stop_and_join();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().any(|message| message[0] & 0xf0 == 0x80));
        assert_eq!(sender.status().state(), MidiOutputWorkerState::Stopped);
    }

    #[test]
    fn any_device_send_failure_sets_panic_even_for_an_ordinary_packet() {
        struct FailingSink;
        impl MidiOutputSink for FailingSink {
            fn send(&mut self, _message: &[u8]) -> Result<(), ()> {
                Err(())
            }
        }

        let (mut sender, mut worker) = start_output_worker(FailingSink, 2, 2).unwrap();
        sender
            .try_send(MidiOutputPacket::from_message(1, &[0x90, 60, 100]).unwrap())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline && sender.status().state() == MidiOutputWorkerState::Running
        {
            thread::yield_now();
        }
        worker.stop_and_join();
        assert_eq!(sender.status().state(), MidiOutputWorkerState::SendFailed);
        assert!(sender.status().take_panic_required());
    }

    #[test]
    fn disconnect_closes_admission_waits_and_drains_the_last_admitted_packet() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let (mut sender, worker) = start_output_worker(MockSink(Arc::clone(&sent)), 2, 2).unwrap();
        let admission = MidiOutputAdmission::begin(&sender.status).unwrap();
        let stopper = thread::spawn(move || {
            let mut worker = worker;
            worker.stop_and_join();
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if sender.status().state() == MidiOutputWorkerState::Closing {
                break;
            }
            thread::yield_now();
        }
        assert_eq!(sender.status().state(), MidiOutputWorkerState::Closing);

        // This push belongs to the admission that observed Running before disconnect started.
        sender
            .ordinary
            .push(MidiOutputPacket::from_message(1, &[0x90, 64, 100]).unwrap())
            .unwrap();
        drop(admission);
        assert_eq!(
            sender.try_send(MidiOutputPacket::from_message(2, &[0x90, 65, 100]).unwrap()),
            Err(MidiOutputPushError::WorkerUnavailable)
        );
        stopper.join().unwrap();
        assert_eq!(sender.status().state(), MidiOutputWorkerState::Stopped);
        assert_eq!(sent.lock().unwrap().as_slice(), &[[0x90, 64, 100]]);
    }

    #[test]
    fn full_emergency_lane_sets_explicit_panic_flag() {
        // Test the producer boundary deterministically without starting the draining worker.
        let (ordinary, _ordinary_rx) = RingBuffer::new(1);
        let (mut emergency, _emergency_rx) = RingBuffer::new(1);
        emergency
            .push(MidiOutputPacket::from_message(1, &[0x80, 1, 0]).unwrap())
            .unwrap();
        let status = Arc::new(MidiOutputStatus::default());
        let mut sender = MidiOutputSender {
            ordinary,
            emergency,
            worker_thread: thread::current(),
            status: Arc::clone(&status),
        };
        assert_eq!(
            sender.try_send(MidiOutputPacket::from_message(2, &[0xb0, 123, 0]).unwrap()),
            Err(MidiOutputPushError::Full)
        );
        assert!(status.take_panic_required());
    }

    #[test]
    fn counters_never_emit_zero_after_wrap() {
        let mut counter = NonZeroCounter(u64::MAX);
        assert_eq!(counter.take(), u64::MAX);
        assert_eq!(counter.take(), 1);
    }
}
