//! Explicit Windows load-response timing, never protocol output or plugin data.
//!
//! Only the exact opt-in value enables this module. The Windows startup check uses
//! stack storage; disabled command paths do not read a clock, enter TLS, allocate a
//! diagnostic buffer, or touch stderr. Each enabled load is correlated on the
//! command-owning thread through its actual response flush attempt. A successful
//! write/flush proves only the helper's I/O results, not receipt by the parent.

use std::cell::Cell;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

#[cfg(any(windows, test))]
const OPT_IN: &str = "windows-load-response-timing-v1";
static ENABLED: AtomicBool = AtomicBool::new(false);
static NEXT_LOAD: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static ACTIVE: Cell<Option<Trace>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
struct Trace {
    load: u64,
    start: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LoadOutcome {
    PluginInfo,
    Error,
    UnexpectedResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HelperLoadPhase {
    HostBuild,
    HostLoad,
    OutputChannelQuery,
    PluginPublication,
}

impl HelperLoadPhase {
    fn name(self) -> &'static str {
        match self {
            Self::HostBuild => "host_build",
            Self::HostLoad => "host_load",
            Self::OutputChannelQuery => "output_channel_query",
            Self::PluginPublication => "plugin_publication",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhaseOrigin {
    Helper,
    #[cfg(any(windows, test))]
    Sdk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SerializationOutcome {
    Ok {
        json_bytes: usize,
    },
    Error {
        category: &'static str,
        line: usize,
        column: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IoOutcome {
    Ok,
    Error {
        kind: io::ErrorKind,
        os_code: Option<i32>,
    },
}

impl IoOutcome {
    fn from_result<T>(result: &io::Result<T>) -> Self {
        match result {
            Ok(_) => Self::Ok,
            Err(error) => Self::Error {
                kind: error.kind(),
                os_code: error.raw_os_error(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    HandleEntry,
    HandleReturned(LoadOutcome),
    LoadPhase {
        origin: PhaseOrigin,
        phase: &'static str,
        before: bool,
        sdk_result: Option<i32>,
    },
    #[cfg(any(windows, test))]
    PluginChangedBefore,
    #[cfg(any(windows, test))]
    PluginChangedAfter,
    SerializeBefore,
    SerializeReturned(SerializationOutcome),
    WriteBefore {
        expected_bytes: usize,
    },
    WriteReturned {
        outcome: IoOutcome,
        accepted_bytes: usize,
        expected_bytes: usize,
    },
    FlushBefore,
    FlushReturned(IoOutcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Record {
    load: u64,
    elapsed_ns: u128,
    event: Event,
}

impl Trace {
    fn record(
        self,
        event: Event,
        clock: &mut impl FnMut() -> Instant,
        sink: &mut impl FnMut(Record),
    ) {
        let now = clock();
        sink(Record {
            load: self.load,
            elapsed_ns: now.saturating_duration_since(self.start).as_nanos(),
            event,
        });
    }
}

pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub(super) fn initialize_from_environment() {
    let enabled = environment_opt_in();
    ENABLED.store(enabled, Ordering::Relaxed);
    #[cfg(windows)]
    if enabled {
        // Only the exact existing startup opt-in installs the SDK gate. The
        // callback borrows fixed stage names and uses this thread's load clock.
        let _ = vst3_host::install_load_phase_diagnostics(sdk_load_phase);
    }
}

#[cfg(any(windows, test))]
fn exact_opt_in(value: &[u16]) -> bool {
    value.len() == OPT_IN.len()
        && value
            .iter()
            .copied()
            .zip(OPT_IN.bytes())
            .all(|(wide, byte)| wide == u16::from(byte))
}

#[cfg(windows)]
fn environment_opt_in() -> bool {
    const NAME_BYTES: &[u8] = b"CITRUS_VST3_HELPER_LOAD_DIAGNOSTIC\0";
    const fn wide_name<const N: usize>(bytes: &[u8]) -> [u16; N] {
        let mut wide = [0; N];
        let mut i = 0;
        while i < N {
            wide[i] = bytes[i] as u16;
            i += 1;
        }
        wide
    }
    const NAME: [u16; NAME_BYTES.len()] = wide_name(NAME_BYTES);
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetEnvironmentVariableW(name: *const u16, buffer: *mut u16, size: u32) -> u32;
    }
    let mut value = [0_u16; 64];
    // SAFETY: a NUL-terminated constant key and live stack output of the supplied
    // size. Missing or overlong values disable the diagnostic; no environment edit.
    let length =
        unsafe { GetEnvironmentVariableW(NAME.as_ptr(), value.as_mut_ptr(), value.len() as u32) }
            as usize;
    length < value.len() && exact_opt_in(&value[..length])
}

#[cfg(not(windows))]
fn environment_opt_in() -> bool {
    false
}

pub(super) fn begin_load() {
    if !enabled() {
        return;
    }
    let trace = Trace {
        load: NEXT_LOAD.fetch_add(1, Ordering::Relaxed),
        start: Instant::now(),
    };
    ACTIVE.with(|active| active.set(Some(trace)));
    // The entry is the same sample used as this load's monotonic origin.
    emit_stderr(Record {
        load: trace.load,
        elapsed_ns: 0,
        event: Event::HandleEntry,
    });
}

fn current_trace() -> Option<Trace> {
    if enabled() {
        // Diagnostics can also arrive from SDK cleanup. An unavailable TLS
        // scope must not turn that cleanup into a diagnostic panic.
        ACTIVE.try_with(Cell::get).ok().flatten()
    } else {
        None
    }
}

fn mark(event: Event) {
    if let Some(trace) = current_trace() {
        trace.record(event, &mut Instant::now, &mut emit_stderr);
    }
}

pub(super) fn load_returned(outcome: LoadOutcome) {
    mark(Event::HandleReturned(outcome));
}

/// Preserve the original call and returned value, including Err/None. A panic
/// or blocked call has no fabricated returned marker.
pub(super) fn around_load_phase<T>(phase: HelperLoadPhase, invoke: impl FnOnce() -> T) -> T {
    if !enabled() {
        return invoke();
    }
    around_phase(PhaseOrigin::Helper, phase.name(), invoke, &mut mark)
}

fn around_phase<T>(
    origin: PhaseOrigin,
    phase: &'static str,
    invoke: impl FnOnce() -> T,
    sink: &mut impl FnMut(Event),
) -> T {
    sink(Event::LoadPhase {
        origin,
        phase,
        before: true,
        sdk_result: None,
    });
    let returned = invoke();
    sink(Event::LoadPhase {
        origin,
        phase,
        before: false,
        sdk_result: None,
    });
    returned
}

#[cfg(windows)]
fn sdk_load_phase(phase: &'static str, before: bool, sdk_result: Option<i32>) {
    if shutdown::sdk_phase(phase, before, sdk_result) {
        return;
    }
    mark(Event::LoadPhase {
        origin: PhaseOrigin::Sdk,
        phase,
        before,
        sdk_result,
    });
}

/// Separate from ACTIVE: responding to LoadPlugin must not end a later exit
/// trace. Only Windows production callers exist; the core has pure local tests.
#[cfg(any(windows, test))]
pub(super) mod shutdown {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    const MAX_RECORDS: usize = 128;
    #[cfg(windows)]
    static RECORDS: AtomicUsize = AtomicUsize::new(0);
    #[cfg(windows)]
    thread_local! {
        static READER: Slot = const { Slot::new() };
        static MAIN: Slot = const { Slot::new() };
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Role {
        Reader,
        Main,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Cause {
        Shutdown,
        Eof,
        ReadError,
        Disconnected,
        NativeQuit,
        RunReturn,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Phase {
        ReaderShutdownDecoded,
        ReaderShutdownEnqueue,
        ReaderEof,
        ReaderReadError,
        ReaderEofEnqueue,
        MainShutdownConsumed,
        MainEofConsumed,
        MainDisconnected,
        MainNativeQuit,
        LoopExited,
        DispatcherDrop,
        EditorClose,
        PluginSlotClear,
        OleUninitialize,
        WindowsRunReturned,
        SdkInitializedComponentDrop,
        SdkPluginDrop,
        SdkDisconnect,
        SdkControllerTerminate,
        SdkComponentTerminate,
        SdkExitDll,
    }
    impl Phase {
        fn name(self) -> &'static str {
            match self {
                Self::ReaderShutdownDecoded => "reader_shutdown_decoded",
                Self::ReaderShutdownEnqueue => "reader_shutdown_enqueue_completed",
                Self::ReaderEof => "reader_eof_observed",
                Self::ReaderReadError => "reader_read_error_observed",
                Self::ReaderEofEnqueue => "reader_eof_enqueue_completed",
                Self::MainShutdownConsumed => "main_shutdown_consumed",
                Self::MainEofConsumed => "main_eof_consumed",
                Self::MainDisconnected => "main_receiver_disconnected",
                Self::MainNativeQuit => "main_native_quit_consumed",
                Self::LoopExited => "main_loop_exited",
                Self::DispatcherDrop => "dispatcher_drop",
                Self::EditorClose => "editor_close",
                Self::PluginSlotClear => "plugin_slot_clear",
                Self::OleUninitialize => "ole_uninitialize",
                Self::WindowsRunReturned => "windows_run_returned",
                Self::SdkInitializedComponentDrop => "initialized_component_drop_teardown",
                Self::SdkPluginDrop => "plugin_drop_teardown",
                Self::SdkDisconnect => "disconnect_component_controller",
                Self::SdkControllerTerminate => "controller_terminate",
                Self::SdkComponentTerminate => "component_terminate",
                Self::SdkExitDll => "windows_exit_dll",
            }
        }
        fn sdk(name: &str) -> Option<Self> {
            match name {
                "initialized_component_drop_teardown" => Some(Self::SdkInitializedComponentDrop),
                "plugin_drop_teardown" => Some(Self::SdkPluginDrop),
                "disconnect_component_controller" => Some(Self::SdkDisconnect),
                "controller_terminate" => Some(Self::SdkControllerTerminate),
                "component_terminate" => Some(Self::SdkComponentTerminate),
                "windows_exit_dll" => Some(Self::SdkExitDll),
                _ => None,
            }
        }
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Edge {
        Observed,
        Before,
        Returned,
    }
    #[derive(Clone, Copy)]
    struct ShutdownTrace {
        role: Role,
        cause: Cause,
        start: Instant,
    }
    struct Slot(Cell<Option<ShutdownTrace>>);
    impl Slot {
        const fn new() -> Self {
            Self(Cell::new(None))
        }
    }
    #[derive(Clone, Copy)]
    struct ShutdownRecord {
        trace: ShutdownTrace,
        elapsed_ns: u128,
        phase: Phase,
        edge: Edge,
        queued: Option<bool>,
        sdk_result: Option<i32>,
    }

    /// Saturating shared budget: even concurrent callers can reserve no more
    /// than MAX_RECORDS. Reservation precedes clock/formatting/sink access.
    fn reserve(budget: &AtomicUsize) -> bool {
        budget
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < MAX_RECORDS).then_some(used + 1)
            })
            .is_ok()
    }
    struct Event {
        phase: Phase,
        edge: Edge,
        queued: Option<bool>,
        sdk_result: Option<i32>,
    }
    impl Event {
        fn observed(phase: Phase) -> Self {
            Self {
                phase,
                edge: Edge::Observed,
                queued: None,
                sdk_result: None,
            }
        }
    }
    fn record(
        enabled: bool,
        slot: &Slot,
        start: Option<(Role, Cause)>,
        event: Event,
        budget: &AtomicUsize,
        clock: &mut impl FnMut() -> Instant,
        sink: &mut impl FnMut(ShutdownRecord),
    ) {
        if !enabled {
            return;
        }
        let current = slot.0.get();
        if (current.is_none() && start.is_none()) || !reserve(budget) {
            return;
        }
        let now = clock();
        let trace = match current {
            Some(trace) => trace,
            None => {
                let Some((role, cause)) = start else { return };
                let trace = ShutdownTrace {
                    role,
                    cause,
                    start: now,
                };
                slot.0.set(Some(trace));
                trace
            }
        };
        sink(ShutdownRecord {
            trace,
            elapsed_ns: now.saturating_duration_since(trace.start).as_nanos(),
            phase: event.phase,
            edge: event.edge,
            queued: event.queued,
            sdk_result: event.sdk_result,
        });
    }
    fn write_record(stderr: &mut impl Write, record: ShutdownRecord) {
        // Fixed vocabulary and primitive facts only. This prefix is not a Load
        // response, metadata receipt, process exit, or proof of OS child reap.
        let _ = writeln!(
            stderr,
            "[citrus-shutdown-timing-v1] trace={:?} cause={:?} elapsed_ns={} stage={} edge={:?} queued={:?} sdk_result={:?}",
            record.trace.role,
            record.trace.cause,
            record.elapsed_ns,
            record.phase.name(),
            record.edge,
            record.queued,
            record.sdk_result,
        );
    }
    #[cfg(windows)]
    fn emit(record: ShutdownRecord) {
        write_record(&mut io::stderr().lock(), record);
    }
    #[cfg(windows)]
    fn reader(cause: Option<Cause>, event: Event) {
        if !enabled() {
            return;
        }
        let _ = READER.try_with(|slot| {
            record(
                true,
                slot,
                cause.map(|cause| (Role::Reader, cause)),
                event,
                &RECORDS,
                &mut Instant::now,
                &mut emit,
            )
        });
    }
    #[cfg(windows)]
    fn main(cause: Option<Cause>, event: Event) {
        if !enabled() {
            return;
        }
        let _ = MAIN.try_with(|slot| {
            record(
                true,
                slot,
                cause.map(|cause| (Role::Main, cause)),
                event,
                &RECORDS,
                &mut Instant::now,
                &mut emit,
            )
        });
    }
    #[cfg(windows)]
    pub(crate) fn reader_shutdown_decoded() {
        reader(
            Some(Cause::Shutdown),
            Event::observed(Phase::ReaderShutdownDecoded),
        );
    }
    #[cfg(windows)]
    pub(crate) fn reader_shutdown_enqueued(queued: bool) {
        reader(
            None,
            Event {
                queued: Some(queued),
                ..Event::observed(Phase::ReaderShutdownEnqueue)
            },
        );
    }
    #[cfg(windows)]
    pub(crate) fn reader_eof() {
        reader(Some(Cause::Eof), Event::observed(Phase::ReaderEof));
    }
    #[cfg(windows)]
    pub(crate) fn reader_read_error() {
        reader(
            Some(Cause::ReadError),
            Event::observed(Phase::ReaderReadError),
        );
    }
    #[cfg(windows)]
    pub(crate) fn reader_eof_enqueued(queued: bool) {
        reader(
            None,
            Event {
                queued: Some(queued),
                ..Event::observed(Phase::ReaderEofEnqueue)
            },
        );
    }
    #[cfg(windows)]
    pub(crate) fn main_consumed(cause: Cause) {
        let phase = match cause {
            Cause::Shutdown => Phase::MainShutdownConsumed,
            Cause::Eof => Phase::MainEofConsumed,
            Cause::Disconnected => Phase::MainDisconnected,
            Cause::NativeQuit => Phase::MainNativeQuit,
            // Reader-only causes never masquerade as main-loop input.
            Cause::ReadError | Cause::RunReturn => return,
        };
        main(Some(cause), Event::observed(phase));
    }
    #[cfg(windows)]
    pub(crate) fn main_loop_exited() {
        main(None, Event::observed(Phase::LoopExited));
    }
    #[cfg(windows)]
    pub(crate) fn windows_run_returned() {
        main(
            Some(Cause::RunReturn),
            Event::observed(Phase::WindowsRunReturned),
        );
    }
    fn around<T>(invoke: impl FnOnce() -> T, mut marker: impl FnMut(Edge)) -> T {
        marker(Edge::Before);
        let returned = invoke();
        marker(Edge::Returned);
        returned
    }
    #[cfg(windows)]
    pub(crate) fn around_phase<T>(phase: Phase, invoke: impl FnOnce() -> T) -> T {
        if !enabled() {
            return invoke();
        }
        around(invoke, |edge| {
            main(
                None,
                Event {
                    edge,
                    ..Event::observed(phase)
                },
            )
        })
    }
    fn record_sdk(
        slot: &Slot,
        phase: &'static str,
        before: bool,
        sdk_result: Option<i32>,
        budget: &AtomicUsize,
        clock: &mut impl FnMut() -> Instant,
        sink: &mut impl FnMut(ShutdownRecord),
    ) -> bool {
        if slot.0.get().is_none() {
            return false;
        }
        if let Some(phase) = Phase::sdk(phase) {
            record(
                true,
                slot,
                None,
                Event {
                    phase,
                    edge: if before { Edge::Before } else { Edge::Returned },
                    queued: None,
                    sdk_result,
                },
                budget,
                clock,
                sink,
            );
        }
        // A terminal main trace wins even when its budget is exhausted or an
        // SDK stage is outside the closed teardown vocabulary. Never relabel
        // terminal SDK teardown as another active load/reply.
        true
    }
    #[cfg(windows)]
    pub(super) fn sdk_phase(phase: &'static str, before: bool, sdk_result: Option<i32>) -> bool {
        if !enabled() {
            return false;
        }
        MAIN.try_with(|slot| {
            record_sdk(
                slot,
                phase,
                before,
                sdk_result,
                &RECORDS,
                &mut Instant::now,
                &mut emit,
            )
        })
        .unwrap_or(false)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::RefCell;
        use std::time::Duration;

        #[derive(Default)]
        struct ClosedSink {
            writes: usize,
        }
        impl Write for ClosedSink {
            fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
                self.writes += 1;
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }

        #[test]
        fn disabled_shutdown_does_not_read_clock_sink_slot_or_budget() {
            let slot = Slot::new();
            let budget = AtomicUsize::new(0);
            record(
                false,
                &slot,
                Some((Role::Main, Cause::Shutdown)),
                Event::observed(Phase::MainShutdownConsumed),
                &budget,
                &mut || panic!("disabled shutdown clock"),
                &mut |_| panic!("disabled shutdown sink"),
            );
            assert!(slot.0.get().is_none());
            assert_eq!(budget.load(Ordering::Relaxed), 0);
        }

        #[test]
        fn reader_and_main_keep_separate_origins_and_first_causes() {
            let reader = Slot::new();
            let main = Slot::new();
            let budget = AtomicUsize::new(0);
            let start = Instant::now();
            let mut records = Vec::new();
            record(
                true,
                &reader,
                Some((Role::Reader, Cause::Shutdown)),
                Event::observed(Phase::ReaderShutdownDecoded),
                &budget,
                &mut || start,
                &mut |row| records.push(row),
            );
            record(
                true,
                &main,
                Some((Role::Main, Cause::Eof)),
                Event::observed(Phase::MainEofConsumed),
                &budget,
                &mut || start + Duration::from_nanos(7),
                &mut |row| records.push(row),
            );
            record(
                true,
                &reader,
                Some((Role::Reader, Cause::Eof)),
                Event::observed(Phase::ReaderEof),
                &budget,
                &mut || start + Duration::from_nanos(9),
                &mut |row| records.push(row),
            );
            record(
                true,
                &main,
                Some((Role::Main, Cause::RunReturn)),
                Event::observed(Phase::WindowsRunReturned),
                &budget,
                &mut || start + Duration::from_nanos(12),
                &mut |row| records.push(row),
            );
            assert_eq!(records[0].trace.role, Role::Reader);
            assert_eq!(records[1].trace.role, Role::Main);
            assert_eq!(records[2].trace.cause, Cause::Shutdown);
            assert_eq!(records[3].trace.cause, Cause::Eof);
            assert_eq!(records[2].elapsed_ns, 9);
            assert_eq!(records[3].elapsed_ns, 5);

            let terminal = Slot::new();
            for (cause, phase) in [
                (Cause::Shutdown, Phase::MainShutdownConsumed),
                (Cause::Eof, Phase::MainEofConsumed),
                (Cause::RunReturn, Phase::WindowsRunReturned),
            ] {
                record(
                    true,
                    &terminal,
                    Some((Role::Main, cause)),
                    Event::observed(phase),
                    &budget,
                    &mut || start,
                    &mut |row| assert_eq!(row.trace.cause, Cause::Shutdown),
                );
            }
        }

        #[test]
        fn shared_shutdown_record_cap_stops_before_clock_and_sink() {
            let reader = Slot::new();
            let main = Slot::new();
            let budget = AtomicUsize::new(0);
            let mut count = 0;
            let start = Instant::now();
            for index in 0..MAX_RECORDS {
                let (slot, role) = if index % 2 == 0 {
                    (&reader, Role::Reader)
                } else {
                    (&main, Role::Main)
                };
                record(
                    true,
                    slot,
                    Some((role, Cause::Shutdown)),
                    Event::observed(Phase::MainShutdownConsumed),
                    &budget,
                    &mut || start,
                    &mut |_| count += 1,
                );
            }
            record(
                true,
                &main,
                None,
                Event::observed(Phase::LoopExited),
                &budget,
                &mut || panic!("budgeted clock"),
                &mut |_| panic!("budgeted sink"),
            );
            assert_eq!(count, MAX_RECORDS);
            assert_eq!(budget.load(Ordering::Relaxed), MAX_RECORDS);
            assert!(record_sdk(
                &main,
                "windows_exit_dll",
                false,
                None,
                &budget,
                &mut || panic!("budgeted SDK clock"),
                &mut |_| panic!("budgeted SDK sink"),
            ));
        }

        #[test]
        fn sdk_shutdown_markers_use_main_exit_trace_after_load_reply_scope_ended() {
            let main = Slot::new();
            let reader = Slot::new();
            let budget = AtomicUsize::new(0);
            let start = Instant::now();
            assert!(!record_sdk(
                &main,
                "component_terminate",
                false,
                Some(-17),
                &budget,
                &mut || panic!("inactive SDK clock"),
                &mut |_| panic!("inactive SDK sink")
            ));
            record(
                true,
                &reader,
                Some((Role::Reader, Cause::Shutdown)),
                Event::observed(Phase::ReaderShutdownDecoded),
                &budget,
                &mut || start,
                &mut |_| {},
            );
            // Reader activity does not activate main-thread SDK tracing.
            assert!(!record_sdk(
                &main,
                "component_terminate",
                false,
                Some(-17),
                &budget,
                &mut || panic!("reader SDK clock"),
                &mut |_| {}
            ));
            record(
                true,
                &main,
                Some((Role::Main, Cause::Shutdown)),
                Event::observed(Phase::MainShutdownConsumed),
                &budget,
                &mut || start,
                &mut |_| {},
            );
            let mut output = Vec::new();
            assert!(record_sdk(
                &main,
                "component_terminate",
                false,
                Some(-17),
                &budget,
                &mut || start + Duration::from_nanos(3),
                &mut |row| write_record(&mut output, row)
            ));
            let text = std::str::from_utf8(&output).unwrap();
            assert!(text.starts_with(
                "[citrus-shutdown-timing-v1] trace=Main cause=Shutdown elapsed_ns=3 "
            ));
            assert!(text.contains("stage=component_terminate edge=Returned"));
            assert!(text.contains("sdk_result=Some(-17)"));
            assert!(!text.contains("load="));
            assert!(record_sdk(
                &main,
                "component_initialize",
                true,
                None,
                &budget,
                &mut || panic!("non-teardown SDK clock"),
                &mut |_| {}
            ));
        }

        #[test]
        fn closed_shutdown_sink_preserves_original_calls_order_return_and_panic() {
            let operations = RefCell::new(Vec::new());
            let mut diagnostic = ClosedSink::default();
            let slot = Slot::new();
            let budget = AtomicUsize::new(0);
            let start = Instant::now();
            record(
                true,
                &slot,
                Some((Role::Main, Cause::Shutdown)),
                Event::observed(Phase::MainShutdownConsumed),
                &budget,
                &mut || start,
                &mut |_| {},
            );
            let original = Box::new([13_u8, 17]);
            let pointer = original.as_ptr();
            let returned: Result<(), Box<[u8; 2]>> = around(
                || {
                    operations.borrow_mut().push("original");
                    Err(original)
                },
                |edge| {
                    operations.borrow_mut().push(match edge {
                        Edge::Before => "before",
                        Edge::Returned => "returned",
                        Edge::Observed => unreachable!(),
                    });
                    record(
                        true,
                        &slot,
                        None,
                        Event {
                            edge,
                            ..Event::observed(Phase::PluginSlotClear)
                        },
                        &budget,
                        &mut || start,
                        &mut |row| write_record(&mut diagnostic, row),
                    );
                },
            );
            let returned = returned.unwrap_err();
            assert_eq!(returned.as_ptr(), pointer);
            assert_eq!(*operations.borrow(), ["before", "original", "returned"]);
            assert_eq!(diagnostic.writes, 2);
            let mut edges = Vec::new();
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                around(|| std::panic::panic_any(71_u32), |edge| edges.push(edge));
            }));
            assert_eq!(*panic.unwrap_err().downcast::<u32>().unwrap(), 71);
            assert_eq!(edges, [Edge::Before]);
        }

        #[test]
        fn dispatcher_aggregate_unwind_preserves_protocol_then_receiver_and_no_returned_edge() {
            struct DropStep<'a> {
                name: &'static str,
                operations: &'a RefCell<Vec<&'static str>>,
                panics: bool,
            }
            impl Drop for DropStep<'_> {
                fn drop(&mut self) {
                    self.operations.borrow_mut().push(self.name);
                    if self.panics {
                        std::panic::panic_any(83_u32);
                    }
                }
            }
            // Match CommandDispatcher's aggregate field order; never unpack
            // these owners into locals, whose unwind order would be reversed.
            struct Dispatcher<'a> {
                _editor: DropStep<'a>,
                _protocol: DropStep<'a>,
                _receiver: DropStep<'a>,
            }
            for diagnostics in [false, true] {
                for editor_panics in [false, true] {
                    let operations = RefCell::new(Vec::new());
                    let invocations = Cell::new(0);
                    let dispatcher = Dispatcher {
                        _editor: DropStep {
                            name: "editor",
                            operations: &operations,
                            panics: editor_panics,
                        },
                        _protocol: DropStep {
                            name: "protocol",
                            operations: &operations,
                            panics: false,
                        },
                        _receiver: DropStep {
                            name: "receiver",
                            operations: &operations,
                            panics: false,
                        },
                    };
                    let mut edges = Vec::new();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let invoke = || {
                            invocations.set(invocations.get() + 1);
                            drop(dispatcher);
                        };
                        if diagnostics {
                            around(invoke, |edge| edges.push(edge));
                        } else {
                            invoke();
                        }
                    }));
                    assert_eq!(invocations.get(), 1);
                    assert_eq!(*operations.borrow(), ["editor", "protocol", "receiver"]);
                    if editor_panics {
                        assert_eq!(*result.unwrap_err().downcast::<u32>().unwrap(), 83);
                    } else {
                        assert!(result.is_ok());
                    }
                    let expected = match (diagnostics, editor_panics) {
                        (false, _) => Vec::new(),
                        (true, true) => vec![Edge::Before],
                        (true, false) => vec![Edge::Before, Edge::Returned],
                    };
                    assert_eq!(edges, expected);
                }
            }
        }

        #[test]
        fn shutdown_vocabulary_is_closed_and_logs_only_fixed_primitive_facts() {
            let phases = [
                Phase::ReaderShutdownDecoded,
                Phase::ReaderShutdownEnqueue,
                Phase::ReaderEof,
                Phase::ReaderReadError,
                Phase::ReaderEofEnqueue,
                Phase::MainShutdownConsumed,
                Phase::MainEofConsumed,
                Phase::MainDisconnected,
                Phase::MainNativeQuit,
                Phase::LoopExited,
                Phase::DispatcherDrop,
                Phase::EditorClose,
                Phase::PluginSlotClear,
                Phase::OleUninitialize,
                Phase::WindowsRunReturned,
                Phase::SdkInitializedComponentDrop,
                Phase::SdkPluginDrop,
                Phase::SdkDisconnect,
                Phase::SdkControllerTerminate,
                Phase::SdkComponentTerminate,
                Phase::SdkExitDll,
            ];
            let names = phases.map(Phase::name);
            for (index, name) in names.iter().enumerate() {
                assert!(
                    name.bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                );
                assert!(!names[..index].contains(name));
            }
            for unknown in [
                "",
                "component_initialize",
                "C:/private",
                "windows_free_library",
            ] {
                assert!(Phase::sdk(unknown).is_none());
            }
            for cause in [
                Cause::Shutdown,
                Cause::Eof,
                Cause::ReadError,
                Cause::Disconnected,
                Cause::NativeQuit,
                Cause::RunReturn,
            ] {
                let mut output = Vec::new();
                write_record(
                    &mut output,
                    ShutdownRecord {
                        trace: ShutdownTrace {
                            role: Role::Main,
                            cause,
                            start: Instant::now(),
                        },
                        elapsed_ns: 0,
                        phase: Phase::ReaderShutdownEnqueue,
                        edge: Edge::Observed,
                        queued: Some(false),
                        sdk_result: None,
                    },
                );
                assert!(
                    std::str::from_utf8(&output)
                        .unwrap()
                        .contains("queued=Some(false) sdk_result=None")
                );
            }
        }
    }
}

#[cfg(windows)]
pub(super) fn plugin_changed_before() {
    mark(Event::PluginChangedBefore);
}

#[cfg(windows)]
pub(super) fn plugin_changed_after() {
    mark(Event::PluginChangedAfter);
}

/// Keep the production serializer and formatted write, once each, including the
/// original flush attempt after a failed write. Only a currently active load logs.
pub(super) fn respond<W: Write>(
    protocol: &mut W,
    serialize: impl FnOnce() -> serde_json::Result<String>,
) {
    respond_pipeline(
        protocol,
        serialize,
        current_trace(),
        &mut Instant::now,
        &mut emit_stderr,
    );
    if enabled() {
        ACTIVE.with(|active| active.set(None));
    }
}

fn respond_pipeline<W: Write>(
    protocol: &mut W,
    serialize: impl FnOnce() -> serde_json::Result<String>,
    trace: Option<Trace>,
    clock: &mut impl FnMut() -> Instant,
    sink: &mut impl FnMut(Record),
) {
    let Some(trace) = trace else {
        if let Ok(json) = serialize() {
            let _ = writeln!(protocol, "{}", json);
            let _ = protocol.flush();
        }
        return;
    };
    trace.record(Event::SerializeBefore, clock, sink);
    let serialized = serialize();
    let json = match serialized {
        Ok(json) => {
            trace.record(
                Event::SerializeReturned(SerializationOutcome::Ok {
                    json_bytes: json.len(),
                }),
                clock,
                sink,
            );
            json
        }
        Err(error) => {
            let category = match error.classify() {
                serde_json::error::Category::Io => "io",
                serde_json::error::Category::Syntax => "syntax",
                serde_json::error::Category::Data => "data",
                serde_json::error::Category::Eof => "eof",
            };
            trace.record(
                Event::SerializeReturned(SerializationOutcome::Error {
                    category,
                    line: error.line(),
                    column: error.column(),
                }),
                clock,
                sink,
            );
            return;
        }
    };
    let expected_bytes = json.len().saturating_add(1);
    trace.record(Event::WriteBefore { expected_bytes }, clock, sink);
    let mut counted = CountedWrite {
        inner: protocol,
        accepted_bytes: 0,
    };
    let write = writeln!(counted, "{}", json);
    trace.record(
        Event::WriteReturned {
            outcome: IoOutcome::from_result(&write),
            accepted_bytes: counted.accepted_bytes,
            expected_bytes,
        },
        clock,
        sink,
    );
    trace.record(Event::FlushBefore, clock, sink);
    let flush = counted.flush();
    trace.record(
        Event::FlushReturned(IoOutcome::from_result(&flush)),
        clock,
        sink,
    );
}

/// ProtocolChannel uses Write's default write_fmt/write_all; this wrapper does too.
/// Each write and its exact buffer are forwarded once, retaining the original
/// formatted write's partial-write/Interrupted handling. Under Write's contract,
/// Ok(n) is the accepted byte range; Err accepts no bytes from that individual call.
/// Counts describe accepted writes, never a parent acknowledgement or persisted data.
struct CountedWrite<'a, W> {
    inner: &'a mut W,
    accepted_bytes: usize,
}

impl<W: Write> Write for CountedWrite<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self.inner.write(bytes);
        if let Ok(count) = &result {
            self.accepted_bytes = self.accepted_bytes.saturating_add(*count);
        }
        result
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn emit_stderr(record: Record) {
    write_record(&mut io::stderr().lock(), record);
}

fn write_record(stderr: &mut impl Write, record: Record) {
    // No arbitrary Error Display, plugin identity, path, state, parameters or PCM.
    // A closed diagnostic sink must not panic or alter the actual protocol result.
    let _ = write!(
        stderr,
        "[citrus-load-response-timing-v1] load={} elapsed_ns={} ",
        record.load, record.elapsed_ns
    );
    let _ = match record.event {
        Event::HandleEntry => writeln!(stderr, "stage=handle_entry"),
        Event::HandleReturned(outcome) => {
            writeln!(stderr, "stage=handle_returned outcome={outcome:?}")
        }
        Event::LoadPhase {
            origin,
            phase,
            before,
            sdk_result,
        } => writeln!(
            stderr,
            "stage=load_phase origin={origin:?} phase={phase} edge={} sdk_result={sdk_result:?}",
            if before { "before" } else { "returned" },
        ),
        #[cfg(any(windows, test))]
        Event::PluginChangedBefore => writeln!(stderr, "stage=plugin_changed_before"),
        #[cfg(any(windows, test))]
        Event::PluginChangedAfter => writeln!(stderr, "stage=plugin_changed_after"),
        Event::SerializeBefore => writeln!(stderr, "stage=serialize_before"),
        Event::SerializeReturned(outcome) => {
            writeln!(stderr, "stage=serialize_returned outcome={outcome:?}")
        }
        Event::WriteBefore { expected_bytes } => {
            writeln!(stderr, "stage=write_before expected_bytes={expected_bytes}")
        }
        Event::WriteReturned {
            outcome,
            accepted_bytes,
            expected_bytes,
        } => writeln!(
            stderr,
            "stage=write_returned outcome={outcome:?} accepted_byte_range=0..{accepted_bytes} expected_bytes={expected_bytes}"
        ),
        Event::FlushBefore => writeln!(stderr, "stage=flush_before"),
        Event::FlushReturned(outcome) => {
            writeln!(stderr, "stage=flush_returned outcome={outcome:?}")
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn helper_phase_returns_the_original_error_once_without_result_invention() {
        let original = Box::new([3_u8, 5, 7]);
        let pointer = original.as_ptr();
        let mut invocations = 0;
        let mut events = Vec::new();
        let result: Result<(), Box<[u8; 3]>> = around_phase(
            PhaseOrigin::Helper,
            HelperLoadPhase::HostLoad.name(),
            || {
                invocations += 1;
                Err(original)
            },
            &mut |event| events.push(event),
        );
        assert_eq!(invocations, 1);
        let returned_error = result.unwrap_err();
        assert_eq!(returned_error.as_ptr(), pointer);
        assert_eq!(
            events,
            [
                Event::LoadPhase {
                    origin: PhaseOrigin::Helper,
                    phase: "host_load",
                    before: true,
                    sdk_result: None,
                },
                Event::LoadPhase {
                    origin: PhaseOrigin::Helper,
                    phase: "host_load",
                    before: false,
                    sdk_result: None,
                },
            ]
        );
    }

    #[test]
    fn unwinding_load_phase_has_no_fabricated_returned_marker() {
        let mut events = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            around_phase(
                PhaseOrigin::Helper,
                HelperLoadPhase::PluginPublication.name(),
                || std::panic::panic_any(31_u32),
                &mut |event| events.push(event),
            );
        }));
        assert_eq!(*result.unwrap_err().downcast::<u32>().unwrap(), 31);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            Event::LoadPhase {
                origin: PhaseOrigin::Helper,
                phase: "plugin_publication",
                before: true,
                sdk_result: None,
            }
        );
    }

