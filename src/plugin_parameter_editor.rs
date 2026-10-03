//! UI-independent state for browsing one running plug-in's generic parameter catalog.
//!
//! This module deliberately has no dependency on the project model. Opening a catalog and
//! observing pages only changes this transient session cache; callers must perform any project
//! edit (for example, creating an automation lane) explicitly elsewhere.

use std::collections::HashSet;

use crate::plugins::plugin_runtime::{
    MAX_PLUGIN_PARAMETER_CATALOG_ITEMS, MAX_PLUGIN_PARAMETER_NAME_BYTES,
    MAX_PLUGIN_PARAMETER_PAGE_ITEMS, MAX_PLUGIN_PARAMETER_UNIT_BYTES, PluginParameterCommand,
    PluginParameterDescriptor, RuntimeEvent,
};

/// Identity to which the currently open catalog session is bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginParameterCatalogBinding {
    pub project_session: u64,
    pub endpoint_id: u64,
    pub instance_id: u64,
    pub slot: usize,
    pub request_id: u64,
}

/// Coarse state suitable for rendering without exposing mutable catalog storage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginParameterCatalogPhase {
    #[default]
    Closed,
    Loading,
    Complete,
    Faulted,
}

/// Request-side handshake state. It separates retryable queue pressure from an actual in-flight
/// request, preventing duplicate page requests while the worker is processing one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginParameterCatalogRequestState {
    NeedRequest { cursor: u32 },
    InFlight { cursor: u32 },
}

/// Result of reducing one runtime event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginParameterCatalogUpdate {
    /// The event did not belong to this catalog session (or was unrelated to catalog loading).
    Ignored,
    /// The accepted page was valid and the caller should request this exact next cursor.
    NeedNext { cursor: u32 },
    /// The complete catalog is now available through the read-only accessors.
    Complete,
    /// The matching catalog stream violated its contract and the state is now faulted.
    Faulted { message: String },
}

/// Transient reducer/cache for one plug-in parameter browser session.
#[derive(Clone, Debug, Default)]
pub struct PluginParameterEditorState {
    binding: Option<PluginParameterCatalogBinding>,
    phase: PluginParameterCatalogPhase,
    expected_cursor: u32,
    request_state: Option<PluginParameterCatalogRequestState>,
    catalog_revision: Option<u64>,
    descriptors: Vec<PluginParameterDescriptor>,
    ids: HashSet<u32>,
    fault: Option<String>,
}

impl PluginParameterEditorState {
    /// Start (or replace) a browser session. This only resets transient catalog state.
    ///
    /// Endpoint zero is reserved as "unbound" and is rejected by closing the state and returning
    /// `false`; a valid nonzero endpoint returns `true`.
    pub fn begin(
        &mut self,
        project_session: u64,
        endpoint_id: u64,
        instance_id: u64,
        slot: usize,
        request_id: u64,
    ) -> bool {
        if endpoint_id == 0 {
            self.close();
            return false;
        }
        self.binding = Some(PluginParameterCatalogBinding {
            project_session,
            endpoint_id,
            instance_id,
            slot,
            request_id,
        });
        self.phase = PluginParameterCatalogPhase::Loading;
        self.expected_cursor = 0;
        self.request_state = Some(PluginParameterCatalogRequestState::NeedRequest { cursor: 0 });
        self.catalog_revision = None;
        self.descriptors.clear();
        self.ids.clear();
        self.fault = None;
        true
    }

    /// Close the browser and discard all transient catalog data.
    pub fn close(&mut self) {
        self.binding = None;
        self.phase = PluginParameterCatalogPhase::Closed;
        self.expected_cursor = 0;
        self.request_state = None;
        self.catalog_revision = None;
        self.descriptors.clear();
        self.ids.clear();
        self.fault = None;
    }

    pub fn phase(&self) -> PluginParameterCatalogPhase {
        self.phase
    }

