//! Run-owned, test-only panic boundaries. No collector mutex, backend call or
//! event/receipt consumption; an inactive worker has no trace to publish.
use super::{BridgeMetrics, PluginWorkerGuard};
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Terminal {
    ProcessOutput { epoch: u64, sequence: u64 },
    ParameterReceipt(u64),
}

pub(crate) struct Trace {
    started: Instant,
    terminal: Terminal,
    phases: [AtomicU64; 4],
}

#[derive(Debug)]
pub(crate) struct Snapshot {
    pub elapsed_us: u64,
    pub panic_requested_us: Option<u64>,
    pub unwind_started_us: Option<u64>,
    pub catch_returned_us: Option<u64>,
    pub terminal_published_us: Option<u64>,
}

impl Trace {
    pub(crate) fn new(terminal: Terminal) -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            terminal,
            phases: std::array::from_fn(|_| AtomicU64::new(0)),
        })
    }

    fn elapsed_us(&self) -> u64 {
        self.started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX - 1)) as u64
    }

    fn mark(&self, phase: usize) {
        // Zero means unobserved; same-microsecond observations remain present.
        let _ = self.phases[phase].compare_exchange(
            0,
            self.elapsed_us() + 1,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    pub(crate) fn terminal(&self) -> Terminal {
        self.terminal
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        // Observe later phases first. Seeing their Release publication then
        // reading earlier phases must not manufacture a missing predecessor.
        let read = |phase: usize| self.phases[phase].load(Ordering::Acquire).checked_sub(1);
        let terminal_published_us = read(3);
        let catch_returned_us = read(2);
        let unwind_started_us = read(1);
        let panic_requested_us = read(0);
        Snapshot {
            elapsed_us: self.elapsed_us(),
            panic_requested_us,
            unwind_started_us,
            catch_returned_us,
            terminal_published_us,
        }
    }

    pub(super) fn describe(
        &self,
        metrics: &BridgeMetrics,
        guard: &PluginWorkerGuard,
        outstanding_parameter_edits: u32,
    ) -> String {
        format!(
            "panic_terminal={:?}; panic_boundary={:?}; metrics={:?}; outstanding_parameter_edits={}; worker_finished={:?}",
            self.terminal,
            self.snapshot(),
            metrics.snapshot(),
            outstanding_parameter_edits,
            guard
                .join
                .as_ref()
                .map(std::thread::JoinHandle::is_finished),
        )
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Arc<Trace>>> = const { RefCell::new(None) };
}

pub(crate) struct UnwindProbe(Arc<Trace>);

impl Drop for UnwindProbe {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.mark(1);
        }
    }
}

pub(crate) fn panic_requested(trace: &Arc<Trace>) -> UnwindProbe {
    ACTIVE.with(|active| *active.borrow_mut() = Some(Arc::clone(trace)));
    trace.mark(0);
    UnwindProbe(Arc::clone(trace))
}

pub(super) fn catch_returned() {
    ACTIVE.with(|active| {
        if let Some(trace) = active.borrow().as_ref() {
            trace.mark(2);
        }
    });
}

fn terminal_published(terminal: Terminal) {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        if active
            .as_ref()
            .is_some_and(|trace| trace.terminal == terminal)
        {
            let trace = active.take().expect("matching active panic trace");
            trace.mark(3);
        }
    });
}

pub(super) fn process_output_published(epoch: u64, sequence: u64) {
    terminal_published(Terminal::ProcessOutput { epoch, sequence });
}

pub(super) fn parameter_receipt_published(edit_id: u64) {
    terminal_published(Terminal::ParameterReceipt(edit_id));
}

#[test]
fn process_publication_requires_exact_worker_epoch_and_later_sequence() {
    let trace = Trace::new(Terminal::ProcessOutput {
        epoch: 7,
        sequence: 23,
    });
    let probe = panic_requested(&trace);
    // A different worker cannot publish this thread's active trace, even with
    // the same block identity. No process-wide panic hook is installed.
    std::thread::spawn(|| process_output_published(7, 23))
        .join()
        .unwrap();
    assert!(trace.snapshot().terminal_published_us.is_none());
    process_output_published(8, 23);
    process_output_published(7, 22);
    process_output_published(7, 1);
    assert!(trace.snapshot().terminal_published_us.is_none());
    catch_returned();
    process_output_published(7, 23);
    assert!(trace.snapshot().terminal_published_us.is_some());
    assert!(ACTIVE.with(|active| active.borrow().is_none()));
    drop(probe);
}