    #[test]
    fn sdk_phases_share_the_actual_load_clock_and_original_signed_hresult() {
        let start = Instant::now();
        let trace = Trace { load: 23, start };
        let mut tick = 0;
        let mut records = Vec::new();
        for (before, sdk_result) in [(true, None), (false, Some(-2_147_467_259))] {
            trace.record(
                Event::LoadPhase {
                    origin: PhaseOrigin::Sdk,
                    phase: "component_initialize",
                    before,
                    sdk_result,
                },
                &mut || {
                    tick += 1;
                    start + Duration::from_nanos(tick)
                },
                &mut |record| records.push(record),
            );
        }
        assert!(records.iter().all(|record| record.load == 23));
        assert_eq!(records[0].elapsed_ns, 1);
        assert_eq!(records[1].elapsed_ns, 2);
        let mut output = Vec::new();
        write_record(&mut output, records[1]);
        assert_eq!(
            std::str::from_utf8(&output).unwrap(),
            "[citrus-load-response-timing-v1] load=23 elapsed_ns=2 stage=load_phase origin=Sdk phase=component_initialize edge=returned sdk_result=Some(-2147467259)\n"
        );
    }

    #[test]
    fn closed_phase_diagnostic_sink_preserves_original_success_and_return_order() {
        let mut output = ProtocolProbe {
            fail_after: Some(0),
            ..Default::default()
        };
        let mut events = Vec::new();
        let start = Instant::now();
        let returned = around_phase(
            PhaseOrigin::Helper,
            HelperLoadPhase::OutputChannelQuery.name(),
            || 64_i32,
            &mut |event| {
                events.push(event);
                write_record(
                    &mut output,
                    Record {
                        load: 9,
                        elapsed_ns: start.elapsed().as_nanos(),
                        event,
                    },
                );
            },
        );
        assert_eq!(returned, 64);
        assert!(output.bytes.is_empty());
        assert_eq!(events.len(), 2);
        assert!(matches!(events[1], Event::LoadPhase { before: false, .. }));
    }