    pub fn binding(&self) -> Option<PluginParameterCatalogBinding> {
        self.binding
    }

    pub fn expected_cursor(&self) -> Option<u32> {
        (self.phase == PluginParameterCatalogPhase::Loading).then_some(self.expected_cursor)
    }

    pub fn request_state(&self) -> Option<PluginParameterCatalogRequestState> {
        self.request_state
    }

    /// Cursor that should be submitted now. `None` means closed, faulted, complete, or already
    /// in flight. A queue-full result must leave this value untouched for a later retry.
    pub fn request_to_send(&self) -> Option<u32> {
        match self.request_state {
            Some(PluginParameterCatalogRequestState::NeedRequest { cursor }) => Some(cursor),
            _ => None,
        }
    }

    /// Acknowledge successful submission to the worker queue.
    ///
    /// Returns `true` only for the exact currently-needed cursor. Queue-full callers simply do
    /// not call this method, leaving the request retryable.
    pub fn mark_request_queued(&mut self, cursor: u32) -> bool {
        if self.phase != PluginParameterCatalogPhase::Loading
            || self.request_state
                != Some(PluginParameterCatalogRequestState::NeedRequest { cursor })
        {
            return false;
        }
        self.request_state = Some(PluginParameterCatalogRequestState::InFlight { cursor });
        true
    }

    pub fn catalog_revision(&self) -> Option<u64> {
        self.catalog_revision
    }

