//! Retained, owner-thread default-class metadata probes on Linux and Windows.
//!
//! This ordinary process-isolation API loads exactly once with the scanner's
//! original default class and 48 kHz/Q128 configuration. It exposes no editor,
//! state, DSP, generic command, recovery or request replay operations. Actual
//! metadata and actual child/pipe cleanup remain separate acceptance facts.

use crate::fresh_helper::{self, FreshHelperSession, ReplyShape, RequestView};
use crate::helper_lifecycle::unsent;
use std::process::Command;
use std::time::{Duration, Instant};

pub use crate::fresh_helper::{LimitEvidence, LimitSite};
pub use crate::helper_lifecycle::{
    CancelHandle, CleanupObservation, CleanupState, Delivery, Failure, FailureKind, LoadedPlugin,
    RequestContext,
};

/// A post-spawn failure retains its actual child owner for checked cleanup.
#[must_use = "retain the cleanup owner; dropping transfers unresolved ownership to quarantine"]
pub struct SpawnFailure {
    /// Original typed setup/admission failure, before any LoadPlugin request.
    pub failure: Failure,
    /// Present when a real child exists despite failed setup or spawn expiry.
    pub cleanup: Option<Box<Probe>>,
}

/// One default-class metadata attempt and its same-thread cleanup authority.
/// No Clone, Send, Sync or raw-child interface is available. Dropping transfers
/// an unresolved child into the existing process-wide quarantine reservation.
#[must_use = "metadata does not prove child termination; finish checked disposal"]
pub struct Probe {
    session: FreshHelperSession,
    attempted: bool,
}

impl Probe {
    /// Spawn once with owned pipes and inherited stderr. The caller supplies
    /// the trusted helper command and an original absolute spawn deadline.
    /// Sample the separate full exchange deadline only after successful spawn.
    pub fn spawn(command: &mut Command, deadline: Instant) -> Result<Self, SpawnFailure> {
        FreshHelperSession::spawn(command, deadline)
            .map(|session| Self {
                session,
                attempted: false,
            })
            .map_err(|failure| SpawnFailure {
                failure: unsent(failure.cause),
                cleanup: failure.cleanup.map(|session| {
                    Box::new(Self {
                        session: *session,
                        attempted: false,
                    })
                }),
            })
    }

    /// Obtain sticky cancellation-only authority before the single exchange.
    pub fn cancellation(&self) -> CancelHandle {
        CancelHandle(self.session.cancellation())
    }

    /// Read the actual directly owned child ID before cleanup. A later None
    /// means ownership was released; only Reaped proves child and pipe cleanup.
    pub fn helper_pid(&self) -> Option<u32> {
        self.session.helper_pid()
    }

    /// Load only the runtime's default class: None, 48 kHz, block128, tempo120,
    /// meter4/4. One original absolute deadline covers encoding, delivery,
    /// response and validated decode. There is no retry even on helper crash.
    pub fn load_default_class(
        &mut self,
        path: &str,
        deadline: Instant,
    ) -> Result<LoadedPlugin, Failure> {
        if self.attempted {
            return Err(unsent(fresh_helper::ErrorKind::Terminal));
        }
        self.attempted = true;
        let request = RequestView::LoadPlugin {
            path,
            sample_rate: 48_000.0,
            block_size: 128,
            tempo: 120.0,
            time_sig_numerator: 4,
            time_sig_denominator: 4,
            class_id: None,
        };
        let context = crate::helper_lifecycle::RequestContext::from_request(request);
        self.session
            .exchange_view(request, ReplyShape::PluginInfo, deadline)
            .map(LoadedPlugin::from_plugin_info)
            .map_err(|error| {
                let mut failure = Failure::from(error);
                failure.request_context = Some(context);
                failure
            })
    }

    /// Seal load admission, send at most one healthy Shutdown within the
    /// earlier original protocol/cleanup deadline, close owned pipes and reap.
    /// Preserve the scanner's two-second natural-exit preference with 250 ms
    /// reserved for kill/reap, clamped to the original cleanup deadline.
    /// Failed, cancelled or terminal loads keep the transport's 100 ms policy.
    /// Pending/Busy and kill_requested alone never prove resource completion.
    pub fn dispose(
        &mut self,
        protocol_deadline: Instant,
        cleanup_deadline: Instant,
    ) -> CleanupObservation {
        self.session
            .dispose_with_deadlines_and_natural_grace(
                protocol_deadline,
                cleanup_deadline,
                Duration::from_secs(2),
                Duration::from_millis(250),
            )
            .into()
    }

    /// Perform one nonwaiting continuation on this same disposed owner. This
    /// never sends Shutdown, renews grace, sleeps or replays a load request.
    pub fn continue_cleanup(&mut self) -> Result<CleanupObservation, Failure> {
        self.session
            .continue_cleanup()
            .map(Into::into)
            .map_err(unsent)
    }

    /// Observe/reap an abandoned owner, without protocol or a new child. Busy
    /// means another live owner still holds the shared global reservation.
    pub fn cleanup_abandoned(deadline: Instant) -> CleanupObservation {
        FreshHelperSession::cleanup_abandoned(deadline).into()
    }
}

#[cfg(test)]
#[path = "default_class_probe_tests.rs"]
mod tests;
