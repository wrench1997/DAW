//! UI/control-plane transaction state for reliable generic plug-in parameter edits.
//!
//! The reducer is deliberately independent of `Project`, the audio engine, and plug-in runtime
//! types. A caller maps its concrete endpoint and receipt types into the small `Copy` protocol
//! below. In particular, this module never mutates the project's parameter base value itself.

use std::collections::{HashMap, HashSet, VecDeque};

pub const MAX_PENDING_PLUGIN_PARAMETER_EDITS: usize = 128;
pub const MAX_PENDING_PLUGIN_PARAMETER_EDITS_PER_ENDPOINT: usize = 16;
const MAX_REMEMBERED_COMPLETIONS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParameterEndpointKind {
    Insert,
    Generator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParameterEndpoint {
    pub kind: ParameterEndpointKind,
    pub id: u64,
}

/// Exact identity of the callback route that owns an edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParameterEditRoute {
    pub project_session: u64,
    pub endpoint: ParameterEndpoint,
    pub instance_id: u64,
    pub slot: usize,
    pub parameter_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParameterEditId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterEditSubmission {
    pub edit_id: ParameterEditId,
    pub route: ParameterEditRoute,
    pub normalized: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParameterEditRequest {
    /// The caller must try to enqueue this exact command, then call `confirm_submitted` only when
    /// that enqueue succeeded. Queue pressure may retry the same draft without changing state.
    Submit(ParameterEditSubmission),
    /// One edit is already pending for this target. The desired value was retained locally and
    /// replaces any older retained value; no second command should be enqueued yet.
    Coalesced {
        in_flight: ParameterEditId,
        latest_desired: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterEditRequestError {
    NonFiniteValue,
    OutOfRangeValue,
    GlobalCapacity,
    EndpointCapacity,
    DraftAlreadyOutstanding,
    EditIdExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterEditConfirmError {
    UnknownDraft,
    DraftMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackRejectReason {
    TimelineOwned,
    StaleProjectSession,
    StaleEndpoint,
    MissingInstance,
    MissingSlot,
    InvalidParameter,
    QueueFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerFailureReason {
    BackendRejected,
    InstanceFaulted,
    MissingSlot,
    InvalidParameter,
    ProtocolFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterEditCancelReason {
    ProjectReplaced,
    EndpointReplaced,
    TopologyFault,
    EditorClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterEditFailureReason {
    CallbackRejected(CallbackRejectReason),
    WorkerFailed(WorkerFailureReason),
    NonFiniteAppliedValue,
    OutOfRangeAppliedValue,
    Cancelled(ParameterEditCancelReason),
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterEditFailure {
    pub submission: ParameterEditSubmission,
    pub reason: ParameterEditFailureReason,
    /// False only when cancellation/fail-all discarded a command that had not been acknowledged
    /// as queued. Drafts never make `has_pending()` true.
    pub was_pending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParameterEditEffect {
    /// The receipt was stale, duplicated, or did not match the exact active route.
    Ignored,
    /// A newer coalesced value should now be submitted. The completed intermediate value must not
    /// be committed to the project base map.
    SubmitNext(ParameterEditSubmission),
    /// The final effective worker value is safe for the caller to commit explicitly.
    CommitBase {
        submission: ParameterEditSubmission,
        effective_normalized: f32,
    },
    Failed(ParameterEditFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackEditReceipt {
    Rejected {
        edit_id: ParameterEditId,
        route: ParameterEditRoute,
        reason: CallbackRejectReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorkerEditReceipt {
    Applied {
        edit_id: ParameterEditId,
        route: ParameterEditRoute,
        effective_normalized: f32,
    },
    Failed {
        edit_id: ParameterEditId,
        route: ParameterEditRoute,
        reason: WorkerFailureReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterEditCancelScope {
    Route(ParameterEditRoute),
    Endpoint {
        project_session: u64,
        endpoint: ParameterEndpoint,
    },
    ProjectSession(u64),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParameterEditIgnoredCounts {
    pub duplicate: u64,
    pub stale_edit: u64,
    pub stale_project_session: u64,
    pub endpoint_mismatch: u64,
    pub route_mismatch: u64,
    pub invalid_receipt: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TargetEditState {
    Draft(ParameterEditSubmission),
    Pending {
        submission: ParameterEditSubmission,
        latest_desired: Option<f32>,
    },
}

impl TargetEditState {
    fn submission(self) -> ParameterEditSubmission {
        match self {
            Self::Draft(submission) | Self::Pending { submission, .. } => submission,
        }
    }

    fn is_pending(self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

/// Single-threaded control/UI reducer. It performs no I/O and owns no project data.
#[derive(Debug)]
pub struct PluginParameterEditState {
    next_edit_id: u64,
    targets: HashMap<ParameterEditRoute, TargetEditState>,
    completed_order: VecDeque<(ParameterEditRoute, ParameterEditId)>,
    completed: HashSet<(ParameterEditRoute, ParameterEditId)>,
    ignored: ParameterEditIgnoredCounts,
}

impl Default for PluginParameterEditState {
    fn default() -> Self {
        Self {
            next_edit_id: 1,
            targets: HashMap::new(),
            completed_order: VecDeque::new(),
            completed: HashSet::new(),
            ignored: ParameterEditIgnoredCounts::default(),
        }
    }
}

impl PluginParameterEditState {
    pub fn request_edit(
        &mut self,
        route: ParameterEditRoute,
        normalized: f32,
    ) -> Result<ParameterEditRequest, ParameterEditRequestError> {
        if !normalized.is_finite() {
            return Err(ParameterEditRequestError::NonFiniteValue);
        }
        if !(0.0..=1.0).contains(&normalized) {
            return Err(ParameterEditRequestError::OutOfRangeValue);
        }

        if let Some(state) = self.targets.get_mut(&route) {
            return match state {
                TargetEditState::Draft(_) => {
                    Err(ParameterEditRequestError::DraftAlreadyOutstanding)
                }
                TargetEditState::Pending {
                    submission,
                    latest_desired,
                } => {
                    *latest_desired = Some(normalized);
                    Ok(ParameterEditRequest::Coalesced {
                        in_flight: submission.edit_id,
                        latest_desired: normalized,
                    })
                }
            };
        }

        self.insert_draft(route, normalized)
            .map(ParameterEditRequest::Submit)
    }

    /// Mark the exact draft as successfully enqueued to the audio command queue. A draft reserves
    /// capacity, but only this transition makes it pending and visible to the save barrier.
    pub fn confirm_submitted(
        &mut self,
        draft: ParameterEditSubmission,
    ) -> Result<(), ParameterEditConfirmError> {
        let Some(state) = self.targets.get_mut(&draft.route) else {
            return Err(ParameterEditConfirmError::UnknownDraft);
        };
        let TargetEditState::Draft(current) = *state else {
            return Err(ParameterEditConfirmError::DraftMismatch);
        };
        if !same_submission(current, draft) {
            return Err(ParameterEditConfirmError::DraftMismatch);
        }
        *state = TargetEditState::Pending {
            submission: draft,
            latest_desired: None,
        };
        Ok(())
    }

    /// Discard an exact draft after a non-retryable enqueue failure. Queue-full callers may simply
    /// keep the draft and retry it later.
    pub fn abandon_draft(&mut self, draft: ParameterEditSubmission) -> bool {
        let exact = matches!(
            self.targets.get(&draft.route),
            Some(TargetEditState::Draft(current)) if same_submission(*current, draft)
        );
        if exact {
            self.targets.remove(&draft.route);
        }
        exact
    }

    pub fn observe_callback(&mut self, receipt: CallbackEditReceipt) -> ParameterEditEffect {
        let CallbackEditReceipt::Rejected {
            edit_id,
            route,
            reason,
        } = receipt;
        self.finish_failure(
            route,
            edit_id,
            ParameterEditFailureReason::CallbackRejected(reason),
        )
    }

    pub fn observe_worker(&mut self, receipt: WorkerEditReceipt) -> ParameterEditEffect {
        match receipt {
            WorkerEditReceipt::Failed {
                edit_id,
                route,
                reason,
            } => self.finish_failure(
                route,
                edit_id,
                ParameterEditFailureReason::WorkerFailed(reason),
            ),
            WorkerEditReceipt::Applied {
                edit_id,
                route,
                effective_normalized,
            } => self.finish_applied(route, edit_id, effective_normalized),
        }
    }

    pub fn has_pending(&self) -> bool {
        self.targets.values().any(|state| state.is_pending())
    }

    pub fn pending_len(&self) -> usize {
        self.targets
            .values()
            .filter(|state| state.is_pending())
            .count()
    }

    pub fn reserved_len(&self) -> usize {
        self.targets.len()
    }

    /// Drafts retained after an audio-command queue-full result, in deterministic retry order.
    ///
    /// The returned list is bounded by [`MAX_PENDING_PLUGIN_PARAMETER_EDITS`]. Callers must only
    /// call [`Self::confirm_submitted`] after the exact draft was accepted by the audio queue.
    pub fn draft_submissions(&self) -> Vec<ParameterEditSubmission> {
        let mut drafts: Vec<_> = self
            .targets
            .values()
            .filter_map(|state| match state {
                TargetEditState::Draft(submission) => Some(*submission),
                TargetEditState::Pending { .. } => None,
            })
            .collect();
        drafts.sort_by_key(|submission| submission.edit_id);
        drafts
    }

    pub fn endpoint_reserved_len(
        &self,
        project_session: u64,
        endpoint: ParameterEndpoint,
    ) -> usize {
        self.targets
            .keys()
            .filter(|route| route.project_session == project_session && route.endpoint == endpoint)
            .count()
    }

    pub fn ignored_counts(&self) -> ParameterEditIgnoredCounts {
        self.ignored
    }

    pub fn cancel_scope(
        &mut self,
        scope: ParameterEditCancelScope,
        reason: ParameterEditCancelReason,
    ) -> Vec<ParameterEditFailure> {
        let routes: Vec<_> = self
            .targets
            .keys()
            .copied()
            .filter(|route| scope.matches(*route))
            .collect();
        self.remove_as_failures(routes, ParameterEditFailureReason::Cancelled(reason))
    }

    /// Explicit timeout/fault escape hatch for the save barrier.
    pub fn fail_all(&mut self) -> Vec<ParameterEditFailure> {
        let routes: Vec<_> = self.targets.keys().copied().collect();
        self.remove_as_failures(routes, ParameterEditFailureReason::TimedOut)
    }

    fn insert_draft(
        &mut self,
        route: ParameterEditRoute,
        normalized: f32,
    ) -> Result<ParameterEditSubmission, ParameterEditRequestError> {
        if self.targets.len() >= MAX_PENDING_PLUGIN_PARAMETER_EDITS {
            return Err(ParameterEditRequestError::GlobalCapacity);
        }
        if self.endpoint_reserved_len(route.project_session, route.endpoint)
            >= MAX_PENDING_PLUGIN_PARAMETER_EDITS_PER_ENDPOINT
        {
            return Err(ParameterEditRequestError::EndpointCapacity);
        }
        let edit_id = ParameterEditId(self.next_edit_id);
        self.next_edit_id = self
            .next_edit_id
            .checked_add(1)
            .ok_or(ParameterEditRequestError::EditIdExhausted)?;
        let submission = ParameterEditSubmission {
            edit_id,
            route,
            normalized,
        };
        self.targets
            .insert(route, TargetEditState::Draft(submission));
        Ok(submission)
    }

    fn finish_applied(
        &mut self,
        route: ParameterEditRoute,
        edit_id: ParameterEditId,
        effective_normalized: f32,
    ) -> ParameterEditEffect {
        let Some(TargetEditState::Pending {
            submission,
            latest_desired,
        }) = self.exact_pending(route, edit_id)
        else {
            return ParameterEditEffect::Ignored;
        };

        self.targets.remove(&route);
        self.remember_completion(route, edit_id);
        if !effective_normalized.is_finite() {
            self.ignored.invalid_receipt = self.ignored.invalid_receipt.saturating_add(1);
            return ParameterEditEffect::Failed(ParameterEditFailure {
                submission,
                reason: ParameterEditFailureReason::NonFiniteAppliedValue,
                was_pending: true,
            });
        }
        if !(0.0..=1.0).contains(&effective_normalized) {
            self.ignored.invalid_receipt = self.ignored.invalid_receipt.saturating_add(1);
            return ParameterEditEffect::Failed(ParameterEditFailure {
                submission,
                reason: ParameterEditFailureReason::OutOfRangeAppliedValue,
                was_pending: true,
            });
        }

        if let Some(latest_desired) = latest_desired {
            // Removing A first guarantees its capacity reservation transfers atomically to N+1.
            let next = self
                .insert_draft(route, latest_desired)
                .expect("a completed target must leave capacity for its coalesced successor");
            ParameterEditEffect::SubmitNext(next)
        } else {
            ParameterEditEffect::CommitBase {
                submission,
                effective_normalized,
            }
        }
    }

    fn finish_failure(
        &mut self,
        route: ParameterEditRoute,
        edit_id: ParameterEditId,
        reason: ParameterEditFailureReason,
    ) -> ParameterEditEffect {
        let Some(TargetEditState::Pending { submission, .. }) = self.exact_pending(route, edit_id)
        else {
            return ParameterEditEffect::Ignored;
        };
        self.targets.remove(&route);
        self.remember_completion(route, edit_id);
        ParameterEditEffect::Failed(ParameterEditFailure {
            submission,
            reason,
            was_pending: true,
        })
    }

    fn exact_pending(
        &mut self,
        route: ParameterEditRoute,
        edit_id: ParameterEditId,
    ) -> Option<TargetEditState> {
        if let Some(state @ TargetEditState::Pending { submission, .. }) =
            self.targets.get(&route).copied()
            && submission.edit_id == edit_id
        {
            return Some(state);
        }
        self.count_ignored(route, edit_id);
        None
    }

    fn count_ignored(&mut self, route: ParameterEditRoute, edit_id: ParameterEditId) {
        if self.completed.contains(&(route, edit_id)) {
            self.ignored.duplicate = self.ignored.duplicate.saturating_add(1);
            return;
        }

        if let Some(active_route) = self.targets.iter().find_map(|(active_route, state)| {
            (state.submission().edit_id == edit_id).then_some(*active_route)
        }) {
            if active_route.project_session != route.project_session {
                self.ignored.stale_project_session =
                    self.ignored.stale_project_session.saturating_add(1);
            } else if active_route.endpoint != route.endpoint {
                self.ignored.endpoint_mismatch = self.ignored.endpoint_mismatch.saturating_add(1);
            } else {
                self.ignored.route_mismatch = self.ignored.route_mismatch.saturating_add(1);
            }
        } else {
            self.ignored.stale_edit = self.ignored.stale_edit.saturating_add(1);
        }
    }

    fn remember_completion(&mut self, route: ParameterEditRoute, edit_id: ParameterEditId) {
        if self.completed.insert((route, edit_id)) {
            self.completed_order.push_back((route, edit_id));
        }
        while self.completed_order.len() > MAX_REMEMBERED_COMPLETIONS {
            if let Some(oldest) = self.completed_order.pop_front() {
                self.completed.remove(&oldest);
            }
        }
    }

    fn remove_as_failures(
        &mut self,
        routes: Vec<ParameterEditRoute>,
        reason: ParameterEditFailureReason,
    ) -> Vec<ParameterEditFailure> {
        let mut failures = Vec::with_capacity(routes.len());
        for route in routes {
            if let Some(state) = self.targets.remove(&route) {
                let submission = state.submission();
                self.remember_completion(route, submission.edit_id);
                failures.push(ParameterEditFailure {
                    submission,
                    reason,
                    was_pending: state.is_pending(),
                });
            }
        }
        failures.sort_by_key(|failure| failure.submission.edit_id);
        failures
    }
}

impl ParameterEditCancelScope {
    fn matches(self, route: ParameterEditRoute) -> bool {
        match self {
            Self::Route(exact) => route == exact,
            Self::Endpoint {
                project_session,
                endpoint,
            } => route.project_session == project_session && route.endpoint == endpoint,
            Self::ProjectSession(project_session) => route.project_session == project_session,
        }
    }
}

fn same_submission(left: ParameterEditSubmission, right: ParameterEditSubmission) -> bool {
    left.edit_id == right.edit_id
        && left.route == right.route
        && left.normalized.to_bits() == right.normalized.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(kind: ParameterEndpointKind, id: u64) -> ParameterEndpoint {
        ParameterEndpoint { kind, id }
    }

    fn route(
        kind: ParameterEndpointKind,
        endpoint_id: u64,
        parameter_id: u32,
    ) -> ParameterEditRoute {
        ParameterEditRoute {
            project_session: 7,
            endpoint: endpoint(kind, endpoint_id),
            instance_id: 90,
            slot: if kind == ParameterEndpointKind::Insert {
                3
            } else {
                0
            },
            parameter_id,
        }
    }

    fn submit(
        state: &mut PluginParameterEditState,
        route: ParameterEditRoute,
        value: f32,
    ) -> ParameterEditSubmission {
        let pending_before = state.pending_len();
        let ParameterEditRequest::Submit(draft) = state.request_edit(route, value).unwrap() else {
            panic!("expected draft")
        };
        assert_eq!(state.pending_len(), pending_before);
        state.confirm_submitted(draft).unwrap();
        assert_eq!(state.pending_len(), pending_before + 1);
        draft
    }

    #[test]
    fn insert_and_generator_routes_apply_normally() {
        let mut state = PluginParameterEditState::default();
        for (kind, endpoint_id) in [
            (ParameterEndpointKind::Insert, 11),
            (ParameterEndpointKind::Generator, 12),
        ] {
            let draft = submit(&mut state, route(kind, endpoint_id, 4), 0.25);
            assert_eq!(
                state.observe_worker(WorkerEditReceipt::Applied {
                    edit_id: draft.edit_id,
                    route: draft.route,
                    effective_normalized: 0.3,
                }),
                ParameterEditEffect::CommitBase {
                    submission: draft,
                    effective_normalized: 0.3,
                }
            );
        }
        assert!(!state.has_pending());
    }

    #[test]
    fn a_b_c_coalesces_to_c_and_never_commits_a() {
        let mut state = PluginParameterEditState::default();
        let route = route(ParameterEndpointKind::Insert, 21, 5);
        let a = submit(&mut state, route, 0.1);
        assert!(matches!(
            state.request_edit(route, 0.2).unwrap(),
            ParameterEditRequest::Coalesced {
                latest_desired: 0.2,
                ..
            }
        ));
        assert!(matches!(
            state.request_edit(route, 0.3).unwrap(),
            ParameterEditRequest::Coalesced {
                latest_desired: 0.3,
                ..
            }
        ));

        let ParameterEditEffect::SubmitNext(c) = state.observe_worker(WorkerEditReceipt::Applied {
            edit_id: a.edit_id,
            route,
            effective_normalized: 0.11,
        }) else {
            panic!("A must yield C instead of a base commit")
        };
        assert_eq!(c.normalized, 0.3);
        assert_eq!(c.edit_id.0, a.edit_id.0 + 1);
        assert!(!state.has_pending());
        state.confirm_submitted(c).unwrap();
        assert!(state.has_pending());
        assert!(matches!(
            state.observe_worker(WorkerEditReceipt::Applied {
                edit_id: c.edit_id,
                route,
                effective_normalized: 0.31,
            }),
            ParameterEditEffect::CommitBase {
                effective_normalized: 0.31,
                ..
            }
        ));
    }

    #[test]
    fn old_n_receipt_cannot_complete_n_plus_one() {
        let mut state = PluginParameterEditState::default();
        let route = route(ParameterEndpointKind::Generator, 33, 8);
        let n = submit(&mut state, route, 0.4);
        state.request_edit(route, 0.5).unwrap();
        let ParameterEditEffect::SubmitNext(next) =
            state.observe_worker(WorkerEditReceipt::Applied {
                edit_id: n.edit_id,
                route,
                effective_normalized: 0.4,
            })
        else {
            panic!("expected successor")
        };
        state.confirm_submitted(next).unwrap();
        assert_eq!(
            state.observe_worker(WorkerEditReceipt::Applied {
                edit_id: n.edit_id,
                route,
                effective_normalized: 0.4,
            }),
            ParameterEditEffect::Ignored
        );
        assert!(state.has_pending());
        assert_eq!(state.ignored_counts().duplicate, 1);
    }

    #[test]
    fn seventeenth_target_on_one_endpoint_is_rejected_before_enqueue() {
        let mut state = PluginParameterEditState::default();
        for parameter_id in 0..16 {
            let request = state
                .request_edit(route(ParameterEndpointKind::Insert, 42, parameter_id), 0.2)
                .unwrap();
            assert!(matches!(request, ParameterEditRequest::Submit(_)));
        }
        assert_eq!(state.reserved_len(), 16);
        assert_eq!(
            state.request_edit(route(ParameterEndpointKind::Insert, 42, 16), 0.2),
            Err(ParameterEditRequestError::EndpointCapacity)
        );
        assert!(!state.has_pending());
    }

    #[test]
    fn global_capacity_is_128_across_endpoints() {
        let mut state = PluginParameterEditState::default();
        for endpoint_id in 0..8 {
            for parameter_id in 0..16 {
                state
                    .request_edit(
                        route(
                            ParameterEndpointKind::Insert,
                            100 + endpoint_id,
                            parameter_id,
                        ),
                        0.1,
                    )
                    .unwrap();
            }
        }
        assert_eq!(
            state.request_edit(route(ParameterEndpointKind::Generator, 999, 0), 0.1),
            Err(ParameterEditRequestError::GlobalCapacity)
        );
    }

    #[test]
    fn stale_duplicate_session_endpoint_and_route_receipts_are_counted() {
        let mut state = PluginParameterEditState::default();
        let route = route(ParameterEndpointKind::Insert, 51, 2);
        let edit = submit(&mut state, route, 0.6);

        let mut stale_session = route;
        stale_session.project_session += 1;
        let mut wrong_endpoint = route;
        wrong_endpoint.endpoint.id += 1;
        let mut wrong_route = route;
        wrong_route.parameter_id += 1;
        for bad_route in [stale_session, wrong_endpoint, wrong_route] {
            assert_eq!(
                state.observe_worker(WorkerEditReceipt::Applied {
                    edit_id: edit.edit_id,
                    route: bad_route,
                    effective_normalized: 0.6,
                }),
                ParameterEditEffect::Ignored
            );
        }
        state.observe_worker(WorkerEditReceipt::Applied {
            edit_id: edit.edit_id,
            route,
            effective_normalized: 0.6,
        });
        assert_eq!(
            state.observe_worker(WorkerEditReceipt::Applied {
                edit_id: edit.edit_id,
                route,
                effective_normalized: 0.6,
            }),
            ParameterEditEffect::Ignored
        );
        let counts = state.ignored_counts();
        assert_eq!(counts.stale_project_session, 1);
        assert_eq!(counts.endpoint_mismatch, 1);
        assert_eq!(counts.route_mismatch, 1);
        assert_eq!(counts.duplicate, 1);
    }

    #[test]
    fn exact_cancel_scope_does_not_touch_replacement_endpoint() {
        let mut state = PluginParameterEditState::default();
        let old = submit(&mut state, route(ParameterEndpointKind::Insert, 60, 1), 0.1);
        let replacement = submit(&mut state, route(ParameterEndpointKind::Insert, 61, 1), 0.2);
        let cancelled = state.cancel_scope(
            ParameterEditCancelScope::Endpoint {
                project_session: 7,
                endpoint: old.route.endpoint,
            },
            ParameterEditCancelReason::EndpointReplaced,
        );
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].submission, old);
        assert!(state.has_pending());
        assert!(matches!(
            state.observe_worker(WorkerEditReceipt::Applied {
                edit_id: replacement.edit_id,
                route: replacement.route,
                effective_normalized: 0.2,
            }),
            ParameterEditEffect::CommitBase { .. }
        ));
    }

    #[test]
    fn retained_draft_is_visible_for_retry_and_save_barrier_reservation() {
        let mut state = PluginParameterEditState::default();
        let ParameterEditRequest::Submit(draft) = state
            .request_edit(route(ParameterEndpointKind::Generator, 70, 1), 0.7)
            .unwrap()
        else {
            panic!("expected draft")
        };
        assert!(!state.has_pending());
        assert_eq!(state.reserved_len(), 1);
        assert_eq!(state.draft_submissions(), vec![draft]);
        state.confirm_submitted(draft).unwrap();
        assert!(state.has_pending());
        assert!(state.draft_submissions().is_empty());
        state.observe_callback(CallbackEditReceipt::Rejected {
            edit_id: draft.edit_id,
            route: draft.route,
            reason: CallbackRejectReason::TimelineOwned,
        });
        assert!(!state.has_pending());
    }

    #[test]
    fn worker_failure_is_explicit_and_drops_coalesced_value() {
        let mut state = PluginParameterEditState::default();
        let route = route(ParameterEndpointKind::Insert, 80, 7);
        let edit = submit(&mut state, route, 0.2);
        state.request_edit(route, 0.9).unwrap();
        assert_eq!(
            state.observe_worker(WorkerEditReceipt::Failed {
                edit_id: edit.edit_id,
                route,
                reason: WorkerFailureReason::BackendRejected,
            }),
            ParameterEditEffect::Failed(ParameterEditFailure {
                submission: edit,
                reason: ParameterEditFailureReason::WorkerFailed(
                    WorkerFailureReason::BackendRejected
                ),
                was_pending: true,
            })
        );
        assert_eq!(state.reserved_len(), 0);
    }

    #[test]
    fn invalid_inputs_and_receipts_never_commit() {
        let mut state = PluginParameterEditState::default();
        let route = route(ParameterEndpointKind::Generator, 90, 3);
        assert_eq!(
            state.request_edit(route, f32::NAN),
            Err(ParameterEditRequestError::NonFiniteValue)
        );
        assert_eq!(
            state.request_edit(route, -0.01),
            Err(ParameterEditRequestError::OutOfRangeValue)
        );
        assert_eq!(
            state.request_edit(route, 1.01),
            Err(ParameterEditRequestError::OutOfRangeValue)
        );
        let edit = submit(&mut state, route, 0.5);
        assert_eq!(
            state.observe_worker(WorkerEditReceipt::Applied {
                edit_id: edit.edit_id,
                route,
                effective_normalized: f32::INFINITY,
            }),
            ParameterEditEffect::Failed(ParameterEditFailure {
                submission: edit,
                reason: ParameterEditFailureReason::NonFiniteAppliedValue,
                was_pending: true,
            })
        );
        assert_eq!(state.ignored_counts().invalid_receipt, 1);
        assert!(!state.has_pending());

        let edit = submit(&mut state, route, 0.5);
        assert_eq!(
            state.observe_worker(WorkerEditReceipt::Applied {
                edit_id: edit.edit_id,
                route,
                effective_normalized: 1.01,
            }),
            ParameterEditEffect::Failed(ParameterEditFailure {
                submission: edit,
                reason: ParameterEditFailureReason::OutOfRangeAppliedValue,
                was_pending: true,
            })
        );
        assert_eq!(state.ignored_counts().invalid_receipt, 2);
        assert!(!state.has_pending());
    }

    #[test]
    fn fail_all_releases_save_barrier_and_reports_drafts_separately() {
        let mut state = PluginParameterEditState::default();
        let pending = submit(
            &mut state,
            route(ParameterEndpointKind::Insert, 101, 1),
            0.1,
        );
        let ParameterEditRequest::Submit(draft) = state
            .request_edit(route(ParameterEndpointKind::Generator, 102, 2), 0.2)
            .unwrap()
        else {
            panic!("expected draft")
        };
        let failures = state.fail_all();
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].submission, pending);
        assert!(failures[0].was_pending);
        assert_eq!(failures[1].submission, draft);
        assert!(!failures[1].was_pending);
        assert!(!state.has_pending());
        assert_eq!(state.reserved_len(), 0);
    }
}