    pub fn fault_message(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// All descriptors accepted so far. The slice intentionally provides no mutable access.
    pub fn descriptors(&self) -> &[PluginParameterDescriptor] {
        &self.descriptors
    }

    /// Query one accepted descriptor by its plug-in parameter ID.
    pub fn descriptor(&self, id: u32) -> Option<&PluginParameterDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.id == id)
    }

    /// Case-insensitive read-only filtering by name, unit, or decimal parameter ID.
    pub fn filtered<'a>(
        &'a self,
        query: &'a str,
    ) -> impl Iterator<Item = &'a PluginParameterDescriptor> + 'a {
        let needle = query.trim().to_lowercase();
        self.descriptors.iter().filter(move |descriptor| {
            needle.is_empty()
                || descriptor.name.to_lowercase().contains(&needle)
                || descriptor.unit.to_lowercase().contains(&needle)
                || descriptor.id.to_string().contains(&needle)
        })
    }

    /// Reduce a worker event with the control-plane identity that routed it.
    ///
    /// Events from an old project session, another instance, slot, or request are ignored before
    /// any phase checks, so late events cannot poison a newly opened catalog.
    pub fn observe_runtime_event(
        &mut self,
        project_session: u64,
        endpoint_id: u64,
        instance_id: u64,
        event: &RuntimeEvent,
    ) -> PluginParameterCatalogUpdate {
        match event {
            RuntimeEvent::ParameterCatalogPage {
                slot,
                request_id,
                cursor,
                next_cursor,
                done,
                catalog_revision,
                items,
            } => {
                if !self.matches(
                    project_session,
                    endpoint_id,
                    instance_id,
                    *slot,
                    *request_id,
                ) {
                    return PluginParameterCatalogUpdate::Ignored;
                }
                self.apply_page(*cursor, *next_cursor, *done, *catalog_revision, items)
            }
            RuntimeEvent::ParameterCommandFailed {
                slot,
                request_id,
                command: PluginParameterCommand::Catalog,
                message,
                ..
            } => {
                if !self.matches(
                    project_session,
                    endpoint_id,
                    instance_id,
                    *slot,
                    *request_id,
                ) {
                    return PluginParameterCatalogUpdate::Ignored;
                }
                self.fail(format!("parameter catalog request failed: {message}"))
            }
            _ => PluginParameterCatalogUpdate::Ignored,
        }
    }

    fn matches(
        &self,
        project_session: u64,
        endpoint_id: u64,
        instance_id: u64,
        slot: usize,
        request_id: u64,
    ) -> bool {
        self.binding.is_some_and(|binding| {
            binding.project_session == project_session
                && binding.endpoint_id == endpoint_id
                && binding.instance_id == instance_id
                && binding.slot == slot
                && binding.request_id == request_id
        })
    }

    fn apply_page(
        &mut self,
        cursor: u32,
        next_cursor: Option<u32>,
        done: bool,
        catalog_revision: u64,
        items: &[PluginParameterDescriptor],
    ) -> PluginParameterCatalogUpdate {
        match self.phase {
            PluginParameterCatalogPhase::Loading => {}
            PluginParameterCatalogPhase::Faulted => {
                return PluginParameterCatalogUpdate::Faulted {
                    message: self
                        .fault
                        .clone()
                        .unwrap_or_else(|| "catalog faulted".into()),
                };
            }
            PluginParameterCatalogPhase::Closed => {
                return PluginParameterCatalogUpdate::Ignored;
            }
            PluginParameterCatalogPhase::Complete => {
                return self.fail("received a parameter catalog page after completion".into());
            }
        }

        if cursor != self.expected_cursor {
            return self.fail(format!(
                "parameter catalog cursor {cursor} is not the expected cursor {}",
                self.expected_cursor
            ));
        }
        if self.request_state != Some(PluginParameterCatalogRequestState::InFlight { cursor }) {
            return self.fail(format!(
                "parameter catalog page at cursor {cursor} arrived without that request in flight"
            ));
        }
        if items.len() > MAX_PLUGIN_PARAMETER_PAGE_ITEMS {
            return self.fail(format!(
                "parameter catalog page contains {} items; the page limit is {MAX_PLUGIN_PARAMETER_PAGE_ITEMS}",
                items.len()
            ));
        }
        if catalog_revision == 0 {
            return self.fail("parameter catalog revision must be nonzero".into());
        }
        if let Some(expected_revision) = self.catalog_revision
            && catalog_revision != expected_revision
        {
            return self.fail(format!(
                "parameter catalog revision changed from {expected_revision} to {catalog_revision} while paging"
            ));
        }

        let new_len = match self.descriptors.len().checked_add(items.len()) {
            Some(new_len) if new_len <= MAX_PLUGIN_PARAMETER_CATALOG_ITEMS => new_len,
            _ => {
                return self.fail(format!(
                    "parameter catalog exceeds the {MAX_PLUGIN_PARAMETER_CATALOG_ITEMS}-item limit"
                ));
            }
        };
        let computed_next = match u32::try_from(new_len) {
            Ok(cursor) => cursor,
            Err(_) => return self.fail("parameter catalog cursor overflow".into()),
        };

        if done {
            if next_cursor.is_some() {
                return self
                    .fail("a completed parameter catalog page must not have a next cursor".into());
            }
        } else {
            if items.is_empty() {
                return self.fail("an incomplete parameter catalog page must make progress".into());
            }
            if next_cursor != Some(computed_next) {
                return self.fail(format!(
                    "parameter catalog next cursor {next_cursor:?} does not equal {computed_next}"
                ));
            }
            if new_len == MAX_PLUGIN_PARAMETER_CATALOG_ITEMS {
                return self.fail(format!(
                    "parameter catalog claims more than {MAX_PLUGIN_PARAMETER_CATALOG_ITEMS} items"
                ));
            }
        }

        let mut page_ids = HashSet::with_capacity(items.len());
        for descriptor in items {
            if let Err(message) = validate_descriptor(descriptor) {
                return self.fail(message);
            }
            if self.ids.contains(&descriptor.id) || !page_ids.insert(descriptor.id) {
                return self.fail(format!(
                    "parameter catalog contains duplicate parameter id {}",
                    descriptor.id
                ));
            }
        }

        // Commit only after validating the whole page, so a fault never exposes a partial page.
        self.catalog_revision.get_or_insert(catalog_revision);
        for descriptor in items {
            self.ids.insert(descriptor.id);
            self.descriptors.push(descriptor.clone());
        }
        self.expected_cursor = computed_next;

        if done {
            self.phase = PluginParameterCatalogPhase::Complete;
            self.request_state = None;
            PluginParameterCatalogUpdate::Complete
        } else {
            self.request_state = Some(PluginParameterCatalogRequestState::NeedRequest {
                cursor: computed_next,
            });
            PluginParameterCatalogUpdate::NeedNext {
                cursor: computed_next,
            }
        }
    }

    fn fail(&mut self, message: String) -> PluginParameterCatalogUpdate {
        self.phase = PluginParameterCatalogPhase::Faulted;
        self.request_state = None;
        self.fault = Some(message.clone());
        PluginParameterCatalogUpdate::Faulted { message }
    }
}

