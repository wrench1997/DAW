//! One process-wide reservation, initialized before a child can be spawned.
//! The slot has process lifetime and no background thread. Its mutex is never
//! held during spawn, pipe I/O, poll, kill, wait, or a caller callback.

use super::{pump, ChildOwner, CleanupReport, CleanupState, ErrorKind};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

enum Slot {
    Empty,
    Reserved(u64),
    Abandoned { id: u64, owner: ChildOwner },
}

struct Registry {
    next: u64,
    slot: Slot,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY
        .get_or_init(|| {
            Mutex::new(Registry {
                next: 0,
                slot: Slot::Empty,
            })
        })
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Only this module constructs permits. Drop releases only a Reserved slot.
/// Callers must retain it alongside a child until proven reap or deposit.
pub(super) struct Permit {
    id: u64,
    armed: bool,
}

pub(super) fn reserve() -> Result<Permit, ErrorKind> {
    let mut registry = registry();
    if !matches!(registry.slot, Slot::Empty) {
        return Err(ErrorKind::Busy);
    }
    let id = registry.next.checked_add(1).ok_or(ErrorKind::Limit)?;
    registry.next = id;
    registry.slot = Slot::Reserved(id);
    Ok(Permit { id, armed: true })
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut registry = registry();
        assert!(
            matches!(registry.slot, Slot::Reserved(id) if id == self.id),
            "cleanup permit must match its unique reservation"
        );
        registry.slot = Slot::Empty;
    }
}

pub(super) fn deposit(mut permit: Permit, owner: ChildOwner) {
    let mut registry = registry();
    assert!(
        matches!(registry.slot, Slot::Reserved(id) if id == permit.id),
        "child transfers only into its pre-reserved cleanup slot"
    );
    registry.slot = Slot::Abandoned {
        id: permit.id,
        owner,
    };
    permit.armed = false;
    // The disarmed permit cannot reacquire the mutex during destruction.
}

struct CleanupLoan {
    owner: Option<ChildOwner>,
    permit: Option<Permit>,
}
impl Drop for CleanupLoan {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            deposit(self.permit.take().expect("cleanup loan reservation"), owner);
        }
    }
}

pub(super) fn cleanup(deadline: Instant) -> CleanupReport {
    let mut report = CleanupReport::pending();
    let mut loan = {
        let mut registry = registry();
        let id = match &registry.slot {
            Slot::Empty => {
                report.state = CleanupState::Empty;
                return report;
            }
            Slot::Reserved(_) => {
                report.state = CleanupState::Busy;
                return report;
            }
            Slot::Abandoned { id, .. } => *id,
        };
        let old = std::mem::replace(&mut registry.slot, Slot::Reserved(id));
        match old {
            Slot::Abandoned { owner, .. } => CleanupLoan {
                owner: Some(owner),
                permit: Some(Permit { id, armed: true }),
            },
            _ => unreachable!("slot matched while exclusively locked"),
        }
    }; // Global guard ends before any pipe close/kill/wait.
    let owner = loan.owner.as_mut().expect("abandoned child");
    owner.stdin.take();
    owner.stdout.take();
    pump::reap(owner, deadline, None, &mut report);
    if matches!(report.state, CleanupState::Reaped(_)) {
        loan.owner.take();
        loan.permit.take();
    }
    // Pending, OS error, or Rust unwind restores the still-owned child. A second
    // cleanup cannot concurrently acquire it while the slot is Reserved.
    report
}

#[cfg(test)]
pub(super) fn abandoned_pid() -> Option<u32> {
    match &registry().slot {
        Slot::Abandoned { owner, .. } => Some(owner.child.id()),
        _ => None,
    }
}
