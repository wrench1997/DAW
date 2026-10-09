//! Prepared operating limits for the asynchronous Q128 plug-in workers.
//!
//! A device buffer request and an observed callback are not promises about future callbacks.
//! Every raw callback must pass the active plan before internal splitting or worker submission.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

pub const PLUGIN_QUANTUM_FRAMES: u32 = 128;
pub const MAX_PLUGIN_CALLBACK_FRAMES: u32 = 2048;
pub const MAX_PLUGIN_SAMPLE_RATE: u32 = 384_000;
pub const PLUGIN_GUARD_MILLISECONDS: u32 = 4;
pub const PLUGIN_QUEUE_CAPACITY: usize = 48;
pub const MAX_PLUGIN_LOOKAHEAD_QUANTA: usize = 28;
pub const PLUGIN_CALLBACK_PROFILES: [u32; 4] = [128, 256, 512, 2048];

/// Value-only control-thread preparation; graph activation binds this to the exact endpoint
/// manifests and latency revisions in its existing preallocated identity table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedPluginTimingPlan {
    pub revision: u64,
    pub sample_rate: u32,
    pub callback_budget_frames: u32,
    pub guard_quanta: u32,
    pub lookahead_quanta: u32,
    pub bridge_latency_frames: u32,
}

impl PreparedPluginTimingPlan {
    pub fn new(revision: u64, sample_rate: u32, budget: u32) -> Result<Self, &'static str> {
        if revision == 0 || sample_rate == 0 || sample_rate > MAX_PLUGIN_SAMPLE_RATE {
            return Err(
                "Plug-in timing requires a nonzero revision and a sample rate of 1–384000 Hz",
            );
        }
        if !PLUGIN_CALLBACK_PROFILES.contains(&budget) {
            return Err("Choose a whole-callback plug-in budget of 128, 256, 512 or 2048 frames");
        }
        let guard_quanta =
            (sample_rate * PLUGIN_GUARD_MILLISECONDS).div_ceil(1000 * PLUGIN_QUANTUM_FRAMES);
        let callback_quanta = budget.div_ceil(PLUGIN_QUANTUM_FRAMES);
        let lookahead_quanta = callback_quanta + guard_quanta;
        // One outstanding lookahead, a complete raw callback burst, and two sequence/reset
        // slots. The input, output and future-output queues are all at least this large.
        if lookahead_quanta as usize > MAX_PLUGIN_LOOKAHEAD_QUANTA
            || (lookahead_quanta + callback_quanta + 2) as usize > PLUGIN_QUEUE_CAPACITY
        {
            return Err("The plug-in timing plan exceeds the preallocated worker queue capacity");
        }
        Ok(Self {
            revision,
            sample_rate,
            callback_budget_frames: budget,
            guard_quanta,
            lookahead_quanta,
            bridge_latency_frames: (lookahead_quanta + 1) * PLUGIN_QUANTUM_FRAMES,
        })
    }

    pub fn conservative(sample_rate: u32) -> Result<Self, &'static str> {
        Self::new(1, sample_rate, MAX_PLUGIN_CALLBACK_FRAMES)
    }

    pub const fn admits_callback(self, frames: usize) -> bool {
        frames > 0 && frames <= self.callback_budget_frames as usize
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum PluginProcessingHealth {
    #[default]
    Priming,
    Running,
    Faulted,
    Recovered,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum PluginProcessingFaultReason {
    #[default]
    CallbackBudgetExceeded = 1,
    UnsupportedCallback,
    DeadlineMiss,
    InputLoss,
    OutputLoss,
    LatencyDrift,
    EventLoss,
    EndpointChanged,
    WorkerFault,
}

impl PluginProcessingFaultReason {
    fn from_raw(raw: u32) -> Self {
        match raw {
            2 => Self::UnsupportedCallback,
            3 => Self::DeadlineMiss,
            4 => Self::InputLoss,
            5 => Self::OutputLoss,
            6 => Self::LatencyDrift,
            7 => Self::EventLoss,
            8 => Self::EndpointChanged,
            9 => Self::WorkerFault,
            _ => Self::CallbackBudgetExceeded,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginProcessingFault {
    pub endpoint_id: u64,
    pub epoch: u64,
    pub expected_sequence: u64,
    pub raw_callback_frames: u32,
    pub callback_budget_frames: u32,
    pub lookahead_quanta: u32,
    pub timing_revision: u64,
    pub reason: PluginProcessingFaultReason,
}

#[derive(Clone, Copy, Debug)]
pub struct PluginProcessingSnapshot {
    pub plan: PreparedPluginTimingPlan,
    pub health: PluginProcessingHealth,
    pub fault: Option<PluginProcessingFault>,
    pub fault_count: u64,
    pub recovered_count: u64,
    pub deadline_misses: u64,
    pub input_losses: u64,
    pub output_losses: u64,
}

/// One callback writer, control-only readers. No best-effort event ring can displace the fault.
#[derive(Debug)]
pub struct PluginProcessingTelemetry {
    sequence: AtomicU64,
    words: [AtomicU64; 17],
    pub requested_revision: AtomicU64,
    pub requested_budget: AtomicU32,
}

impl Default for PluginProcessingTelemetry {
    fn default() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
            requested_revision: AtomicU64::new(1),
            requested_budget: AtomicU32::new(MAX_PLUGIN_CALLBACK_FRAMES),
        }
    }
}

impl PluginProcessingTelemetry {
    pub fn publish(&self, snapshot: PluginProcessingSnapshot) {
        let p = snapshot.plan;
        let f = snapshot.fault;
        let values = [
            p.revision,
            u64::from(p.sample_rate),
            u64::from(p.callback_budget_frames),
            u64::from(p.guard_quanta),
            u64::from(p.lookahead_quanta),
            u64::from(p.bridge_latency_frames),
            snapshot.health as u64,
            snapshot.fault_count,
            snapshot.recovered_count,
            snapshot.deadline_misses,
            snapshot.input_losses,
            snapshot.output_losses,
            f.map_or(0, |f| f.reason as u64),
            f.map_or(0, |f| f.endpoint_id),
            f.map_or(0, |f| f.epoch),
            f.map_or(0, |f| f.expected_sequence),
            f.map_or(0, |f| u64::from(f.raw_callback_frames)),
        ];
        self.sequence.fetch_add(1, Ordering::AcqRel);
        for (word, value) in self.words.iter().zip(values) {
            word.store(value, Ordering::Relaxed);
        }
        self.sequence.fetch_add(1, Ordering::Release);
    }

    pub fn snapshot(&self, sample_rate: u32) -> PluginProcessingSnapshot {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let v = self
                .words
                .each_ref()
                .map(|word| word.load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            if self.sequence.load(Ordering::Relaxed) != before {
                continue;
            }
            let plan = if v[0] == 0 {
                PreparedPluginTimingPlan::conservative(sample_rate)
                    .expect("engine validates the supported plug-in sample rate")
            } else {
                PreparedPluginTimingPlan {
                    revision: v[0],
                    sample_rate: v[1] as u32,
                    callback_budget_frames: v[2] as u32,
                    guard_quanta: v[3] as u32,
                    lookahead_quanta: v[4] as u32,
                    bridge_latency_frames: v[5] as u32,
                }
            };
            return PluginProcessingSnapshot {
                plan,
                health: match v[6] {
                    1 => PluginProcessingHealth::Running,
                    2 => PluginProcessingHealth::Faulted,
                    3 => PluginProcessingHealth::Recovered,
                    _ => PluginProcessingHealth::Priming,
                },
                fault: (v[12] != 0).then_some(PluginProcessingFault {
                    reason: PluginProcessingFaultReason::from_raw(v[12] as u32),
                    endpoint_id: v[13],
                    epoch: v[14],
                    expected_sequence: v[15],
                    raw_callback_frames: v[16] as u32,
                    callback_budget_frames: plan.callback_budget_frames,
                    lookahead_quanta: plan.lookahead_quanta,
                    timing_revision: plan.revision,
                }),
                fault_count: v[7],
                recovered_count: v[8],
                deadline_misses: v[9],
                input_losses: v[10],
                output_losses: v[11],
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_prove_queue_capacity_and_explicit_guard_at_maximum_sample_rate() {
        for rate in [1, 44_100, 48_000, 96_000, 192_000, 384_000] {
            for budget in PLUGIN_CALLBACK_PROFILES {
                let p = PreparedPluginTimingPlan::new(7, rate, budget).unwrap();
                assert!(p.guard_quanta > 0);
                assert!(p.lookahead_quanta >= budget.div_ceil(128) + p.guard_quanta);
                assert_eq!(p.bridge_latency_frames, (p.lookahead_quanta + 1) * 128);
                assert!(p.admits_callback(budget as usize));
                assert!(!p.admits_callback(budget as usize + 1));
            }
        }
        assert!(PreparedPluginTimingPlan::new(1, 384_001, 2048).is_err());
        assert!(PreparedPluginTimingPlan::new(1, 48_000, 2049).is_err());
        assert_eq!(
            PreparedPluginTimingPlan::new(1, 48_000, 128)
                .unwrap()
                .bridge_latency_frames,
            512
        );
        assert_eq!(
            PreparedPluginTimingPlan::conservative(48_000)
                .unwrap()
                .bridge_latency_frames,
            2432
        );
    }
}