fn validate_descriptor(descriptor: &PluginParameterDescriptor) -> Result<(), String> {
    if descriptor.name.len() > MAX_PLUGIN_PARAMETER_NAME_BYTES {
        return Err(format!(
            "parameter {} name exceeds {MAX_PLUGIN_PARAMETER_NAME_BYTES} UTF-8 bytes",
            descriptor.id
        ));
    }
    if descriptor.unit.len() > MAX_PLUGIN_PARAMETER_UNIT_BYTES {
        return Err(format!(
            "parameter {} unit exceeds {MAX_PLUGIN_PARAMETER_UNIT_BYTES} UTF-8 bytes",
            descriptor.id
        ));
    }
    if !is_normalized(descriptor.current_normalized) {
        return Err(format!(
            "parameter {} current value is not finite and normalized",
            descriptor.id
        ));
    }
    if descriptor
        .default_normalized
        .is_some_and(|value| !is_normalized(value))
    {
        return Err(format!(
            "parameter {} default value is not finite and normalized",
            descriptor.id
        ));
    }
    Ok(())
}

fn is_normalized(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: u64 = 11;
    const ENDPOINT: u64 = 12;
    const INSTANCE: u64 = 22;
    const SLOT: usize = 3;
    const REQUEST: u64 = 44;

    fn descriptor(id: u32) -> PluginParameterDescriptor {
        PluginParameterDescriptor {
            id,
            name: format!("Parameter {id}"),
            unit: "%".into(),
            current_normalized: 0.5,
            default_normalized: Some(0.25),
            step_count: None,
            automatable: true,
            read_only: false,
            bypass: false,
        }
    }

    fn page(
        cursor: u32,
        next_cursor: Option<u32>,
        done: bool,
        revision: u64,
        items: Vec<PluginParameterDescriptor>,
    ) -> RuntimeEvent {
        RuntimeEvent::ParameterCatalogPage {
            slot: SLOT,
            request_id: REQUEST,
            cursor,
            next_cursor,
            done,
            catalog_revision: revision,
            items,
        }
    }

    fn open() -> PluginParameterEditorState {
        let mut state = PluginParameterEditorState::default();
        assert!(state.begin(SESSION, ENDPOINT, INSTANCE, SLOT, REQUEST));
        state
    }

    fn queue_needed(state: &mut PluginParameterEditorState) {
        let cursor = state.request_to_send().expect("a page request is needed");
        assert!(state.mark_request_queued(cursor));
        assert_eq!(state.request_to_send(), None);
    }

    #[test]
    fn accepts_two_exact_pages_and_exposes_read_only_queries() {
        let mut state = open();
        let first = page(0, Some(2), false, 7, vec![descriptor(10), descriptor(20)]);
        queue_needed(&mut state);
        assert_eq!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &first),
            PluginParameterCatalogUpdate::NeedNext { cursor: 2 }
        );

        let second = page(2, None, true, 7, vec![descriptor(30)]);
        queue_needed(&mut state);
        assert_eq!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &second),
            PluginParameterCatalogUpdate::Complete
        );
        assert_eq!(state.phase(), PluginParameterCatalogPhase::Complete);
        assert_eq!(state.catalog_revision(), Some(7));
        assert_eq!(state.descriptors().len(), 3);
        assert_eq!(state.descriptor(20).map(|item| item.id), Some(20));
        assert_eq!(
            state
                .filtered("parameter 3")
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![30]
        );
    }

    #[test]
    fn ignores_stale_session_instance_request_and_slot() {
        let event = page(0, None, true, 1, vec![descriptor(1)]);
        for (session, instance) in [(SESSION + 1, INSTANCE), (SESSION, INSTANCE + 1)] {
            let mut state = open();
            assert_eq!(
                state.observe_runtime_event(session, ENDPOINT, instance, &event),
                PluginParameterCatalogUpdate::Ignored
            );
            assert!(state.is_empty());
        }

        let mut wrong_request = event.clone();
        if let RuntimeEvent::ParameterCatalogPage { request_id, .. } = &mut wrong_request {
            *request_id += 1;
        }
        let mut state = open();
        assert_eq!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &wrong_request),
            PluginParameterCatalogUpdate::Ignored
        );

        let mut wrong_slot = event;
        if let RuntimeEvent::ParameterCatalogPage { slot, .. } = &mut wrong_slot {
            *slot += 1;
        }
        assert_eq!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &wrong_slot),
            PluginParameterCatalogUpdate::Ignored
        );
    }

    #[test]
    fn ignores_old_endpoint_even_when_every_other_identity_matches() {
        let mut state = open();
        queue_needed(&mut state);
        let event = page(0, None, true, 1, vec![descriptor(1)]);

        assert_eq!(
            state.observe_runtime_event(SESSION, ENDPOINT + 1, INSTANCE, &event),
            PluginParameterCatalogUpdate::Ignored
        );
        assert!(state.is_empty());
        assert_eq!(
            state.request_state(),
            Some(PluginParameterCatalogRequestState::InFlight { cursor: 0 })
        );

        assert_eq!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &event),
            PluginParameterCatalogUpdate::Complete
        );
    }

    #[test]
    fn rejects_reserved_zero_endpoint() {
        let mut state = open();
        assert!(!state.begin(SESSION, 0, INSTANCE, SLOT, REQUEST));
        assert_eq!(state.phase(), PluginParameterCatalogPhase::Closed);
        assert_eq!(state.binding(), None);
        assert_eq!(state.request_to_send(), None);
    }

    #[test]
    fn faults_on_cursor_gap() {
        let mut state = open();
        queue_needed(&mut state);
        let event = page(1, None, true, 1, vec![descriptor(1)]);
        assert!(matches!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &event),
            PluginParameterCatalogUpdate::Faulted { .. }
        ));
    }

    #[test]
    fn faults_when_revision_drifts() {
        let mut state = open();
        let first = page(0, Some(1), false, 1, vec![descriptor(1)]);
        queue_needed(&mut state);
        state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &first);
        let second = page(1, None, true, 2, vec![descriptor(2)]);
        queue_needed(&mut state);
        assert!(matches!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &second),
            PluginParameterCatalogUpdate::Faulted { .. }
        ));
    }

    #[test]
    fn faults_on_duplicate_id_across_pages_without_committing_bad_page() {
        let mut state = open();
        let first = page(0, Some(1), false, 1, vec![descriptor(9)]);
        queue_needed(&mut state);
        state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &first);
        let second = page(1, None, true, 1, vec![descriptor(9)]);
        queue_needed(&mut state);
        assert!(matches!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &second),
            PluginParameterCatalogUpdate::Faulted { .. }
        ));
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn faults_on_duplicate_id_within_one_page() {
        let mut state = open();
        queue_needed(&mut state);
        let event = page(0, None, true, 1, vec![descriptor(9), descriptor(9)]);
        assert!(matches!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &event),
            PluginParameterCatalogUpdate::Faulted { .. }
        ));
        assert!(state.is_empty());
    }

    #[test]
    fn faults_when_catalog_would_exceed_global_limit() {
        let mut state = open();
        for page_index in 0..(MAX_PLUGIN_PARAMETER_CATALOG_ITEMS / 64) {
            let cursor = (page_index * 64) as u32;
            let items = (cursor..cursor + 64).map(descriptor).collect();
            let event = page(cursor, Some(cursor + 64), false, 1, items);
            queue_needed(&mut state);
            let result = state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &event);
            if page_index + 1 == MAX_PLUGIN_PARAMETER_CATALOG_ITEMS / 64 {
                assert!(matches!(
                    result,
                    PluginParameterCatalogUpdate::Faulted { .. }
                ));
                assert_eq!(state.len(), MAX_PLUGIN_PARAMETER_CATALOG_ITEMS - 64);
                return;
            }
            assert_eq!(
                result,
                PluginParameterCatalogUpdate::NeedNext {
                    cursor: cursor + 64
                }
            );
        }
        panic!("catalog limit was not enforced");
    }

    #[test]
    fn faults_on_invalid_descriptor_fields() {
        let mut cases = Vec::new();
        let mut too_long_name = descriptor(1);
        too_long_name.name = "x".repeat(MAX_PLUGIN_PARAMETER_NAME_BYTES + 1);
        cases.push(too_long_name);
        let mut too_long_unit = descriptor(2);
        too_long_unit.unit = "x".repeat(MAX_PLUGIN_PARAMETER_UNIT_BYTES + 1);
        cases.push(too_long_unit);
        let mut nan_current = descriptor(3);
        nan_current.current_normalized = f32::NAN;
        cases.push(nan_current);
        let mut high_current = descriptor(4);
        high_current.current_normalized = 1.01;
        cases.push(high_current);
        let mut invalid_default = descriptor(5);
        invalid_default.default_normalized = Some(f32::INFINITY);
        cases.push(invalid_default);

        for invalid in cases {
            let mut state = open();
            let event = page(0, None, true, 1, vec![invalid]);
            queue_needed(&mut state);
            assert!(matches!(
                state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &event),
                PluginParameterCatalogUpdate::Faulted { .. }
            ));
            assert!(state.is_empty());
        }
    }

    #[test]
    fn faults_on_inconsistent_done_and_next_cursor() {
        for event in [
            page(0, Some(1), true, 1, vec![descriptor(1)]),
            page(0, None, false, 1, vec![descriptor(1)]),
            page(0, Some(0), false, 1, Vec::new()),
        ] {
            let mut state = open();
            queue_needed(&mut state);
            assert!(matches!(
                state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &event),
                PluginParameterCatalogUpdate::Faulted { .. }
            ));
        }
    }

    #[test]
    fn queue_pressure_is_retryable_and_in_flight_requests_are_not_duplicated() {
        let mut state = open();
        assert_eq!(state.request_to_send(), Some(0));

        // A failed enqueue is represented by doing nothing: the same request remains available.
        assert_eq!(state.request_to_send(), Some(0));
        assert!(state.mark_request_queued(0));
        assert_eq!(
            state.request_state(),
            Some(PluginParameterCatalogRequestState::InFlight { cursor: 0 })
        );
        assert_eq!(state.request_to_send(), None);
        assert!(!state.mark_request_queued(0));

        let first = page(0, Some(1), false, 3, vec![descriptor(1)]);
        assert_eq!(
            state.observe_runtime_event(SESSION, ENDPOINT, INSTANCE, &first),
            PluginParameterCatalogUpdate::NeedNext { cursor: 1 }
        );
        assert_eq!(state.request_to_send(), Some(1));
    }
}