    #[derive(Default)]
    struct ProtocolProbe {
        bytes: Vec<u8>,
        calls: Vec<&'static str>,
        write_limit: Option<usize>,
        fail_after: Option<usize>,
        interrupted_once: bool,
        fail_flush: bool,
    }

    impl Write for ProtocolProbe {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls.push("write");
            if self.interrupted_once {
                self.interrupted_once = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            let remaining = self
                .fail_after
                .map_or(usize::MAX, |limit| limit.saturating_sub(self.bytes.len()));
            if remaining == 0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let count = bytes
                .len()
                .min(remaining)
                .min(self.write_limit.unwrap_or(usize::MAX));
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.calls.push("flush");
            if self.fail_flush {
                Err(io::ErrorKind::Other.into())
            } else {
                Ok(())
            }
        }
    }

    fn run(probe: &mut ProtocolProbe, serialized: serde_json::Result<String>) -> Vec<Record> {
        let start = Instant::now();
        let trace = Trace { load: 7, start };
        let mut ticks = 0;
        let mut records = Vec::new();
        respond_pipeline(
            probe,
            || serialized,
            Some(trace),
            &mut || {
                ticks += 1;
                start + Duration::from_nanos(ticks)
            },
            &mut |record| records.push(record),
        );
        records
    }

    #[test]
    fn opt_in_is_exact_and_bounded() {
        assert!(exact_opt_in(&OPT_IN.encode_utf16().collect::<Vec<_>>()));
        for value in [
            "",
            "1",
            "windows-load-response-timing-v1 ",
            "WINDOWS-LOAD-RESPONSE-TIMING-V1",
            "windows-load-response-timing-v2",
        ] {
            assert!(!exact_opt_in(&value.encode_utf16().collect::<Vec<_>>()));
        }
    }

    #[test]
    fn disabled_path_never_invokes_clock_or_diagnostic_sink() {
        let mut protocol = ProtocolProbe::default();
        let mut serializations = 0;
        respond_pipeline(
            &mut protocol,
            || {
                serializations += 1;
                serde_json::to_string(&"unchanged reply")
            },
            None,
            &mut || panic!("disabled clock"),
            &mut |_| panic!("disabled sink"),
        );
        assert_eq!(serializations, 1);
        assert_eq!(protocol.bytes, b"\"unchanged reply\"\n");
        assert_eq!(protocol.calls.last(), Some(&"flush"));
    }

    #[test]
    fn disabled_serialization_failure_has_no_clock_log_write_or_flush() {
        use serde::ser::Error;
        let mut protocol = ProtocolProbe::default();
        respond_pipeline(
            &mut protocol,
            || Err(serde_json::Error::custom("unused payload")),
            None,
            &mut || panic!("disabled clock"),
            &mut |_| panic!("disabled sink"),
        );
        assert!(protocol.bytes.is_empty());
        assert!(protocol.calls.is_empty());
    }

    #[test]
    fn closed_diagnostic_sink_cannot_stop_a_response_or_report_an_acknowledgement() {
        let start = Instant::now();
        let mut protocol = ProtocolProbe::default();
        let mut diagnostics = ProtocolProbe {
            fail_after: Some(0),
            ..Default::default()
        };
        let mut stages = Vec::new();
        respond_pipeline(
            &mut protocol,
            || Ok("reply".into()),
            Some(Trace { load: 1, start }),
            &mut || start,
            &mut |record| {
                stages.push(record.event);
                write_record(&mut diagnostics, record);
            },
        );
        assert!(diagnostics.bytes.is_empty());
        assert_eq!(protocol.bytes, b"reply\n");
        assert_eq!(protocol.calls.last(), Some(&"flush"));
        assert_eq!(stages.last(), Some(&Event::FlushReturned(IoOutcome::Ok)));
    }

    #[test]
    fn control_markers_and_response_share_one_load_clock_and_order() {
        let start = Instant::now();
        let trace = Trace { load: 19, start };
        let mut tick = 0;
        let mut clock = || {
            tick += 1;
            start + Duration::from_nanos(tick)
        };
        let mut records = Vec::new();
        let mut sink = |record| records.push(record);
        trace.record(Event::HandleEntry, &mut clock, &mut sink);
        trace.record(
            Event::HandleReturned(LoadOutcome::Error),
            &mut clock,
            &mut sink,
        );
        trace.record(Event::PluginChangedBefore, &mut clock, &mut sink);
        trace.record(Event::PluginChangedAfter, &mut clock, &mut sink);
        let mut protocol = ProtocolProbe::default();
        respond_pipeline(
            &mut protocol,
            || Ok("error reply".into()),
            Some(trace),
            &mut clock,
            &mut sink,
        );
        assert_eq!(records.len(), 10);
        assert!(records.iter().all(|record| record.load == 19));
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].elapsed_ns < pair[1].elapsed_ns)
        );
        assert_eq!(records[4].event, Event::SerializeBefore);
        assert_eq!(records[9].event, Event::FlushReturned(IoOutcome::Ok));
        assert_eq!(protocol.bytes, b"error reply\n");
    }

    #[test]
    fn successful_response_records_order_without_changing_wire_bytes_or_writes() {
        let json = serde_json::to_string(&[1, 2, 3]).unwrap();
        let mut reference = ProtocolProbe::default();
        writeln!(&mut reference, "{}", json).unwrap();
        reference.flush().unwrap();
        let mut observed = ProtocolProbe::default();
        let records = run(&mut observed, Ok(json.clone()));
        assert_eq!(observed.bytes, reference.bytes);
        assert_eq!(observed.calls, reference.calls);
        assert_eq!(
            records
                .iter()
                .map(|record| record.event)
                .collect::<Vec<_>>(),
            [
                Event::SerializeBefore,
                Event::SerializeReturned(SerializationOutcome::Ok {
                    json_bytes: json.len()
                }),
                Event::WriteBefore {
                    expected_bytes: json.len() + 1
                },
                Event::WriteReturned {
                    outcome: IoOutcome::Ok,
                    accepted_bytes: json.len() + 1,
                    expected_bytes: json.len() + 1
                },
                Event::FlushBefore,
                Event::FlushReturned(IoOutcome::Ok),
            ]
        );
        assert!(records.iter().all(|record| record.load == 7));
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].elapsed_ns < pair[1].elapsed_ns)
        );
    }

    #[test]
    fn serialization_failure_is_not_a_written_or_flushed_reply() {
        use serde::ser::Error;
        let error = serde_json::Error::custom("sensitive path or state must never be logged");
        let mut protocol = ProtocolProbe::default();
        let records = run(&mut protocol, Err(error));
        assert!(protocol.bytes.is_empty());
        assert!(protocol.calls.is_empty());
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[1].event,
            Event::SerializeReturned(SerializationOutcome::Error {
                category: "data",
                line: 0,
                column: 0
            })
        );
    }

    #[test]
    fn partial_write_error_keeps_actual_range_and_still_attempts_original_flush() {
        let mut protocol = ProtocolProbe {
            fail_after: Some(3),
            ..Default::default()
        };
        let records = run(&mut protocol, Ok("abcdef".into()));
        assert_eq!(protocol.bytes, b"abc");
        assert_eq!(protocol.calls.last(), Some(&"flush"));
        assert_eq!(
            records[3].event,
            Event::WriteReturned {
                outcome: IoOutcome::Error {
                    kind: io::ErrorKind::BrokenPipe,
                    os_code: None
                },
                accepted_bytes: 3,
                expected_bytes: 7,
            }
        );
        assert_eq!(records[5].event, Event::FlushReturned(IoOutcome::Ok));
    }

    #[test]
    fn failed_flush_stays_failed_after_successful_complete_write() {
        let mut protocol = ProtocolProbe {
            fail_flush: true,
            ..Default::default()
        };
        let records = run(&mut protocol, Ok("reply".into()));
        assert_eq!(protocol.bytes, b"reply\n");
        assert_eq!(
            records[3].event,
            Event::WriteReturned {
                outcome: IoOutcome::Ok,
                accepted_bytes: 6,
                expected_bytes: 6
            }
        );
        assert_eq!(
            records[5].event,
            Event::FlushReturned(IoOutcome::Error {
                kind: io::ErrorKind::Other,
                os_code: None
            })
        );
    }

    #[test]
    fn interrupted_and_partial_writes_preserve_existing_write_fmt_behavior() {
        let json = String::from("abcdef");
        let mut reference = ProtocolProbe {
            write_limit: Some(2),
            interrupted_once: true,
            ..Default::default()
        };
        writeln!(&mut reference, "{}", json).unwrap();
        reference.flush().unwrap();
        let mut observed = ProtocolProbe {
            write_limit: Some(2),
            interrupted_once: true,
            ..Default::default()
        };
        let records = run(&mut observed, Ok(json));
        assert_eq!(observed.bytes, reference.bytes);
        assert_eq!(observed.calls, reference.calls);
        assert_eq!(
            records[3].event,
            Event::WriteReturned {
                outcome: IoOutcome::Ok,
                accepted_bytes: 7,
                expected_bytes: 7
            }
        );
    }
}
